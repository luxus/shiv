//! The interchange format: export the installed library to a bundle, and
//! import one back (the same shape Deadlock Mod Manager and Grimoire read
//! and write).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use shiv_core::game::GamePaths;
use shiv_core::install;
use shiv_core::interchange::{self, Document, InterchangeError, RemoteRef};
use shiv_core::sidecar;
use tempfile::{TempDir, tempdir};

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

fn names(mods: &[sidecar::InstalledMod]) -> Vec<String> {
    mods.iter()
        .map(|m| {
            m.sidecar
                .as_ref()
                .map(|s| s.name.clone())
                .unwrap_or_default()
        })
        .collect()
}

/// Install a local mod named `name` from a VPK holding `inner` files.
fn install_mod(e: &Env, name: &str, inner: &[&str], tag: &str) -> sidecar::Sidecar {
    let file = e.work.join(format!("{}.vpk", name.replace(' ', "_")));
    vpk(&file, inner, tag);
    install::install_local_with(&file, name, &e.paths, || false).unwrap()
}

#[test]
fn export_writes_a_bundle_another_manager_can_read() {
    let e = env();
    let a = install_mod(&e, "Haze Skin", &["materials/haze/body.vmat_c"], "a");
    install_mod(&e, "Hitsound", &["sounds/hit.vsnd_c"], "h");
    fs::rename(
        e.paths.addons.join("pak02_dir.vpk"),
        e.paths.addons.join("pak02_dir.vpk.disabled"),
    )
    .unwrap();

    let dest = e.work.join("exports");
    fs::create_dir_all(&dest).unwrap();
    let report = interchange::export(&e.paths.addons, &dest, &|_| None).unwrap();

    // A new folder under the asked one, holding the manifest and the VPKs.
    assert!(report
        .root
        .to_string_lossy()
        .contains("deadlock-mods-export-"), "{:?}", report.root);
    assert!(report.root.join("mod-interchange.json").is_file());
    assert_eq!(report.exported.len(), 2);
    assert!(report.skipped.is_empty());

    let (dir, doc) = interchange::read_manifest(&report.root).unwrap();
    assert_eq!(dir, report.root);
    assert_eq!(doc.format, "deadlock-mod-interchange");
    assert_eq!(doc.version, 1);
    assert_eq!(doc.contents, Some(vec!["mods".into()]));
    assert_eq!(doc.source.as_ref().unwrap().manager, "shiv");
    assert_eq!(doc.mods.len(), 2);

    // Load order ascending: the enabled mod at pak01 first, the disabled
    // one second, each with its state and stable local keys.
    let mut mods = doc.mods.clone();
    mods.sort_by_key(|m| m.order);
    assert_eq!(mods[0].name, "Haze Skin");
    assert!(mods[0].enabled && mods[0].files[0].selected);
    assert_eq!(mods[1].name, "Hitsound");
    assert!(!mods[1].enabled && mods[1].files[0].selected);
    for m in &mods {
        assert!(m.key.starts_with("local:sha256:"), "{}", m.key);
        assert!(m.origin.is_none());
        // Notes travel as the description; Shiv's id rides in extensions.
        assert!(m.extensions.is_some());
        for f in &m.files {
            assert!(interchange::bundle_file(&dir, &f.path).is_some_and(|p| p.is_file()));
        }
    }
    // A disabled VPK rides along under its enabled-form name, selected as
    // part of the mod; the mod's `enabled` carries the off state.
    let hit = mods[1].files[0].clone();
    assert_eq!(hit.name, "pak02_dir.vpk");
    assert!(interchange::bundle_file(&dir, &hit.path).unwrap().is_file());

    // The addons folder is untouched by an export.
    assert!(e.paths.addons.join("pak01_dir.vpk").is_file());
    assert!(e.paths.addons.join("pak02_dir.vpk.disabled").is_file());
    let _ = a;
}

#[test]
fn exported_keys_stay_stable_across_exports() {
    let e = env();
    install_mod(&e, "Outlines", &["scripts/outline.vmat_c"], "o");
    let dest = e.work.join("exports");
    fs::create_dir_all(&dest).unwrap();
    let one = interchange::export(&e.paths.addons, &dest, &|_| None).unwrap();
    let two = interchange::export(&e.paths.addons, &dest, &|_| None).unwrap();
    let (_, doc1) = interchange::read_manifest(&one.root).unwrap();
    let (_, doc2) = interchange::read_manifest(&two.root).unwrap();
    assert_eq!(doc1.mods[0].key, doc2.mods[0].key);
    assert_eq!(doc1.mods[0].files[0].sha256, doc2.mods[0].files[0].sha256);
}

#[test]
fn export_maps_catalog_mods_to_gamebanana_keys() {
    let e = env();
    // A catalog mod: Shiv's own sidecar, as Browse installs it.
    vpk(&e.paths.addons.join("pak01_dir.vpk"), &["a/x.vmat_c"], "cat");
    sidecar::write(
        &e.paths.addons,
        &sidecar::Sidecar {
            mod_id: "12345".into(),
            name: "QOL Lock".into(),
            version: None,
            thumbnail_url: None,
            vpks: vec!["pak01_dir.vpk".into()],
            nsfw: Some(false),
            hero: None,
            category: None,
            notes: None,
            source: None,
        },
    )
    .unwrap();
    let dest = e.work.join("exports");
    fs::create_dir_all(&dest).unwrap();
    let report = interchange::export(&e.paths.addons, &dest, &|id| {
        (id == "12345").then(|| RemoteRef {
            submission_id: "650634".into(),
            is_audio: false,
        })
    })
    .unwrap();
    let (_, doc) = interchange::read_manifest(&report.root).unwrap();
    let m = &doc.mods[0];
    assert_eq!(m.key, "gamebanana:mod:650634");
    let origin = m.origin.as_ref().unwrap();
    assert_eq!(origin.provider, "gamebanana");
    assert_eq!(origin.submission_id.as_deref(), Some("650634"));
}

#[test]
fn import_round_trips_mods_enabled_state_and_order() {
    // Export from one library...
    let source = env();
    install_mod(&source, "First", &["a/1.vmat_c"], "1");
    install_mod(&source, "Second", &["b/2.vmat_c"], "2");
    fs::rename(
        source.paths.addons.join("pak02_dir.vpk"),
        source.paths.addons.join("pak02_dir.vpk.disabled"),
    )
    .unwrap();
    let dest = source.work.join("exports");
    fs::create_dir_all(&dest).unwrap();
    let report = interchange::export(&source.paths.addons, &dest, &|_| None).unwrap();

    // ...into a fresh one.
    let target = env();
    interchange::import(&report.root, &target.paths, || false, &|_| None).unwrap();
    let mods = sidecar::group(&target.paths.addons);
    assert_eq!(names(&mods), ["First", "Second"]);
    assert!(mods[0].is_enabled());
    assert!(!mods[1].is_enabled());
    // The files arrived byte for byte under fresh pak slots.
    let first = files(&target.paths.addons);
    assert!(first["pak01_dir.vpk"].len() > 0);
    assert!(first.contains_key("pak02_dir.vpk.disabled"));
    // The bundle is never touched by an import.
    assert!(report.root.join("mod-interchange.json").is_file());
}

#[test]
fn a_reimport_does_not_duplicate_mods() {
    let source = env();
    install_mod(&source, "Once", &["a/1.vmat_c"], "1");
    let dest = source.work.join("exports");
    fs::create_dir_all(&dest).unwrap();
    let report = interchange::export(&source.paths.addons, &dest, &|_| None).unwrap();

    let target = env();
    interchange::import(&report.root, &target.paths, || false, &|_| None).unwrap();
    let second = interchange::import(&report.root, &target.paths, || false, &|_| None).unwrap();
    assert!(second.installed.is_empty() && second.adopted.is_empty());
    assert_eq!(second.skipped.len(), 1);
    assert_eq!(second.skipped[0].0, "Once");
    assert_eq!(second.skipped[0].1, "already installed");
    let mods = sidecar::group(&target.paths.addons);
    assert_eq!(mods.len(), 1);
}

#[test]
fn import_recognises_a_mod_the_catalog_already_knows() {
    let e = env();
    install_mod(&e, "From Bundle", &["a/1.vmat_c"], "x");
    let dest = e.work.join("exports");
    fs::create_dir_all(&dest).unwrap();
    let report = interchange::export(&e.paths.addons, &dest, &|_| None).unwrap();
    let (_, mut doc) = interchange::read_manifest(&report.root).unwrap();

    // The same mod arrives from elsewhere under its GameBanana key, and
    // the catalog says that is Shiv mod "777".
    doc.mods[0].key = "gamebanana:mod:650634".into();
    rewrite_manifest(&report.root, &doc);

    let target = env();
    sidecar::write(
        &target.paths.addons,
        &sidecar::Sidecar {
            mod_id: "777".into(),
            name: "Existing".into(),
            version: None,
            thumbnail_url: None,
            vpks: vec!["pak01_dir.vpk".into()],
            nsfw: Some(false),
            hero: None,
            category: None,
            notes: None,
            source: None,
        },
    )
    .unwrap();
    fs::write(target.paths.addons.join("pak01_dir.vpk"), b"already here").unwrap();
    let imported =
        interchange::import(&report.root, &target.paths, || false, &|key| {
            (key == "gamebanana:mod:650634").then(|| "777".to_owned())
        })
        .unwrap();
    assert!(imported.installed.is_empty());
    assert_eq!(imported.skipped[0], ("From Bundle".into(), "already installed".into()));
    assert_eq!(sidecar::group(&target.paths.addons).len(), 1);
}

/// Overwrite a bundle's manifest (test scaffolding).
fn rewrite_manifest(root: &Path, doc: &Document) {
    fs::write(
        root.join("mod-interchange.json"),
        serde_json::to_vec_pretty(doc).unwrap(),
    )
    .unwrap();
}

#[test]
fn import_adopts_files_already_in_the_addons_folder() {
    let e = env();
    install_mod(&e, "Adoptable", &["a/1.vmat_c"], "adopt me");
    let dest = e.work.join("exports");
    fs::create_dir_all(&dest).unwrap();
    let report = interchange::export(&e.paths.addons, &dest, &|_| None).unwrap();

    // A second library already holds the same bytes, hand-added.
    let target = env();
    let file = report
        .root
        .join(&interchange::read_manifest(&report.root).unwrap().1.mods[0].files[0].path);
    fs::copy(&file, target.paths.addons.join("pak55_dir.vpk")).unwrap();

    let imported = interchange::import(&report.root, &target.paths, || false, &|_| None).unwrap();
    assert_eq!(imported.adopted, ["Adoptable".to_owned()]);
    // Adopted in place: the file was not duplicated.
    assert!(target.paths.addons.join("pak55_dir.vpk").is_file());
    assert_eq!(sidecar::group(&target.paths.addons).len(), 1);
    let sc = sidecar::group(&target.paths.addons)
        .pop()
        .unwrap()
        .sidecar
        .unwrap();
    assert_eq!(sc.name, "Adoptable");
    assert_eq!(sc.vpks, ["pak55_dir.vpk"]);
}

#[test]
fn import_skips_broken_entries_and_carries_on() {
    let e = env();
    install_mod(&e, "Good", &["a/1.vmat_c"], "g");
    let dest = e.work.join("exports");
    fs::create_dir_all(&dest).unwrap();
    let report = interchange::export(&e.paths.addons, &dest, &|_| None).unwrap();
    let (_, mut doc) = interchange::read_manifest(&report.root).unwrap();

    // A second entry whose file is missing, one that points outside the
    // bundle, one with no selected files, and a good extra one.
    let dir = report.root.join("files").join("9999-extra");
    fs::create_dir_all(&dir).unwrap();
    let extra = dir.join("extra.vpk");
    vpk(&extra, &["x/y.vmat_c"], "e");
    let sha = {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(fs::read(&extra).unwrap());
        format!("{:x}", h.finalize())
    };
    let size = fs::metadata(&extra).unwrap().len();
    doc.mods.extend([
        interchange::InterchangeMod {
            key: "local:sha256:missing".into(),
            name: "Missing File".into(),
            enabled: true,
            order: 1,
            origin: None,
            author: None,
            description: None,
            category: None,
            hero: None,
            thumbnail_url: None,
            link: None,
            nsfw: None,
            files: vec![interchange::InterchangeFile {
                name: "gone.vpk".into(),
                path: "files/9999-missing/gone.vpk".into(),
                sha256: None,
                size: None,
                selected: true,
            }],
            extensions: None,
        },
        interchange::InterchangeMod {
            key: "local:sha256:escape".into(),
            name: "Escape".into(),
            enabled: true,
            order: 2,
            origin: None,
            author: None,
            description: None,
            category: None,
            hero: None,
            thumbnail_url: None,
            link: None,
            nsfw: None,
            files: vec![interchange::InterchangeFile {
                name: "evil.vpk".into(),
                path: "../../evil.vpk".into(),
                sha256: None,
                size: None,
                selected: true,
            }],
            extensions: None,
        },
        interchange::InterchangeMod {
            key: "local:sha256:unselected".into(),
            name: "Only Variants".into(),
            enabled: true,
            order: 3,
            origin: None,
            author: None,
            description: None,
            category: None,
            hero: None,
            thumbnail_url: None,
            link: None,
            nsfw: None,
            files: vec![interchange::InterchangeFile {
                name: "variant.vpk".into(),
                path: "files/9999-extra/extra.vpk".into(),
                sha256: Some(sha.clone()),
                size: Some(size),
                selected: false,
            }],
            extensions: None,
        },
        interchange::InterchangeMod {
            key: "local:sha256:extra".into(),
            name: "Extra".into(),
            enabled: true,
            order: 4,
            origin: None,
            author: None,
            description: None,
            category: None,
            hero: None,
            thumbnail_url: None,
            link: None,
            nsfw: None,
            files: vec![interchange::InterchangeFile {
                name: "extra.vpk".into(),
                path: "files/9999-extra/extra.vpk".into(),
                sha256: Some(sha),
                size: Some(size),
                selected: true,
            }],
            extensions: None,
        },
    ]);
    rewrite_manifest(&report.root, &doc);

    let target = env();
    let imported = interchange::import(&report.root, &target.paths, || false, &|_| None).unwrap();
    assert_eq!(imported.installed, ["Good".to_owned(), "Extra".to_owned()]);
    let skips: Vec<_> = imported
        .skipped
        .iter()
        .map(|(n, why)| (n.as_str(), why.as_str()))
        .collect();
    assert_eq!(
        skips,
        [
            ("Missing File", "files/9999-missing/gone.vpk is missing from the bundle"),
            ("Escape", "../../evil.vpk does not stay inside the bundle"),
            ("Only Variants", "no selected VPK files in the bundle"),
        ]
    );
    assert_eq!(sidecar::group(&target.paths.addons).len(), 2);
}

#[test]
fn manifests_are_validated_before_anything_is_imported() {
    let e = env();
    let bundle = e.work.join("bundle");
    fs::create_dir_all(bundle.join("files")).unwrap();

    // A manifest with the wrong format is refused.
    fs::write(
        bundle.join("mod-interchange.json"),
        r#"{"format":"something-else","version":1,"mods":[]}"#,
    )
    .unwrap();
    let err = interchange::import(&bundle, &e.paths, || false, &|_| None).unwrap_err();
    assert!(matches!(err, InterchangeError::WrongFormat(_)), "{err:?}");

    // A newer major version is refused, not guessed.
    fs::write(
        bundle.join("mod-interchange.json"),
        r#"{"format":"deadlock-mod-interchange","version":2,"mods":[]}"#,
    )
    .unwrap();
    let err = interchange::import(&bundle, &e.paths, || false, &|_| None).unwrap_err();
    assert!(matches!(err, InterchangeError::Version(2)), "{err:?}");

    // A folder without a manifest is not a bundle.
    let empty = e.work.join("empty");
    fs::create_dir_all(&empty).unwrap();
    let err = interchange::import(&empty, &e.paths, || false, &|_| None).unwrap_err();
    assert!(matches!(err, InterchangeError::NotABundle(_)), "{err:?}");
}

#[test]
fn bundle_paths_must_stay_inside_the_bundle() {
    let e = env();
    let bundle = e.work.join("bundle");
    fs::create_dir_all(bundle.join("files")).unwrap();
    let outside = e.work.join("outside.vpk");
    fs::write(&outside, b"do not touch").unwrap();
    let dir = bundle.canonicalize().unwrap();
    fs::write(
        bundle.join("mod-interchange.json"),
        serde_json::json!({
            "format": "deadlock-mod-interchange",
            "version": 1,
            "mods": [{
                "key": "local:sha256:x",
                "name": "Sneaky",
                "enabled": true,
                "order": 0,
                "files": [{
                    "name": "outside.vpk",
                    "path": "../outside.vpk",
                    "selected": true
                }]
            }]
        })
        .to_string(),
    )
    .unwrap();
    let _ = dir;
    let target = env();
    let imported = interchange::import(&bundle, &target.paths, || false, &|_| None).unwrap();
    assert!(imported.installed.is_empty());
    assert_eq!(imported.skipped[0].0, "Sneaky");
    // The file outside the bundle is untouched.
    assert_eq!(fs::read(&outside).unwrap(), b"do not touch");
}

#[test]
fn import_refuses_while_the_game_runs_and_exports_into_a_new_folder() {
    let e = env();
    install_mod(&e, "Any", &["a/1.vmat_c"], "1");
    let dest = e.work.join("exports");
    fs::create_dir_all(&dest).unwrap();
    let report = interchange::export(&e.paths.addons, &dest, &|_| None).unwrap();
    let before = files(&e.paths.addons);

    let err = interchange::import(&report.root, &e.paths, || true, &|_| None).unwrap_err();
    assert!(err.to_string().contains("Deadlock is running"), "{err}");
    assert_eq!(files(&e.paths.addons), before);

    // An export never overwrites an existing bundle folder.
    let err = interchange::export_into(&e.paths.addons, &report.root, &|_| None).unwrap_err();
    assert!(
        matches!(err, InterchangeError::DestinationExists(_)),
        "{err:?}"
    );
    // A hand-added file with no sidecar is exported as a local mod too.
    fs::write(e.paths.addons.join("custom.vpk"), b"\x34\x12\xaa\x55\x02\x00\x00\x00hand").unwrap();
    let report2 = interchange::export(&e.paths.addons, &dest, &|_| None).unwrap();
    let (_, doc) = interchange::read_manifest(&report2.root).unwrap();
    assert_eq!(doc.mods.len(), 2);
    assert!(doc.mods.iter().any(|m| m.name == "custom.vpk"));
}
