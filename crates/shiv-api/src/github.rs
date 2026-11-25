//! The GitHub releases client that feeds Shiv's self-updater.
//!
//! [`latest_release`] parses the `releases/latest` answer with the
//! transport injected, so the tests stub it; [`release_json`] and
//! [`asset_digest`] are the live transports, their hops pinned to
//! [`API_ORIGIN`] and [`ASSET_ORIGINS`].

use std::time::Duration;

use crate::error::ApiError;
use crate::http::{READ_TIMEOUT, build_agent, get_with_redirects};

/// The repository Shiv's releases are published to.
pub const UPDATES_REPO: &str = "luxus/shiv";

/// The origin the release-metadata call is pinned to.
pub const API_ORIGIN: &str = "https://api.github.com";

/// The origins an asset or its `.sha256` may be fetched from: the link
/// starts on `github.com` and redirects to
/// `objects.githubusercontent.com`.
pub const ASSET_ORIGINS: &[&str] = &[
    "https://github.com",
    "https://objects.githubusercontent.com",
];

/// A release answer with its asset list stays far under this.
const RELEASE_JSON_LIMIT: u64 = 2 * 1024 * 1024;

/// A `.sha256` file is one `sha256sum` line.
const DIGEST_LIMIT: u64 = 1024;

/// More assets than any release ships: a bound on a hostile answer.
const MAX_ASSETS: usize = 64;

pub struct Release {
    /// The tag, for example `v0.6.1`.
    pub tag: String,
    /// The generated release notes (markdown).
    pub notes: String,
    /// The downloadable files of the release.
    pub assets: Vec<ReleaseAsset>,
}

impl Release {
    /// The asset named `name`, if the release publishes one.
    #[must_use]
    pub fn asset(&self, name: &str) -> Option<&ReleaseAsset> {
        self.assets.iter().find(|asset| asset.name == name)
    }
}

/// One downloadable file of a [`Release`].
pub struct ReleaseAsset {
    /// The file name, for example `Shiv-0.6.1-x86_64.AppImage`.
    pub name: String,
    /// The absolute download URL.
    pub url: String,
}

/// True when `tag` (`vX.Y.Z`, the `v` optional) is strictly newer than
/// `current` (also `X.Y.Z`). A tag that does not parse is never newer:
/// a malformed or pre-release tag cannot nag anyone into updating.
#[must_use]
pub fn tag_is_newer(tag: &str, current: &str) -> bool {
    match (triple(tag), triple(current)) {
        (Some(tag), Some(current)) => tag > current,
        _ => false,
    }
}

/// True when `tag` (`vX.Y.Z`, the `v` optional) is equal to or newer than
/// `minimum` (also `X.Y.Z`). A version that does not parse returns `false`.
#[must_use]
pub fn tag_is_at_least(tag: &str, minimum: &str) -> bool {
    match (triple(tag), triple(minimum)) {
        (Some(tag), Some(minimum)) => tag >= minimum,
        _ => false,
    }
}

/// The `X.Y.Z` of a `vX.Y.Z` (the `v` optional), as a comparable triple.
fn triple(version: &str) -> Option<(u64, u64, u64)> {
    let rest = version.strip_prefix('v').unwrap_or(version);
    let mut parts = rest.split('.');
    let major = number(parts.next()?)?;
    let minor = number(parts.next()?)?;
    let patch = number(parts.next()?)?;
    parts.next().is_none().then_some((major, minor, patch))
}

fn number(part: &str) -> Option<u64> {
    if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    part.parse().ok()
}

/// The latest release; `fetch` receives the API URL and returns the
/// JSON body, so the tests inject a stub.
///
/// # Errors
///
/// [`ApiError::Parse`] when the body is not a release of the expected
/// shape: a JSON object with a `tag_name` string and an `assets` array
/// whose entries carry `name` and `browser_download_url` strings.
pub fn latest_release(
    fetch: impl FnOnce(&str) -> Result<String, ApiError>,
) -> Result<Release, ApiError> {
    let url = format!("{API_ORIGIN}/repos/{UPDATES_REPO}/releases/latest");
    parse_release(&fetch(&url)?)
}

/// The live `fetch` behind [`latest_release`]: a GET bearing
/// `user_agent` (GitHub refuses requests without one), pinned to
/// [`API_ORIGIN`], the body capped.
///
/// # Errors
///
/// [`ApiError::Http`] for a non-2xx answer; [`ApiError::Network`] for a
/// transport failure or an oversized body; [`ApiError::Parse`] for a
/// non-UTF-8 body.
pub fn release_json(url: &str, user_agent: &str) -> Result<String, ApiError> {
    pinned_text(
        url,
        &[API_ORIGIN.to_owned()],
        user_agent,
        RELEASE_JSON_LIMIT,
    )
}

/// The `.sha256` text of an asset, pinned to [`ASSET_ORIGINS`].
///
/// # Errors
///
/// As [`release_json`].
pub fn asset_digest(url: &str) -> Result<String, ApiError> {
    pinned_text(url, &asset_origins(), "shiv", DIGEST_LIMIT)
}

/// Download a text release asset (such as CompLock's `meta.json` or
/// `pack-data.json`), pinned to [`ASSET_ORIGINS`] on every hop and capped
/// at `limit` bytes.
///
/// # Errors
///
/// [`ApiError::Http`] for a non-2xx answer; [`ApiError::Network`] for a
/// transport failure or a body exceeding `limit`; [`ApiError::Parse`] when
/// `url` or any redirect target sits outside [`ASSET_ORIGINS`], or when the
/// body is not valid UTF-8.
pub fn release_asset_text(url: &str, user_agent: &str, limit: u64) -> Result<String, ApiError> {
    pinned_text(url, &asset_origins(), user_agent, limit)
}

/// The digest of [`asset_digest`]'s answer: the first token of a
/// `sha256sum` line, when it is 64 hex characters. A malformed digest
/// file is the same as none.
#[must_use]
pub fn digest_hex(digest_file: &str) -> Option<&str> {
    digest_file
        .split_whitespace()
        .next()
        .filter(|token| token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn asset_origins() -> Vec<String> {
    ASSET_ORIGINS
        .iter()
        .map(|origin| (*origin).to_owned())
        .collect()
}

fn pinned_text(
    url: &str,
    origins: &[String],
    user_agent: &str,
    limit: u64,
) -> Result<String, ApiError> {
    let agent = build_agent(READ_TIMEOUT, Some(Duration::from_secs(10)));
    let mut resp = get_with_redirects(&agent, url, &[("user-agent", user_agent)], Some(origins))?;
    let bytes = resp
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .map_err(|e| ApiError::Network(e.to_string()))?;
    String::from_utf8(bytes).map_err(|e| ApiError::Parse(e.utf8_error().to_string()))
}

fn parse_release(json: &str) -> Result<Release, ApiError> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| ApiError::Parse(e.to_string()))?;
    let tag = string_field(&value, "tag_name")?;
    let notes = value
        .get("body")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let assets = value
        .get("assets")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| ApiError::Parse("release carries no assets array".into()))?;
    if assets.len() > MAX_ASSETS {
        return Err(ApiError::Parse(format!(
            "release lists {} assets, more than the bound of {MAX_ASSETS}",
            assets.len()
        )));
    }
    let mut parsed = Vec::new();
    for asset in assets {
        parsed.push(ReleaseAsset {
            name: string_field(asset, "name")?,
            url: string_field(asset, "browser_download_url")?,
        });
    }
    Ok(Release {
        tag,
        notes,
        assets: parsed,
    })
}

fn string_field(value: &serde_json::Value, field: &str) -> Result<String, ApiError> {
    value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ApiError::Parse(format!("release field is missing: {field}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    // No `"#` may appear inside: it would close the raw string.
    const RELEASE: &str = r#"{
        "tag_name": "v0.6.1",
        "body": "Features and fixes:\n- one\n- two",
        "assets": [
            {"name": "Shiv-0.6.1-x86_64.AppImage",
             "browser_download_url": "https://github.com/luxus/shiv/releases/download/v0.6.1/Shiv-0.6.1-x86_64.AppImage"},
            {"name": "Shiv-0.6.1-x86_64.AppImage.sha256",
             "browser_download_url": "https://github.com/luxus/shiv/releases/download/v0.6.1/Shiv-0.6.1-x86_64.AppImage.sha256"}
        ]
    }"#;

    #[test]
    fn a_release_parses_its_tag_notes_and_assets() {
        let release = super::parse_release(RELEASE).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(release.tag, "v0.6.1");
        assert_eq!(release.notes, "Features and fixes:\n- one\n- two");
        let asset = release.asset("Shiv-0.6.1-x86_64.AppImage");
        assert!(asset.is_some_and(|a| a.url.contains("/download/v0.6.1/")));
        assert!(release.asset("Shiv-0.6.1-x86_64.flatpak").is_none());
    }

    #[test]
    fn a_release_without_a_body_has_empty_notes() {
        let json = RELEASE.replace("\"body\": \"Features and fixes:\\n- one\\n- two\",", "");
        let release = super::parse_release(&json).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(release.notes, "");
    }

    #[test]
    fn a_release_without_a_tag_name_or_assets_is_refused() {
        assert!(super::parse_release("{}").is_err());
        assert!(super::parse_release(r#"{"tag_name": "v0.6.1"}"#).is_err());
        assert!(super::parse_release("not json").is_err());
    }

    #[test]
    fn latest_release_hands_the_fetch_the_pinned_url() {
        let mut seen = String::new();
        let release = super::latest_release(|url| {
            seen = url.to_owned();
            Ok(RELEASE.to_owned())
        })
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            seen,
            "https://api.github.com/repos/luxus/shiv/releases/latest"
        );
        assert_eq!(release.tag, "v0.6.1");
    }

    #[test]
    fn tag_is_newer_compares_versions_not_strings() {
        assert!(tag_is_newer("v0.10.0", "0.9.9"));
        assert!(!tag_is_newer("v0.9.10", "0.10.0"));
        assert!(tag_is_newer("0.6.1", "v0.6.0"));
        assert!(!tag_is_newer("v0.6.0", "0.6.0"));
        assert!(!tag_is_newer("v0.5.9", "0.6.0"));
    }

    #[test]
    fn a_tag_that_does_not_parse_is_never_newer() {
        assert!(!tag_is_newer("v0.6.1-rc1", "0.6.0"));
        assert!(!tag_is_newer("complock-data", "0.6.1"));
        assert!(!tag_is_newer("complock-data", "0.0.0"));
        assert!(!tag_is_newer("latest", "0.6.0"));
        assert!(!tag_is_newer("", "0.6.0"));
        assert!(!tag_is_newer("v0.6.1", "dev"));
        assert!(!tag_is_newer("v0.6.1.2", "0.6.0"));
    }

    #[test]
    fn tag_is_at_least_checks_inclusive_semver_order() {
        assert!(tag_is_at_least("v0.7.0", "0.7.0"));
        assert!(tag_is_at_least("0.7.0", "v0.7.0"));
        assert!(tag_is_at_least("v0.10.0", "0.9.9"));
        assert!(!tag_is_at_least("v0.6.1", "0.7.0"));
        assert!(!tag_is_at_least("complock-data", "0.7.0"));
        assert!(!tag_is_at_least("v0.7.0-rc1", "0.7.0"));
        assert!(!tag_is_at_least("v0.7.0", ""));
    }

    #[test]
    fn release_asset_text_and_pinned_text_enforce_origins_redirects_and_size_caps() {
        let err = release_asset_text("https://evil.example/meta.json", "shiv", 1024).unwrap_err();
        assert!(matches!(err, ApiError::Parse(_)), "{err:?}");

        let server = tiny_http::Server::http("127.0.0.1:0").expect("stub");
        let origin = format!("http://{}", server.server_addr().to_ip().expect("ip"));
        std::thread::spawn(move || {
            for _ in 0..3 {
                let Ok(req) = server.recv() else { return };
                let resp = match req.url() {
                    "/ok" => tiny_http::Response::from_string("hello").with_status_code(200),
                    "/big" => {
                        tiny_http::Response::from_string("0123456789abcdef").with_status_code(200)
                    }
                    "/redirect-evil" => tiny_http::Response::from_string("")
                        .with_status_code(302)
                        .with_header(
                            tiny_http::Header::from_bytes("Location", "https://evil.example/x")
                                .unwrap(),
                        ),
                    _ => tiny_http::Response::from_string("").with_status_code(404),
                };
                let _ = req.respond(resp);
            }
        });
        let origins = vec![origin.clone()];

        let ok = pinned_text(&format!("{origin}/ok"), &origins, "shiv", 16)
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(ok, "hello");

        let too_big = pinned_text(&format!("{origin}/big"), &origins, "shiv", 8).unwrap_err();
        assert!(matches!(too_big, ApiError::Network(_)), "{too_big:?}");

        let cross_origin =
            pinned_text(&format!("{origin}/redirect-evil"), &origins, "shiv", 16).unwrap_err();
        assert!(
            matches!(cross_origin, ApiError::Parse(_)),
            "{cross_origin:?}"
        );
    }

    #[test]
    fn digest_hex_takes_the_first_token_when_it_is_sha256() {
        let hex = "a".repeat(64);
        let line = format!("{hex}  Shiv-0.6.1-x86_64.AppImage");
        assert_eq!(digest_hex(&line), Some(hex.as_str()));
        assert_eq!(digest_hex("deadbeef  file"), None);
        assert_eq!(digest_hex(""), None);
    }
}
