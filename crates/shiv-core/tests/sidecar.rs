use shiv_core::sidecar::{self, Sidecar, SidecarError};
use std::fs;
use tempfile::tempdir;

fn sc(id: &str, vpks: &[&str]) -> Sidecar {
    Sidecar {
        mod_id: id.into(),
        name: format!("Mod {id}"),
        version: Some("1.0".into()),
        thumbnail_url: None,
        vpks: vpks.iter().map(|s| s.to_string()).collect(),
        nsfw: None,
        hero: None,
        category: None,
        notes: None,
        source: None,
    }
}

#[test]
fn write_then_read_roundtrips() {
    let d = tempdir().unwrap();
    let s = sc("abc-1", &["pak01_dir.vpk"]);
    sidecar::write(d.path(), &s).unwrap();
    assert_eq!(sidecar::read(d.path(), "abc-1").unwrap(), Some(s));
    assert_eq!(sidecar::read(d.path(), "nope").unwrap(), None);
}

#[test]
fn nsfw_flag_roundtrips_and_old_sidecars_read_as_unknown() {
    let d = tempdir().unwrap();
    for (id, flag) in [
        ("spicy", Some(true)),
        ("plain", Some(false)),
        ("unknown", None),
    ] {
        let mut s = sc(id, &["pak01_dir.vpk"]);
        s.nsfw = flag;
        sidecar::write(d.path(), &s).unwrap();
        assert_eq!(sidecar::read(d.path(), id).unwrap(), Some(s));
    }
    // Unknown is not written, so the file matches what older builds wrote.
    let raw = fs::read_to_string(d.path().join(".shiv/unknown.json")).unwrap();
    assert!(!raw.contains("nsfw"), "{raw}");
    let raw = fs::read_to_string(d.path().join(".shiv/plain.json")).unwrap();
    assert!(raw.contains("\"nsfw\": false"), "{raw}");
    // Written before the field existed.
    fs::write(
        d.path().join(".shiv/old.json"),
        r#"{"mod_id":"old","name":"Old","version":null,"thumbnail_url":null,"vpks":["pak02_dir.vpk"]}"#,
    )
    .unwrap();
    let old = sidecar::read(d.path(), "old").unwrap().unwrap();
    assert_eq!(old.nsfw, None);
    assert_eq!(old.vpks, ["pak02_dir.vpk"]);
}

#[test]
fn group_joins_vpks_to_their_mod() {
    let d = tempdir().unwrap();
    fs::write(d.path().join("pak01_dir.vpk"), b"x").unwrap();
    fs::write(d.path().join("pak02_dir.vpk"), b"x").unwrap();
    sidecar::write(d.path(), &sc("m1", &["pak01_dir.vpk", "pak02_dir.vpk"])).unwrap();
    let g = sidecar::group(d.path());
    assert_eq!(g.len(), 1);
    assert_eq!(g[0].sidecar.as_ref().unwrap().mod_id, "m1");
    assert_eq!(g[0].vpks.len(), 2);
}

#[test]
fn group_matches_disabled_vpks() {
    let d = tempdir().unwrap();
    fs::write(d.path().join("pak03_dir.vpk.disabled"), b"x").unwrap();
    sidecar::write(d.path(), &sc("m1", &["pak03_dir.vpk"])).unwrap();
    let g = sidecar::group(d.path());
    assert_eq!(g.len(), 1);
    assert!(g[0].sidecar.is_some());
    assert!(!g[0].vpks[0].enabled);
}

#[test]
fn unknown_vpk_groups_alone() {
    let d = tempdir().unwrap();
    fs::write(d.path().join("pak01_dir.vpk"), b"x").unwrap();
    fs::write(d.path().join("123456_mod.vpk"), b"x").unwrap();
    sidecar::write(d.path(), &sc("m1", &["pak01_dir.vpk"])).unwrap();
    let g = sidecar::group(d.path());
    assert_eq!(g.len(), 2);
    assert!(g[0].sidecar.is_some());
    assert!(g[1].sidecar.is_none());
    assert_eq!(g[1].vpks.len(), 1);
}

#[test]
fn corrupt_sidecar_is_ignored_not_fatal() {
    let d = tempdir().unwrap();
    fs::write(d.path().join("pak01_dir.vpk"), b"x").unwrap();
    fs::create_dir(d.path().join(".shiv")).unwrap();
    fs::write(d.path().join(".shiv/bad.json"), b"{not json").unwrap();
    fs::write(d.path().join(".shiv/readme.txt"), b"hi").unwrap();
    fs::create_dir(d.path().join(".shiv/tmp")).unwrap();
    let g = sidecar::group(d.path());
    assert_eq!(g.len(), 1);
    assert!(g[0].sidecar.is_none());
    assert!(matches!(
        sidecar::read(d.path(), "bad"),
        Err(SidecarError::Parse { .. })
    ));
}

#[test]
fn rejects_mod_id_with_path_separators() {
    let d = tempdir().unwrap();
    for id in ["../evil", "a/b", "a\\b", "", "a.b"] {
        assert!(matches!(
            sidecar::read(d.path(), id),
            Err(SidecarError::InvalidModId(_))
        ));
        assert!(matches!(
            sidecar::write(d.path(), &sc(id, &[])),
            Err(SidecarError::InvalidModId(_))
        ));
    }
    assert!(!d.path().join(".shiv").exists());
}

#[test]
fn group_does_not_show_chunks_as_mods() {
    let d = tempdir().unwrap();
    fs::write(d.path().join("pak01_dir.vpk"), b"x").unwrap();
    fs::write(d.path().join("pak01_000.vpk"), b"x").unwrap();
    fs::write(d.path().join("pak01_001.vpk"), b"x").unwrap();
    sidecar::write(d.path(), &sc("m1", &["pak01_dir.vpk"])).unwrap();
    let g = sidecar::group(d.path());
    assert_eq!(g.len(), 1);
    assert_eq!(g[0].sidecar.as_ref().unwrap().mod_id, "m1");
    assert_eq!(g[0].vpks[0].chunks.len(), 2);
}

fn installed(
    id: &str,
    hero: Option<&str>,
    category: Option<&str>,
    enabled: bool,
) -> shiv_core::sidecar::InstalledMod {
    let mut s = sc(id, &["pak01_dir.vpk"]);
    s.hero = hero.map(Into::into);
    s.category = category.map(Into::into);
    shiv_core::sidecar::InstalledMod {
        sidecar: Some(s),
        vpks: vec![shiv_core::addons::InstalledVpk {
            file: format!("/a/pak01_dir.vpk{}", if enabled { "" } else { ".disabled" }).into(),
            pak: Some(1),
            enabled,
            chunks: Vec::new(),
        }],
    }
}

#[test]
fn conflicts_are_enabled_mods_for_the_same_hero_and_category() {
    let mods = vec![
        installed("same", Some("Haze"), Some("Skins"), true),
        installed("off", Some("Haze"), Some("Skins"), false),
        installed("other-hero", Some("Seven"), Some("Skins"), true),
        installed("other-kind", Some("Haze"), Some("Audio"), true),
        installed("unknown", None, None, true),
        installed("me", Some("Haze"), Some("Skins"), true),
    ];
    let ids: Vec<&str> = sidecar::conflicts(&mods, "me", "Haze", Some("Skins"))
        .iter()
        .map(|s| s.mod_id.as_str())
        .collect();
    assert_eq!(ids, ["same"]);
}

#[test]
fn conflicts_with_falls_back_to_the_catalog_for_old_sidecars() {
    let mods = vec![
        installed("old", None, None, true),
        installed("new", Some("Bebop"), Some("Skins"), true),
    ];
    let kind =
        |id: &str| (id == "old").then(|| (Some("Bebop".to_string()), Some("Skins".to_string())));
    assert_eq!(
        sidecar::conflicts_with(&mods, "new", kind),
        [("old".to_string(), "Mod old".to_string())]
    );
    assert!(sidecar::conflicts_with(&mods, "new", |_| None).is_empty());
}

#[test]
fn hero_and_category_roundtrip_and_stay_out_of_old_sidecars() {
    let d = tempdir().unwrap();
    let mut s = sc("h1", &["pak01_dir.vpk"]);
    s.hero = Some("Haze".into());
    s.category = Some("Skins".into());
    sidecar::write(d.path(), &s).unwrap();
    assert_eq!(sidecar::read(d.path(), "h1").unwrap(), Some(s));
    let plain = sc("h2", &["pak02_dir.vpk"]);
    sidecar::write(d.path(), &plain).unwrap();
    let raw = fs::read_to_string(d.path().join(".shiv/h2.json")).unwrap();
    assert!(!raw.contains("hero") && !raw.contains("category"), "{raw}");
}

fn touch(dir: &std::path::Path, names: &[&str]) {
    for n in names {
        fs::write(dir.join(n), n.as_bytes()).unwrap();
    }
}

fn ids(dir: &std::path::Path) -> Vec<String> {
    sidecar::group(dir)
        .iter()
        .map(|m| {
            m.sidecar
                .as_ref()
                .map_or_else(String::new, |s| s.mod_id.clone())
        })
        .collect()
}

#[test]
fn group_lists_mods_in_load_order() {
    let d = tempdir().unwrap();
    touch(
        d.path(),
        &["pak01_dir.vpk", "pak02_dir.vpk", "pak03_dir.vpk.disabled"],
    );
    sidecar::write(d.path(), &sc("a", &["pak03_dir.vpk"])).unwrap();
    sidecar::write(d.path(), &sc("b", &["pak01_dir.vpk"])).unwrap();
    sidecar::write(d.path(), &sc("c", &["pak02_dir.vpk"])).unwrap();
    assert_eq!(ids(d.path()), ["b", "c", "a"]);
}

#[test]
fn move_top_renumbers_files_chunks_and_sidecars() {
    let d = tempdir().unwrap();
    touch(
        d.path(),
        &[
            "pak01_dir.vpk",
            "pak01_000.vpk",
            "pak02_dir.vpk.disabled",
            "pak05_dir.vpk",
        ],
    );
    sidecar::write(d.path(), &sc("a", &["pak01_dir.vpk"])).unwrap();
    sidecar::write(d.path(), &sc("b", &["pak02_dir.vpk"])).unwrap();
    sidecar::write(d.path(), &sc("c", &["pak05_dir.vpk"])).unwrap();
    let mods = sidecar::group(d.path());
    sidecar::move_mod(d.path(), &mods, 2, sidecar::Move::Top).unwrap();

    assert_eq!(ids(d.path()), ["c", "a", "b"]);
    // Numbers 1, 2, 5 are redistributed; the gap at 5 stays.
    for f in [
        "pak01_dir.vpk",
        "pak02_dir.vpk",
        "pak02_000.vpk",
        "pak05_dir.vpk.disabled",
    ] {
        assert!(d.path().join(f).is_file(), "{f}");
    }
    assert_eq!(
        fs::read(d.path().join("pak02_000.vpk")).unwrap(),
        b"pak01_000.vpk"
    );
    assert_eq!(
        fs::read(d.path().join("pak01_dir.vpk")).unwrap(),
        b"pak05_dir.vpk"
    );
    assert!(!d.path().join("pak01_000.vpk").exists());
    assert_eq!(
        sidecar::read(d.path(), "c").unwrap().unwrap().vpks,
        ["pak01_dir.vpk"]
    );
    assert_eq!(
        sidecar::read(d.path(), "a").unwrap().unwrap().vpks,
        ["pak02_dir.vpk"]
    );
    assert_eq!(
        sidecar::read(d.path(), "b").unwrap().unwrap().vpks,
        ["pak05_dir.vpk"]
    );
}

#[test]
fn move_up_and_down_swap_neighbours_and_stop_at_the_ends() {
    let d = tempdir().unwrap();
    touch(
        d.path(),
        &["pak01_dir.vpk", "pak02_dir.vpk", "pak03_dir.vpk"],
    );
    for (id, f) in [
        ("a", "pak01_dir.vpk"),
        ("b", "pak02_dir.vpk"),
        ("c", "pak03_dir.vpk"),
    ] {
        sidecar::write(d.path(), &sc(id, &[f])).unwrap();
    }
    let mods = sidecar::group(d.path());
    sidecar::move_mod(d.path(), &mods, 0, sidecar::Move::Up).unwrap();
    assert_eq!(ids(d.path()), ["a", "b", "c"]);
    sidecar::move_mod(d.path(), &mods, 0, sidecar::Move::Down).unwrap();
    assert_eq!(ids(d.path()), ["b", "a", "c"]);
    let mods = sidecar::group(d.path());
    sidecar::move_mod(d.path(), &mods, 2, sidecar::Move::Down).unwrap();
    assert_eq!(ids(d.path()), ["b", "a", "c"]);
}

#[test]
fn move_to_places_a_mod_at_a_position_and_clamps() {
    let d = tempdir().unwrap();
    touch(
        d.path(),
        &["pak01_dir.vpk", "pak02_dir.vpk", "pak03_dir.vpk"],
    );
    for (id, f) in [
        ("a", "pak01_dir.vpk"),
        ("b", "pak02_dir.vpk"),
        ("c", "pak03_dir.vpk"),
    ] {
        sidecar::write(d.path(), &sc(id, &[f])).unwrap();
    }
    let mods = sidecar::group(d.path());
    sidecar::move_mod(d.path(), &mods, 0, sidecar::Move::To(1)).unwrap();
    assert_eq!(ids(d.path()), ["b", "a", "c"]);
    let mods = sidecar::group(d.path());
    sidecar::move_mod(d.path(), &mods, 0, sidecar::Move::To(99)).unwrap();
    assert_eq!(ids(d.path()), ["a", "c", "b"]);
}

#[test]
fn move_fails_cleanly_when_a_file_is_gone() {
    let d = tempdir().unwrap();
    touch(d.path(), &["pak01_dir.vpk", "pak02_dir.vpk"]);
    sidecar::write(d.path(), &sc("a", &["pak01_dir.vpk"])).unwrap();
    sidecar::write(d.path(), &sc("b", &["pak02_dir.vpk"])).unwrap();
    let mods = sidecar::group(d.path());
    fs::remove_file(d.path().join("pak02_dir.vpk")).unwrap();
    assert!(sidecar::move_mod(d.path(), &mods, 1, sidecar::Move::Top).is_err());
    assert!(d.path().join("pak01_dir.vpk").is_file());
    assert_eq!(
        sidecar::read(d.path(), "a").unwrap().unwrap().vpks,
        ["pak01_dir.vpk"]
    );
}

#[test]
fn set_pak_inserts_the_mod_and_shifts_higher_numbers_up() {
    let d = tempdir().unwrap();
    touch(
        d.path(),
        &[
            "pak01_dir.vpk",
            "pak02_dir.vpk",
            "pak02_000.vpk",
            "pak05_dir.vpk.disabled",
        ],
    );
    sidecar::write(d.path(), &sc("a", &["pak01_dir.vpk"])).unwrap();
    sidecar::write(d.path(), &sc("b", &["pak02_dir.vpk"])).unwrap();
    sidecar::write(d.path(), &sc("c", &["pak05_dir.vpk"])).unwrap();
    let mods = sidecar::group(d.path());
    // `c` sits at index 2; give it pak 2. `a` keeps 01, `b` shifts to 03.
    sidecar::set_pak(d.path(), &mods, 2, 2).unwrap();

    for f in [
        "pak01_dir.vpk",
        "pak02_dir.vpk.disabled",
        "pak03_dir.vpk",
        "pak03_000.vpk",
    ] {
        assert!(d.path().join(f).is_file(), "{f}");
    }
    for gone in ["pak02_dir.vpk", "pak02_000.vpk", "pak05_dir.vpk.disabled"] {
        assert!(!d.path().join(gone).exists(), "{gone}");
    }
    // The chunk followed its dir file; the disabled state stayed.
    assert_eq!(
        fs::read(d.path().join("pak02_dir.vpk.disabled")).unwrap(),
        b"pak05_dir.vpk.disabled"
    );
    assert_eq!(
        fs::read(d.path().join("pak03_000.vpk")).unwrap(),
        b"pak02_000.vpk"
    );
    assert_eq!(ids(d.path()), ["a", "c", "b"]);
    assert_eq!(
        sidecar::read(d.path(), "c").unwrap().unwrap().vpks,
        ["pak02_dir.vpk"]
    );
    assert_eq!(
        sidecar::read(d.path(), "b").unwrap().unwrap().vpks,
        ["pak03_dir.vpk"]
    );
}

#[test]
fn set_pak_moving_down_to_an_occupied_number_shifts_the_rest_up() {
    let d = tempdir().unwrap();
    touch(
        d.path(),
        &["pak01_dir.vpk", "pak02_dir.vpk", "pak03_dir.vpk"],
    );
    for (id, f) in [
        ("a", "pak01_dir.vpk"),
        ("b", "pak02_dir.vpk"),
        ("c", "pak03_dir.vpk"),
    ] {
        sidecar::write(d.path(), &sc(id, &[f])).unwrap();
    }
    let mods = sidecar::group(d.path());
    // `a` moves down to the occupied 3: b keeps 02, c shifts to 04.
    sidecar::set_pak(d.path(), &mods, 0, 3).unwrap();
    for f in ["pak02_dir.vpk", "pak03_dir.vpk", "pak04_dir.vpk"] {
        assert!(d.path().join(f).is_file(), "{f}");
    }
    assert!(!d.path().join("pak01_dir.vpk").exists());
    assert_eq!(ids(d.path()), ["b", "a", "c"]);
    assert_eq!(
        sidecar::read(d.path(), "c").unwrap().unwrap().vpks,
        ["pak04_dir.vpk"]
    );
}

#[test]
fn set_pak_shifts_by_the_target_size_not_by_one() {
    let d = tempdir().unwrap();
    touch(
        d.path(),
        &[
            "pak01_dir.vpk",
            "pak03_dir.vpk",
            "pak04_dir.vpk",
            "pak07_dir.vpk",
            "pak08_dir.vpk",
        ],
    );
    sidecar::write(d.path(), &sc("a", &["pak01_dir.vpk"])).unwrap();
    sidecar::write(d.path(), &sc("b", &["pak03_dir.vpk"])).unwrap();
    sidecar::write(d.path(), &sc("c", &["pak04_dir.vpk"])).unwrap();
    sidecar::write(d.path(), &sc("d", &["pak07_dir.vpk", "pak08_dir.vpk"])).unwrap();
    let mods = sidecar::group(d.path());
    // The two-VPK mod `d` takes pak 2-3, so b and c shift by two: 5 and 6.
    sidecar::set_pak(d.path(), &mods, 3, 2).unwrap();
    for f in [
        "pak01_dir.vpk",
        "pak02_dir.vpk",
        "pak03_dir.vpk",
        "pak05_dir.vpk",
        "pak06_dir.vpk",
    ] {
        assert!(d.path().join(f).is_file(), "{f}");
    }
    assert_eq!(
        sidecar::read(d.path(), "b").unwrap().unwrap().vpks,
        ["pak05_dir.vpk"]
    );
    assert_eq!(
        sidecar::read(d.path(), "c").unwrap().unwrap().vpks,
        ["pak06_dir.vpk"]
    );
    assert_eq!(ids(d.path()), ["a", "d", "b", "c"]);
}

#[test]
fn set_pak_renumbers_a_target_straddling_other_mods() {
    let d = tempdir().unwrap();
    touch(
        d.path(),
        &["pak01_dir.vpk", "pak03_dir.vpk", "pak05_dir.vpk"],
    );
    sidecar::write(d.path(), &sc("wide", &["pak01_dir.vpk", "pak05_dir.vpk"])).unwrap();
    sidecar::write(d.path(), &sc("mid", &["pak03_dir.vpk"])).unwrap();
    let mods = sidecar::group(d.path());
    // The target {1, 5} takes 3-4; the mod at 3 shifts to 5.
    sidecar::set_pak(d.path(), &mods, 0, 3).unwrap();
    for f in ["pak03_dir.vpk", "pak04_dir.vpk", "pak05_dir.vpk"] {
        assert!(d.path().join(f).is_file(), "{f}");
    }
    // The names moved with the content: wide's two VPKs are at 3-4, and
    // the pak05 name now holds what used to be pak03 (mid's file).
    assert_eq!(
        fs::read(d.path().join("pak03_dir.vpk")).unwrap(),
        b"pak01_dir.vpk"
    );
    assert_eq!(
        fs::read(d.path().join("pak04_dir.vpk")).unwrap(),
        b"pak05_dir.vpk"
    );
    assert_eq!(
        fs::read(d.path().join("pak05_dir.vpk")).unwrap(),
        b"pak03_dir.vpk"
    );
    assert!(!d.path().join("pak01_dir.vpk").exists());
    assert_eq!(
        sidecar::read(d.path(), "wide").unwrap().unwrap().vpks,
        ["pak03_dir.vpk", "pak04_dir.vpk"]
    );
    assert_eq!(
        sidecar::read(d.path(), "mid").unwrap().unwrap().vpks,
        ["pak05_dir.vpk"]
    );
    assert_eq!(ids(d.path()), ["wide", "mid"]);
}

#[test]
fn set_pak_to_the_mods_own_range_touches_nothing() {
    let d = tempdir().unwrap();
    touch(d.path(), &["pak07_dir.vpk", "pak08_dir.vpk"]);
    let before = fs::read(d.path().join("pak07_dir.vpk")).unwrap();
    sidecar::write(d.path(), &sc("d", &["pak07_dir.vpk", "pak08_dir.vpk"])).unwrap();
    let sc_before = sidecar::read(d.path(), "d").unwrap().unwrap();
    let mods = sidecar::group(d.path());
    sidecar::set_pak(d.path(), &mods, 0, 7).unwrap();
    assert_eq!(fs::read(d.path().join("pak07_dir.vpk")).unwrap(), before);
    assert_eq!(
        fs::metadata(d.path().join("pak08_dir.vpk")).unwrap().len(),
        "pak08_dir.vpk".len() as u64
    );
    assert_eq!(sidecar::read(d.path(), "d").unwrap().unwrap(), sc_before);
}

#[test]
fn set_pak_fails_cleanly_when_a_file_is_gone() {
    let d = tempdir().unwrap();
    touch(d.path(), &["pak01_dir.vpk", "pak02_dir.vpk"]);
    sidecar::write(d.path(), &sc("a", &["pak01_dir.vpk"])).unwrap();
    sidecar::write(d.path(), &sc("b", &["pak02_dir.vpk"])).unwrap();
    let mods = sidecar::group(d.path());
    fs::remove_file(d.path().join("pak02_dir.vpk")).unwrap();
    assert!(sidecar::set_pak(d.path(), &mods, 1, 1).is_err());
    assert!(d.path().join("pak01_dir.vpk").is_file());
    assert_eq!(
        sidecar::read(d.path(), "a").unwrap().unwrap().vpks,
        ["pak01_dir.vpk"]
    );
}

#[test]
fn set_pak_moves_only_pak_numbered_files_of_a_mixed_mod() {
    let d = tempdir().unwrap();
    touch(d.path(), &["pak03_dir.vpk", "custom.vpk"]);
    let mut mixed = sc("m", &["pak03_dir.vpk", "custom.vpk"]);
    mixed.notes = Some("hand-added file inside a managed mod".into());
    sidecar::write(d.path(), &mixed).unwrap();
    let mods = sidecar::group(d.path());
    sidecar::set_pak(d.path(), &mods, 0, 5).unwrap();
    assert!(d.path().join("pak05_dir.vpk").is_file());
    assert!(!d.path().join("pak03_dir.vpk").exists());
    assert_eq!(
        fs::read(d.path().join("custom.vpk")).unwrap(),
        b"custom.vpk",
        "the hand-added file stays byte for byte"
    );
    let after = sidecar::read(d.path(), "m").unwrap().unwrap();
    assert_eq!(after.vpks, ["pak05_dir.vpk", "custom.vpk"]);
}

#[test]
fn set_pak_refuses_a_broken_slice_instead_of_overwriting() {
    let d = tempdir().unwrap();
    touch(d.path(), &["pak01_dir.vpk", "pak02_dir.vpk"]);
    // A slice `group` could never produce: two mods claiming pak 02. When
    // the target takes pak 1, both shift onto 3 — a plan that would
    // rename one file onto another.
    let vpk = |pak: u32, f: &str| shiv_core::addons::InstalledVpk {
        file: d.path().join(f),
        pak: Some(pak),
        enabled: true,
        chunks: Vec::new(),
    };
    let m = |pak: u32, f: &str| shiv_core::sidecar::InstalledMod {
        sidecar: Some(sc("x", &[f])),
        vpks: vec![vpk(pak, f)],
    };
    let mods = vec![
        m(1, "pak01_dir.vpk"),
        m(2, "pak02_dir.vpk"),
        m(2, "pak02_dir.vpk"),
    ];
    let err = sidecar::set_pak(d.path(), &mods, 0, 1).unwrap_err();
    assert!(matches!(err, SidecarError::DuplicatePak(_)), "{err:?}");
    // The files on disk survive untouched.
    assert_eq!(
        fs::read(d.path().join("pak01_dir.vpk")).unwrap(),
        b"pak01_dir.vpk"
    );
    assert_eq!(
        fs::read(d.path().join("pak02_dir.vpk")).unwrap(),
        b"pak02_dir.vpk"
    );
}

#[test]
fn set_pak_on_a_free_number_keeps_the_others_in_place() {
    let d = tempdir().unwrap();
    touch(d.path(), &["pak01_dir.vpk", "pak02_dir.vpk"]);
    sidecar::write(d.path(), &sc("a", &["pak01_dir.vpk"])).unwrap();
    sidecar::write(d.path(), &sc("b", &["pak02_dir.vpk"])).unwrap();
    let mods = sidecar::group(d.path());
    // A number nothing holds is taken literally; the load order follows it.
    sidecar::set_pak(d.path(), &mods, 0, 40).unwrap();
    assert!(d.path().join("pak40_dir.vpk").is_file());
    assert!(!d.path().join("pak01_dir.vpk").exists());
    assert!(d.path().join("pak02_dir.vpk").is_file());
    assert_eq!(
        sidecar::read(d.path(), "a").unwrap().unwrap().vpks,
        ["pak40_dir.vpk"]
    );
    assert_eq!(ids(d.path()), ["b", "a"]);
}

#[test]
fn set_pak_gives_a_multi_vpk_mod_consecutive_numbers() {
    let d = tempdir().unwrap();
    touch(
        d.path(),
        &["pak01_dir.vpk", "pak07_dir.vpk", "pak08_dir.vpk"],
    );
    sidecar::write(d.path(), &sc("a", &["pak01_dir.vpk"])).unwrap();
    sidecar::write(d.path(), &sc("d", &["pak07_dir.vpk", "pak08_dir.vpk"])).unwrap();
    let mods = sidecar::group(d.path());
    // `d` takes pak 2 and pak 3; `a` stays at 01.
    sidecar::set_pak(d.path(), &mods, 1, 2).unwrap();
    for f in ["pak01_dir.vpk", "pak02_dir.vpk", "pak03_dir.vpk"] {
        assert!(d.path().join(f).is_file(), "{f}");
    }
    assert!(!d.path().join("pak07_dir.vpk").exists());
    assert!(!d.path().join("pak08_dir.vpk").exists());
    assert_eq!(
        sidecar::read(d.path(), "d").unwrap().unwrap().vpks,
        ["pak02_dir.vpk", "pak03_dir.vpk"]
    );
    assert_eq!(ids(d.path()), ["a", "d"]);
}

#[test]
fn set_pak_refuses_numbers_that_do_not_fit_and_changes_nothing() {
    let d = tempdir().unwrap();
    touch(d.path(), &["pak98_dir.vpk", "pak99_dir.vpk"]);
    sidecar::write(d.path(), &sc("a", &["pak98_dir.vpk"])).unwrap();
    sidecar::write(d.path(), &sc("b", &["pak99_dir.vpk"])).unwrap();
    let mods = sidecar::group(d.path());
    // The mod at 98 to pak 99 would push the one at 99 past the limit;
    // the asked number itself is in range, so the reason says so.
    let err = sidecar::set_pak(d.path(), &mods, 0, 99).unwrap_err();
    assert!(matches!(err, SidecarError::PakPush { pak: 99 }), "{err:?}");
    assert_eq!(
        err.to_string(),
        "setting pak 99 would push another VPK past 99"
    );
    // An out-of-range number is refused as such.
    let err = sidecar::set_pak(d.path(), &mods, 0, 0).unwrap_err();
    assert!(matches!(err, SidecarError::BadPak(0)), "{err:?}");
    assert!(d.path().join("pak98_dir.vpk").is_file());
    assert!(d.path().join("pak99_dir.vpk").is_file());
    assert_eq!(
        sidecar::read(d.path(), "a").unwrap().unwrap().vpks,
        ["pak98_dir.vpk"]
    );
    assert_eq!(
        sidecar::read(d.path(), "b").unwrap().unwrap().vpks,
        ["pak99_dir.vpk"]
    );

    // A file without a pak number is not movable: a no-op, not an error.
    touch(d.path(), &["custom.vpk"]);
    let mods = sidecar::group(d.path());
    let hand = mods.iter().position(|m| m.sidecar.is_none()).unwrap();
    sidecar::set_pak(d.path(), &mods, hand, 5).unwrap();
    assert!(d.path().join("custom.vpk").is_file());
    assert!(!d.path().join("pak05_dir.vpk").exists());
}
