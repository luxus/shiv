//! Mod sources: the built-in catalog plus the native registries the user
//! added. Adding a registry fetches and validates its descriptor and shows
//! it for review before it is saved; browsing merges each registry's
//! listings into the catalog view, and installing one downloads the
//! artifact of its current release, checks the published checksum, and
//! hands the file to the normal install path. Updates compare the
//! installed content revision with the current one, exactly like a
//! catalog date.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use shiv_api::{
    ApiError, NativeListing, NativeSourceClient, SourceDescriptor, is_https_or_loopback, origin_of,
};
use shiv_core::game::{GameError, GamePaths};
use shiv_core::registry::{self, AddedSource, RegistryError};
use slint::{ComponentHandle, Model, ModelRc, VecModel, Weak};

use crate::browse;
use crate::{App, SourceRow, SourcesState};

/// Where the game is: injectable so tests never touch the real install.
pub(crate) type Locate = Arc<dyn Fn() -> Result<GamePaths, GameError> + Send + Sync>;

thread_local! {
    static SOURCES: RefCell<Option<Rc<Sources>>> = const { RefCell::new(None) };
}

fn with_sources(f: impl FnOnce(&Rc<Sources>)) {
    if let Some(s) = SOURCES.with(|c| c.borrow().clone()) {
        f(&s);
    }
}

/// The Sources card's status line: how many mods the source's catalog
/// lists (once loaded) and how many of them cannot be installed, and, for
/// an entry an older Shiv saved without the descriptor's origins, that it
/// should be added again. Says which catalog the card stands for, so a
/// source saved from an outdated descriptor is visible as such.
pub(crate) fn source_detail(source: &AddedSource, listings: Option<&[NativeListing]>) -> String {
    let mut parts = Vec::new();
    if let Some(listings) = listings {
        let shown: Vec<&NativeListing> = listings.iter().filter(|l| !l.removed).collect();
        let blocked = shown.iter().filter(|l| !l.installable()).count();
        parts.push(match shown.len() {
            1 => "1 mod".to_owned(),
            n => format!("{n} mods"),
        });
        if blocked > 0 {
            parts.push(format!("{blocked} cannot be installed"));
        }
    }
    if source.predates_origins() {
        parts.push("saved by an older Shiv: add its descriptor again to review it".to_owned());
    }
    parts.join(" \u{b7} ")
}

/// The banner line for the entries a loaded catalog refused, if any: each
/// one's name and why, so a refused mod is never silently missing.
pub(crate) fn refused_text(source: &AddedSource, listings: &[NativeListing]) -> Option<String> {
    let refused: Vec<String> = listings
        .iter()
        .filter(|l| !l.removed)
        .filter_map(|l| {
            let problem = l.problem.as_deref()?;
            let name = if l.title.is_empty() { &l.id } else { &l.title };
            Some(format!("{name} ({problem})"))
        })
        .collect();
    (!refused.is_empty()).then(|| {
        format!(
            "{} lists mods Shiv refused: {}",
            source.name,
            refused.join("; ")
        )
    })
}

/// One registry's catalog, as Browse shows it.
#[derive(Clone)]
struct Catalog {
    source: AddedSource,
    listings: Vec<NativeListing>,
}

struct Sources {
    weak: Weak<App>,
    locate: Locate,
    rows: Rc<VecModel<SourceRow>>,
    catalogs: RefCell<Vec<Catalog>>,
    /// A descriptor the user reviewed, waiting for the Add button.
    pending: RefCell<Option<SourceDescriptor>>,
}

pub fn attach(app: &App) {
    attach_with(app, Arc::new(crate::locate_game));
}

pub(crate) fn attach_with(app: &App, locate: Locate) {
    let rows = Rc::new(VecModel::default());
    let controller = Rc::new(Sources {
        weak: app.as_weak(),
        locate,
        rows: rows.clone(),
        catalogs: RefCell::new(Vec::new()),
        pending: RefCell::new(None),
    });
    SOURCES.with(|c| *c.borrow_mut() = Some(controller));
    let state = app.global::<SourcesState>();
    state.set_rows(ModelRc::from(rows));
    state.on_refresh(|| with_sources(|s| s.refresh()));
    state.on_add(|| with_sources(|s| s.start_review()));
    state.on_confirm_add(|| with_sources(|s| s.confirm_add()));
    state.on_cancel_add(|| with_sources(|s| s.cancel_review()));
    state.on_remove(|origin, id| {
        with_sources(|s| s.remove(origin.as_str(), id.as_str()));
    });
    state.on_dismiss_banner(|| {
        with_sources(|s| {
            s.with_state(|s| s.set_banner_text("".into()));
        });
    });
}

/// Load the saved sources and their catalogs (at startup).
pub(crate) fn load() {
    with_sources(|s| s.refresh());
}

impl Sources {
    fn with_state(&self, f: impl FnOnce(&SourcesState<'_>)) {
        if let Some(app) = self.weak.upgrade() {
            f(&app.global::<SourcesState>());
        }
    }

    /// Show the saved sources and load every registry's catalog.
    fn refresh(&self) {
        let saved = self
            .registry()
            .map(|r| r.sources().to_vec())
            .unwrap_or_default();
        let rows: Vec<SourceRow> = saved
            .iter()
            .map(|s| SourceRow {
                origin: s.origin.as_str().into(),
                id: s.source_id.as_str().into(),
                name: s.name.as_str().into(),
                descriptor: s.descriptor_url.as_str().into(),
                detail: source_detail(s, None).into(),
            })
            .collect();
        self.rows.set_vec(rows);
        let weak = self.weak.clone();
        let started = crate::spawn_worker(
            "shiv-load-sources",
            move || fetch_catalogs(&saved),
            move |result| {
                let _ = weak.upgrade_in_event_loop(move |_| {
                    with_sources(|s| s.catalogs_loaded(result));
                });
            },
        );
        if !started {
            self.with_state(|s| {
                s.set_banner_text("Could not start a background thread.".into());
            });
        }
    }

    fn catalogs_loaded(&self, result: Result<Vec<CatalogOutcome>, String>) {
        let mut banners = Vec::new();
        let mut catalogs = Vec::new();
        match result {
            Ok(loaded) => {
                for (source, outcome) in loaded {
                    match outcome {
                        Ok(listings) => catalogs.push(Catalog { source, listings }),
                        Err(e) => banners.push(format!(
                            "Could not load {}: {}",
                            source.name,
                            browse::catalog_error_text(&e)
                        )),
                    }
                }
            }
            Err(text) => banners.push(text),
        }
        for catalog in &catalogs {
            banners.extend(refused_text(&catalog.source, &catalog.listings));
        }
        if !banners.is_empty() {
            self.with_state(|s| s.set_banner_text(banners.join("\n").into()));
        }
        for row in 0..self.rows.row_count() {
            let Some(mut item) = self.rows.row_data(row) else {
                continue;
            };
            let loaded = catalogs.iter().find(|c| {
                c.source.origin == item.origin.as_str() && c.source.source_id == item.id.as_str()
            });
            if let Some(c) = loaded {
                item.detail = source_detail(&c.source, Some(&c.listings)).into();
                self.rows.set_row_data(row, item);
            }
        }
        *self.catalogs.borrow_mut() = catalogs;
        browse::refresh_sources();
        crate::installed::catalogs_ready();
    }

    fn start_review(&self) {
        let Some(app) = self.weak.upgrade() else {
            return;
        };
        let state = app.global::<SourcesState>();
        let url = state.get_add_url().to_string();
        if url.trim().is_empty() {
            state.set_add_error("Paste the URL of a dmm-source.json descriptor.".into());
            return;
        }
        if !is_https_or_loopback(&url) {
            state.set_add_error(
                "The descriptor must be a public HTTPS URL (loopback for development).".into(),
            );
            return;
        }
        state.set_add_busy(true);
        state.set_add_error("".into());
        let weak = self.weak.clone();
        let started = crate::spawn_worker(
            "shiv-review-source",
            move || {
                NativeSourceClient::new()
                    .fetch_descriptor(url.trim())
                    .map_err(|e| browse::catalog_error_text(&e))
            },
            move |result| {
                let result = result.and_then(|r| r);
                let _ = weak.upgrade_in_event_loop(move |_| {
                    with_sources(|s| s.reviewed(result));
                });
            },
        );
        if !started {
            state.set_add_busy(false);
            state.set_add_error("Could not start a background thread.".into());
        }
    }

    /// A fetched descriptor is offered for review: the name and the one
    /// origin its downloads may come from, before anything is saved.
    fn reviewed(&self, result: Result<SourceDescriptor, String>) {
        self.with_state(|s| s.set_add_busy(false));
        let Some(app) = self.weak.upgrade() else {
            return;
        };
        let state = app.global::<SourcesState>();
        match result {
            Err(text) => state.set_add_error(text.into()),
            Ok(descriptor) => {
                let Some(origin) = origin_of(&descriptor.descriptor_url) else {
                    state.set_add_error("The descriptor URL has no usable origin.".into());
                    return;
                };
                if let Some(existing) = self
                    .registry()
                    .ok()
                    .and_then(|r| r.find(&origin, &descriptor.source_id).cloned())
                    .filter(|existing| !existing.predates_origins())
                {
                    state.set_add_error(
                        format!(
                            "{} is already added from {}",
                            existing.name, existing.descriptor_url
                        )
                        .into(),
                    );
                    return;
                }
                state.set_review_name(descriptor.name.clone().into());
                state.set_review_origin(origin.into());
                state.set_review_id(descriptor.source_id.clone().into());
                state.set_review_descriptor(descriptor.descriptor_url.clone().into());
                state.set_review_open(true);
                *self.pending.borrow_mut() = Some(descriptor);
            }
        }
    }

    fn confirm_add(&self) {
        let Some(app) = self.weak.upgrade() else {
            return;
        };
        let state = app.global::<SourcesState>();
        let Some(descriptor) = self.pending.borrow().clone() else {
            state.set_review_open(false);
            return;
        };
        let origin = origin_of(&descriptor.descriptor_url).unwrap_or_default();
        let source = AddedSource {
            descriptor_url: descriptor.descriptor_url,
            origin,
            source_id: descriptor.source_id,
            name: descriptor.name,
            api_origins: descriptor.api_origins,
            download_origins: descriptor.download_origins,
        };
        let outcome = self.registry().and_then(|mut r| r.add(source));
        match outcome {
            Ok(()) => {
                state.set_review_open(false);
                state.set_add_url("".into());
                state.set_add_error("".into());
                *self.pending.borrow_mut() = None;
                self.refresh();
            }
            Err(e) => state.set_add_error(e.to_string().into()),
        }
    }

    fn cancel_review(&self) {
        self.with_state(|s| s.set_review_open(false));
        *self.pending.borrow_mut() = None;
    }

    fn remove(&self, origin: &str, source_id: &str) {
        let outcome = self
            .registry()
            .and_then(|mut r| r.remove(origin, source_id));
        match outcome {
            Ok(()) => self.refresh(),
            Err(e) => {
                self.with_state(|s| s.set_banner_text(e.to_string().into()));
            }
        }
    }
}

impl Sources {
    fn registry(&self) -> Result<registry::Registry, RegistryError> {
        let paths = (self.locate)().map_err(|e| RegistryError::Refused(e.to_string()))?;
        registry::Registry::load(&paths)
    }
}

/// One registry's catalog, as Browse shows it — or why it did not load.
type CatalogOutcome = (AddedSource, Result<Vec<NativeListing>, ApiError>);

/// Every added registry's catalog. A failed registry never takes the others
/// down: its slot says why.
pub(crate) fn fetch_catalogs(saved: &[AddedSource]) -> Vec<CatalogOutcome> {
    let client = NativeSourceClient::new();
    saved
        .iter()
        .map(|source| {
            let outcome = client
                .fetch_descriptor(&source.descriptor_url)
                .and_then(|descriptor| client.fetch_catalog(&descriptor));
            (source.clone(), outcome)
        })
        .collect()
}

/// The catalogs Browse merges into its list: `(source, listings)` pairs.
pub(crate) fn catalogs() -> Vec<(AddedSource, Vec<NativeListing>)> {
    SOURCES.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|s| {
                s.catalogs
                    .borrow()
                    .iter()
                    .map(|c| (c.source.clone(), c.listings.clone()))
                    .collect()
            })
            .unwrap_or_default()
    })
}
