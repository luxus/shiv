//! Performance configs: the catalog and source-preserving `gameinfo.gi` edits.

use std::fs;
use std::path::Path;

use shiv_core::game::GamePaths;
use shiv_core::perf::{
    self, Catalog, CatalogCache, ConfigEntry, ConvarKind, GameinfoState, PerfError, Tier,
};

const GAMEINFO: &str = "\"GameInfo\"\r\n{\r\n\tgame\t\t\"citadel\"\r\n\tFileSystem\r\n\t{\r\n\t\tSearchPaths\r\n\t\t{\r\n\t\t\tGame\t\t\t\tcitadel/addons\r\n\t\t\tGame\t\t\t\tcitadel\r\n\t\t}\r\n\t}\r\n\tConVars\r\n\t{\r\n\t\t\"r_ssao\"\t\t\"1\"\t// ambient occlusion\r\n\t\tfps_max 400\r\n\t\t\"r_shadows\"\t\t\"true\"\r\n\t}\r\n\tSceneSystem\r\n\t{\r\n\t\tCSMCascadeResolution\t2048\r\n\t}\r\n}\r\n";

fn catalog_json(schema: u32) -> String {
    let preset = |id: &str, tier: &str, recommended: bool, value: &str| {
        format!(
            r#"{{"id":"{id}","name":"{id} name","author":"someone","tier":"{tier}","blurb":"{tier} blurb","recommended":{recommended},"source":{{"kind":"bundled"}},"entries":[{{"path":["ConVars","r_ssao"],"value":"{value}"}},{{"path":["ConVars","sv_cheats"],"value":"1"}},{{"path":["FileSystem","x"],"value":"1"}},{{"path":["SceneSystem","CSMCascadeResolution"],"value":"512"}},{{"path":["ConVars","host_timescale"],"value":"2"}},{{"path":["ConVars","r_blocked"],"value":"1"}},{{"path":["ConVars","r_shadows"],"value":null}}]}}"#
        )
    };
    let presets = [
        preset("pretty-one", "pretty", false, "1"),
        preset("balanced-other", "balanced", false, "9"),
        preset("balanced-rec", "balanced", true, "0"),
        // A tier Shiv does not offer: skipped, the catalog still parses.
        preset("lean-one", "lean", false, "0"),
        preset("future-one", "ultra", false, "0"),
        preset("sweaty-one", "sweaty", false, "0"),
        preset("potato-one", "potato", false, "0"),
    ]
    .join(",");
    format!(
        r#"{{"schemaVersion":{schema},"version":"2026.11.07-1","generatedAt":"2026-11-07T00:00:00Z",
        "categories":[{{"id":"lighting","label":"Lighting","weight":2}}],
        "convars":[
          {{"name":"r_ssao","kind":"bool","default":"1","help":"Screen-space ambient occlusion","category":"lighting"}},
          {{"name":"fps_max","kind":"int","min":0,"max":1000,"category":"other","flags":["archive"]}},
          {{"name":"r_shadows","kind":"bool","category":"lighting","status":"active"}},
          {{"name":"r_blocked","kind":"bool","category":"other","status":"blocked"}},
          {{"name":"old_thing","kind":"float","category":"other","status":"removed","statusSinceBuild":6000}}
        ],
        "stock":[{{"build":1,"date":"2026-01-01","entries":[]}}],
        "community":[],
        "presets":[{presets}],
        "rules":{{"excludedSections":["FileSystem","MaterialSystem2/RenderModes"],"guardedSections":["SceneSystem"],
          "denied":[{{"path":["ConVars","sv_cheats"],"reason":"cheats"}},{{"pattern":"^(host_)?timescale$","reason":"speed"}}]}}}}"#
    )
}

fn fixture(text: &str) -> (tempfile::TempDir, GamePaths) {
    let dir = tempfile::tempdir().unwrap();
    let paths = GamePaths::from_root(dir.path().join("Deadlock"));
    fs::create_dir_all(paths.gameinfo.parent().unwrap()).unwrap();
    fs::write(&paths.gameinfo, text).unwrap();
    (dir, paths)
}

fn entry(path: &[&str], value: Option<&str>) -> ConfigEntry {
    ConfigEntry {
        path: path.iter().map(|s| (*s).to_string()).collect(),
        value: value.map(str::to_string),
    }
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

fn backup(paths: &GamePaths) -> std::path::PathBuf {
    paths.gameinfo.with_file_name("gameinfo.gi.bak")
}

#[test]
fn parse_rejects_wrong_schema_version() {
    let err = Catalog::parse(catalog_json(2).as_bytes()).unwrap_err();
    assert!(
        matches!(
            err,
            PerfError::SchemaVersion {
                found: 2,
                expected: 1
            }
        ),
        "{err:?}"
    );
    assert!(matches!(
        Catalog::parse(b"{not json"),
        Err(PerfError::InvalidCatalog(_))
    ));
    let catalog = Catalog::parse(catalog_json(1).as_bytes()).unwrap();
    assert_eq!(catalog.version(), "2026.11.07-1");
    let ssao = catalog.convar("R_SSAO").unwrap();
    assert_eq!(ssao.kind, ConvarKind::Bool);
    assert_eq!(ssao.help.as_deref(), Some("Screen-space ambient occlusion"));
    assert_eq!(catalog.convar("fps_max").unwrap().max, Some(1000.0));
}

#[test]
fn preset_returns_entries_for_each_tier() {
    let catalog = Catalog::parse(catalog_json(1).as_bytes()).unwrap();
    for tier in Tier::ALL {
        let entries = catalog.preset(tier);
        // Denied, blocked, excluded and engine-section entries are dropped; ConVars
        // edits, including commenting out, are kept.
        assert_eq!(entries.len(), 2, "{tier:?}: {entries:?}");
        assert_eq!(entries[1], entry(&["ConVars", "r_shadows"], None));
        assert!(catalog.preset_info(tier).is_some());
    }
    // The recommended preset wins within a tier.
    assert_eq!(
        catalog.preset(Tier::Balanced)[0],
        entry(&["ConVars", "r_ssao"], Some("0"))
    );
    assert_eq!(
        catalog.preset_info(Tier::Balanced).unwrap().id,
        "balanced-rec"
    );
    assert_eq!(Tier::ALL.len(), 4, "no lean tier");
    let mut bad = catalog_json(1);
    bad = bad.replacen(r#""tier":"potato""#, r#""tier":7"#, 1);
    assert!(
        Catalog::parse(bad.as_bytes()).is_err(),
        "a malformed tier still fails the catalog"
    );
    assert_eq!(
        catalog.denied_reason(&["convars".into(), "TIMESCALE".into()]),
        Some("speed")
    );
}

#[test]
fn admitted_counts_what_it_leaves_out() {
    let catalog = Catalog::parse(catalog_json(1).as_bytes()).unwrap();
    let admitted = catalog.admitted(&[
        entry(&["ConVars", "r_ssao"], Some("1")),
        entry(&["ConVars", "r_ssao"], Some("0")),
        entry(&["ConVars", "fps_max"], None),
        entry(&["ConVars", "sv_cheats"], Some("1")),
        entry(&["ConVars", "host_timescale"], Some("2")),
        entry(&["ConVars", "r_blocked"], Some("1")),
        entry(&["SceneSystem", "CSMCascadeResolution"], Some("512")),
        entry(&["FileSystem", "x"], Some("1")),
    ]);
    // The last entry for a path wins without counting as left out; denied
    // (by path and by pattern), blocked and non-ConVars entries are left
    // out and counted.
    assert_eq!(
        admitted.entries,
        vec![
            entry(&["ConVars", "r_ssao"], Some("0")),
            entry(&["ConVars", "fps_max"], None),
        ]
    );
    assert_eq!(admitted.skipped, 5);
}

#[test]
fn apply_changes_only_listed_keys() {
    let (_d, paths) = fixture(GAMEINFO);
    perf::apply(
        &paths,
        &[
            entry(&["ConVars", "r_ssao"], Some("0")),
            entry(&["convars", "FPS_MAX"], Some("240")),
            entry(&["ConVars", "r_new"], Some("2")),
            entry(&["SceneSystem", "CSMCascadeResolution"], Some("512")),
        ],
    )
    .unwrap();
    let expected = GAMEINFO
        .replace(
            "\"r_ssao\"\t\t\"1\"\t// ambient",
            "\"r_ssao\"\t\t\"0\"\t// ambient",
        )
        .replace("fps_max 400", "fps_max 240")
        .replace(
            "\"r_shadows\"\t\t\"true\"\r\n",
            "\"r_shadows\"\t\t\"true\"\r\n\t\t\"r_new\"\t\t\"2\"\r\n",
        )
        .replace("CSMCascadeResolution\t2048", "CSMCascadeResolution\t512");
    assert_eq!(read(&paths.gameinfo), expected);

    let current = perf::read_current(&paths).unwrap();
    assert!(current.contains(&entry(&["ConVars", "fps_max"], Some("240"))));
    assert!(current.contains(&entry(&["ConVars", "r_new"], Some("2"))));
    assert!(
        !current.iter().any(|e| e.path[0] == "FileSystem"),
        "FileSystem is not a setting"
    );

    // Applying the same values again writes nothing new.
    perf::apply(&paths, &[entry(&["ConVars", "fps_max"], Some("240"))]).unwrap();
    assert_eq!(read(&paths.gameinfo), expected);

    // Nothing outside a block, nothing in FileSystem, nothing unquotable.
    for bad in [
        entry(&["FileSystem", "SearchPaths", "Game"], Some("x")),
        entry(&["ConVars", "r_ssao"], Some("a\"b")),
        entry(&["ConVars"], Some("1")),
        entry(&["Missing", "key"], Some("1")),
    ] {
        assert!(
            perf::apply(&paths, std::slice::from_ref(&bad)).is_err(),
            "{bad:?}"
        );
        assert_eq!(read(&paths.gameinfo), expected);
    }
}

#[test]
fn apply_none_comments_key_out() {
    let (_d, paths) = fixture(GAMEINFO);
    perf::apply(&paths, &[entry(&["ConVars", "fps_max"], None)]).unwrap();
    let text = read(&paths.gameinfo);
    assert!(text.contains("\t\t// shiv off: fps_max 400\r\n"), "{text}");
    let current = perf::read_current(&paths).unwrap();
    assert!(!current.iter().any(|e| e.path.last().unwrap() == "fps_max"));
    // A key that is not there stays absent.
    perf::apply(&paths, &[entry(&["ConVars", "nope"], None)]).unwrap();
    assert_eq!(read(&paths.gameinfo), text);
    // Setting it again brings back the commented line, not a second one.
    perf::apply(&paths, &[entry(&["ConVars", "fps_max"], Some("300"))]).unwrap();
    assert_eq!(
        read(&paths.gameinfo),
        GAMEINFO.replace("fps_max 400", "fps_max 300")
    );
}

#[cfg(unix)]
#[test]
fn apply_fails_cleanly_on_readonly_gameinfo() {
    use std::os::unix::fs::PermissionsExt;
    let (_d, paths) = fixture(GAMEINFO);
    fs::set_permissions(&paths.gameinfo, fs::Permissions::from_mode(0o444)).unwrap();
    let err = perf::apply(&paths, &[entry(&["ConVars", "fps_max"], Some("1"))]).unwrap_err();
    assert!(matches!(err, PerfError::ReadOnly(_)), "{err:?}");
    assert_eq!(read(&paths.gameinfo), GAMEINFO);
    assert!(!backup(&paths).exists());
    let leftovers: Vec<_> = fs::read_dir(paths.gameinfo.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(leftovers.len(), 1, "{leftovers:?}");
    let mode = fs::metadata(&paths.gameinfo).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o444);
}

#[test]
fn apply_keeps_backup() {
    let (_d, paths) = fixture(GAMEINFO);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&paths.gameinfo, fs::Permissions::from_mode(0o640)).unwrap();
    }
    perf::apply(&paths, &[entry(&["ConVars", "r_ssao"], Some("0"))]).unwrap();
    assert_eq!(read(&backup(&paths)), GAMEINFO);
    assert_ne!(read(&paths.gameinfo), GAMEINFO);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&paths.gameinfo).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640);
    }
    // A failed apply leaves both files alone.
    let before = read(&paths.gameinfo);
    assert!(perf::apply(&paths, &[entry(&["Nope", "x"], Some("1"))]).is_err());
    assert_eq!(read(&paths.gameinfo), before);
    assert_eq!(read(&backup(&paths)), GAMEINFO);
}

#[test]
fn unparsable_gameinfo_is_refused() {
    let (_d, paths) = fixture("GameInfo { ConVars { a 1 }");
    assert!(perf::read_current(&paths).is_err());
    assert!(perf::apply(&paths, &[entry(&["ConVars", "a"], Some("2"))]).is_err());
    assert_eq!(read(&paths.gameinfo), "GameInfo { ConVars { a 1 }");
}

#[test]
fn catalog_cache_keeps_etag_and_ignores_bad_bodies() {
    let dir = tempfile::tempdir().unwrap();
    let cache = CatalogCache::new(dir.path().join("perf"));
    assert!(cache.load().is_none());
    assert!(cache.store("\"v1\"", catalog_json(2).as_bytes()).is_err());
    assert!(cache.load().is_none());
    let stored = cache
        .store("\"v1\"", catalog_json(1).as_bytes())
        .expect("store");
    assert_eq!(stored.version(), "2026.11.07-1");
    let (etag, loaded) = cache.load().expect("cached");
    assert_eq!(etag.as_deref(), Some("\"v1\""));
    assert_eq!(loaded.version(), "2026.11.07-1");
}

#[test]
fn values_compare_like_the_engine_reads_them() {
    assert!(perf::values_equal("true", "1"));
    assert!(perf::values_equal("FALSE", "0"));
    assert!(perf::values_equal("1.50", "1.5"));
    assert!(!perf::values_equal("1", "2"));
    assert!(!perf::values_equal("abc", "abd"));
}

#[test]
fn shiv_orig_keeps_the_file_from_before_the_first_apply() {
    let (_d, paths) = fixture(GAMEINFO);
    let orig = perf::original_backup_path(&paths);
    assert_eq!(orig, paths.gameinfo.with_file_name("gameinfo.gi.shiv-orig"));
    perf::apply(&paths, &[entry(&["ConVars", "r_ssao"], Some("0"))]).unwrap();
    let after_first = read(&paths.gameinfo);
    perf::apply(&paths, &[entry(&["ConVars", "fps_max"], Some("100"))]).unwrap();
    assert_eq!(read(&orig), GAMEINFO, "a second apply is not a reset");
    assert_eq!(read(&backup(&paths)), after_first, "the rolling backup");
    assert!(read(&paths.gameinfo).contains("fps_max 100"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&orig), mode(&paths.gameinfo));
    }
}

/// Same text is Ok. The same file outside `SearchPaths`, once it mounts
/// `citadel/addons`, is Ok too. A recorded write that is no longer the
/// expected text is Outdated. No mount and not our bytes is Reset. A mount
/// that is neither is Edited.
#[test]
fn gameinfo_state_follows_the_recorded_write_then_the_addons_mount() {
    let head = "GameInfo\n{\n\tFileSystem\n\t{\n\t\tSearchPaths\n\t\t{\n";
    let tail = "\t\t}\n\t}\n\tConVars\n\t{\n\t\tfps_max 400\n\t}\n}\n";
    let expected = format!("{head}\t\t\tGame\t\t\tcitadel\n{tail}");
    let mounted = format!("{head}\t\t\tGame\t\t\tcitadel/addons\n\t\t\tGame\t\t\tcitadel\n{tail}");
    let moved = expected.replace("fps_max 400", "fps_max 100");
    assert_ne!(mounted, expected);
    assert_eq!(
        perf::classify_gameinfo(&expected, &expected, None),
        GameinfoState::Ok
    );
    assert_eq!(
        perf::classify_gameinfo(&mounted, &expected, None),
        GameinfoState::Ok
    );
    assert_eq!(
        perf::classify_gameinfo(&expected, &moved, Some(perf::gameinfo_stamp(&expected))),
        GameinfoState::Outdated
    );
    assert_eq!(
        perf::classify_gameinfo(&expected, &moved, None),
        GameinfoState::Reset
    );
    assert_eq!(
        perf::classify_gameinfo(&mounted, &moved, Some(perf::gameinfo_stamp(&expected))),
        GameinfoState::Edited
    );
}

/// A game update that drops the addons mount replaces `.shiv-orig` with that
/// file. A later apply, and a hand edit that keeps the mount, leave the
/// refreshed copy in place.
#[test]
fn a_reset_gameinfo_refreshes_shiv_orig() {
    let (_d, paths) = fixture(GAMEINFO);
    let orig = perf::original_backup_path(&paths);
    perf::apply(&paths, &[entry(&["ConVars", "r_ssao"], Some("0"))]).unwrap();
    assert_eq!(read(&orig), GAMEINFO);

    let stock = "\"GameInfo\"\r\n{\r\n\tFileSystem\r\n\t{\r\n\t\tSearchPaths\r\n\t\t{\r\n\t\t\tGame\t\t\t\tcitadel\r\n\t\t}\r\n\t}\r\n\tConVars\r\n\t{\r\n\t\tfps_max 400\r\n\t}\r\n}\r\n";
    fs::write(&paths.gameinfo, stock).unwrap();
    perf::apply(&paths, &[entry(&["ConVars", "fps_max"], Some("100"))]).unwrap();
    assert_eq!(read(&orig), stock, "the reset file is the new original");
    assert!(read(&paths.gameinfo).contains("fps_max 100"));

    let after_reset = read(&paths.gameinfo);
    perf::apply(&paths, &[entry(&["ConVars", "fps_max"], Some("50"))]).unwrap();
    assert_eq!(read(&orig), stock, "the next apply is not a reset");
    assert_ne!(read(&paths.gameinfo), after_reset);

    fs::write(&paths.gameinfo, GAMEINFO).unwrap();
    perf::apply(&paths, &[entry(&["ConVars", "fps_max"], Some("80"))]).unwrap();
    assert_eq!(read(&orig), stock, "a mounted file is an edit, not a reset");
}

#[test]
fn bom_and_already_commented_keys_are_kept() {
    let text =
        "\u{feff}GameInfo\n{\n\tConVars\n\t{\n\t\t// \"r_old\" \"1\"\n\t\tfps_max 400\n\t}\n}\n";
    let (_d, paths) = fixture(text);
    perf::apply(&paths, &[entry(&["ConVars", "fps_max"], Some("200"))]).unwrap();
    assert_eq!(
        read(&paths.gameinfo),
        text.replace("fps_max 400", "fps_max 200")
    );
    // A key commented out by hand counts as absent: commenting it out again
    // changes nothing, and setting it adds a line (the hand comment stays).
    perf::apply(&paths, &[entry(&["ConVars", "r_old"], None)]).unwrap();
    assert_eq!(
        read(&paths.gameinfo),
        text.replace("fps_max 400", "fps_max 200")
    );
    perf::apply(&paths, &[entry(&["ConVars", "r_old"], Some("0"))]).unwrap();
    assert_eq!(
        read(&paths.gameinfo),
        text.replace("fps_max 400\n", "fps_max 200\n\t\t\"r_old\"\t\t\"0\"\n")
    );
    assert!(read(&paths.gameinfo).starts_with('\u{feff}'));
}
