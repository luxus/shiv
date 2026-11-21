//! The interchange import's streaming proof (docs/proof-plan.md,
//! "Interchange import streams"): a multi-mod bundle imports with total
//! allocation within 1.2× its largest VPK — the manifest is the only file
//! read whole, hashing (the adoption path) is chunked, the VPKs are
//! copied through the kernel's own buffer and renamed into place, and
//! nothing ever holds a whole VPK in memory. A path that re-buffers one
//! VPK allocates at least its size and fails the gate.
//!
//! The counting allocator stands in for peak RSS, as in
//! `tests/alloc_ceilings.rs`: total bytes handed out during the import
//! (frees are not subtracted), worst of three measured runs on fresh
//! libraries after one unmeasured warmup priming lazy statics.
//!
//! Fixture sizes are chosen so the gate discriminates, not for realism
//! alone: 90 mods put the import's per-mod bookkeeping (manifest parse,
//! paths, sidecars, the ledger) near a visible share of the largest VPK,
//! so a re-buffered VPK pushes the total past 1.2× while the streaming
//! path stays far below. 90 is also near the realizable domain limit for
//! a fresh import — every imported VPK takes a pak slot, and the engine
//! mounts at most 99 per search path (`addons::PAK_CAPACITY`), the same
//! bound the reorder bench records.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use shiv_core::alloc_count::{self, CountingAlloc};
use shiv_core::game::GamePaths;
use shiv_core::interchange;
use shiv_core::sidecar;
use tempfile::{TempDir, tempdir};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

/// The counter is process-global, so the measured tests take turns: while
/// one holds this, the others are parked on it, not allocating into the
/// run being measured.
static FLOW_TURN: Mutex<()> = Mutex::new(());

/// Measured runs per flow; the worst one is compared against the gate, so
/// a lucky run cannot flake it.
const RUNS: usize = 3;

/// Mods in the bundle. Every one imports as its own mod with one VPK, so
/// the count is bounded by the pak capacity (see the module docs).
const MODS: usize = 90;

/// The largest VPK's inner payload. 12 MiB against the observed worst
/// runs (2026-11-21, Apple M1 Pro, debug profile: ~5.0 MB installing,
/// ~7.2 MB adopting) is a ratio of ~0.4/0.6: far under the 1.2× gate on
/// the streaming path, while a single re-buffered VPK (the gate's
/// failure shape) adds the full 12 MiB and lands past 1.5×.
const LARGEST_PAYLOAD: usize = 12 * 1024 * 1024;

/// The import's allocation gate: 1.2× the largest VPK's size on disk.
const RATIO_NUMERATOR: u64 = 6;
const RATIO_DENOMINATOR: u64 = 5;

/// A fresh library under `root`: the game paths with an empty addons
/// folder, as the Env pattern in `tests/interchange.rs` builds them.
fn library(root: &Path) -> GamePaths {
    let paths = GamePaths::from_root(root.join("Deadlock"));
    fs::create_dir_all(&paths.addons).unwrap();
    paths
}

/// A deterministic byte pattern (the catalog bench's LCG shape): sizes
/// stay stable no matter how the VPK writer stores the payload.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }

    fn fill(&mut self, out: &mut [u8]) {
        for chunk in out.chunks_mut(8) {
            let n = self.next().to_le_bytes();
            chunk.copy_from_slice(&n[..chunk.len()]);
        }
    }
}

/// A real VPK at `out` holding one inner file of `size` bytes.
fn vpk_of_size(out: &Path, size: usize) {
    let src = tempdir().unwrap();
    let file = src.path().join("materials/payload.vmat_c");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    let mut bytes = vec![0u8; size];
    Lcg(0x5eed_0000 + size as u64).fill(&mut bytes);
    fs::write(file, bytes).unwrap();
    vpkmanager::pack_directory(src.path(), out).unwrap();
}

/// The inner payload of mod `i`: the last carries the largest, the others
/// distinct sizes so the adoption path's same-size search never hashes
/// every candidate for every file.
fn payload_of(i: usize) -> usize {
    if i + 1 == MODS {
        LARGEST_PAYLOAD
    } else {
        1024 + i * 17
    }
}

/// The synthetic bundle: `MODS` mods, each one VPK under `files/<i>/`,
/// and its manifest. Returns the bundle folder (inside the returned
/// tempdir, which must outlive the import) and the largest VPK's size on
/// disk, which the gate is measured against.
fn bundle() -> (TempDir, PathBuf, u64) {
    let root = tempdir().unwrap();
    let dir = root.path().join("bundle");
    let mut largest = 0;
    for i in 0..MODS {
        let name = format!("m{i:03}.vpk");
        let dir_i = dir.join("files").join(format!("{i:04}"));
        fs::create_dir_all(&dir_i).unwrap();
        vpk_of_size(&dir_i.join(&name), payload_of(i));
        largest = largest.max(fs::metadata(dir_i.join(&name)).unwrap().len());
    }
    let mut mods = Vec::new();
    for i in 0..MODS {
        mods.push(serde_json::json!({
            "key": format!("local:sha256:probe-{i}"),
            "name": format!("Probe Mod {i}"),
            "enabled": true,
            "order": i,
            "files": [{
                "name": format!("m{i:03}.vpk"),
                "path": format!("files/{i:04}/m{i:03}.vpk"),
                "selected": true,
            }],
        }));
    }
    let doc = serde_json::json!({
        "format": interchange::FORMAT,
        "version": 1,
        "mods": mods,
    });
    fs::write(
        dir.join(interchange::MANIFEST),
        serde_json::to_string_pretty(&doc).unwrap(),
    )
    .unwrap();
    (root, dir, largest)
}

/// Copy every bundle VPK into `addons` as hand-added files: the state the
/// adoption path matches by size and hash.
fn copy_bundle_files_into(bundle_dir: &Path, addons: &Path) {
    for i in 0..MODS {
        let from = bundle_dir
            .join("files")
            .join(format!("{i:04}"))
            .join(format!("m{i:03}.vpk"));
        let to = addons.join(format!("hand-{i:03}.vpk"));
        fs::copy(&from, &to).unwrap();
    }
}

/// The bytes `flow` allocated, plus its result. Fixture setup before this
/// call is not counted; the flow's own allocations are.
fn alloc_measured<T>(flow: impl FnOnce() -> T) -> (u64, T) {
    alloc_count::reset_allocated_bytes();
    let done = flow();
    (alloc_count::allocated_bytes(), done)
}

#[test]
fn importing_a_multi_mod_bundle_stays_within_its_largest_vpk() {
    let _turn = FLOW_TURN.lock().unwrap();
    let (_root, bundle_dir, largest) = bundle();

    let warmup = tempdir().unwrap();
    let warmup_paths = library(warmup.path());
    interchange::import(&bundle_dir, &warmup_paths, || false, &|_| None).unwrap();
    let mut worst_install = 0;
    let mut worst_adopt = 0;
    for _ in 0..RUNS {
        let install_target = tempdir().unwrap();
        let install_paths = library(install_target.path());
        let (bytes, installed) = alloc_measured(|| {
            interchange::import(&bundle_dir, &install_paths, || false, &|_| None).unwrap()
        });
        assert_eq!(installed.installed.len(), MODS);
        assert!(installed.adopted.is_empty() && installed.skipped.is_empty());
        assert_eq!(sidecar::group(&install_paths.addons).len(), MODS);
        worst_install = worst_install.max(bytes);

        let adopt_target = tempdir().unwrap();
        let adopt_paths = library(adopt_target.path());
        copy_bundle_files_into(&bundle_dir, &adopt_paths.addons);
        let (bytes, adopted) = alloc_measured(|| {
            interchange::import(&bundle_dir, &adopt_paths, || false, &|_| None).unwrap()
        });
        assert_eq!(adopted.adopted.len(), MODS);
        assert!(adopted.installed.is_empty() && adopted.skipped.is_empty());
        assert_eq!(sidecar::group(&adopt_paths.addons).len(), MODS);
        worst_adopt = worst_adopt.max(bytes);
    }

    let limit = largest * RATIO_NUMERATOR / RATIO_DENOMINATOR;
    assert!(
        worst_install <= limit,
        "importing the {MODS}-mod bundle allocated {worst_install} bytes over {RUNS} runs, \
         above 1.2× the largest VPK ({largest} bytes): something re-buffered a VPK instead \
         of streaming it (the manifest is the only whole-file read, hashing is chunked, \
         copies go through the kernel's buffer, placement renames)"
    );
    assert!(
        worst_adopt <= limit,
        "adopting the {MODS}-mod bundle in place allocated {worst_adopt} bytes over {RUNS} \
         runs, above 1.2× the largest VPK ({largest} bytes): the adoption hashes are chunked \
         (64 KiB reads) and must never hold a VPK in memory"
    );
}
