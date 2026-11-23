//! Per-flow allocation ceilings (docs/proof-plan.md, "One allocation
//! profile per flow"): each user flow runs under this binary's counting
//! allocator and must stay under its recorded ceiling. A new allocation
//! above a ceiling fails the test; the author removes it or raises the
//! ceiling with a written reason saying what the bytes are.
//!
//! The flows are the real ones the UI drives — `install_local_with` of an
//! archive, `set_pak` on a grouped library, interchange export and import
//! — on the same fixtures as the flow tests in this directory, not
//! synthetic loops. Each flow runs once as an unmeasured warmup (priming
//! lazy statics, so a ceiling measures the flow, not first-time
//! initialization), then [`RUNS`] measured times, and the worst run is
//! compared. Totals are compared, not live bytes: free timing varies
//! between runs, totals do not (see `shiv_core::alloc_count`).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use shiv_core::alloc_count::{self, CountingAlloc};
use shiv_core::game::GamePaths;
use shiv_core::install;
use shiv_core::interchange;
use shiv_core::sidecar::{self, Sidecar};
use tempfile::{TempDir, tempdir};
use zip::write::SimpleFileOptions;

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

/// The counter is process-global, so the measured tests take turns: while
/// one holds this, the others are parked on it, not allocating into the
/// run being measured.
static FLOW_TURN: Mutex<()> = Mutex::new(());

/// Measured runs per flow; the worst one is compared against the ceiling,
/// so a lucky run cannot flake the gate.
const RUNS: usize = 3;

/// A minimal VPK: the v2 signature followed by some payload bytes.
const VPK: &[u8] = b"\x34\x12\xaa\x55\x02\x00\x00\x00payload";

/// Ceiling: 620,288 = 594,965 observed 2026-11-21 (Apple M1 Pro, macOS,
/// debug profile) + the same fixture-variance slack as before. The step
/// up from 70,691 is the extraction's two 256 KiB buffers (write staging
/// and the zip read buffer, `install.rs` `ENTRY_COPY_BYTES`): writing
/// archive entries in 8 KiB pieces cost tens of thousands of small file
/// writes on a large archive, and staging them measured 2.2× faster end
/// to end (docs/perf-results.md, issue #10).
const INSTALL_LOCAL_ZIP_CEILING: u64 = 620_288;

/// Ceiling: 80,000 = 57,957 observed 2026-11-21 (Apple M1 Pro, macOS,
/// debug profile) + slack for fixture variance (temp-path lengths differ
/// between machines; run-to-run totals drift a few bytes).
const SET_PAK_25_MODS_CEILING: u64 = 80_000;

/// Ceiling: 36,000 = 26,162 observed 2026-11-21 (Apple M1 Pro, macOS,
/// debug profile) + slack for fixture variance (temp-path lengths differ
/// between machines; run-to-run totals drift a few bytes).
const INTERCHANGE_EXPORT_3_MODS_CEILING: u64 = 36_000;

/// Ceiling: 72,000 = 53,464 observed 2026-11-21 (Apple M1 Pro, macOS,
/// debug profile) + slack for fixture variance (temp-path lengths differ
/// between machines; run-to-run totals drift a few bytes).
const INTERCHANGE_IMPORT_3_MODS_CEILING: u64 = 72_000;

struct Env {
    _root: TempDir,
    paths: GamePaths,
    work: PathBuf,
}

fn env() -> Env {
    let root = tempdir().unwrap();
    let paths = GamePaths::from_root(root.path().join("Deadlock"));
    fs::create_dir_all(&paths.addons).unwrap();
    let work = root.path().join("work");
    fs::create_dir_all(&work).unwrap();
    Env {
        _root: root,
        paths,
        work,
    }
}

/// The bytes `flow` allocated, plus its result. Fixture setup before this
/// call is not counted; the flow's own allocations are.
fn alloc_measured<T>(flow: impl FnOnce() -> T) -> (u64, T) {
    alloc_count::reset_allocated_bytes();
    let done = flow();
    (alloc_count::allocated_bytes(), done)
}

/// One unmeasured warmup run of `flow`, then [`RUNS`] measured runs; the
/// worst bytes each measured part of the flow reported.
fn worst_bytes<const MEASURED: usize>(flow: impl Fn() -> [u64; MEASURED]) -> [u64; MEASURED] {
    flow();
    let mut worst = [0; MEASURED];
    for _ in 0..RUNS {
        let bytes = flow();
        for (worst, bytes) in worst.iter_mut().zip(bytes) {
            *worst = (*worst).max(bytes);
        }
    }
    worst
}

fn assert_under_ceiling(bytes: u64, ceiling: u64, flow: &str) {
    assert!(
        bytes <= ceiling,
        "{flow} allocated {bytes} bytes over {RUNS} runs, above the {ceiling}-byte ceiling; \
         remove the allocation or raise the ceiling with a written reason"
    );
}

/// A real VPK at `out` holding `inner` files, each containing `tag`.
fn vpk(out: &Path, inner: &[&str], tag: &str) {
    let src = tempdir().unwrap();
    for p in inner {
        let f = src.path().join(p);
        fs::create_dir_all(f.parent().unwrap()).unwrap();
        fs::write(f, tag).unwrap();
    }
    vpkmanager::pack_directory(src.path(), out).unwrap();
}

/// Install a local mod named `name` from a VPK holding `inner` files.
fn install_mod(e: &Env, name: &str, inner: &[&str], tag: &str) -> Sidecar {
    let file = e.work.join(format!("{}.vpk", name.replace(' ', "_")));
    vpk(&file, inner, tag);
    install::install_local_with(
        &file,
        name,
        &e.paths,
        || false,
        shiv_core::install::InstallGuard::acquire(&e.paths.addons)
            .unwrap()
            .permit(),
    )
    .unwrap()
}

fn install_local_zip_flow() -> [u64; 1] {
    let e = env();
    let src = e.work.join("mod.zip");
    let mut z = zip::ZipWriter::new(fs::File::create(&src).unwrap());
    z.start_file("inner/whatever.vpk", SimpleFileOptions::default())
        .unwrap();
    z.write_all(VPK).unwrap();
    z.finish().unwrap();

    let (bytes, sc) = alloc_measured(|| {
        install::install_local_with(
            &src,
            "Zipped",
            &e.paths,
            || false,
            shiv_core::install::InstallGuard::acquire(&e.paths.addons)
                .unwrap()
                .permit(),
        )
        .unwrap()
    });
    assert_eq!(sc.vpks, ["pak01_dir.vpk"]);
    assert!(sidecar::is_local(&sc.mod_id));
    [bytes]
}

fn sidecar_claiming(id: &str, vpk: &str) -> Sidecar {
    Sidecar {
        mod_id: id.into(),
        name: format!("Mod {id}"),
        version: Some("1.0".into()),
        thumbnail_url: None,
        vpks: vec![vpk.into()],
        nsfw: None,
        hero: None,
        category: None,
        notes: None,
        source: None,
        stamps: None,
        unverified: None,
        generated: None,
    }
}

fn set_pak_flow() -> [u64; 1] {
    let dir = tempdir().unwrap();
    for i in 1..=25u32 {
        let vpk = format!("pak{i:02}_dir.vpk");
        fs::write(dir.path().join(&vpk), vpk.as_bytes()).unwrap();
        sidecar::write(dir.path(), &sidecar_claiming(&format!("m{i:02}"), &vpk)).unwrap();
    }
    let mods = sidecar::group(dir.path());

    let (bytes, ()) = alloc_measured(|| {
        sidecar::set_pak(
            dir.path(),
            &mods,
            mods.len() - 1,
            1,
            shiv_core::install::InstallGuard::acquire(dir.path())
                .unwrap()
                .permit(),
        )
        .unwrap()
    });
    assert_eq!(
        sidecar::read(dir.path(), "m25").unwrap().unwrap().vpks,
        ["pak01_dir.vpk"]
    );
    assert_eq!(
        sidecar::read(dir.path(), "m01").unwrap().unwrap().vpks,
        ["pak02_dir.vpk"]
    );
    [bytes]
}

fn interchange_round_trip_flow() -> [u64; 2] {
    let source = env();
    install_mod(&source, "First", &["a/1.vmat_c"], "1");
    install_mod(&source, "Second", &["b/2.vmat_c"], "2");
    install_mod(&source, "Third", &["c/3.vmat_c"], "3");
    fs::rename(
        source.paths.addons.join("pak02_dir.vpk"),
        source.paths.addons.join("pak02_dir.vpk.disabled"),
    )
    .unwrap();
    let dest = source.work.join("exports");
    fs::create_dir_all(&dest).unwrap();

    let (export_bytes, report) =
        alloc_measured(|| interchange::export(&source.paths.addons, &dest, &|_| None).unwrap());
    let target = env();
    let (import_bytes, imported) = alloc_measured(|| {
        interchange::import(&report.root, &target.paths, || false, &|_| None).unwrap()
    });
    assert_eq!(report.exported.len(), 3);
    assert_eq!(imported.installed.len(), 3);
    let names: Vec<String> = sidecar::group(&target.paths.addons)
        .iter()
        .map(|m| m.sidecar.as_ref().unwrap().name.clone())
        .collect();
    assert_eq!(names, ["First", "Second", "Third"]);
    [export_bytes, import_bytes]
}

#[test]
fn install_local_with_of_a_small_zip_stays_under_its_ceiling() {
    let _turn = FLOW_TURN.lock().unwrap();
    let [worst] = worst_bytes(install_local_zip_flow);
    assert_under_ceiling(
        worst,
        INSTALL_LOCAL_ZIP_CEILING,
        "install_local_with of a one-VPK zip",
    );
}

#[test]
fn set_pak_on_a_25_mod_library_stays_under_its_ceiling() {
    let _turn = FLOW_TURN.lock().unwrap();
    let [worst] = worst_bytes(set_pak_flow);
    assert_under_ceiling(
        worst,
        SET_PAK_25_MODS_CEILING,
        "set_pak of the last of 25 mods to pak 1",
    );
}

#[test]
fn interchange_export_and_import_stay_under_their_ceilings() {
    let _turn = FLOW_TURN.lock().unwrap();
    let [export, import] = worst_bytes(interchange_round_trip_flow);
    assert_under_ceiling(
        export,
        INTERCHANGE_EXPORT_3_MODS_CEILING,
        "interchange export of a 3-mod library",
    );
    assert_under_ceiling(
        import,
        INTERCHANGE_IMPORT_3_MODS_CEILING,
        "interchange import of a 3-mod bundle",
    );
}
