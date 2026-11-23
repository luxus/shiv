//! `deadlocker.net` names which `GameBanana` submission one of its mod
//! pages stands for: `GET /api/v1/mods/<slug>` answers with `mod_link`, the
//! mod's `gamebanana.com` page. The link is untrusted input; the caller
//! validates it before resolving it.

use std::time::Duration;

use serde::Deserialize;
use ureq::Agent;

use crate::error::ApiError;
use crate::http::{READ_TIMEOUT, build_agent, get_with_redirects};

const DEFAULT_BASE: &str = "https://deadlocker.net/api/v1";
/// The answer is a few hundred bytes of JSON; images arrive as URLs.
const BODY_LIMIT: u64 = 1024 * 1024;
const GLOBAL_TIMEOUT: Duration = Duration::from_secs(30);
/// A mod page slug is one URL segment (`mod_1770149787580`).
const MAX_SLUG: usize = 64;

/// Client for deadlocker.net's public mod-page API.
pub struct DeadlockerClient {
    base: String,
    agent: Agent,
}

#[derive(Deserialize)]
struct ModResponse {
    #[serde(rename = "mod", default)]
    entry: Option<ModWire>,
}

#[derive(Deserialize)]
struct ModWire {
    #[serde(rename = "mod_link", default)]
    mod_link: Option<String>,
}

impl Default for DeadlockerClient {
    fn default() -> Self {
        Self::new(DEFAULT_BASE)
    }
}

impl DeadlockerClient {
    /// Construct a client targeting `base_url`.
    #[must_use]
    pub fn new(base_url: &str) -> Self {
        Self {
            base: base_url.trim_end_matches('/').to_owned(),
            agent: build_agent(READ_TIMEOUT, Some(GLOBAL_TIMEOUT)),
        }
    }

    /// The `gamebanana.com` page URL of deadlocker.net mod page `slug`.
    ///
    /// # Errors
    ///
    /// [`ApiError::Parse`] when `slug` is not a page id or the answer names
    /// no `GameBanana` mod, [`ApiError::Http`] when the site answers a
    /// non-2xx status, [`ApiError::Network`] on a transport failure.
    pub fn mod_link(&self, slug: &str) -> Result<String, ApiError> {
        if !valid_slug(slug) {
            return Err(ApiError::Parse(format!(
                "invalid deadlocker.net mod id {slug:?}"
            )));
        }
        let url = format!("{}/mods/{slug}", self.base);
        let mut resp = get_with_redirects(&self.agent, &url, &[], None)?;
        let bytes = resp
            .body_mut()
            .with_config()
            .limit(BODY_LIMIT)
            .read_to_vec()
            .map_err(|e| ApiError::Network(e.to_string()))?;
        let parsed: ModResponse =
            serde_json::from_slice(&bytes).map_err(|e| ApiError::Parse(e.to_string()))?;
        parsed
            .entry
            .and_then(|m| m.mod_link)
            .as_deref()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| ApiError::Parse("the answer named no GameBanana mod".into()))
    }
}

/// A slug is one URL segment: letters, digits, `_`, `-` or `.`; `.` and `..`
/// never name a page.
fn valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= MAX_SLUG
        && slug != "."
        && slug != ".."
        && slug
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_one_plain_segment() {
        for good in ["mod_1770149787580", "mod-1", "a.b_c"] {
            assert!(valid_slug(good), "{good:?}");
        }
        for bad in [
            "",
            ".",
            "..",
            "a/b",
            "../x",
            "a b",
            "a%20b",
            "a?b",
            "a#b",
            &"a".repeat(65),
        ] {
            assert!(!valid_slug(bad), "{bad:?}");
        }
    }
}
