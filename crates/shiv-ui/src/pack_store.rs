//! Shared live CompLock pack store.
//!
//! Holds the active `complock::PackLoad` and the latest known released Shiv
//! tag, refreshed in the background on startup and periodically thereafter.
//! Controllers snapshot `current()` instead of keeping independent copies.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use shiv_core::complock;

use crate::App;

#[cfg(not(test))]
const USER_AGENT: &str = concat!("shiv/", env!("CARGO_PKG_VERSION"));

/// Fetches an artifact text file (`meta.json` or `pack-data.json`) by URL.
pub(crate) type PackFetch = Arc<dyn Fn(&str) -> Result<String, String> + Send + Sync>;

/// Shared CompLock pack and release state read by UI controllers.
pub(crate) struct PackStore {
    pack: RwLock<Arc<complock::PackLoad>>,
    latest_release: RwLock<Option<String>>,
    fetch: Option<PackFetch>,
    cache_dir: Option<PathBuf>,
    fetched_at: Mutex<Option<Instant>>,
    busy: AtomicBool,
}

impl PackStore {
    /// Production store backed by pinned GitHub release assets and the user
    /// cache directory; offline and baked-only under unit tests.
    pub(crate) fn system() -> Self {
        #[cfg(not(test))]
        {
            Self::new(Arc::new(fetch_artifact), complock::pack_cache_dir())
        }
        #[cfg(test)]
        {
            Self::baked()
        }
    }

    /// Store using `fetch` and `cache_dir`, seeded immediately with
    /// [`complock::baked_pack`].
    pub(crate) fn new(fetch: PackFetch, cache_dir: Option<PathBuf>) -> Self {
        Self {
            pack: RwLock::new(Arc::new(complock::baked_pack())),
            latest_release: RwLock::new(None),
            fetch: Some(fetch),
            cache_dir,
            fetched_at: Mutex::new(None),
            busy: AtomicBool::new(false),
        }
    }

    /// Offline store pinned to [`complock::baked_pack`].
    #[cfg(test)]
    pub(crate) fn baked() -> Self {
        Self::fixed(complock::baked_pack())
    }

    /// Offline store pinned to `pack`.
    #[cfg(test)]
    pub(crate) fn fixed(pack: complock::PackLoad) -> Self {
        Self {
            pack: RwLock::new(Arc::new(pack)),
            latest_release: RwLock::new(None),
            fetch: None,
            cache_dir: None,
            fetched_at: Mutex::new(None),
            busy: AtomicBool::new(false),
        }
    }

    /// Snapshot of the currently active CompLock pack.
    pub(crate) fn current(&self) -> Arc<complock::PackLoad> {
        self.pack
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Whether a background artifact fetch is currently in flight.
    #[cfg(test)]
    pub(crate) fn is_busy(&self) -> bool {
        self.busy.load(Ordering::Acquire)
    }

    /// Latest known published Shiv release tag (e.g. `v0.7.0`), if checked.
    pub(crate) fn latest_release(&self) -> Option<String> {
        self.latest_release
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Record the latest published Shiv release tag observed by the updater.
    pub(crate) fn set_latest_release(&self, tag: Option<String>) {
        *self
            .latest_release
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = tag;
    }

    /// Whether `pack` requires prompting the user to update Shiv given the
    /// latest known release.
    pub(crate) fn needs_update(&self, pack: &complock::PackLoad) -> bool {
        let latest = self.latest_release();
        pack_needs_update(pack, latest.as_deref())
    }

    /// Refresh from the published artifact if [`complock::PACK_REFRESH`] has
    /// elapsed since the last live check.
    pub(crate) fn refresh_if_due(weak: &slint::Weak<App>, store: &Arc<Self>) {
        let last = *store
            .fetched_at
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if complock::pack_refresh_due(last, Instant::now()) {
            Self::refresh(weak, store);
        }
    }

    /// Start a background artifact refresh unless offline or already running.
    pub(crate) fn refresh(weak: &slint::Weak<App>, store: &Arc<Self>) {
        let Some(fetch) = store.fetch.clone() else {
            return;
        };
        if store.busy.swap(true, Ordering::AcqRel) {
            return;
        }
        let cache = store.cache_dir.clone();
        let store_done = store.clone();
        let weak_done = weak.clone();
        let started = crate::spawn_worker(
            "shiv-complock-pack",
            move || complock::load_pack(|url| fetch(url), cache.as_deref()),
            move |result| {
                if let Ok(load) = result {
                    if load.origin() == complock::PackOrigin::Live
                        || load.waiting()
                        || load.too_old()
                    {
                        *store_done
                            .fetched_at
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) =
                            Some(Instant::now());
                    }
                    *store_done
                        .pack
                        .write()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(load);
                    store_done.busy.store(false, Ordering::Release);
                    let _ = weak_done.upgrade_in_event_loop(|_| {
                        crate::installed::pack_changed();
                        crate::performance::pack_changed();
                    });
                } else {
                    store_done.busy.store(false, Ordering::Release);
                }
            },
        );
        if !started {
            store.busy.store(false, Ordering::Release);
        }
    }
}

/// Whether `pack` should prompt [`complock::UPDATE_NEEDED`] given `latest_release`.
pub(crate) fn pack_needs_update(pack: &complock::PackLoad, latest_release: Option<&str>) -> bool {
    pack.needs_update(|min| latest_release.is_some_and(|tag| shiv_api::tag_is_at_least(tag, min)))
}

/// Worker body — fetch a CompLock artifact file with a per-file size cap.
#[cfg(not(test))]
fn fetch_artifact(url: &str) -> Result<String, String> {
    let limit = if url == complock::ARTIFACT_META_URL {
        complock::MAX_META_BYTES
    } else {
        complock::MAX_PACK_BYTES
    };
    let cap = u64::try_from(limit).unwrap_or(u64::MAX);
    shiv_api::release_asset_text(url, USER_AGENT, cap).map_err(|e| e.to_string())
}
