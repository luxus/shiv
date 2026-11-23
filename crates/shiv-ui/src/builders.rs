//! CompLock Hitsound and HP Bar builders on the Performance screen.
//! The form lives in the Slint state. Decoding, the pinned script and the
//! install run on a worker.

use shiv_core::game::GamePaths;
use shiv_core::generated::{self, ClipIn, HpPreset, Spec, Value};
use shiv_core::install;
use shiv_core::sidecar;
use slint::{ComponentHandle, ModelRc, VecModel, Weak};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::Path;

use crate::installed::{Locate, Running};
use crate::performance::Deps;
use crate::{App, PerfState};

thread_local! {
    static PRESETS: std::cell::RefCell<Vec<HpPreset>> = const { std::cell::RefCell::new(Vec::new()) };
    static LOADED: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn wire(app: &App, deps: &Deps) {
    let state = app.global::<PerfState>();
    if generated::builder_review() {
        state.set_builder_note(
            "Builder changed, review. Shiv will not run a newer script until this copy is updated."
                .into(),
        );
    }
    let locate = deps.locate.clone();
    let running = deps.running.clone();
    let weak = app.as_weak();
    state.on_browse_body({
        let weak = weak.clone();
        move || pick_sound(&weak, true)
    });
    state.on_browse_head({
        let weak = weak.clone();
        move || pick_sound(&weak, false)
    });
    state.on_import_hitsound({
        let weak = weak.clone();
        move || import_hitsound(&weak)
    });
    state.on_pick_hp_preset({
        let weak = weak.clone();
        move |index| apply_preset(&weak, index)
    });
    state.on_build_hitsound({
        let weak = weak.clone();
        let locate = locate.clone();
        let running = running.clone();
        move || build_hitsound(&weak, &locate, &running)
    });
    state.on_build_hpbar({
        let weak = weak.clone();
        let locate = locate.clone();
        let running = running.clone();
        move || build_hpbar(&weak, &locate, &running)
    });
}

/// Load presets and any installed builder, once, when the screen is shown.
pub(crate) fn refresh(weak: Weak<App>, locate: Locate) {
    if LOADED.with(Cell::get) {
        return;
    }
    LOADED.with(|c| c.set(true));
    let started = crate::spawn_worker(
        "shiv-builders",
        move || {
            let presets = generated::hp_presets().map_err(|e| e.to_string())?;
            let paths = locate().map_err(|e| e.to_string())?;
            let hit = read_spec(&paths, generated::HITSOUND_ID);
            let hp = read_spec(&paths, generated::HPBAR_ID);
            Ok::<_, String>((presets, hit, hp))
        },
        move |result| {
            let _ = weak.upgrade_in_event_loop(move |app| {
                let Ok(Ok((presets, hit, hp))) = result else {
                    LOADED.with(|c| c.set(false));
                    return;
                };
                PRESETS.with(|p| p.borrow_mut().clone_from(&presets));
                let labels: Vec<slint::SharedString> =
                    presets.iter().map(|p| p.title.as_str().into()).collect();
                let state = app.global::<PerfState>();
                if !labels.is_empty() {
                    state.set_hp_presets(ModelRc::new(VecModel::from(labels)));
                }
                if let Some(Spec::Hitsound { name, body, head }) = hit {
                    state.set_hit_name(name.into());
                    state.set_body_path(body.source_name.into());
                    state.set_body_start(body.start.to_string().into());
                    state.set_body_end(body.end.to_string().into());
                    state.set_body_db(fmt_num(body.db).into());
                    state.set_body_pitch(fmt_num(body.pitch).into());
                    state.set_head_path(head.source_name.into());
                    state.set_head_start(head.start.to_string().into());
                    state.set_head_end(head.end.to_string().into());
                    state.set_head_db(fmt_num(head.db).into());
                    state.set_head_pitch(fmt_num(head.pitch).into());
                }
                if let Some(Spec::HpBar { name, options }) = hp {
                    state.set_hp_name(name.into());
                    state.set_hp_link(generated::hash_from_options(&options).into());
                }
            });
        },
    );
    if !started {
        LOADED.with(|c| c.set(false));
    }
}

fn build_hitsound(weak: &Weak<App>, locate: &Locate, running: &Running) {
    let Some(app) = weak.upgrade() else {
        return;
    };
    let state = app.global::<PerfState>();
    if state.get_builder_busy() {
        return;
    }
    let form = HitForm {
        name: state.get_hit_name().to_string(),
        body_path: state.get_body_path().to_string(),
        head_path: state.get_head_path().to_string(),
        body_start: state.get_body_start().to_string(),
        body_end: state.get_body_end().to_string(),
        body_db: state.get_body_db().to_string(),
        body_pitch: state.get_body_pitch().to_string(),
        head_start: state.get_head_start().to_string(),
        head_end: state.get_head_end().to_string(),
        head_db: state.get_head_db().to_string(),
        head_pitch: state.get_head_pitch().to_string(),
    };
    state.set_builder_busy(true);
    state.set_builder_status("Building the hitsound.".into());
    let locate = locate.clone();
    let running = running.clone();
    let weak = weak.clone();
    let started = crate::spawn_worker(
        "shiv-build-hitsound",
        move || install_hitsound(&locate, &running, &form),
        {
            let weak = weak.clone();
            move |result| finish(&weak, result)
        },
    );
    if !started {
        finish_now(&weak, Err("Could not start a background thread.".into()));
    }
}

fn build_hpbar(weak: &Weak<App>, locate: &Locate, running: &Running) {
    let Some(app) = weak.upgrade() else {
        return;
    };
    let state = app.global::<PerfState>();
    if state.get_builder_busy() {
        return;
    }
    let name = state.get_hp_name().to_string();
    let link = state.get_hp_link().to_string();
    let index = state.get_hp_preset();
    state.set_builder_busy(true);
    state.set_builder_status("Building the HP bar.".into());
    let locate = locate.clone();
    let running = running.clone();
    let weak = weak.clone();
    let presets = PRESETS.with(|p| p.borrow().clone());
    let started = crate::spawn_worker(
        "shiv-build-hpbar",
        move || install_hpbar(&locate, &running, &name, &link, index, &presets),
        {
            let weak = weak.clone();
            move |result| finish(&weak, result)
        },
    );
    if !started {
        finish_now(&weak, Err("Could not start a background thread.".into()));
    }
}

fn install_hitsound(locate: &Locate, running: &Running, form: &HitForm) -> Result<String, String> {
    let paths = locate().map_err(|e| e.to_string())?;
    if running() {
        return Err("Deadlock is running. Close it, then build again.".into());
    }
    let stored = read_spec(&paths, generated::HITSOUND_ID);
    let (stored_body, stored_head) = match &stored {
        Some(Spec::Hitsound { body, head, .. }) => (
            generated::stored_clip(generated::HITSOUND_ID, body).ok(),
            generated::stored_clip(generated::HITSOUND_ID, head).ok(),
        ),
        _ => (None, None),
    };
    let body = clip_in(
        "Body",
        &form.body_path,
        &form.body_start,
        &form.body_end,
        &form.body_db,
        &form.body_pitch,
        stored_body,
    )?;
    let head = clip_in(
        "Head",
        &form.head_path,
        &form.head_start,
        &form.head_end,
        &form.head_db,
        &form.head_pitch,
        stored_head,
    )?;
    let guard = install::InstallGuard::acquire(&paths.addons).map_err(|e| e.to_string())?;
    generated::install_hitsound(
        &paths,
        &form.name,
        &body,
        &head,
        running.as_ref(),
        guard.permit(),
    )
    .map_err(|e| e.to_string())?;
    Ok(format!("Installed {}.", form.name.trim()))
}

fn install_hpbar(
    locate: &Locate,
    running: &Running,
    name: &str,
    link: &str,
    index: i32,
    presets: &[HpPreset],
) -> Result<String, String> {
    let paths = locate().map_err(|e| e.to_string())?;
    if running() {
        return Err("Deadlock is running. Close it, then build again.".into());
    }
    let options = hp_options(link, index, presets)?;
    let guard = install::InstallGuard::acquire(&paths.addons).map_err(|e| e.to_string())?;
    generated::install_hpbar(&paths, name, &options, running.as_ref(), guard.permit())
        .map_err(|e| e.to_string())?;
    Ok(format!("Installed {}.", name.trim()))
}

fn hp_options(
    link: &str,
    index: i32,
    presets: &[HpPreset],
) -> Result<BTreeMap<String, Value>, String> {
    let hash = link.rsplit_once('#').map_or(link, |(_, hash)| hash).trim();
    if !hash.is_empty() {
        return Ok(generated::options_from_hash(hash));
    }
    let index = usize::try_from(index).unwrap_or(0);
    if let Some(preset) = presets.get(index) {
        return Ok(preset.options.clone());
    }
    if index == 0 {
        return Ok(BTreeMap::new());
    }
    Err("The presets are still loading. Try again in a moment.".into())
}

/// Worker body: reads a chosen sound, or the samples stored for a rebuild.
fn clip_in(
    which: &str,
    path: &str,
    start: &str,
    end: &str,
    db: &str,
    pitch: &str,
    stored: Option<ClipIn>,
) -> Result<ClipIn, String> {
    let db = parse_f64(which, "volume", db)?;
    let pitch = parse_f64(which, "pitch", pitch)?;
    let path = path.trim();
    let (samples, source_name) = if Path::new(path).is_file() {
        let bytes = std::fs::read(path).map_err(|e| format!("Could not read {path}: {e}"))?;
        let samples = generated::decode_mono_44k(&bytes).map_err(|e| format!("{which}: {e}"))?;
        let source_name = Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(path)
            .to_owned();
        (samples, source_name)
    } else if let Some(stored) = stored {
        (stored.samples, stored.source_name)
    } else {
        return Err(format!("{which}: choose a sound file."));
    };
    let len = u32::try_from(samples.len()).unwrap_or(u32::MAX);
    let start = if start.trim().is_empty() {
        0
    } else {
        parse_u32(which, "start", start)?
    };
    let end = if end.trim().is_empty() {
        len
    } else {
        parse_u32(which, "end", end)?
    };
    Ok(ClipIn {
        samples,
        source_name,
        start,
        end,
        db,
        pitch,
    })
}

fn pick_sound(weak: &Weak<App>, body: bool) {
    let weak = weak.clone();
    crate::spawn_worker(
        "shiv-pick-sound",
        move || {
            rfd::FileDialog::new()
                .set_title(if body {
                    "Body hitsound"
                } else {
                    "Head hitsound"
                })
                .add_filter("Audio", &["wav", "mp3", "ogg", "flac"])
                .pick_file()
        },
        move |picked| {
            let Some(path) = picked.ok().flatten() else {
                return;
            };
            let text = path.display().to_string();
            let _ = weak.upgrade_in_event_loop(move |app| {
                let state = app.global::<PerfState>();
                if body {
                    state.set_body_path(text.into());
                } else {
                    state.set_head_path(text.into());
                }
            });
        },
    );
}

fn import_hitsound(weak: &Weak<App>) {
    let weak = weak.clone();
    crate::spawn_worker(
        "shiv-import-hitsound",
        || {
            let path = rfd::FileDialog::new()
                .set_title("Import a hitsound pak")
                .add_filter("VPK", &["vpk"])
                .pick_file();
            let Some(path) = path else {
                return Ok(None);
            };
            let bytes = std::fs::read(&path).map_err(|e| format!("Could not read the pak: {e}"))?;
            let facts = generated::read_hitsound(&bytes)
                .ok_or_else(|| "That pak is not a CompLock hitsound.".to_owned())?;
            Ok(Some(facts))
        },
        move |result| {
            let _ = weak.upgrade_in_event_loop(move |app| {
                let state = app.global::<PerfState>();
                match result {
                    Ok(Ok(Some(facts))) => {
                        state.set_body_start("0".into());
                        state.set_body_end(facts.body_samples.to_string().into());
                        state.set_body_db(fmt_num(facts.body_db).into());
                        state.set_body_pitch(fmt_num(facts.body_pitch).into());
                        state.set_head_start("0".into());
                        state.set_head_end(facts.head_samples.to_string().into());
                        state.set_head_db(fmt_num(facts.head_db).into());
                        state.set_head_pitch(fmt_num(facts.head_pitch).into());
                        state.set_builder_status(
                            "Read the volume, pitch and length. The pak does not store the sound files or where the trim started, so choose them again."
                                .into(),
                        );
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(text)) | Err(text) => state.set_builder_status(text.into()),
                }
            });
        },
    );
}

fn apply_preset(weak: &Weak<App>, index: i32) {
    let Some(app) = weak.upgrade() else {
        return;
    };
    let options = PRESETS.with(|p| {
        usize::try_from(index)
            .ok()
            .and_then(|i| p.borrow().get(i).map(|preset| preset.options.clone()))
    });
    if let Some(options) = options {
        app.global::<PerfState>()
            .set_hp_link(generated::hash_from_options(&options).into());
    }
}

fn finish(weak: &Weak<App>, result: Result<Result<String, String>, String>) {
    let (ok, text) = match result {
        Ok(Ok(text)) => (true, text),
        Ok(Err(text)) | Err(text) => (false, text),
    };
    let _ = weak.upgrade_in_event_loop(move |app| {
        let state = app.global::<PerfState>();
        state.set_builder_busy(false);
        state.set_builder_status(text.into());
        if ok {
            crate::installed::reload();
            crate::launch::refresh();
        }
    });
}

fn finish_now(weak: &Weak<App>, result: Result<String, String>) {
    finish(weak, Ok(result));
}

fn read_spec(paths: &GamePaths, id: &str) -> Option<Spec> {
    sidecar::read(&paths.addons, id)
        .ok()
        .flatten()
        .and_then(|side| side.generated.as_ref().and_then(generated::spec_of))
}

fn parse_f64(which: &str, label: &str, text: &str) -> Result<f64, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err(format!("{which} {label} is empty."));
    }
    text.parse()
        .map_err(|_| format!("{which} {label} is not a number."))
}

fn parse_u32(which: &str, label: &str, text: &str) -> Result<u32, String> {
    text.trim()
        .parse()
        .map_err(|_| format!("{which} {label} is not a whole number."))
}

fn fmt_num(n: f64) -> String {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < 9_223_372_036_854_775_808.0 {
        #[allow(
            clippy::cast_possible_truncation,
            reason = "n is verified finite, integral, and strictly below 2^63"
        )]
        let whole = n as i64;
        format!("{whole}")
    } else {
        format!("{n}")
    }
}

struct HitForm {
    name: String,
    body_path: String,
    head_path: String,
    body_start: String,
    body_end: String,
    body_db: String,
    body_pitch: String,
    head_start: String,
    head_end: String,
    head_db: String,
    head_pitch: String,
}
