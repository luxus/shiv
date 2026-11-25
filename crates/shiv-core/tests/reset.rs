//! Reset to release and Restore previous (issue #21), over real installs in
//! a temp addons folder: the acceptance path — a pak replaced by any VPK
//! shows Modified, reset brings back the release bytes, the replaced file
//! sits in the backup, and Restore previous swaps it back.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use shiv_core::drift::{self, Drift, Published};
use shiv_core::game::GamePaths;
use shiv_core::install::{self, ModMeta};
use shiv_core::reset::{self, ResetError};
use shiv_core::{addons, sidecar};
use tempfile::{TempDir, tempdir};

struct Env {
    _root: TempDir,
    paths: GamePaths,
    release: Vec<PathBuf>,
}

/// A temp game with mod `m` installed from its release (one dir VPK with a
/// chunk), moved to pak 05, and a hand-added mod on pak 01 so slots matter.
fn env() -> Env {
    let root = tempdir().unwrap();
    let paths = GamePaths::from_root(root.path().join("Deadlock"));
    fs::create_dir_all(&paths.addons).unwrap();
    let work = root.path().join("release");
    fs::create_dir_all(&work).unwrap();
    let dir = work.join("pak01_dir.vpk");
    vpk(&dir, "the release");
    let chunk = work.join("pak01_000.vpk");
    fs::write(&chunk, "release chunk").unwrap();
    let release = vec![dir, chunk];
    install::install_files(
        &release,
        meta(),
        &paths,
        || false,
        hold(&paths.addons).permit(),
    )
    .unwrap();
    let mods = sidecar::group(&paths.addons);
    let index = index_of(&mods, "m");
    sidecar::set_pak(&paths.addons, &mods, index, 5, hold(&paths.addons).permit()).unwrap();
    vpk(&paths.addons.join("pak01_dir.vpk"), "someone else's mod");
    Env {
        _root: root,
        paths,
        release,
    }
}

fn meta() -> ModMeta {
    ModMeta {
        mod_id: "m".into(),
        name: "M".into(),
        version: Some("r1".into()),
        version_label: None,
        thumbnail_url: None,
        nsfw: false,
        hero: None,
        category: None,
    }
}

fn vpk(out: &Path, tag: &str) {
    let src = tempdir().unwrap();
    fs::create_dir_all(src.path().join("panorama")).unwrap();
    fs::write(src.path().join("panorama/tag.txt"), tag).unwrap();
    vpkmanager::pack_directory(src.path(), out).unwrap();
}

fn index_of(mods: &[sidecar::InstalledMod], id: &str) -> usize {
    mods.iter()
        .position(|m| m.sidecar.as_ref().is_some_and(|s| s.mod_id == id))
        .unwrap()
}

fn the_mod(e: &Env) -> sidecar::InstalledMod {
    let mods = sidecar::group(&e.paths.addons);
    let i = index_of(&mods, "m");
    mods.into_iter().nth(i).unwrap()
}

fn state(e: &Env) -> Drift {
    drift::drift(&the_mod(e), &Published::Current(["r1".to_owned()].into()))
}

/// The pak files of addons (no `.shiv`), name to bytes.
fn paks(e: &Env) -> BTreeMap<String, Vec<u8>> {
    fs::read_dir(&e.paths.addons)
        .unwrap()
        .map(|d| d.unwrap().path())
        .filter(|p| p.is_file())
        .map(|p| {
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                fs::read(&p).unwrap(),
            )
        })
        .collect()
}

fn hold(addons: &std::path::Path) -> install::InstallGuard {
    install::InstallGuard::acquire(addons).unwrap()
}

fn reinstall<'a>(
    e: &'a Env,
    permit: install::InstallPermit<'a>,
) -> impl FnOnce() -> Result<(), String> + 'a {
    move || {
        install::install_files(&e.release, meta(), &e.paths, || false, permit)
            .map(|_| ())
            .map_err(|err| err.to_string())
    }
}

/// Replace the mod's dir VPK with another build: the hand-patched case.
fn tamper(e: &Env) -> Vec<u8> {
    let file = the_mod(e).vpks[0].file.clone();
    vpk(&file, "a hand-patched test build");
    fs::read(file).unwrap()
}

#[test]
fn reset_restores_the_release_in_its_slot_and_keeps_the_replaced_file() {
    let e = env();
    let release_bytes = paks(&e);
    assert_eq!(state(&e), Drift::Pristine);
    let patched = tamper(&e);
    assert_eq!(state(&e), Drift::Modified);

    let guard = hold(&e.paths.addons);
    let backup = reset::reset_with(
        &e.paths,
        "m",
        || false,
        reinstall(&e, guard.permit()),
        guard.permit(),
    )
    .unwrap();
    assert_eq!(state(&e), Drift::Pristine, "the release is back");
    assert_eq!(
        paks(&e),
        release_bytes,
        "same bytes, same slot (pak05), the other mod untouched"
    );
    assert_eq!(the_mod(&e).load_position(), 5);
    assert_eq!(fs::read(backup.dir.join("pak05_dir.vpk")).unwrap(), patched);
    assert!(
        backup.dir.join("pak05_000.vpk").is_file(),
        "the chunk went with it"
    );
    assert_eq!(reset::latest_backup(&e.paths.addons, "m"), Some(backup));
}

#[test]
fn restore_previous_swaps_the_backup_back_and_forth() {
    let e = env();
    let release_bytes = paks(&e);
    let patched = tamper(&e);
    let guard = hold(&e.paths.addons);
    reset::reset_with(
        &e.paths,
        "m",
        || false,
        reinstall(&e, guard.permit()),
        guard.permit(),
    )
    .unwrap();
    drop(guard);

    reset::restore_previous(&e.paths, "m", || false, hold(&e.paths.addons).permit()).unwrap();
    assert_eq!(
        paks(&e)["pak05_dir.vpk"],
        patched,
        "the patched build is back"
    );
    assert_eq!(
        state(&e),
        Drift::Modified,
        "and still differs from the release"
    );

    reset::restore_previous(&e.paths, "m", || false, hold(&e.paths.addons).permit()).unwrap();
    assert_eq!(
        paks(&e),
        release_bytes,
        "a second restore swaps back to the release"
    );
    assert_eq!(state(&e), Drift::Pristine);
}

#[test]
fn a_running_game_refuses_both_before_anything_moves() {
    let e = env();
    tamper(&e);
    let before = paks(&e);
    let called = std::cell::Cell::new(false);
    let guard = hold(&e.paths.addons);
    let refused = reset::reset_with(
        &e.paths,
        "m",
        || true,
        || {
            called.set(true);
            Ok(())
        },
        guard.permit(),
    );
    drop(guard);
    assert!(matches!(refused, Err(ResetError::GameRunning)));
    assert!(!called.get(), "the install never ran");
    assert_eq!(paks(&e), before);
    assert!(reset::latest_backup(&e.paths.addons, "m").is_none());

    let guard = hold(&e.paths.addons);
    reset::reset_with(
        &e.paths,
        "m",
        || false,
        reinstall(&e, guard.permit()),
        guard.permit(),
    )
    .unwrap();
    drop(guard);
    let after_reset = paks(&e);
    assert!(matches!(
        reset::restore_previous(&e.paths, "m", || true, hold(&e.paths.addons).permit()),
        Err(ResetError::GameRunning)
    ));
    assert_eq!(paks(&e), after_reset);
}

#[test]
fn a_failed_install_puts_the_files_back() {
    let e = env();
    tamper(&e);
    let before = paks(&e);
    let guard = hold(&e.paths.addons);
    let failed = reset::reset_with(
        &e.paths,
        "m",
        || false,
        || Err("404 from the source".into()),
        guard.permit(),
    );
    assert!(matches!(failed, Err(ResetError::Install(ref m)) if m.contains("404")));
    assert_eq!(paks(&e), before);
    assert_eq!(state(&e), Drift::Modified);
    assert!(
        reset::latest_backup(&e.paths.addons, "m").is_none(),
        "nothing kept: nothing replaced"
    );
}

#[test]
fn a_disabled_mod_stays_disabled_and_a_taken_slot_refuses_the_restore() {
    let e = env();
    for v in &the_mod(&e).vpks {
        addons::set_enabled(v, false).unwrap();
    }
    let guard = hold(&e.paths.addons);
    reset::reset_with(
        &e.paths,
        "m",
        || false,
        reinstall(&e, guard.permit()),
        guard.permit(),
    )
    .unwrap();
    drop(guard);
    let m = the_mod(&e);
    assert!(m.vpks.iter().all(|v| !v.enabled), "reset keeps it disabled");
    assert_eq!(m.load_position(), 5);

    for v in &m.vpks {
        addons::set_enabled(v, true).unwrap();
    }
    let mods = sidecar::group(&e.paths.addons);
    sidecar::set_pak(
        &e.paths.addons,
        &mods,
        index_of(&mods, "m"),
        9,
        hold(&e.paths.addons).permit(),
    )
    .unwrap();
    let backup = reset::latest_backup(&e.paths.addons, "m").unwrap();
    let old_name = backup.files[0].trim_end_matches(".disabled").to_owned();
    vpk(&e.paths.addons.join(&old_name), "a newcomer took the slot");
    fs::rename(
        e.paths.addons.join(&old_name),
        e.paths.addons.join(&backup.files[0]),
    )
    .unwrap();
    let before = paks(&e);
    assert!(matches!(
        reset::restore_previous(&e.paths, "m", || false, hold(&e.paths.addons).permit()),
        Err(ResetError::SlotTaken(_))
    ));
    assert_eq!(paks(&e), before, "nothing moved");
}

#[test]
fn restore_without_a_backup_says_so() {
    let e = env();
    assert!(matches!(
        reset::restore_previous(&e.paths, "m", || false, hold(&e.paths.addons).permit()),
        Err(ResetError::NoBackup(_))
    ));
    let guard = hold(&e.paths.addons);
    assert!(matches!(
        reset::reset_with(&e.paths, "nobody", || false, || Ok(()), guard.permit()),
        Err(ResetError::NotFound(_))
    ));
}

/// The acceptance run against a real registry release, in a temp game
/// folder: `SHIV_REGISTRY_ARTIFACT` is the downloaded artifact,
/// `SHIV_REGISTRY_SHA256` the sha256 the registry publishes for it,
/// `SHIV_FOREIGN_VPK` any other VPK to drop in its place.
#[test]
#[ignore = "needs a downloaded registry artifact (see the doc comment)"]
fn a_real_registry_release_resets_and_restores() {
    use shiv_core::registry;
    let artifact = PathBuf::from(std::env::var("SHIV_REGISTRY_ARTIFACT").unwrap());
    let sha = std::env::var("SHIV_REGISTRY_SHA256").unwrap();
    let foreign = PathBuf::from(std::env::var("SHIV_FOREIGN_VPK").unwrap());
    let root = tempdir().unwrap();
    let paths = GamePaths::from_root(root.path().join("Deadlock"));
    fs::create_dir_all(&paths.addons).unwrap();
    let source = registry::AddedSource {
        descriptor_url: "https://luxus.github.io/deadlock-mods/dmm-source.json".into(),
        origin: "https://luxus.github.io".into(),
        source_id: "luxus".into(),
        name: "luxus Deadlock Mods".into(),
        api_origins: vec!["https://luxus.github.io".into()],
        download_origins: vec!["https://github.com".into()],
    };
    let install = |paths: &GamePaths, permit: install::InstallPermit<'_>| {
        let copy = root.path().join("download.vpk");
        fs::copy(&artifact, &copy).unwrap();
        registry::verify_download(&copy, Some(&sha)).map_err(|e| e.to_string())?;
        registry::install_registry_mod(
            paths,
            &source,
            "improved-quickbuy",
            "Improved Quickbuy",
            "sha256-57dc1d4fb2f81d33",
            Some("1.0.0"),
            None,
            false,
            "improved-quickbuy-1.0.0.vpk",
            &copy,
            true,
            permit,
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
    };
    let guard = hold(&paths.addons);
    install(&paths, guard.permit()).unwrap();
    drop(guard);
    let id = source.mod_id_of("improved-quickbuy");
    let published = Published::Current(["sha256-57dc1d4fb2f81d33".to_owned()].into());
    let find = || {
        sidecar::group(&paths.addons)
            .into_iter()
            .find(|m| m.sidecar.as_ref().is_some_and(|s| s.mod_id == id))
            .unwrap()
    };
    let sha_of = |p: &Path| {
        use sha2::Digest;
        format!("{:x}", sha2::Sha256::digest(fs::read(p).unwrap()))
    };
    assert_eq!(drift::drift(&find(), &published), Drift::Pristine);
    let installed = find().vpks[0].file.clone();
    assert_eq!(
        sha_of(&installed),
        sha,
        "a plain-vpk artifact lands byte for byte"
    );
    fs::copy(&foreign, &installed).unwrap();
    assert_eq!(drift::drift(&find(), &published), Drift::Modified);
    println!(
        "replaced {} with {} -> Modified",
        installed.display(),
        foreign.display()
    );

    let guard = hold(&paths.addons);
    let backup = reset::reset_with(
        &paths,
        &id,
        || false,
        || install(&paths, guard.permit()),
        guard.permit(),
    )
    .unwrap();
    drop(guard);
    let back = find().vpks[0].file.clone();
    assert_eq!(drift::drift(&find(), &published), Drift::Pristine);
    assert_eq!(sha_of(&back), sha);
    let kept = backup.dir.join(&backup.files[0]);
    assert_eq!(sha_of(&kept), sha_of(&foreign));
    println!(
        "reset -> Pristine, sha256 {sha}; the foreign VPK is kept in {}",
        backup.dir.display()
    );

    reset::restore_previous(&paths, &id, || false, hold(&paths.addons).permit()).unwrap();
    assert_eq!(sha_of(&find().vpks[0].file), sha_of(&foreign));
    assert_eq!(drift::drift(&find(), &published), Drift::Modified);
    println!("restore previous -> the foreign VPK is back, Modified");
}
