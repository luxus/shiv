//! CompLock-generated mods: the pinned scripts rebuild the site's paks, a
//! template change is an update, and a script change is not.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::Value;
use shiv_core::drift::{self, Drift, Published};
use shiv_core::generated::{self, ClipIn, RemoteBuilders};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data/complock-gen")
        .join(name)
}

fn samples(name: &str) -> Vec<f32> {
    let bytes = std::fs::read(fixture(name)).expect(name);
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

fn kai_clips() -> (ClipIn, ClipIn) {
    let body = samples("ploop.f32");
    let head = samples("quake3.f32");
    let body_len = u32::try_from(body.len()).expect("body len");
    let head_len = u32::try_from(head.len()).expect("head len");
    let _ = body_len;
    (
        ClipIn {
            samples: body,
            source_name: "ploop".into(),
            start: 407,
            end: 3046,
            db: -2.0,
            pitch: 1.0,
        },
        ClipIn {
            samples: head,
            source_name: "quake 3".into(),
            start: 0,
            end: head_len,
            db: 0.0,
            pitch: 1.0,
        },
    )
}

#[test]
fn kai_hitsound_pak_matches_the_site_byte_for_byte() {
    let (body, head) = kai_clips();
    let pak = generated::build_hitsound(&body, &head).expect("build");
    let kai = std::fs::read(fixture("kai_hitsound.vpk")).expect("kai");
    assert_eq!(pak, kai);
}

#[test]
fn a_hitsound_pak_roundtrips_the_levels_it_was_built_with() {
    let (body, head) = kai_clips();
    let pak = generated::build_hitsound(&body, &head).expect("build");
    let facts = generated::read_hitsound(&pak).expect("the pak is a hitsound");
    assert_eq!(facts.body_samples, body.end - body.start);
    assert_eq!(facts.head_samples, head.end - head.start);
    assert!((facts.body_db - body.db).abs() < 1e-9);
    assert!((facts.head_db - head.db).abs() < 1e-9);
    assert!((facts.body_pitch - body.pitch).abs() < 1e-9);
    assert!((facts.head_pitch - head.pitch).abs() < 1e-9);
}

#[test]
fn hp_bar_presets_match_the_site_script() {
    for id in [
        "flat", "ticks", "slim", "bold", "block", "outline", "classic", "neon", "minimal",
    ] {
        let pak = generated::build_hpbar_preset(id).expect(id);
        let site = std::fs::read(fixture(&format!("hp-{id}.vpk"))).expect(id);
        assert_eq!(pak, site, "{id}");
    }
}

#[test]
fn hp_bar_copy_link_parses_like_the_site() {
    let golden: Value =
        serde_json::from_str(&std::fs::read_to_string(fixture("hp-links.json")).expect("links"))
            .expect("json");
    for row in golden["presets"].as_array().expect("presets") {
        let hash = row["hash"].as_str().expect("hash");
        let options = generated::options_from_hash(hash);
        let expected: BTreeMap<String, Value> =
            serde_json::from_value(row["options"].clone()).expect("options");
        assert_eq!(options, expected, "{}", row["id"]);
        let rebuilt = generated::build_hpbar(&options).expect("build");
        let id = row["id"].as_str().expect("id");
        let site = std::fs::read(fixture(&format!("hp-{id}.vpk"))).expect(id);
        assert_eq!(rebuilt, site, "{id}");
    }
    for key in ["custom", "defaultDropped", "clamped"] {
        let row = &golden[key];
        let options = generated::options_from_hash(row["hash"].as_str().expect("hash"));
        let expected: BTreeMap<String, Value> =
            serde_json::from_value(row["options"].clone()).expect("options");
        assert_eq!(options, expected, "{key}");
    }
}

#[test]
fn version_tracks_the_template_and_the_inputs() {
    let (body, head) = kai_clips();
    let template = generated::hitsound_template_bytes();
    assert!(!template.is_empty());
    let version = generated::hitsound_version("Kai", &body, &head);
    assert!(version.starts_with("sha256:"));
    assert_eq!(version.len(), "sha256:".len() + 64);
    let louder = ClipIn {
        db: -6.0,
        ..body_clone(&body)
    };
    assert_ne!(version, generated::hitsound_version("Kai", &louder, &head));
    assert_ne!(
        generated::version_of(&template, b"inputs"),
        generated::version_of(b"other-template", b"inputs")
    );
    assert_ne!(version, generated::version_of(&[], b"inputs"));
}

fn body_clone(body: &ClipIn) -> ClipIn {
    ClipIn {
        samples: body.samples.clone(),
        source_name: body.source_name.clone(),
        start: body.start,
        end: body.end,
        db: body.db,
        pitch: body.pitch,
    }
}

#[test]
fn a_template_change_is_an_update_and_a_script_change_is_not() {
    let current = generated::version_of(b"template-now", b"inputs");
    let previous = generated::version_of(b"template-before", b"inputs");
    assert_ne!(current, previous);
    assert_eq!(
        drift::state(
            Some(true),
            Some(&previous),
            &Published::Current([current.clone()].into())
        ),
        Drift::UpdateAvailable
    );
    assert_eq!(
        drift::state(
            Some(true),
            Some(current.as_str()),
            &Published::Current([current.clone()].into()),
        ),
        Drift::Pristine
    );
    assert_eq!(
        drift::state(
            Some(true),
            Some("sha256:old"),
            &generated::publication(true, Some("sha256:new"))
        ),
        Drift::BuilderReview
    );
    assert!(!generated::builder_review());
}

#[test]
fn complock_generated_ids_are_not_retired_pack_options() {
    assert_eq!(
        shiv_core::complock::published(generated::HITSOUND_ID),
        Published::Unknown
    );
    assert_eq!(
        shiv_core::complock::published(generated::HPBAR_ID),
        Published::Unknown
    );
}

#[test]
fn the_watch_updates_templates_and_holds_script_changes() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/complock-gen");
    let hit = std::fs::read_to_string(root.join("hit_builder.js")).expect("hit");
    let lame = std::fs::read(root.join("lame.min.js")).expect("lame");
    let hp = std::fs::read_to_string(root.join("hp_hpbars.js")).expect("hp");
    let base = std::fs::read(root.join("hpbars_base.vpk")).expect("base");
    let same = RemoteBuilders {
        hit_page: &hit,
        lame: &lame,
        hpbars_js: &hp,
        base_pak: &base,
    };
    assert_eq!(generated::classify(&same), generated::Watch::Current);

    let mut new_base = base.clone();
    new_base[20] ^= 0x01;
    let template = RemoteBuilders {
        base_pak: &new_base,
        ..same
    };
    match generated::classify(&template) {
        generated::Watch::Template {
            hit_builder,
            base_pak,
        } => {
            assert_eq!(hit_builder, hit);
            assert_eq!(base_pak.as_deref(), Some(new_base.as_slice()));
        }
        other => panic!("expected a template update, got {other:?}"),
    }

    let changed_js = format!("{hp}\n// reviewed-separately\n");
    let review = RemoteBuilders {
        hpbars_js: &changed_js,
        ..same
    };
    match generated::classify(&review) {
        generated::Watch::Review { changed } => assert_eq!(changed, ["hp_hpbars.js"]),
        other => panic!("expected a review, got {other:?}"),
    }
    let _ = changed_js;
}
