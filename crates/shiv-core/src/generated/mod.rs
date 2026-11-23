//! CompLock builders that have no pack-data entry. Shiv stores the inputs
//! and runs the pinned site script to produce the pak. The version is the
//! hash of the current template and those inputs, so a template the watch
//! has pinned shows up as an update and rebuilds. A changed script does not:
//! [`builder_review`] is set and nothing runs the new file.

mod audio;
mod js;
mod pak;
mod spec;
mod templates;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::drift::Published;
use crate::game::GamePaths;
use crate::install::{self, InstallPermit, ModMeta};
use crate::sidecar::{self, Sidecar};

pub use audio::decode_mono_44k;
pub use pak::{HitsoundFacts, read_hitsound};
pub use serde_json::Value;
pub use spec::{Clip, HITSOUND_ID, HPBAR_ID, Spec, hash_from_options, is_id, options_from_hash};
pub use templates::builder_review;

const MAX_SAMPLES: usize = 44_100 * 180;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),
    #[error("no settings folder to store the builder files")]
    NoSettings,
    #[error("{0}")]
    Build(String),
    #[error(transparent)]
    Audio(#[from] audio::AudioError),
    #[error(transparent)]
    Install(#[from] install::InstallError),
    #[error(transparent)]
    Sidecar(#[from] sidecar::SidecarError),
    #[error("io error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// One hitsound slot as the user entered it, already decoded.
pub struct ClipIn {
    pub samples: Vec<f32>,
    pub source_name: String,
    pub start: u32,
    pub end: u32,
    pub db: f64,
    pub pitch: f64,
}

/// A fetch of the four builder artifacts the watch downloads.
#[derive(Clone, Copy)]
pub struct RemoteBuilders<'a> {
    pub hit_page: &'a str,
    pub lame: &'a [u8],
    pub hpbars_js: &'a str,
    pub base_pak: &'a [u8],
}

/// What the watch should do with a fetch. `Template` keeps the pinned
/// script and swaps in the new template bytes. `Review` changes no script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Watch {
    Current,
    Template {
        hit_builder: String,
        base_pak: Option<Vec<u8>>,
    },
    Review {
        changed: Vec<String>,
    },
    Failed {
        reason: String,
    },
}

/// One HP Bar preset from the pinned script: its id, title, and the options
/// object the site would pass to `buildPak`.
#[derive(Clone)]
pub struct HpPreset {
    pub id: String,
    pub title: String,
    pub options: BTreeMap<String, Value>,
}

/// The HP Bar presets from the pinned script.
pub fn hp_presets() -> Result<Vec<HpPreset>, Error> {
    js::hp_presets()
        .map_err(|e| Error::Build(e.to_string()))?
        .into_iter()
        .map(|(id, title, options)| {
            let options =
                serde_json::from_str(&options).map_err(|e| Error::Build(e.to_string()))?;
            Ok(HpPreset { id, title, options })
        })
        .collect()
}

/// The generated-mod spec stored on a sidecar, when it parses.
#[must_use]
pub fn spec_of(value: &Value) -> Option<Spec> {
    serde_json::from_value(value.clone()).ok()
}

/// [`spec_of`] for the JSON text of a sidecar value.
#[must_use]
pub fn spec_from_json(text: &str) -> Option<Spec> {
    serde_json::from_str(text).ok()
}

/// The sidecar value for `spec`.
#[must_use]
pub fn spec_value(spec: &Spec) -> Option<Value> {
    serde_json::to_value(spec).ok()
}

/// Build the hitsound pak the site's script builds from these clips.
pub fn build_hitsound(body: &ClipIn, head: &ClipIn) -> Result<Vec<u8>, Error> {
    check_clip("Body", body)?;
    check_clip("Head", head)?;
    js::build_hitsound(
        js::Slot {
            samples: &body.samples,
            start: body.start,
            end: body.end,
            levels: js::Levels {
                db: body.db,
                pitch: body.pitch,
            },
        },
        js::Slot {
            samples: &head.samples,
            start: head.start,
            end: head.end,
            levels: js::Levels {
                db: head.db,
                pitch: head.pitch,
            },
        },
    )
    .map_err(|e| Error::Build(e.to_string()))
}

/// Build the HP bar pak for `options` (the copy-link object, defaults omitted).
pub fn build_hpbar(options: &BTreeMap<String, Value>) -> Result<Vec<u8>, Error> {
    js::build_hpbar(options).map_err(|e| Error::Build(e.to_string()))
}

/// Build one pinned HP Bar preset by its id (`flat`, `ticks`, ...).
pub fn build_hpbar_preset(id: &str) -> Result<Vec<u8>, Error> {
    js::build_hpbar_preset(id).map_err(|e| Error::Build(e.to_string()))
}

/// `sha256:` of `template` then `inputs`. This is the sidecar version.
#[must_use]
pub fn version_of(template: &[u8], inputs: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(template);
    hasher.update(inputs);
    format!("sha256:{}", hex(&hasher.finalize()))
}

/// The pinned hitsound template bytes (the two blobs the script patches).
#[must_use]
pub fn hitsound_template_bytes() -> Vec<u8> {
    templates::hitsound_template(templates::HIT_BUILDER).unwrap_or_default()
}

/// The pinned HP bar base pak.
#[must_use]
pub fn hpbar_template_bytes() -> &'static [u8] {
    templates::BASE_PAK
}

/// The version a hitsound spec would record for these exact samples.
#[must_use]
pub fn hitsound_version(name: &str, body: &ClipIn, head: &ClipIn) -> String {
    version_of(
        &hitsound_template_bytes(),
        &hitsound_material(name, body, head),
    )
}

/// The version an HP bar spec would record for these options.
#[must_use]
pub fn hpbar_version(name: &str, options: &BTreeMap<String, Value>) -> String {
    version_of(hpbar_template_bytes(), &hpbar_material(name, options))
}

/// The version of an installed spec, reading its stored samples.
///
/// # Errors
///
/// [`Error::NoSettings`] when there is no settings folder;
/// [`Error::Io`] when a stored sample file cannot be read;
/// [`Error::Invalid`] when the spec's files are missing.
pub fn revision(spec: &Spec) -> Result<String, Error> {
    revision_in(spec, &pcm_root()?)
}

/// [`revision`] with the sample files under `root/<mod id>/`.
pub fn revision_in(spec: &Spec, root: &Path) -> Result<String, Error> {
    let dir = root.join(spec.mod_id());
    match spec {
        Spec::Hitsound { name, body, head } => {
            let body_path = crate::fs_util::safe_child(&dir, &body.pcm)
                .ok_or_else(|| Error::Invalid(format!("unsafe clip path {}", body.pcm)))?;
            let head_path = crate::fs_util::safe_child(&dir, &head.pcm)
                .ok_or_else(|| Error::Invalid(format!("unsafe clip path {}", head.pcm)))?;
            let body_pcm = read_pcm(&body_path)?;
            let head_pcm = read_pcm(&head_path)?;
            let inputs = stored_hitsound(name, body, &body_pcm, head, &head_pcm);
            Ok(version_of(&hitsound_template_bytes(), &inputs))
        }
        Spec::HpBar { name, options } => Ok(version_of(
            hpbar_template_bytes(),
            &hpbar_material(name, options),
        )),
    }
}

/// What the installed screen should treat as published for this spec.
/// A builder review wins, so a script change is not an update.
#[must_use]
pub fn publication(review: bool, current: Option<&str>) -> Published {
    if review {
        return Published::BuilderChanged;
    }
    match current {
        Some(revision) => Published::Current(std::iter::once(revision.to_owned()).collect()),
        None => Published::Unknown,
    }
}

/// Install or replace the hitsound mod from these clips.
///
/// # Errors
///
/// [`Error::Invalid`] when a clip's trim or levels cannot be built;
/// [`Error::NoSettings`] without a settings folder;
/// the install, reset and io errors of writing the pak.
pub fn install_hitsound(
    paths: &GamePaths,
    name: &str,
    body: &ClipIn,
    head: &ClipIn,
    game_running: impl Fn() -> bool,
    permit: InstallPermit<'_>,
) -> Result<Sidecar, Error> {
    let name = check_name(name)?;
    let pak = build_hitsound(body, head)?;
    let version = hitsound_version(&name, body, head);
    let spec = Spec::Hitsound {
        name: name.clone(),
        body: clip_spec("body.f32", body),
        head: clip_spec("head.f32", head),
    };
    place(
        paths,
        &spec,
        &version,
        &pak,
        &[&body.samples, &head.samples],
        game_running,
        permit,
    )
}

/// Install or replace the HP bar mod.
///
/// # Errors
///
/// As [`install_hitsound`], plus a builder failure.
pub fn install_hpbar(
    paths: &GamePaths,
    name: &str,
    options: &BTreeMap<String, Value>,
    game_running: impl Fn() -> bool,
    permit: InstallPermit<'_>,
) -> Result<Sidecar, Error> {
    let name = check_name(name)?;
    let pak = build_hpbar(options)?;
    let version = hpbar_version(&name, options);
    let spec = Spec::HpBar {
        name: name.clone(),
        options: options.clone(),
    };
    place(paths, &spec, &version, &pak, &[], game_running, permit)
}

/// Rebuild the pak an installed spec describes, from the samples in `root`.
pub fn rebuild(spec: &Spec) -> Result<Vec<u8>, Error> {
    rebuild_in(spec, &pcm_root()?)
}

/// [`rebuild`] written to `dest`.
pub fn write_rebuild(spec: &Spec, dest: &Path) -> Result<(), Error> {
    let bytes = rebuild(spec)?;
    std::fs::write(dest, bytes).map_err(|source| Error::Io {
        path: dest.to_path_buf(),
        source,
    })
}

/// The stored samples of one clip under the settings folder.
pub fn stored_clip(mod_id: &str, clip: &spec::Clip) -> Result<ClipIn, Error> {
    load_clip(&pcm_root()?.join(mod_id), clip)
}

/// [`rebuild`] reading samples from `root/<mod id>/`.
pub fn rebuild_in(spec: &Spec, root: &Path) -> Result<Vec<u8>, Error> {
    match spec {
        Spec::Hitsound { body, head, .. } => {
            let dir = root.join(spec.mod_id());
            let body_in = load_clip(&dir, body)?;
            let head_in = load_clip(&dir, head)?;
            build_hitsound(&body_in, &head_in)
        }
        Spec::HpBar { options, .. } => build_hpbar(options),
    }
}

/// Classify a fetch against the pinned files.
#[must_use]
pub fn classify(remote: &RemoteBuilders<'_>) -> Watch {
    if remote.lame.is_empty() || remote.hpbars_js.is_empty() || remote.base_pak.is_empty() {
        return Watch::Failed {
            reason: "a builder file came back empty".into(),
        };
    }
    let Some(fetched) = templates::extract_hit_builder(remote.hit_page) else {
        return Watch::Failed {
            reason: "the hitsound page has no builder script".into(),
        };
    };
    let (Some(pinned_logic), Some(fetched_logic)) = (
        templates::logic_text(templates::HIT_BUILDER),
        templates::logic_text(&fetched),
    ) else {
        return Watch::Failed {
            reason: "the hitsound script has no templates".into(),
        };
    };
    let mut changed = Vec::new();
    if pinned_logic != fetched_logic {
        changed.push("hit_builder.js".into());
    }
    if templates::LAME_JS.as_bytes() != remote.lame {
        changed.push("lame.min.js".into());
    }
    if templates::HPBARS_JS.as_bytes() != remote.hpbars_js.as_bytes() {
        changed.push("hp_hpbars.js".into());
    }
    if !changed.is_empty() {
        return Watch::Review { changed };
    }
    let pinned_tpl = hitsound_template_bytes();
    let fetched_tpl = templates::hitsound_template(&fetched).unwrap_or_default();
    let tpl_changed = pinned_tpl != fetched_tpl;
    let base_changed = templates::BASE_PAK != remote.base_pak;
    if !tpl_changed && !base_changed {
        return Watch::Current;
    }
    let Some(hit_builder) = templates::retarget_hit_builder(templates::HIT_BUILDER, &fetched)
    else {
        return Watch::Failed {
            reason: "the fetched hitsound templates are not base64".into(),
        };
    };
    if templates::logic_text(&hit_builder).as_deref() != Some(pinned_logic.as_str()) {
        return Watch::Failed {
            reason: "retargeting the hitsound script changed its logic".into(),
        };
    }
    Watch::Template {
        hit_builder,
        base_pak: base_changed.then(|| remote.base_pak.to_vec()),
    }
}

fn place(
    paths: &GamePaths,
    spec: &Spec,
    version: &str,
    pak: &[u8],
    samples: &[&[f32]],
    game_running: impl Fn() -> bool,
    permit: InstallPermit<'_>,
) -> Result<Sidecar, Error> {
    if game_running() {
        return Err(install::InstallError::GameRunning.into());
    }
    let root = pcm_root()?;
    let dir = root.join(spec.mod_id());
    std::fs::create_dir_all(&dir).map_err(|source| Error::Io {
        path: dir.clone(),
        source,
    })?;
    let staging = dir.join("build.vpk");
    std::fs::write(&staging, pak).map_err(|source| Error::Io {
        path: staging.clone(),
        source,
    })?;
    let meta = || ModMeta {
        mod_id: spec.mod_id().to_owned(),
        name: spec.name().to_owned(),
        version: Some(version.to_owned()),
        thumbnail_url: None,
        nsfw: false,
        hero: None,
        category: None,
    };
    let install_once = |permit: InstallPermit<'_>| -> Result<Sidecar, Error> {
        let mut side = install::install_files(
            std::slice::from_ref(&staging),
            meta(),
            paths,
            &game_running,
            permit,
        )?;
        let names = ["body.f32", "head.f32"];
        for (name, samples) in names.iter().zip(samples.iter()) {
            write_pcm(&dir.join(name), samples)?;
        }
        side.generated =
            Some(serde_json::to_value(spec).map_err(|e| Error::Invalid(e.to_string()))?);
        side.version = Some(version.to_owned());
        side.unverified = None;
        sidecar::write(&paths.addons, &side)?;
        Ok(side)
    };
    let result = install_once(permit);
    let _ = std::fs::remove_file(&staging);
    result
}

fn load_clip(dir: &Path, clip: &spec::Clip) -> Result<ClipIn, Error> {
    let path = crate::fs_util::safe_child(dir, &clip.pcm)
        .ok_or_else(|| Error::Invalid(format!("unsafe clip path {}", clip.pcm)))?;
    let bytes = read_pcm(&path)?;
    if !bytes.len().is_multiple_of(4) {
        return Err(Error::Invalid(format!("{} is not a sample file", clip.pcm)));
    }
    let samples = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect();
    Ok(ClipIn {
        samples,
        source_name: clip.source_name.clone(),
        start: clip.start,
        end: clip.end,
        db: clip.db,
        pitch: clip.pitch,
    })
}

fn clip_spec(pcm: &str, clip: &ClipIn) -> spec::Clip {
    spec::Clip {
        pcm: pcm.to_owned(),
        source_name: clip.source_name.clone(),
        start: clip.start,
        end: clip.end,
        db: clip.db,
        pitch: clip.pitch,
    }
}

fn check_clip(which: &str, clip: &ClipIn) -> Result<(), Error> {
    if clip.samples.is_empty() {
        return Err(Error::Invalid(format!("{which} has no audio")));
    }
    if clip.samples.len() > MAX_SAMPLES {
        return Err(Error::Invalid(format!(
            "{which} is longer than three minutes"
        )));
    }
    let len = u32::try_from(clip.samples.len()).unwrap_or(u32::MAX);
    if clip.start >= clip.end || clip.end > len {
        return Err(Error::Invalid(format!(
            "{which} trim {}..{} does not fit {} samples",
            clip.start, clip.end, len
        )));
    }
    if !clip.db.is_finite() || !(-100.0..=40.0).contains(&clip.db) {
        return Err(Error::Invalid(format!(
            "{which} volume is not a usable dB value"
        )));
    }
    if !clip.pitch.is_finite() || !(0.05..=8.0).contains(&clip.pitch) {
        return Err(Error::Invalid(format!(
            "{which} pitch must be between 0.05 and 8"
        )));
    }
    Ok(())
}

fn check_name(name: &str) -> Result<String, Error> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
        return Err(Error::Invalid("the name needs 1 to 80 characters".into()));
    }
    Ok(name.to_owned())
}

fn hitsound_material(name: &str, body: &ClipIn, head: &ClipIn) -> Vec<u8> {
    let mut out = Vec::new();
    push_name(&mut out, name);
    push_clip(&mut out, body, &pcm_bytes(&body.samples));
    push_clip(&mut out, head, &pcm_bytes(&head.samples));
    out
}

fn stored_hitsound(
    name: &str,
    body: &spec::Clip,
    body_pcm: &[u8],
    head: &spec::Clip,
    head_pcm: &[u8],
) -> Vec<u8> {
    let mut out = Vec::new();
    push_name(&mut out, name);
    push_stored(
        &mut out,
        &body.source_name,
        body.start,
        body.end,
        body.db,
        body.pitch,
        body_pcm,
    );
    push_stored(
        &mut out,
        &head.source_name,
        head.start,
        head.end,
        head.db,
        head.pitch,
        head_pcm,
    );
    out
}

fn hpbar_material(name: &str, options: &BTreeMap<String, Value>) -> Vec<u8> {
    let mut out = Vec::new();
    push_name(&mut out, name);
    if let Ok(json) = serde_json::to_vec(options) {
        out.extend(json);
    }
    out
}

fn push_name(out: &mut Vec<u8>, name: &str) {
    out.extend(name.as_bytes());
    out.push(0);
}

fn push_clip(out: &mut Vec<u8>, clip: &ClipIn, pcm: &[u8]) {
    push_stored(
        out,
        &clip.source_name,
        clip.start,
        clip.end,
        clip.db,
        clip.pitch,
        pcm,
    );
}

fn push_stored(
    out: &mut Vec<u8>,
    source: &str,
    start: u32,
    end: u32,
    db: f64,
    pitch: f64,
    pcm: &[u8],
) {
    out.extend(u64::try_from(pcm.len()).unwrap_or(0).to_le_bytes());
    out.extend(pcm);
    out.extend(start.to_le_bytes());
    out.extend(end.to_le_bytes());
    out.extend(db.to_le_bytes());
    out.extend(pitch.to_le_bytes());
    out.extend(source.as_bytes());
    out.push(0);
}

fn pcm_bytes(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 4);
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

fn write_pcm(path: &Path, samples: &[f32]) -> Result<(), Error> {
    std::fs::write(path, pcm_bytes(samples)).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn read_pcm(path: &Path) -> Result<Vec<u8>, Error> {
    std::fs::read(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn pcm_root() -> Result<PathBuf, Error> {
    crate::settings_dir()
        .map(|dir| dir.join("complock-generated"))
        .ok_or(Error::NoSettings)
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    out
}
