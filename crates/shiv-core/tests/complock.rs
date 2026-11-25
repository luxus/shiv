//! CompLock's pack data: the options asset's shape, the part download urls,
//! the CRC-32 the site's pack builder uses, and the piece-assembly contract
//! (a big pak's numbered pieces concatenate into one raw-DEFLATE stream).
//!
//! The expectations are derived from the asset, not hand-computed
//! (docs/code-quality.md): the shape invariants read whatever the embedded
//! asset holds, and the soundcap part fixture in `tests/data/` — fetched
//! once from the url the site declared for it, with its declared crc, size
//! and compressed size pinned beside it — proves the assembly contract
//! against recorded live bytes, offline.

use std::collections::BTreeSet;
use std::path::PathBuf;

use shiv_core::complock;

/// The content and CRC of the tiny fixture, made by the same python that
/// verified the live parts (zlib wbits=-15): 100 bytes whose CRC the
/// table-based implementation must reproduce.
const TINY_CONTENT: &[u8] = b"CompLock part fixture: the sound stack update cap writes VPK bytes whose crc the pack data declares.";
const TINY_CRC: u32 = 2_680_929_023;

/// Where the first piece of a split stream ends: any split point proves the
/// concatenation contract, because the site's pieces break mid-stream.
fn split_at_half(bytes: &[u8]) -> (&[u8], &[u8]) {
    let at = bytes.len() / 2;
    let (first, second) = bytes.split_at(at);
    assert!(!first.is_empty() && !second.is_empty());
    (first, second)
}

/// The live soundcap part, fetched once (2026-11-21, build 6860) from the
/// url the pack data declared for it: `/dl/pack/soundcap.af99a150bd.z`.
/// Its compressed size, decompressed size and CRC are pinned here as the
/// site declared them at fetch time — the fixture proves the crc32, inflate
/// and unpack contracts against recorded live bytes, while the watch
/// workflow owns keeping the embedded asset current with the site.
const SOUNDCAP_CSIZE: u64 = 7_611;
const SOUNDCAP_SIZE: u64 = 24_989;
const SOUNDCAP_CRC: u32 = 3_218_033_789;

#[test]
fn options_carry_ids_groups_and_unique_pak_urls() {
    let options = complock::options();
    assert!(!options.is_empty(), "the site's options are there");
    let ids: BTreeSet<&str> = options.iter().map(|o| o.id.as_str()).collect();
    assert_eq!(ids.len(), options.len(), "option ids are unique");
    let group_ids: BTreeSet<&str> = complock::groups().iter().map(|g| g.id.as_str()).collect();
    assert!(!group_ids.is_empty());
    for option in options {
        assert!(!option.title.is_empty(), "{} has no title", option.id);
        assert!(
            option
                .group
                .as_deref()
                .is_none_or(|g| group_ids.contains(g)),
            "{} names a group the site does not list: {:?}",
            option.id,
            option.group
        );
        let mut urls = BTreeSet::new();
        for part in option.pak.iter().flat_map(|p| &p.parts) {
            assert!(
                !part.urls.is_empty(),
                "{} has a part without urls",
                option.id
            );
            assert!(part.size > 0, "{} names no size", option.id);
            for url in &part.urls {
                assert!(
                    complock::part_url(url)
                        .is_some_and(|full| full.starts_with("https://deadlockconfig.com/")),
                    "{}'s url {url} does not resolve on the site",
                    option.id
                );
                assert!(urls.insert(url.as_str()), "{} repeats {url}", option.id);
            }
        }
    }
}

/// Any option that carries off values addresses the lines its on values
/// write. Build 6860 has none left (ragdolls went opt-in, #37); the site may
/// bring them back on any card.
#[test]
fn convar_options_carry_matching_on_and_off_values() {
    for option in complock::options()
        .iter()
        .filter(|o| !o.off_convars.is_empty())
    {
        assert!(
            !option.convars.is_empty(),
            "{} has off values but no on values",
            option.id
        );
        let on: BTreeSet<Vec<String>> = option
            .convars
            .iter()
            .map(|e| {
                e.path
                    .iter()
                    .map(|k| k.to_ascii_lowercase())
                    .collect::<Vec<_>>()
            })
            .collect();
        let off: BTreeSet<Vec<String>> = option
            .off_convars
            .iter()
            .map(|e| {
                e.path
                    .iter()
                    .map(|k| k.to_ascii_lowercase())
                    .collect::<Vec<_>>()
            })
            .collect();
        assert_eq!(
            on, off,
            "{}'s off state must address the lines its on state writes",
            option.id
        );
        for pair in option.convars.iter().zip(&option.off_convars) {
            assert_ne!(
                pair.0.value, pair.1.value,
                "{}'s off value is its on value",
                option.id
            );
        }
    }
}

#[test]
fn choices_are_distinct_with_at_most_one_default() {
    for option in complock::options() {
        let ids: BTreeSet<&str> = option.choices.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(
            ids.len(),
            option.choices.len(),
            "{} repeats a choice id",
            option.id
        );
        assert!(
            option.choices.iter().filter(|c| c.default).count() <= 1,
            "{} marks several choices default",
            option.id
        );
    }
    let hud = complock::hud().expect("the HUD++ base");
    let ids: BTreeSet<&str> = hud.choices.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids.len(), hud.choices.len(), "the HUD choices are distinct");
    for (index, _choice) in hud.choices.iter().enumerate() {
        assert!(
            hud.pak_of_choice(index)
                .is_some_and(|p| !p.parts.is_empty()),
            "HUD choice {index} has no pak"
        );
    }
}

#[test]
fn the_fov_field_edits_a_line_their_gameinfo_sets() {
    let fov = complock::fov().expect("the gameinfo card carries the FOV field");
    assert!(!fov.cvar.is_empty());
    assert!(fov.from < fov.to);
    assert!(fov.by > 0.0);
    let published = complock::entries()
        .into_iter()
        .find(|e| e.path.len() == 2 && e.path[1].eq_ignore_ascii_case(&fov.cvar))
        .expect("their published gameinfo sets the FOV convar");
    assert_eq!(
        published.value.as_deref(),
        Some(fov.default.as_str()),
        "the slider's default is the value their gameinfo publishes"
    );
}

#[test]
fn crc32_matches_the_known_vector_and_the_independent_implementation() {
    assert_eq!(complock::crc32(b"123456789"), 0xCBF4_3926);
    assert_eq!(complock::crc32(&[]), 0);
    let mut bytes = Vec::new();
    let mut state = 5_421u64;
    for _ in 0..4096 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        bytes.push((state >> 33) as u8);
    }
    let mut crc = crc32fast::Hasher::new();
    crc.update(&bytes);
    assert_eq!(
        complock::crc32(&bytes),
        crc.finalize(),
        "the table-based crc32 agrees with crc32fast over the same bytes"
    );
}

#[test]
fn pieces_concatenate_into_one_stream_and_verify() {
    let bytes = std::fs::read(soundcap_fixture()).expect("the soundcap fixture");
    let (first, second) = split_at_half(&bytes);
    let stream = complock::assemble(&[first, second]);
    assert_eq!(stream, bytes);
    let unpacked = complock::inflate(&stream).expect("the split stream inflates");
    assert!(
        complock::verify_decompressed(&unpacked, SOUNDCAP_CRC, SOUNDCAP_SIZE),
        "the split-and-reassembled part is still the declared VPK"
    );
    assert_eq!(complock::crc32(TINY_CONTENT), TINY_CRC);
    assert!(!complock::verify_decompressed(
        &unpacked,
        SOUNDCAP_CRC ^ 1,
        SOUNDCAP_SIZE
    ));
    assert!(!complock::verify_decompressed(
        &unpacked,
        SOUNDCAP_CRC,
        SOUNDCAP_SIZE + 1
    ));
    assert!(complock::inflate(b"not a deflate stream").is_err());
}

fn soundcap_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data/complock-soundcap-part.z")
}

#[test]
fn the_soundcap_part_unpacks_to_the_declared_vpk() {
    let bytes = std::fs::read(soundcap_fixture()).expect("the soundcap fixture");
    assert_eq!(
        bytes.len() as u64,
        SOUNDCAP_CSIZE,
        "the fixture is the bytes the site's url served"
    );
    let unpacked = complock::inflate(&bytes).expect("the part is a deflate stream");
    assert!(complock::verify_decompressed(
        &unpacked,
        SOUNDCAP_CRC,
        SOUNDCAP_SIZE
    ));
    let dir = tempfile::tempdir().expect("tempdir");
    let piece = dir.path().join("piece.z");
    std::fs::write(&piece, &bytes).expect("write the piece");
    let dest = dir.path().join("pak03_dir.vpk");
    complock::unpack_pak(
        &[piece],
        &dest,
        SOUNDCAP_CRC,
        SOUNDCAP_SIZE,
        SOUNDCAP_CSIZE,
        "/dl/pack/soundcap.af99a150bd.z",
    )
    .expect("the part unpacks");
    assert_eq!(std::fs::read(&dest).expect("the vpk"), unpacked);
}

#[test]
fn unpack_pak_refuses_mismatched_parts_and_leaves_nothing_behind() {
    let dir = tempfile::tempdir().expect("tempdir");
    let piece = dir.path().join("piece.z");
    let dest = dir.path().join("part.vpk");
    let mut head = [0x34, 0x12, 0xaa, 0x55].to_vec();
    head.extend_from_slice(TINY_CONTENT);
    use std::io::Write as _;
    let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::new(9));
    encoder.write_all(&head).expect("compress");
    let stream = encoder.finish().expect("finish");
    std::fs::write(&piece, &stream).expect("write the vpk piece");
    let crc = complock::crc32(&head);
    let csize = stream.len() as u64;
    complock::unpack_pak(
        std::slice::from_ref(&piece),
        &dest,
        crc,
        head.len() as u64,
        csize,
        "piece.z",
    )
    .expect("a VPK-shaped part unpacks");
    assert_eq!(std::fs::read(&dest).expect("the vpk"), head);
    for (crc, size) in [(crc ^ 1, head.len() as u64), (crc, head.len() as u64 - 1)] {
        assert!(
            complock::unpack_pak(
                std::slice::from_ref(&piece),
                &dest,
                crc,
                size,
                csize,
                "piece.z"
            )
            .is_err(),
            "a wrong crc or size is refused"
        );
        assert!(!dest.exists(), "the refusal left no partial file");
    }
    let mut plain = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::new(9));
    plain.write_all(TINY_CONTENT).expect("compress");
    let plain_bytes = plain.finish().expect("finish");
    let plain_csize = plain_bytes.len() as u64;
    std::fs::write(&piece, &plain_bytes).expect("write the plain piece");
    assert!(
        complock::unpack_pak(
            &[piece],
            &dest,
            TINY_CRC,
            TINY_CONTENT.len() as u64,
            plain_csize,
            "piece.z",
        )
        .is_err(),
        "content without the VPK signature is refused"
    );
    assert!(!dest.exists());
}

/// Output past the declared size stops before that chunk is kept. A compressed
/// length other than `csize`, or a SHA-256 prefix in the url that the
/// decompressed bytes do not have, removes the partial file. The soundcap
/// fixture's own url prefix is accepted by
/// `the_soundcap_part_unpacks_to_the_declared_vpk`.
#[test]
fn unpack_pak_stops_when_the_output_or_the_download_does_not_match() {
    let dir = tempfile::tempdir().expect("tempdir");
    let piece = dir.path().join("piece.z");
    let dest = dir.path().join("part.vpk");
    let mut head = [0x34, 0x12, 0xaa, 0x55].to_vec();
    head.extend_from_slice(TINY_CONTENT);
    use std::io::Write as _;
    let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::new(9));
    encoder.write_all(&head).expect("compress");
    let stream = encoder.finish().expect("finish");
    std::fs::write(&piece, &stream).expect("write the vpk piece");
    let crc = complock::crc32(&head);
    let size = head.len() as u64;
    let csize = stream.len() as u64;
    let too_big = complock::unpack_pak(
        std::slice::from_ref(&piece),
        &dest,
        crc,
        size - 1,
        csize,
        "piece.z",
    )
    .expect_err("output past size");
    assert!(too_big.contains("larger than the site says"), "{too_big}");
    assert!(!dest.exists(), "the oversized part left nothing");
    let bad_csize = complock::unpack_pak(
        std::slice::from_ref(&piece),
        &dest,
        crc,
        size,
        csize + 1,
        "piece.z",
    )
    .expect_err("compressed size");
    assert!(bad_csize.contains("compressed"), "{bad_csize}");
    assert!(!dest.exists());
    let bad_sha = complock::unpack_pak(
        std::slice::from_ref(&piece),
        &dest,
        crc,
        size,
        csize,
        "part.0000000000.z",
    )
    .expect_err("sha prefix");
    assert!(bad_sha.contains("SHA-256"), "{bad_sha}");
    assert!(!dest.exists());
    complock::unpack_pak(
        std::slice::from_ref(&piece),
        &dest,
        crc,
        size,
        csize,
        "piece.z",
    )
    .expect("a url without a hash suffix still unpacks");
    assert_eq!(std::fs::read(&dest).expect("kept"), head);
}

#[test]
fn part_urls_resolve_only_on_the_site() {
    assert_eq!(
        complock::part_url("/dl/pack/hud.z").as_deref(),
        Some("https://deadlockconfig.com/dl/pack/hud.z")
    );
    assert_eq!(
        complock::part_url("https://example.com/x.z").as_deref(),
        Some("https://example.com/x.z")
    );
    assert_eq!(
        complock::part_url("http://127.0.0.1:8080/x.z").as_deref(),
        Some("http://127.0.0.1:8080/x.z")
    );
    assert_eq!(complock::part_url("http://example.com/x.z"), None);
    assert_eq!(complock::part_url("http://127.0.0.1@evil.com/x.z"), None);
    assert_eq!(complock::part_url("//evil.com/x.z"), None);
    assert_eq!(complock::part_url("dl/pack/hud.z"), None);
    assert_eq!(complock::part_url(""), None);
}

/// Every pak the embedded pack data names: the options', their choices',
/// and the HUD base's default and variants.
fn every_pak() -> Vec<&'static complock::Pak> {
    let options = complock::options().iter().flat_map(|o| {
        o.pak
            .iter()
            .chain(o.choices.iter().filter_map(|c| c.pak.as_ref()))
    });
    let hud = complock::hud().into_iter().flat_map(|h| {
        h.default_pak
            .iter()
            .chain(h.variants.values())
            .chain(h.choices.iter().filter_map(|c| c.pak.as_ref()))
    });
    options.chain(hud).collect()
}

/// The live parts against the embedded pack data: every part of every pak,
/// downloaded beforehand into `SHIV_COMPLOCK_PARTS` under its url's file
/// name (`curl -o parts/$(basename url) https://deadlockconfig.com$url`),
/// unpacks through [`complock::unpack_pak`] to the declared size and CRC —
/// the multi-piece paks (nocloth) included. The suite stays offline; this
/// is the proof run that the embedded asset still describes the site's
/// bytes.
#[test]
#[ignore = "needs the live parts downloaded into SHIV_COMPLOCK_PARTS"]
fn live_parts_unpack_to_the_declared_vpks() {
    let dir = PathBuf::from(std::env::var("SHIV_COMPLOCK_PARTS").expect("SHIV_COMPLOCK_PARTS"));
    let out = tempfile::tempdir().expect("tempdir");
    let mut checked = BTreeSet::new();
    for pak in every_pak() {
        for part in &pak.parts {
            let pieces: Vec<PathBuf> = part
                .urls
                .iter()
                .map(|url| dir.join(url.rsplit('/').next().expect("a file name")))
                .collect();
            let key = pieces.clone();
            if !checked.insert(key) {
                continue;
            }
            let dest = out.path().join(&part.name);
            complock::unpack_pak(
                &pieces,
                &dest,
                part.crc,
                part.size,
                part.csize,
                part.urls.first().map(String::as_str).unwrap_or(""),
            )
            .unwrap_or_else(|e| panic!("{} ({:?}): {e}", pak.title, part.urls));
            println!(
                "{} {} pieces={} size={} crc={}",
                pak.title,
                part.name,
                pieces.len(),
                part.size,
                part.crc
            );
            std::fs::remove_file(&dest).expect("the unpacked part");
        }
    }
    assert!(
        checked.iter().any(|pieces| pieces.len() > 1),
        "the data names a multi-piece part"
    );
    println!("{} parts unpacked and CRC-verified", checked.len());
}

const OPTIONS_ASSET: &str = include_str!("../assets/complock-options.json");
const GAMEINFO_ASSET: &str = include_str!("../assets/complock-gameinfo.gi");

/// The embedded options asset as a JSON value to mutate.
fn asset_value() -> serde_json::Value {
    serde_json::from_str(OPTIONS_ASSET).expect("the asset parses")
}

/// The watch's data checks of a mutated asset against the embedded one.
fn check_mutated(mutate: impl FnOnce(&mut serde_json::Value)) -> Vec<complock::PackProblem> {
    let mut fresh = asset_value();
    mutate(&mut fresh);
    complock::check_pack(&fresh.to_string(), GAMEINFO_ASSET, OPTIONS_ASSET)
}

#[test]
fn the_embedded_data_checks_clean_against_itself() {
    assert_eq!(
        complock::check_pack(OPTIONS_ASSET, GAMEINFO_ASSET, OPTIONS_ASSET),
        []
    );
}

#[test]
fn a_new_key_is_reported_with_its_section() {
    let problems = check_mutated(|v| {
        v["option"][0]["renamed_gains"] = serde_json::json!("+5%");
    });
    assert_eq!(
        problems,
        [complock::PackProblem::NewKey {
            section: "option",
            key: "renamed_gains".into()
        }]
    );
}

#[test]
fn a_pak_missing_from_files_is_reported_with_its_card() {
    let (card, pak) = {
        let v = asset_value();
        let option = v["option"]
            .as_array()
            .expect("options")
            .iter()
            .find(|o| o.get("pak").is_some())
            .expect("an option with a pak")
            .clone();
        (
            option["id"].as_str().expect("id").to_owned(),
            option["pak"].as_str().expect("pak").to_owned(),
        )
    };
    let gone = pak.clone();
    let problems = check_mutated(move |v| {
        v["files"].as_object_mut().expect("files").remove(&gone);
    });
    assert!(
        problems.contains(&complock::PackProblem::MissingPak { card, pak }),
        "{problems:?}"
    );
}

#[test]
fn a_retired_option_and_an_unparsable_doc_are_reported() {
    let retired = asset_value()["option"][0]["id"]
        .as_str()
        .expect("id")
        .to_owned();
    let id = retired.clone();
    let problems = check_mutated(move |v| {
        v["option"]
            .as_array_mut()
            .expect("options")
            .retain(|o| o["id"] != id.as_str());
    });
    assert!(
        problems.contains(&complock::PackProblem::GoneOption(retired)),
        "{problems:?}"
    );
    assert!(matches!(
        complock::check_pack("{\"option\": 3}", GAMEINFO_ASSET, OPTIONS_ASSET).as_slice(),
        [complock::PackProblem::Unparsed(_)]
    ));
    assert_eq!(
        complock::check_pack(OPTIONS_ASSET, "GameInfo\n{\n}\n", OPTIONS_ASSET),
        [complock::PackProblem::GameinfoUnmapped]
    );
}

#[test]
fn the_part_check_proves_the_bytes_and_names_a_bad_crc_or_a_failed_download() {
    let url = "/dl/pack/soundcap.af99a150bd.z";
    let doc = |crc: u32| {
        serde_json::json!({
            "option": [{"id": "soundcap", "pak": "soundcap.vpk"}],
            "files": {"soundcap.vpk": {"title": "Sound stack update cap", "parts": [
                {"name": "pak03_dir.vpk", "size": SOUNDCAP_SIZE, "crc": crc,
                 "csize": SOUNDCAP_CSIZE, "url": url}
            ]}}
        })
        .to_string()
    };
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::copy(soundcap_fixture(), dir.path().join("soundcap.af99a150bd.z"))
        .expect("the fixture");
    assert_eq!(complock::check_parts(&doc(SOUNDCAP_CRC), dir.path()), []);
    let bad_crc = complock::check_parts(&doc(SOUNDCAP_CRC ^ 1), dir.path());
    assert!(
        matches!(bad_crc.as_slice(), [complock::PackProblem::PartFailed { reason, .. }] if reason.contains("CRC")),
        "{bad_crc:?}"
    );
    std::fs::remove_file(dir.path().join("soundcap.af99a150bd.z")).expect("remove");
    let missing = complock::check_parts(&doc(SOUNDCAP_CRC), dir.path());
    assert!(
        matches!(missing.as_slice(), [complock::PackProblem::PartFailed { reason, .. }] if reason.contains("did not download")),
        "{missing:?}"
    );
}

/// The page at deadlockconfig.com, then the cache file, then the baked copy.
/// The fetcher is the only source of the page: this test never opens a socket.
#[test]
fn live_pack_data_uses_the_page_then_the_cache_then_the_baked_copy() {
    let asset = std::fs::read_to_string(manifest("assets/complock-options.json")).expect("asset");
    let gameinfo =
        std::fs::read_to_string(manifest("assets/complock-gameinfo.gi")).expect("gameinfo");
    let mut live: serde_json::Value = serde_json::from_str(&asset).expect("asset json");
    live["gameinfo"] = serde_json::Value::String(gameinfo);
    let newer = live.clone();
    live["app_min"] = serde_json::json!(complock::PACK_APP_VERSION);
    let mut ahead = newer;
    ahead["app_min"] = serde_json::json!(complock::PACK_APP_VERSION + 1);
    let current_html = page(&live.to_string());
    let ahead_html = page(&ahead.to_string());
    let baked_ids: BTreeSet<String> = complock::options().iter().map(|o| o.id.clone()).collect();

    let dir = tempfile::tempdir().expect("tempdir");
    let mut fetches = 0u32;
    let html = current_html.clone();
    let loaded = complock::load_pack(
        |url| {
            fetches += 1;
            assert_eq!(url, "https://deadlockconfig.com/");
            Ok(html.clone())
        },
        Some(dir.path()),
    );
    assert_eq!(fetches, 1);
    assert_eq!(loaded.origin(), complock::PackOrigin::Live);
    assert!(!loaded.too_old());
    assert_eq!(
        loaded
            .options()
            .iter()
            .map(|o| o.id.as_str())
            .collect::<BTreeSet<_>>(),
        baked_ids
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
    );
    assert!(
        !loaded.entries().is_empty(),
        "the page's gameinfo has ConVars"
    );
    let cached =
        std::fs::read_to_string(dir.path().join(complock::PACK_CACHE_FILE)).expect("cache");
    assert_eq!(cached, live.to_string());

    let from_cache = complock::load_pack(|_| Err("offline".into()), Some(dir.path()));
    assert_eq!(from_cache.origin(), complock::PackOrigin::Cache);
    assert!(!from_cache.too_old());

    let ahead_load = complock::load_pack(|_| Ok(ahead_html.clone()), Some(dir.path()));
    assert!(ahead_load.too_old());
    assert!(complock::app_requires_newer_shiv(&ahead.to_string()));
    assert!(!complock::app_requires_newer_shiv(&live.to_string()));
    assert!(!complock::app_requires_newer_shiv(
        &serde_json::json!({"files": {}, "base": [], "option": [], "gameinfo": "x", "app_min": 0})
            .to_string()
    ));
    assert!(!complock::app_requires_newer_shiv(
        &serde_json::json!({"files": {}, "base": [], "option": [], "gameinfo": "x", "app_min": "0.3.5"})
            .to_string()
    ));

    std::fs::write(dir.path().join(complock::PACK_CACHE_FILE), "{").expect("corrupt");
    let baked = complock::load_pack(|_| Err("offline".into()), Some(dir.path()));
    assert_eq!(baked.origin(), complock::PackOrigin::Baked);
    assert!(!baked.too_old());
    assert_eq!(
        baked
            .options()
            .iter()
            .map(|o| o.id.as_str())
            .collect::<BTreeSet<_>>(),
        baked_ids
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
    );

    let ignored = complock::load_pack(|_| Ok("<html>no pack</html>".into()), Some(dir.path()));
    assert_eq!(ignored.origin(), complock::PackOrigin::Baked);

    let now = std::time::Instant::now();
    assert!(complock::pack_refresh_due(None, now));
    assert!(!complock::pack_refresh_due(Some(now), now));
    assert!(!complock::pack_refresh_due(
        Some(now),
        now + std::time::Duration::from_secs(1)
    ));
    assert!(complock::pack_refresh_due(
        Some(now),
        now + complock::PACK_REFRESH
    ));
}

/// `app_min` of 1 is this build. `app_min` of 2 asks for an update. A version
/// string is not a number, so it does not.
#[test]
fn app_min_above_the_pack_app_version_asks_for_an_update() {
    let pack = |app_min| {
        serde_json::json!({
            "files": {},
            "base": [],
            "option": [],
            "gameinfo": "GameInfo { }",
            "app_min": app_min,
        })
        .to_string()
    };
    let current = page(&pack(1));
    let newer = page(&pack(2));
    let loaded = complock::load_pack(|_| Ok(current), None);
    assert!(!loaded.too_old());
    assert_eq!(loaded.origin(), complock::PackOrigin::Live);
    let ahead = complock::load_pack(|_| Ok(newer), None);
    assert!(ahead.too_old());
    assert!(complock::app_requires_newer_shiv(&pack(2)));
    assert!(!complock::app_requires_newer_shiv(&pack(1)));
}

/// A browser install left the part's bytes under another pak number, with no
/// sidecar. The part is renamed onto its own number (its chunk follows) and
/// a sidecar is written. A different file already on that number is left
/// alone and the match stays where it was.
#[test]
fn a_pak_with_the_parts_bytes_is_reused_under_any_number() {
    let root = tempfile::tempdir().expect("tempdir");
    let addons = root.path().join("addons");
    std::fs::create_dir(&addons).expect("addons");
    let bytes = b"browser install of a pack pak";
    let part = complock::Part {
        name: "pak01_dir.vpk".into(),
        size: bytes.len() as u64,
        crc: complock::crc32(bytes),
        csize: 1,
        urls: vec!["/x".into()],
    };
    std::fs::write(addons.join("pak12_dir.vpk"), bytes).expect("pak12");
    std::fs::write(addons.join("pak12_000.vpk"), b"chunk").expect("chunk");
    let guard = shiv_core::install::InstallGuard::acquire(&addons).expect("install guard");
    let reused = complock::claim_reusable(&addons, &part, guard.permit())
        .expect("scan")
        .expect("the bytes are on disk");
    assert_eq!(reused.moved_from.as_deref(), Some("pak12_dir.vpk"));
    assert_eq!(std::fs::read(&reused.path).expect("read"), bytes);
    assert!(!addons.join("pak12_dir.vpk").exists());
    assert_eq!(
        std::fs::read(addons.join("pak01_000.vpk")).expect("chunk moved"),
        b"chunk"
    );
    let meta = shiv_core::install::ModMeta {
        mod_id: "complock-hud".into(),
        name: "HUD++".into(),
        version: Some("1".into()),
        version_label: None,
        thumbnail_url: None,
        nsfw: false,
        hero: None,
        category: None,
    };
    complock::adopt_reused(&addons, &meta, std::slice::from_ref(&part.name)).expect("sidecar");
    let side = shiv_core::sidecar::read(&addons, "complock-hud")
        .expect("read")
        .expect("written");
    assert_eq!(side.vpks.as_slice(), std::slice::from_ref(&part.name));
    assert!(side.stamps.is_none());

    std::fs::write(addons.join(&part.name), b"someone else").expect("occupant");
    std::fs::write(addons.join("pak03_dir.vpk"), bytes).expect("other number");
    std::fs::write(addons.join("pak03_000.vpk"), b"other chunk").expect("other chunk");
    let blocked = complock::claim_reusable(&addons, &part, guard.permit())
        .expect("scan")
        .expect("the bytes are still on disk");
    assert!(blocked.moved_from.is_none());
    assert_eq!(blocked.path, addons.join("pak03_dir.vpk"));
    assert_eq!(blocked.chunks.len(), 1);
    assert_eq!(blocked.chunks[0].path, addons.join("pak03_000.vpk"));
    assert_eq!(blocked.chunks[0].name, "pak01_000.vpk");
    assert_eq!(
        std::fs::read(addons.join(&part.name)).expect("occupant stays"),
        b"someone else"
    );
    assert_eq!(
        std::fs::read(addons.join("pak03_000.vpk")).expect("chunk stays"),
        b"other chunk"
    );

    // Unsafe traversal part.name is refused before touching disk.
    let evil = complock::Part {
        name: "../escape.vpk".into(),
        ..part.clone()
    };
    assert_eq!(
        complock::locate_reusable(&addons, &evil)
            .expect_err("traversal part.name refused")
            .kind(),
        std::io::ErrorKind::InvalidInput
    );

    // A VPK owned by an existing sidecar is never silently renamed.
    std::fs::remove_file(addons.join(&part.name)).expect("free pak01");
    std::fs::remove_file(addons.join("pak01_000.vpk")).expect("free pak01 chunk");
    shiv_core::sidecar::remove(&addons, "complock-hud").expect("remove old sidecar");
    shiv_core::sidecar::write(
        &addons,
        &shiv_core::sidecar::Sidecar {
            mod_id: "owner".into(),
            name: "Owner".into(),
            version: None,
            version_label: None,
            thumbnail_url: None,
            vpks: vec!["pak03_dir.vpk".into()],
            nsfw: None,
            hero: None,
            category: None,
            notes: None,
            source: None,
            stamps: None,
            unverified: None,
            generated: None,
        },
    )
    .expect("write owner sidecar");
    let owned = complock::claim_reusable(&addons, &part, guard.permit())
        .expect("scan")
        .expect("still found");
    assert!(
        owned.moved_from.is_none(),
        "a VPK owned by a sidecar must not be renamed"
    );
    assert!(addons.join("pak03_dir.vpk").is_file());
    assert!(!addons.join(&part.name).exists());
}

fn manifest(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn page(json: &str) -> String {
    format!("<html><script id=\"pack-data\" type=\"application/json\">{json}</script></html>")
}

/// The site's ragdolls card as build 6860 serves it (22 Nov 2026): an
/// opt-in option with no `gameinfo_else`, fetched verbatim (#37).
const RAGDOLLS_6860: &str = include_str!("data/complock-6860-ragdolls.json");

/// The embedded asset with its ragdolls card swapped for `card`.
fn with_ragdolls(card: serde_json::Value) -> serde_json::Value {
    let mut doc = asset_value();
    let slot = doc["option"]
        .as_array_mut()
        .expect("options")
        .iter_mut()
        .find(|o| o["id"] == "ragdolls")
        .expect("the asset carries ragdolls");
    *slot = card;
    doc
}

#[test]
fn an_off_state_key_coming_or_going_is_not_a_schema_change() {
    let upstream: serde_json::Value = serde_json::from_str(RAGDOLLS_6860).expect("fixture");
    assert!(upstream.get("gameinfo_else").is_none());
    let mut before = upstream.clone();
    before["gameinfo_else"] = serde_json::json!({"cl_disable_ragdolls": "false"});
    let (now, then) = (
        with_ragdolls(upstream).to_string(),
        with_ragdolls(before).to_string(),
    );
    assert_eq!(complock::check_pack(&now, GAMEINFO_ASSET, &then), []);
    assert_eq!(complock::check_pack(&then, GAMEINFO_ASSET, &now), []);
}

#[test]
fn an_option_without_gameinfo_else_turns_off_by_leaving_its_line_out() {
    let mut live = with_ragdolls(serde_json::from_str(RAGDOLLS_6860).expect("fixture"));
    live["gameinfo"] = serde_json::Value::String(GAMEINFO_ASSET.to_owned());
    let loaded = complock::load_pack(|_| Ok(page(&live.to_string())), None);
    assert_eq!(loaded.origin(), complock::PackOrigin::Live);
    let ragdolls = loaded
        .options()
        .iter()
        .find(|o| o.id == "ragdolls")
        .expect("ragdolls");
    assert_eq!(
        ragdolls
            .convars
            .iter()
            .map(|e| (e.path.last().cloned(), e.value.clone()))
            .collect::<Vec<_>>(),
        [(
            Some("cl_disable_ragdolls".to_owned()),
            Some("true".to_owned())
        )]
    );
    assert!(ragdolls.off_convars.is_empty());
}

#[test]
fn pack_load_exposes_card_updated_dates_and_releases() {
    assert_eq!(complock::updated_date("complock-hud"), Some("2026-11-22"));
    assert!(complock::updated_date("other").is_none());
    let loaded = complock::baked_pack();
    assert_eq!(loaded.updated_date("complock-hud"), Some("2026-11-22"));
    assert!(loaded.release_for("complock-soundcap", None).is_some());
    assert!(matches!(
        loaded.published("complock-soundcap"),
        shiv_core::drift::Published::Current(_)
    ));
}
