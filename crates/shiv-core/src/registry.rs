//! Mod registries the user added: native `dmm-source` catalogs beside the
//! built-in one. A registry's identity is the origin of its descriptor URL
//! plus its declared `sourceId`, so the same source cannot be added twice
//! under another URL and a moved registry asks for a fresh review.
//!
//! A mod installed from a registry gets the sidecar id
//! `src:<sourceId>:<listingId>` and the release's `contentRevision` as its
//! version, so the Installed screen's existing update machinery recognises
//! a newer revision exactly like a catalog date.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::addons;
use crate::game::GamePaths;
use crate::install::{self, InstallError};
use crate::sidecar::{self, Sidecar};

pub const ID_PREFIX: &str = "src-";

/// A sidecar id component as the filesystem accepts it: bytes outside
/// `[A-Za-z0-9_]` become `_xx` (two hex digits), `_` itself `__`. The
/// encoding is reversible, so a mod id maps back to its source and
/// listing, and a registry id can never escape `.shiv` or collide with a
/// plain catalog id.
fn encode_id_component(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' => out.push(char::from(byte)),
            b'_' => out.push_str("__"),
            _ => out.push_str(&format!("_{byte:02x}")),
        }
    }
    out
}

fn decode_id_component(encoded: &str) -> Option<String> {
    let bytes = encoded.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'_' {
            match (bytes.get(i + 1), bytes.get(i + 2)) {
                (Some(b'_'), _) => {
                    out.push(b'_');
                    i += 2;
                }
                (Some(_), Some(_)) => {
                    let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
                    out.push(u8::from_str_radix(hex, 16).ok()?);
                    i += 3;
                }
                _ => return None,
            }
        } else if bytes[i].is_ascii_alphanumeric() {
            out.push(bytes[i]);
            i += 1;
        } else {
            return None;
        }
    }
    String::from_utf8(out).ok()
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("{0}")]
    Refused(String),
    #[error("io error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not read the source list: {0}")]
    Parse(#[source] serde_json::Error),
    #[error(transparent)]
    Install(#[from] InstallError),
    #[error(transparent)]
    Sidecar(#[from] sidecar::SidecarError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddedSource {
    pub descriptor_url: String,
    pub origin: String,
    pub source_id: String,
    pub name: String,
    /// The origins the reviewed descriptor declared, kept with the saved
    /// source so an install can check a resolver's answer against exactly
    /// what the user reviewed.
    pub api_origins: Vec<String>,
    pub download_origins: Vec<String>,
}

impl AddedSource {
    #[must_use]
    pub fn api_origins(&self) -> Vec<String> {
        self.api_origins.clone()
    }

    #[must_use]
    pub fn download_origins(&self) -> Vec<String> {
        self.download_origins.clone()
    }

    #[must_use]
    pub fn mod_id_of(&self, listing_id: &str) -> String {
        format!(
            "{ID_PREFIX}{}-{}",
            encode_id_component(&self.source_id),
            encode_id_component(listing_id)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registry {
    path: PathBuf,
    sources: Vec<AddedSource>,
}

impl Registry {
    pub fn load(paths: &GamePaths) -> Result<Self, RegistryError> {
        let path = paths.root.join("shiv-sources.json");
        let sources = match fs::read(&path) {
            Ok(bytes) => {
                serde_json::from_slice::<Vec<AddedSource>>(&bytes).map_err(RegistryError::Parse)?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(source) => return Err(RegistryError::Io { path, source }),
        };
        Ok(Self { path, sources })
    }

    #[must_use]
    pub fn sources(&self) -> &[AddedSource] {
        &self.sources
    }

    #[must_use]
    pub fn find(&self, origin: &str, source_id: &str) -> Option<&AddedSource> {
        self.sources
            .iter()
            .find(|s| s.origin == origin && s.source_id == source_id)
    }

    #[must_use]
    pub fn by_mod_id(&self, mod_id: &str) -> Option<&AddedSource> {
        let (source_id, _) = split_mod_id(mod_id)?;
        self.sources.iter().find(|s| s.source_id == source_id)
    }

    /// Remember a reviewed source. The origin of its descriptor plus the
    /// declared `sourceId` is its identity: the same pair twice is a
    /// duplicate, whatever URL it was reviewed from.
    pub fn add(&mut self, source: AddedSource) -> Result<(), RegistryError> {
        if let Some(existing) = self.find(&source.origin, &source.source_id) {
            return Err(RegistryError::Refused(format!(
                "{} is already added from {}",
                existing.name, existing.descriptor_url
            )));
        }
        self.sources.push(source);
        self.sources.sort_by(|a, b| {
            a.name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase())
        });
        self.save()
    }

    pub fn remove(&mut self, origin: &str, source_id: &str) -> Result<(), RegistryError> {
        let before = self.sources.len();
        self.sources
            .retain(|s| !(s.origin == origin && s.source_id == source_id));
        if self.sources.len() == before {
            return Ok(());
        }
        self.save()
    }

    fn save(&self) -> Result<(), RegistryError> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir).map_err(|source| RegistryError::Io {
                path: dir.to_path_buf(),
                source,
            })?;
        }
        let json = serde_json::to_vec_pretty(&self.sources).map_err(RegistryError::Parse)?;
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, json).map_err(|source| RegistryError::Io {
            path: tmp.clone(),
            source,
        })?;
        fs::rename(&tmp, &self.path).map_err(|source| RegistryError::Io {
            path: self.path.clone(),
            source,
        })
    }
}

/// The source a registry mod id names: `src-<encoded source>-<encoded
/// listing>` splits into its parts; anything else is not a registry mod.
#[must_use]
pub fn split_mod_id(mod_id: &str) -> Option<(String, String)> {
    let body = mod_id.strip_prefix(ID_PREFIX)?;
    let (source, listing) = body.split_once('-')?;
    let source = decode_id_component(source)?;
    let listing = decode_id_component(listing)?;
    Some((source, listing))
}

/// Whether `installed` predates `current`: a registry records opaque
/// content revisions, so any difference is an update. A sidecar without a
/// version predates the registry and updates too.
#[must_use]
pub fn revision_is_newer(installed: Option<&str>, current: &str) -> bool {
    installed.is_none_or(|old| old != current)
}

/// Where a downloaded registry artifact waits for its hash check: a
/// published checksum the bytes must match, or the download is deleted
/// and refused. `None` published means unverifiable, which passes.
pub fn verify_download(
    downloaded: &Path,
    published_sha256: Option<&str>,
) -> Result<(), RegistryError> {
    let Some(expected) = published_sha256 else {
        return Ok(());
    };
    let actual = sha256_hex_of(downloaded)?;
    if actual != expected {
        fs::remove_file(downloaded).ok();
        return Err(RegistryError::Refused(
            "the download does not match the checksum the registry published".into(),
        ));
    }
    Ok(())
}

fn sha256_hex_of(path: &Path) -> Result<String, RegistryError> {
    let bytes = fs::read(path).map_err(|source| RegistryError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

/// Install the downloaded artifact of a registry listing's current release:
/// the sidecar id names the listing, the version records the content
/// revision, and the file name the download came under becomes the
/// `source`, so a hand-pulled copy of the same release is recognised as
/// the same mod.
#[allow(clippy::too_many_arguments)]
pub fn install_registry_mod(
    paths: &GamePaths,
    source: &AddedSource,
    listing_id: &str,
    listing_title: &str,
    content_revision: &str,
    thumbnail_url: Option<&str>,
    nsfw: bool,
    artifact_file_name: &str,
    downloaded: &Path,
    enabled: bool,
) -> Result<Sidecar, RegistryError> {
    let meta = install::ModMeta {
        mod_id: source.mod_id_of(listing_id),
        name: listing_title.to_owned(),
        version: Some(content_revision.to_owned()),
        thumbnail_url: thumbnail_url.map(str::to_owned),
        nsfw,
        hero: None,
        category: Some("registry".to_owned()),
    };
    let sc = install::install_with(downloaded, meta, paths, crate::game::is_running)?;
    if !enabled {
        disable_by_id(paths, &sc.mod_id)?;
    }
    let mut with_source = sc.clone();
    with_source.source = Some(artifact_file_name.to_owned());
    sidecar::write(&paths.addons, &with_source)?;
    Ok(with_source)
}

fn disable_by_id(paths: &GamePaths, mod_id: &str) -> Result<(), RegistryError> {
    let mods = sidecar::group(&paths.addons);
    let Some(installed) = mods
        .iter()
        .find(|m| m.sidecar.as_ref().is_some_and(|s| s.mod_id == mod_id))
    else {
        return Ok(());
    };
    for vpk in &installed.vpks {
        addons::set_enabled(vpk, false).map_err(|e| RegistryError::Refused(e.to_string()))?;
    }
    Ok(())
}

/// The sidecars of the registry mods in `mods`, keyed by listing id.
#[must_use]
pub fn installed_from(mods: &[sidecar::InstalledMod]) -> BTreeMap<String, &Sidecar> {
    mods.iter()
        .filter_map(|m| {
            let s = m.sidecar.as_ref()?;
            let (_, listing) = split_mod_id(&s.mod_id)?;
            Some((listing, s))
        })
        .collect()
}
