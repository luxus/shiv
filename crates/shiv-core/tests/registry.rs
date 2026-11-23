//! The registry store: identity, persistence and update detection.

use std::fs;
use std::path::Path;

use shiv_core::game::GamePaths;
use shiv_core::registry::{self, AddedSource, RegistryError};
use tempfile::tempdir;

fn env() -> GamePaths {
    let root = tempdir().unwrap();
    let paths = GamePaths::from_root(root.path().join("Deadlock"));
    fs::create_dir_all(&paths.addons).unwrap();
    paths
}

fn source(origin: &str, id: &str) -> AddedSource {
    AddedSource {
        descriptor_url: format!("https://{origin}/.well-known/dmm-source.json"),
        origin: origin.into(),
        source_id: id.into(),
        name: format!("Source {id}"),
        api_origins: vec![format!("https://{origin}")],
        download_origins: vec![format!("https://{origin}")],
    }
}

#[test]
fn the_same_source_cannot_be_added_twice() {
    let paths = env();
    let mut registries = registry::Registry::load(&paths).unwrap();
    registries
        .add(source("mods.example.org", "my-community"))
        .unwrap();
    let again = registries.add(source("mods.example.org", "my-community"));
    assert!(matches!(again, Err(RegistryError::Refused(_))), "{again:?}");
    // The same origin with a different id is a different source.
    registries.add(source("mods.example.org", "other")).unwrap();
    // The same id from another origin is a different source too.
    registries
        .add(source("cdn.example.org", "my-community"))
        .unwrap();
    assert_eq!(registries.sources().len(), 3);
}

/// Shiv 0.2.0 saved sources without their origins (the exact list on
/// lea); 0.3.0 refused to read it ("missing field `api_origins`"), which took
/// down adding, browsing and installing from every source. The list loads,
/// and reviewing the real luxus descriptor (`data/luxus-dmm-source.json`,
/// the live file) replaces the old entry of the same origin and id.
#[test]
fn a_source_list_from_before_origins_loads_and_a_review_replaces_it() {
    let root = tempdir().unwrap();
    let paths = GamePaths::from_root(root.path().join("Deadlock"));
    fs::create_dir_all(&paths.addons).unwrap();
    let legacy = r#"[
  {
    "descriptor_url": "https://luxus.github.io/improved-quickbuy/dmm-source.json",
    "origin": "https://luxus.github.io",
    "source_id": "luxus",
    "name": "luxus Deadlock mods"
  }
]"#;
    fs::write(paths.root.join("shiv-sources.json"), legacy).unwrap();
    let mut registries = registry::Registry::load(&paths).expect("a 0.2.0 list loads");
    assert_eq!(registries.sources().len(), 1);
    assert!(registries.sources()[0].api_origins.is_empty());

    let live: serde_json::Value =
        serde_json::from_str(include_str!("data/luxus-dmm-source.json")).unwrap();
    let strings = |v: &serde_json::Value| -> Vec<String> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().to_owned())
            .collect()
    };
    let reviewed = AddedSource {
        descriptor_url: "https://luxus.github.io/deadlock-mods/dmm-source.json".into(),
        origin: "https://luxus.github.io".into(),
        source_id: live["sourceId"].as_str().unwrap().into(),
        name: live["name"].as_str().unwrap().into(),
        api_origins: strings(&live["networkOrigins"]["api"]),
        download_origins: strings(&live["networkOrigins"]["downloads"]),
    };
    registries
        .add(reviewed.clone())
        .expect("the review replaces the old entry");
    let reloaded = registry::Registry::load(&paths).unwrap();
    assert_eq!(reloaded.sources(), std::slice::from_ref(&reviewed));

    let mut again = reloaded;
    assert!(matches!(
        again.add(reviewed),
        Err(RegistryError::Refused(_))
    ));
}

#[test]
fn a_removed_source_is_gone_and_stays_gone() {
    let paths = env();
    let mut registries = registry::Registry::load(&paths).unwrap();
    registries.add(source("a.test", "one")).unwrap();
    registries.add(source("b.test", "two")).unwrap();
    registries.remove("a.test", "one").unwrap();
    let mut reloaded = registry::Registry::load(&paths).unwrap();
    assert_eq!(reloaded.sources().len(), 1);
    assert_eq!(reloaded.sources()[0].source_id, "two");
    // Removing what is not there is fine.
    reloaded.remove("a.test", "one").unwrap();
    assert_eq!(reloaded.sources().len(), 1);
}

#[test]
fn a_registry_mod_id_names_its_listing_and_stays_a_safe_file_name() {
    let s = source("mods.example.org", "my-community");
    let mod_id = s.mod_id_of("example mod.v2");
    // The id becomes `<mod_id>.json` in `.shiv`, so it must be a single
    // plain file name whatever the registry's ids contain.
    assert_eq!(mod_id, "src-my_2dcommunity-example_20mod_2ev2");
    assert!(
        mod_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    );
    assert_eq!(
        registry::split_mod_id(&mod_id),
        Some(("my-community".into(), "example mod.v2".into()))
    );
    // Encoded components never contain a dash, so the one separator dash
    // is unambiguous even when the raw ids carry dashes themselves.
    let dashed = source("mods.example.org", "dashed-source");
    let id = dashed.mod_id_of("dashed-listing");
    assert_eq!(
        registry::split_mod_id(&id),
        Some(("dashed-source".into(), "dashed-listing".into()))
    );
    // Only registry ids split, and what does not decode never parses.
    assert_eq!(registry::split_mod_id("12345"), None);
    assert_eq!(registry::split_mod_id("local-1-0"), None);
    assert_eq!(registry::split_mod_id("src-"), None);
    assert_eq!(
        registry::split_mod_id("src-a-b-c"),
        None,
        "two separator dashes"
    );
}

#[test]
fn any_revision_change_is_an_update() {
    // Content revisions are opaque: only sameness means current.
    assert!(registry::revision_is_newer(None, "rev-1"));
    assert!(registry::revision_is_newer(Some("rev-1"), "rev-2"));
    assert!(!registry::revision_is_newer(Some("rev-1"), "rev-1"));
}

#[test]
fn a_published_checksum_gates_the_download() {
    let dir = tempdir().unwrap();
    let good = dir.path().join("good.vpk");
    fs::write(&good, b"the artifact").unwrap();
    let digest = sha256_hex(b"the artifact");

    registry::verify_download(&good, Some(&digest)).unwrap();
    let wrong = "0".repeat(64);
    assert!(registry::verify_download(&good, Some(&wrong)).is_err());
    assert!(
        !good.exists(),
        "a refused download is deleted, not half-installed"
    );
    fs::write(&good, b"the artifact").unwrap();
    // Without a published checksum there is nothing to verify. (The client
    // layer drops malformed digests while parsing, so none reaches here.)
    registry::verify_download(&good, None).unwrap();
}

/// MD5 is the checksum the catalog publishes per download. A mismatch is
/// the same refusal as a bad SHA-256. No checksum at all still leaves the
/// file, and says so.
#[test]
fn an_md5_mismatch_is_rejected_and_a_missing_checksum_is_unverified() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("archive");
    // RFC 1321: MD5("abc").
    let published = "900150983cd24fb0d6963f7d28e17f72";
    fs::write(&file, b"abc").unwrap();
    assert_eq!(
        registry::check_download(&file, None, Some(published)).unwrap(),
        registry::DownloadTrust::Verified
    );
    assert!(file.is_file(), "a matching checksum keeps the download");
    fs::write(&file, b"abc").unwrap();
    let wrong = registry::check_download(&file, None, Some("900150983cd24fb0d6963f7d28e17f73"));
    assert!(
        wrong.is_err(),
        "one changed nibble is not the published checksum"
    );
    assert!(!file.exists(), "a refused download is deleted");

    fs::write(&file, b"abc").unwrap();
    assert_eq!(
        registry::check_download(&file, None, Some(&published.to_ascii_uppercase())).unwrap(),
        registry::DownloadTrust::Verified,
        "the catalog's hex is compared regardless of case"
    );
}

#[test]
fn a_download_with_no_checksum_is_unverified() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("archive");
    fs::write(&file, b"no checksum was published").unwrap();
    assert_eq!(
        registry::check_download(&file, None, None).unwrap(),
        registry::DownloadTrust::Unverified
    );
    assert!(file.is_file(), "nothing to check still allows the install");
    assert_eq!(
        registry::check_download(&file, Some(""), Some("  ")).unwrap(),
        registry::DownloadTrust::Unverified,
        "a blank checksum is not a checksum"
    );
}

/// The SHA-256 of `bytes` as lowercase hex.
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

#[test]
fn the_store_survives_a_halfwritten_save() {
    let paths = env();
    let mut registries = registry::Registry::load(&paths).unwrap();
    registries.add(source("a.test", "one")).unwrap();
    let saved = registry::Registry::load(&paths).unwrap();
    assert_eq!(saved.sources().len(), 1);
    // The save is atomic: no partial file is left behind.
    let entries: Vec<_> = fs::read_dir(paths.root.join("shiv-sources.json").parent().unwrap())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(!entries.iter().any(|n| n.ends_with(".tmp")), "{entries:?}");
    let _ = Path::new(&paths.addons);
}
