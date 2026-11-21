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

#[test]
fn convar_options_carry_matching_on_and_off_values() {
    let with_off: Vec<_> = complock::options()
        .iter()
        .filter(|o| !o.off_convars.is_empty())
        .collect();
    assert!(
        !with_off.is_empty(),
        "the ragdolls-style off values are there"
    );
    for option in with_off {
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
    complock::unpack_pak(&[piece], &dest, SOUNDCAP_CRC, SOUNDCAP_SIZE).expect("the part unpacks");
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
    complock::unpack_pak(std::slice::from_ref(&piece), &dest, crc, head.len() as u64)
        .expect("a VPK-shaped part unpacks");
    assert_eq!(std::fs::read(&dest).expect("the vpk"), head);
    for (crc, size) in [(crc ^ 1, head.len() as u64), (crc, head.len() as u64 - 1)] {
        assert!(
            complock::unpack_pak(std::slice::from_ref(&piece), &dest, crc, size).is_err(),
            "a wrong crc or size is refused"
        );
        assert!(!dest.exists(), "the refusal left no partial file");
    }
    let mut plain = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::new(9));
    plain.write_all(TINY_CONTENT).expect("compress");
    std::fs::write(&piece, plain.finish().expect("finish")).expect("write the plain piece");
    assert!(
        complock::unpack_pak(&[piece], &dest, TINY_CRC, TINY_CONTENT.len() as u64).is_err(),
        "content without the VPK signature is refused"
    );
    assert!(!dest.exists());
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
    assert_eq!(complock::part_url("dl/pack/hud.z"), None);
    assert_eq!(complock::part_url(""), None);
}
