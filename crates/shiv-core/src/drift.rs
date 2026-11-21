//! Drift: how an installed mod relates to the release its source publishes
//! (issue #21). Two independent facts make the state: whether the files on
//! disk are still the ones Shiv placed (the sidecar's [`Stamp`]s against the
//! files now), and whether the source still publishes the revision the
//! sidecar records. They are different actions — a modified mod is reset to
//! the release, an outdated one updated — so the state keeps them apart.

use std::collections::BTreeSet;

use crate::sidecar::{InstalledMod, Stamp};

/// What a mod's source publishes now, as far as Shiv can tell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Published {
    /// The revisions the source publishes for this mod today: one for a
    /// registry listing, one per pak variant for a CompLock option. A
    /// sidecar whose `version` is among them is current.
    Current(BTreeSet<String>),
    /// The source no longer publishes the mod: the listing or option is
    /// gone, or the download answers 404.
    Gone,
    /// No source answers for the mod: a local mod, an unreachable source,
    /// or data not loaded yet.
    Unknown,
}

/// An installed mod against its source's release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drift {
    /// The files are the ones Shiv placed and the source publishes their
    /// revision (or cannot be asked).
    Pristine,
    /// The files on disk are not the ones Shiv placed: replaced, patched,
    /// or partly rewritten. Reset brings back the release.
    Modified,
    /// The files are untouched, and the source has moved on to another
    /// revision. Update fetches it.
    UpdateAvailable,
    /// The source no longer publishes the mod: nothing to reset to.
    Retired,
    /// Shiv cannot tell: no sidecar stamps (placed before stamps existed,
    /// adopted, or not Shiv's) and nothing retired.
    Untracked,
}

/// The drift of `m` given what its source publishes. A retired source wins
/// (there is no release to compare with); then a mod without stamps is
/// untracked; then changed files are modified, whatever the source says;
/// then a revision the source no longer lists is an update.
#[must_use]
pub fn drift(m: &InstalledMod, published: &Published) -> Drift {
    if *published == Published::Gone {
        return Drift::Retired;
    }
    let Some(untouched) = files_untouched(m) else {
        return Drift::Untracked;
    };
    if !untouched {
        return Drift::Modified;
    }
    match published {
        Published::Current(revisions) => {
            let current = m
                .sidecar
                .as_ref()
                .and_then(|s| s.version.as_ref())
                .is_some_and(|v| revisions.contains(v));
            if current {
                Drift::Pristine
            } else {
                Drift::UpdateAvailable
            }
        }
        Published::Gone | Published::Unknown => Drift::Pristine,
    }
}

/// Whether every VPK the sidecar names lies on disk with the stamp Shiv
/// recorded when it placed it. `None` when there is nothing to compare: no
/// sidecar, no stamps, or stamps that do not line up with the VPK list. A
/// VPK missing from disk, or a file that cannot be read, is a change.
#[must_use]
pub fn files_untouched(m: &InstalledMod) -> Option<bool> {
    let sidecar = m.sidecar.as_ref()?;
    let stamps = sidecar.stamps.as_ref()?;
    if stamps.len() != sidecar.vpks.len() {
        return None;
    }
    Some(sidecar.vpks.iter().zip(stamps).all(|(name, recorded)| {
        m.vpks
            .iter()
            .find(|v| enabled_name(v) == Some(name.as_str()))
            .and_then(|v| Stamp::of_vpk(v).ok())
            .is_some_and(|now| now == *recorded)
    }))
}

fn enabled_name(vpk: &crate::addons::InstalledVpk) -> Option<&str> {
    let name = vpk.file.file_name()?.to_str()?;
    Some(name.strip_suffix(".disabled").unwrap_or(name))
}
