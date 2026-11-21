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
        stamps: None,
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

#[test]
fn set_pak_refuses_huge_numbers_without_overflowing() {
    let d = tempdir().unwrap();
    touch(d.path(), &["pak01_dir.vpk"]);
    sidecar::write(d.path(), &sc("a", &["pak01_dir.vpk"])).unwrap();
    let mods = sidecar::group(d.path());
    // Near-u32::MAX is refused as out of range by the check that runs
    // before the block arithmetic, not wrapped into a number that fits.
    for bad in [u32::MAX - 1, u32::MAX] {
        let err = sidecar::set_pak(d.path(), &mods, 0, bad).unwrap_err();
        assert!(matches!(err, SidecarError::BadPak(_)), "{err:?} for {bad}");
    }
    assert_eq!(
        fs::read(d.path().join("pak01_dir.vpk")).unwrap(),
        b"pak01_dir.vpk"
    );
}

/// A tiny reproducible generator: a Lehmer LCG, so a failing case can be
/// replayed from its seed.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }

    /// A value in `0..n`.
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Byte snapshot of everything `set_pak` may touch: the files directly in
/// the folder and the sidecars in `.shiv`.
fn snapshot(dir: &std::path::Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    use std::collections::BTreeMap;
    let mut out: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut add = |base: &std::path::Path, prefix: &str| {
        for entry in fs::read_dir(base).unwrap().flatten() {
            if entry.file_type().unwrap().is_file() {
                out.insert(
                    format!("{prefix}{}", entry.file_name().to_string_lossy()),
                    fs::read(entry.path()).unwrap(),
                );
            }
        }
    };
    add(dir, "");
    add(&dir.join(".shiv"), ".shiv/");
    out
}

/// The ids of the pak-movable mods, in load order.
fn load_order(mods: &[shiv_core::sidecar::InstalledMod]) -> Vec<String> {
    mods.iter()
        .filter(|m| m.load_position() != u32::MAX)
        .map(|m| m.sidecar.as_ref().unwrap().mod_id.clone())
        .collect()
}

/// The invariants `set_pak` must hold, checked across random
/// configurations instead of hand-computed examples: after a success,
/// pak numbers stay unique and inside 01..=99, the target occupies exactly
/// its asked block, every other mod keeps its relative order, sidecars
/// point at files that exist and nothing else moves; after a refusal, not
/// one byte changes. This is the test that would have caught a rename
/// plan with duplicate targets (silent file loss) and the shift-amount
/// errors.
#[test]
fn set_pak_invariants_hold_across_random_configurations() {
    let mut rng = Lcg(0x5eed_1234);
    for case in 0..120 {
        let d = tempdir().unwrap();
        // 1..=4 mods, each 1..=2 paks drawn unique from 1..=99, sometimes
        // disabled; a hand-added file sometimes sits among them.
        let n_mods = 1 + rng.below(4) as usize;
        let mut pool: Vec<u32> = (1..=99).collect();
        let mut meta: Vec<(String, Vec<u32>)> = Vec::new();
        for m in 0..n_mods {
            let count = 1 + rng.below(2) as usize;
            let mut paks = Vec::new();
            for _ in 0..count {
                paks.push(pool.remove(rng.below(pool.len() as u64) as usize));
            }
            paks.sort_unstable();
            let disabled = rng.below(4) == 0;
            for &p in &paks {
                let name = format!("pak{p:02}_dir.vpk");
                let file = if disabled {
                    format!("{name}.disabled")
                } else {
                    name.clone()
                };
                fs::write(d.path().join(&file), file.as_bytes()).unwrap();
            }
            let names: Vec<String> = paks.iter().map(|p| format!("pak{p:02}_dir.vpk")).collect();
            let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
            sidecar::write(d.path(), &sc(&format!("m{m}"), &name_refs)).unwrap();
            meta.push((format!("m{m}"), paks));
        }
        let with_hand = rng.below(3) == 0;
        if with_hand {
            fs::write(d.path().join("custom.vpk"), b"custom.vpk").unwrap();
        }

        let mods = sidecar::group(d.path());
        let (target_id, target_paks) = &meta[rng.below(n_mods as u64) as usize];
        let target_pos = mods
            .iter()
            .position(|m| m.sidecar.as_ref().is_some_and(|s| &s.mod_id == target_id))
            .unwrap();
        let others_before: Vec<String> = load_order(&mods)
            .into_iter()
            .filter(|id| id != target_id)
            .collect();
        // 0..=101: refusals (0, past 99) are exercised next to real moves.
        let ask = rng.below(102) as u32;
        let before = snapshot(d.path());
        let result = sidecar::set_pak(d.path(), &mods, target_pos, ask);

        match result {
            Ok(()) => {
                // The asked block fits, or the call would have refused.
                let take = target_paks.len() as u32;
                assert!(
                    ask >= 1 && ask + take - 1 <= 99,
                    "case {case}: accepted pak {ask} for {take} VPKs"
                );
                let after = sidecar::group(d.path());
                // Unique numbers, all inside the capacity.
                let all: Vec<u32> = after
                    .iter()
                    .flat_map(|m| m.vpks.iter().filter_map(|v| v.pak))
                    .collect();
                let mut unique = all.clone();
                unique.sort_unstable();
                unique.dedup();
                assert_eq!(
                    unique.len(),
                    all.len(),
                    "case {case}: duplicate pak numbers"
                );
                assert!(all.iter().all(|p| (1..=99).contains(p)), "case {case}");
                // The target occupies exactly pak..pak+take-1.
                let target = after
                    .iter()
                    .find(|m| m.sidecar.as_ref().is_some_and(|s| &s.mod_id == target_id))
                    .unwrap();
                let mut got: Vec<u32> = target.vpks.iter().map(|v| v.pak.unwrap()).collect();
                got.sort_unstable();
                assert_eq!(
                    got,
                    (ask..ask + take).collect::<Vec<u32>>(),
                    "case {case}: target block"
                );
                // Every other mod keeps its relative load order.
                let others_after: Vec<String> = load_order(&after)
                    .into_iter()
                    .filter(|id| id != target_id)
                    .collect();
                assert_eq!(
                    others_before, others_after,
                    "case {case}: relative order changed"
                );
                // Sidecars point at files that exist, enabled or disabled.
                for m in &after {
                    let Some(s) = &m.sidecar else { continue };
                    for name in &s.vpks {
                        assert!(
                            d.path().join(name).is_file()
                                || d.path().join(format!("{name}.disabled")).is_file(),
                            "case {case}: sidecar {} points at missing {name}",
                            s.mod_id
                        );
                    }
                }
                // A hand-added file never moved.
                if with_hand {
                    assert_eq!(
                        fs::read(d.path().join("custom.vpk")).unwrap(),
                        b"custom.vpk",
                        "case {case}"
                    );
                }
            }
            Err(_) => {
                // A refusal must leave every byte untouched.
                assert_eq!(
                    snapshot(d.path()),
                    before,
                    "case {case}: a refused set_pak changed the disk"
                );
            }
        }
    }
}
