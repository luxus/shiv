//! Which pak numbers a new mod may take.
//!
//! [`occupied`] is the only set of numbers that are not free. [`free_slots`]
//! is the only way to take some for a new mod.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use crate::addons::{self, AddonsError, parse_pak};
use crate::sidecar;

pub(crate) fn occupied(addons: &Path) -> Result<BTreeSet<u32>, AddonsError> {
    let mut used: BTreeSet<u32> = addons::file_pak_numbers(addons)?.into_iter().collect();
    let Ok(entries) = fs::read_dir(addons.join(sidecar::DIR)) else {
        return Ok(used);
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(id) = name.to_str().and_then(|n| n.strip_suffix(".json")) else {
            continue;
        };
        if let Ok(Some(sc)) = sidecar::read(addons, id) {
            used.extend(sc.vpks.iter().filter_map(|v| parse_pak(v).map(|(n, _)| n)));
        }
    }
    Ok(used)
}

pub(crate) fn free_slots(addons: &Path, count: usize) -> Result<Vec<u32>, AddonsError> {
    let used = occupied(addons)?;
    let reserved = complock_reserved_slots(addons);
    let mut free: Vec<u32> = (reserved + 1..=addons::PAK_CAPACITY)
        .chain(1..=reserved)
        .filter(|n| !used.contains(n))
        .take(count)
        .collect();
    if free.len() < count {
        return Err(AddonsError::Full {
            dir: addons.to_path_buf(),
        });
    }
    free.sort_unstable();
    Ok(free)
}

fn complock_reserved_slots(addons_dir: &Path) -> u32 {
    let Ok(entries) = fs::read_dir(addons_dir.join(sidecar::DIR)) else {
        return 0;
    };
    let installed = entries.flatten().any(|entry| {
        entry.file_name().to_str().is_some_and(|name| {
            name.starts_with(crate::complock::MOD_PREFIX) && name.ends_with(".json")
        })
    });
    if installed {
        crate::complock::last_slot().min(addons::PAK_CAPACITY)
    } else {
        0
    }
}
