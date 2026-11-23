//! Updating a local mod from a file: recognising which installed mod a file
//! is a newer version of, and replacing its files in place with a backup.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use shiv_core::game::GamePaths;
use shiv_core::install::{self, InstallError, MatchReason};
use shiv_core::sidecar::{self, InstalledMod, Sidecar};
use tempfile::{TempDir, tempdir};

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

fn by_file<'a>(mods: &'a [InstalledMod], name: &str) -> &'a InstalledMod {
    mods.iter()
        .find(|m| m.vpks.iter().any(|v| v.file.file_name().unwrap() == name))
        .unwrap_or_else(|| panic!("no mod with {name}"))
}

const HERO: [&str; 3] = [
    "models/heroes/haze/haze.vmdl_c",
    "materials/haze/body.vmat_c",
    "materials/haze/body_color.vtex_c",
];

#[test]
fn name_key_ignores_case_punctuation_extension_and_version() {
    assert_eq!(install::name_key("Cool Mod v1.2.zip"), "coolmod");
    assert_eq!(install::name_key("cool_mod_v13.7z"), "coolmod");
    assert_eq!(install::name_key("COOL-MOD.vpk"), "coolmod");
    assert_eq!(install::name_key("Cool Mod"), "coolmod");
    assert_eq!(
        install::name_key("v2.zip"),
        "",
        "too short to mean anything"
    );
    assert_ne!(
        install::name_key("Haze Skin"),
        install::name_key("Haze Hat")
    );
}

#[test]
fn same_source_file_name_matches_the_installed_local_mod() {
    let e = env();
    let src = e.work.join("Haze Skin.zip");
    vpk(&src.with_extension("vpk"), &HERO, "v1");
    fs::rename(src.with_extension("vpk"), &src).unwrap();
    let sc = install::install_local_with(
        &src,
        "My Haze",
        &e.paths,
        || false,
        shiv_core::install::InstallGuard::acquire(&e.paths.addons)
            .unwrap()
            .permit(),
    )
    .unwrap();
    assert_eq!(
        sc.source.as_deref(),
        Some("Haze Skin.zip"),
        "source recorded"
    );

    let newer = e.work.join("newer").join("Haze Skin.zip");
    fs::create_dir_all(newer.parent().unwrap()).unwrap();
    vpk(&newer, &["sounds/other.vsnd_c"], "v2");
    let ins = install::inspect(&newer, &e.paths).unwrap();
    assert_eq!(ins.vpk_count, 1);
    assert!(ins.paths.contains("sounds/other.vsnd_c"), "{:?}", ins.paths);
    assert!(
        !e.paths.addons.join(".shiv/tmp").exists(),
        "scratch dir removed"
    );

    let mods = sidecar::group(&e.paths.addons);
    let m = install::find_update_target(&mods, &ins).expect("match");
    assert_eq!(m.reason, MatchReason::SameSource);
    assert_eq!(mods[m.index].sidecar.as_ref().unwrap().mod_id, sc.mod_id);
}

#[test]
fn file_named_like_the_mod_matches_by_name() {
    let e = env();
    vpk(&e.paths.addons.join("pak03_dir.vpk"), &["a/x.vmat_c"], "v1");
    let mods = sidecar::group(&e.paths.addons);
    sidecar::adopt(&e.paths.addons, &mods[0], "Bebop Hook", None).unwrap();

    let file = e.work.join("bebop_hook_v2.vpk");
    vpk(&file, &["b/y.vmat_c"], "v2");
    let ins = install::inspect(&file, &e.paths).unwrap();
    let mods = sidecar::group(&e.paths.addons);
    let m = install::find_update_target(&mods, &ins).expect("match");
    assert_eq!(m.reason, MatchReason::SameName);
}

#[test]
fn hand_added_pak_matches_by_inner_paths_only_with_enough_overlap() {
    let e = env();
    vpk(&e.paths.addons.join("pak05_dir.vpk"), &HERO, "v1");
    vpk(
        &e.paths.addons.join("pak06_dir.vpk"),
        &["sounds/a.vsnd_c", "sounds/b.vsnd_c"],
        "s",
    );

    // 3 of 4 paths shared: 0.75 >= 0.5.
    let file = e.work.join("whatever.vpk");
    let mut more = HERO.to_vec();
    more.push("materials/haze/extra.vmat_c");
    vpk(&file, &more, "v2");
    let ins = install::inspect(&file, &e.paths).unwrap();
    let mods = sidecar::group(&e.paths.addons);
    let m = install::find_update_target(&mods, &ins).expect("match");
    assert_eq!(m.reason, MatchReason::SameFiles);
    assert_eq!(mods[m.index].vpks[0].pak, Some(5));

    // 1 of 4 shared: no match, so the drop becomes a new install.
    let file = e.work.join("other.vpk");
    vpk(&file, &[HERO[0], "x/1.vmat_c", "x/2.vmat_c"], "v3");
    let ins = install::inspect(&file, &e.paths).unwrap();
    assert_eq!(install::find_update_target(&mods, &ins), None);
}

#[test]
fn catalog_mods_and_different_vpk_counts_never_match() {
    let e = env();
    vpk(&e.paths.addons.join("pak01_dir.vpk"), &HERO, "cat");
    sidecar::write(
        &e.paths.addons,
        &Sidecar {
            mod_id: "12345".into(),
            name: "Haze Skin".into(),
            version: None,
            thumbnail_url: None,
            vpks: vec!["pak01_dir.vpk".into()],
            nsfw: Some(false),
            hero: None,
            category: None,
            notes: None,
            source: Some("Haze Skin.vpk".into()),
            stamps: None,
            unverified: None,
            generated: None,
        },
    )
    .unwrap();
    let file = e.work.join("Haze Skin.vpk");
    vpk(&file, &HERO, "new");
    let ins = install::inspect(&file, &e.paths).unwrap();
    let mods = sidecar::group(&e.paths.addons);
    assert_eq!(
        install::find_update_target(&mods, &ins),
        None,
        "catalog mod"
    );

    let mut two = ins.clone();
    two.vpk_count = 2;
    two.source = "x.vpk".into();
    let e2 = env();
    vpk(&e2.paths.addons.join("pak01_dir.vpk"), &HERO, "hand");
    let mods = sidecar::group(&e2.paths.addons);
    assert_eq!(
        install::find_update_target(&mods, &two),
        None,
        "count differs"
    );
}

#[test]
fn replace_keeps_slot_name_notes_and_state_and_backs_up_the_old_file() {
    let e = env();
    vpk(
        &e.paths.addons.join("pak01_dir.vpk"),
        &["o/1.vmat_c"],
        "other",
    );
    let src = e.work.join("Haze Skin.vpk");
    vpk(&src, &HERO, "v1");
    let sc = install::install_local_with(
        &src,
        "My Haze",
        &e.paths,
        || false,
        shiv_core::install::InstallGuard::acquire(&e.paths.addons)
            .unwrap()
            .permit(),
    )
    .unwrap();
    assert_eq!(sc.vpks, ["pak02_dir.vpk"]);
    sidecar::edit(
        &e.paths.addons,
        &sc.mod_id,
        "My Haze",
        Some("blue variant".into()),
    )
    .unwrap();
    // Disabled by the user; the update must keep it off.
    fs::rename(
        e.paths.addons.join("pak02_dir.vpk"),
        e.paths.addons.join("pak02_dir.vpk.disabled"),
    )
    .unwrap();
    let old_bytes = fs::read(e.paths.addons.join("pak02_dir.vpk.disabled")).unwrap();
    let other_before = fs::read(e.paths.addons.join("pak01_dir.vpk")).unwrap();

    let newer = e.work.join("Haze Skin v2.vpk");
    vpk(&newer, &HERO, "v2-content");
    let mods = sidecar::group(&e.paths.addons);
    let target = mods.iter().find(|m| m.sidecar.is_some()).unwrap();
    let r = install::replace_local_with(
        &newer,
        target,
        &e.paths,
        || false,
        shiv_core::install::InstallGuard::acquire(&e.paths.addons)
            .unwrap()
            .permit(),
    )
    .unwrap();

    assert_eq!(r.sidecar.mod_id, sc.mod_id);
    assert_eq!(r.sidecar.name, "My Haze");
    assert_eq!(r.sidecar.notes.as_deref(), Some("blue variant"));
    assert_eq!(r.sidecar.vpks, ["pak02_dir.vpk"]);
    assert_eq!(r.sidecar.source.as_deref(), Some("Haze Skin v2.vpk"));
    let addons = files(&e.paths.addons);
    assert_eq!(
        addons["pak02_dir.vpk.disabled"],
        fs::read(&newer).unwrap(),
        "new file, same slot, still off"
    );
    assert!(!addons.contains_key("pak02_dir.vpk"));
    assert_eq!(
        addons["pak01_dir.vpk"], other_before,
        "other mods untouched"
    );
    assert!(
        r.backup.starts_with(e.paths.addons.join(".shiv/backup")),
        "{:?}",
        r.backup
    );
    assert_eq!(
        files(&r.backup)["pak02_dir.vpk.disabled"],
        old_bytes,
        "old file kept"
    );
    assert!(newer.exists(), "source file untouched");
    let after = sidecar::read(&e.paths.addons, &sc.mod_id).unwrap().unwrap();
    assert_eq!(after, r.sidecar);
    assert!(after.stamps.is_some(), "stamps refreshed after replacement");
    let installed = sidecar::group(&e.paths.addons)
        .into_iter()
        .find(|m| m.sidecar.as_ref().is_some_and(|s| s.mod_id == sc.mod_id))
        .unwrap();
    assert_eq!(
        shiv_core::drift::drift(&installed, &shiv_core::drift::Published::Unknown),
        shiv_core::drift::Drift::Pristine,
        "updated local mod stays pristine"
    );
    assert!(!e.paths.addons.join(".shiv/tmp").exists());
}

#[test]
fn replacing_a_hand_added_pak_adopts_it_in_the_same_slot() {
    let e = env();
    vpk(&e.paths.addons.join("pak55_dir.vpk"), &HERO, "v1");
    fs::write(e.paths.addons.join("pak55_000.vpk"), b"old chunk").unwrap();
    let newer = e.work.join("Haze Skin.vpk");
    vpk(&newer, &HERO, "v2");
    let mods = sidecar::group(&e.paths.addons);
    let target = by_file(&mods, "pak55_dir.vpk");
    assert!(target.sidecar.is_none());

    let r = install::replace_local_with(
        &newer,
        target,
        &e.paths,
        || false,
        shiv_core::install::InstallGuard::acquire(&e.paths.addons)
            .unwrap()
            .permit(),
    )
    .unwrap();
    assert!(sidecar::is_local(&r.sidecar.mod_id));
    assert_eq!(r.sidecar.name, "Haze Skin");
    assert_eq!(r.sidecar.vpks, ["pak55_dir.vpk"]);
    let addons = files(&e.paths.addons);
    assert_eq!(addons["pak55_dir.vpk"], fs::read(&newer).unwrap());
    assert!(
        !addons.contains_key("pak55_000.vpk"),
        "old chunk moved away"
    );
    let backup = files(&r.backup);
    assert_eq!(backup["pak55_000.vpk"], b"old chunk");
    assert!(backup.contains_key("pak55_dir.vpk"));
    let mods = sidecar::group(&e.paths.addons);
    assert_eq!(mods.len(), 1);
    assert_eq!(mods[0].sidecar.as_ref().unwrap().name, "Haze Skin");
}

#[test]
fn replace_refuses_without_touching_anything() {
    let e = env();
    vpk(&e.paths.addons.join("pak01_dir.vpk"), &HERO, "cat");
    vpk(
        &e.paths.addons.join("pak02_dir.vpk"),
        &["a/b.vmat_c"],
        "hand",
    );
    let catalog = Sidecar {
        mod_id: "777".into(),
        name: "Catalog".into(),
        version: None,
        thumbnail_url: None,
        vpks: vec!["pak01_dir.vpk".into()],
        nsfw: Some(false),
        hero: None,
        category: None,
        notes: None,
        source: None,
        stamps: None,
        unverified: None,
        generated: None,
    };
    sidecar::write(&e.paths.addons, &catalog).unwrap();
    let before = files(&e.paths.addons);
    let file = e.work.join("new.vpk");
    vpk(&file, &HERO, "new");
    let mods = sidecar::group(&e.paths.addons);

    let err = install::replace_local_with(
        &file,
        by_file(&mods, "pak01_dir.vpk"),
        &e.paths,
        || false,
        shiv_core::install::InstallGuard::acquire(&e.paths.addons)
            .unwrap()
            .permit(),
    )
    .unwrap_err();
    assert!(matches!(err, InstallError::NotLocal), "{err:?}");

    let err = install::replace_local_with(
        &file,
        by_file(&mods, "pak02_dir.vpk"),
        &e.paths,
        || true,
        shiv_core::install::InstallGuard::acquire(&e.paths.addons)
            .unwrap()
            .permit(),
    )
    .unwrap_err();
    assert!(matches!(err, InstallError::GameRunning), "{err:?}");

    // Two VPKs for a one-VPK mod.
    let zip = e.work.join("two.zip");
    let mut z = zip::ZipWriter::new(fs::File::create(&zip).unwrap());
    for n in ["a.vpk", "b.vpk"] {
        z.start_file(n, zip::write::SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut z, &fs::read(&file).unwrap()).unwrap();
    }
    z.finish().unwrap();
    let err = install::replace_local_with(
        &zip,
        by_file(&mods, "pak02_dir.vpk"),
        &e.paths,
        || false,
        shiv_core::install::InstallGuard::acquire(&e.paths.addons)
            .unwrap()
            .permit(),
    )
    .unwrap_err();
    assert!(
        matches!(err, InstallError::VpkCountMismatch { old: 1, new: 2 }),
        "{err:?}"
    );

    assert_eq!(files(&e.paths.addons), before);
    assert!(
        !e.paths.addons.join(".shiv/backup").exists()
            || fs::read_dir(e.paths.addons.join(".shiv/backup"))
                .unwrap()
                .next()
                .is_none()
    );
}
