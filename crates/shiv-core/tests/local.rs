//! Local mods: installing a VPK or archive from disk, adopting hand-added
//! paks in place, and editing a mod's name and notes.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use shiv_core::game::GamePaths;
use shiv_core::install;
use shiv_core::sidecar::{self, SidecarError};
use tempfile::{TempDir, tempdir};
use zip::write::SimpleFileOptions;

/// A minimal VPK: the v2 signature followed by some payload bytes.
const VPK: &[u8] = b"\x34\x12\xaa\x55\x02\x00\x00\x00payload";

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

/// File name to bytes for every regular file directly in `dir`.
fn files(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter(|e| e.file_type().unwrap().is_file())
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                fs::read(e.path()).unwrap(),
            )
        })
        .collect()
}

#[test]
fn bare_vpk_installs_into_a_free_slot_and_keeps_the_source() {
    let e = env();
    fs::write(e.paths.addons.join("pak01_dir.vpk"), b"theirs").unwrap();
    let src = e.work.join("Thicker Outlines.vpk");
    fs::write(&src, VPK).unwrap();

    let sc = install::install_local_with(
        &src,
        "",
        &e.paths,
        || false,
        shiv_core::install::InstallGuard::acquire().permit(),
    )
    .unwrap();

    assert!(sidecar::is_local(&sc.mod_id), "{}", sc.mod_id);
    assert_eq!(
        sc.name, "Thicker Outlines",
        "empty name falls back to the file stem"
    );
    assert_eq!(sc.vpks, ["pak02_dir.vpk"]);
    let addons = files(&e.paths.addons);
    assert_eq!(addons["pak01_dir.vpk"], b"theirs");
    assert_eq!(addons["pak02_dir.vpk"], VPK);
    assert_eq!(fs::read(&src).unwrap(), VPK, "source file untouched");
    let mods = sidecar::group(&e.paths.addons);
    let mine = mods.iter().find(|m| m.sidecar.is_some()).unwrap();
    assert_eq!(mine.sidecar.as_ref().unwrap().name, "Thicker Outlines");
}

#[test]
fn pak_named_vpk_brings_its_chunks() {
    let e = env();
    let src = e.work.join("pak07_dir.vpk");
    fs::write(&src, VPK).unwrap();
    fs::write(e.work.join("pak07_000.vpk"), b"chunk0").unwrap();
    fs::write(e.work.join("pak08_000.vpk"), b"other").unwrap();

    let sc = install::install_local_with(
        &src,
        "Hitsound",
        &e.paths,
        || false,
        shiv_core::install::InstallGuard::acquire().permit(),
    )
    .unwrap();

    assert_eq!(sc.name, "Hitsound");
    assert_eq!(sc.vpks, ["pak01_dir.vpk"]);
    let addons = files(&e.paths.addons);
    assert_eq!(addons["pak01_000.vpk"], b"chunk0");
    assert!(!addons.contains_key("pak08_000.vpk"));
}

#[test]
fn archive_installs_as_local_mod() {
    let e = env();
    let src = e.work.join("mod.zip");
    let mut z = zip::ZipWriter::new(fs::File::create(&src).unwrap());
    z.start_file("inner/whatever.vpk", SimpleFileOptions::default())
        .unwrap();
    z.write_all(VPK).unwrap();
    z.finish().unwrap();

    let sc = install::install_local_with(
        &src,
        "Zipped",
        &e.paths,
        || false,
        shiv_core::install::InstallGuard::acquire().permit(),
    )
    .unwrap();
    assert!(sidecar::is_local(&sc.mod_id));
    assert_eq!(sc.vpks, ["pak01_dir.vpk"]);
}

#[test]
fn local_install_refuses_while_the_game_runs() {
    let e = env();
    let src = e.work.join("a.vpk");
    fs::write(&src, VPK).unwrap();
    let err = install::install_local_with(
        &src,
        "A",
        &e.paths,
        || true,
        shiv_core::install::InstallGuard::acquire().permit(),
    )
    .unwrap_err();
    assert!(matches!(err, install::InstallError::GameRunning), "{err:?}");
    assert!(files(&e.paths.addons).is_empty());
}

#[test]
fn adopt_records_a_hand_added_pak_without_touching_it() {
    let e = env();
    fs::write(e.paths.addons.join("pak55_dir.vpk"), b"hits").unwrap();
    fs::write(e.paths.addons.join("pak56_dir.vpk.disabled"), b"lines").unwrap();
    let before = files(&e.paths.addons);

    let mods = sidecar::group(&e.paths.addons);
    let hand = mods
        .iter()
        .find(|m| m.vpks[0].file.ends_with("pak56_dir.vpk.disabled"))
        .unwrap();
    let sc = sidecar::adopt(&e.paths.addons, hand, " Outlines ", Some("HoppCX".into())).unwrap();

    assert!(sidecar::is_local(&sc.mod_id));
    assert_eq!(sc.name, "Outlines");
    assert_eq!(sc.notes.as_deref(), Some("HoppCX"));
    assert_eq!(sc.vpks, ["pak56_dir.vpk"], "enabled-form name");
    assert_eq!(files(&e.paths.addons), before, "no pak file changed");

    let after = sidecar::group(&e.paths.addons);
    let adopted = after
        .iter()
        .find(|m| m.sidecar.as_ref().is_some_and(|s| s.mod_id == sc.mod_id))
        .unwrap();
    assert!(!adopted.is_enabled(), "disabled state kept");
    // Adopting a managed mod again is refused.
    let err = sidecar::adopt(&e.paths.addons, adopted, "x", None).unwrap_err();
    assert!(matches!(err, SidecarError::NotAdoptable), "{err:?}");
}

#[test]
fn edit_changes_only_name_and_notes() {
    let e = env();
    let src = e.work.join("a.vpk");
    fs::write(&src, VPK).unwrap();
    let sc = install::install_local_with(
        &src,
        "A",
        &e.paths,
        || false,
        shiv_core::install::InstallGuard::acquire().permit(),
    )
    .unwrap();

    let edited = sidecar::edit(
        &e.paths.addons,
        &sc.mod_id,
        "Better name",
        Some("note".into()),
    )
    .unwrap();
    assert_eq!(edited.name, "Better name");
    assert_eq!(edited.notes.as_deref(), Some("note"));
    assert_eq!(edited.vpks, sc.vpks);
    let cleared = sidecar::edit(
        &e.paths.addons,
        &sc.mod_id,
        "Better name",
        Some("  ".into()),
    )
    .unwrap();
    assert_eq!(cleared.notes, None);
    assert_eq!(
        sidecar::read(&e.paths.addons, &sc.mod_id).unwrap().unwrap(),
        cleared
    );
    let missing = sidecar::edit(&e.paths.addons, "local-0-0", "x", None).unwrap_err();
    assert!(matches!(missing, SidecarError::NotAdoptable), "{missing:?}");
}

#[test]
fn local_ids_do_not_collide() {
    let e = env();
    let a = e.work.join("a.vpk");
    fs::write(&a, VPK).unwrap();
    let one = install::install_local_with(
        &a,
        "A",
        &e.paths,
        || false,
        shiv_core::install::InstallGuard::acquire().permit(),
    )
    .unwrap();
    let two = install::install_local_with(
        &a,
        "B",
        &e.paths,
        || false,
        shiv_core::install::InstallGuard::acquire().permit(),
    )
    .unwrap();
    assert_ne!(one.mod_id, two.mod_id);
    assert_eq!(two.vpks, ["pak02_dir.vpk"]);
}
