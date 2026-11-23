use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ureq::Agent;

use crate::dto::{CatalogRows, DownloadFile, DownloadsResponse, ModDto};
use crate::error::ApiError;
use crate::http::{READ_TIMEOUT, build_agent, encode_segment, get_with_redirects};

const API_GLOBAL_TIMEOUT: Duration = Duration::from_secs(60);
const DOWNLOAD_BODY_TIMEOUT: Duration = Duration::from_hours(6);
const JSON_LIMIT: u64 = 64 * 1024 * 1024;
const MAX_DOWNLOAD_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// Blocking HTTP client for the Deadlock Mods API and artifact downloads.
pub struct Client {
    base: String,
    api: Agent,
    download: Agent,
}

fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

impl Client {
    /// Construct a client targeting `base_url`.
    #[must_use]
    pub fn new(base_url: &str) -> Client {
        Client {
            base: base_url.trim_end_matches('/').to_string(),
            api: build_agent(READ_TIMEOUT, Some(API_GLOBAL_TIMEOUT)),
            download: build_agent(DOWNLOAD_BODY_TIMEOUT, None),
        }
    }

    fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T, ApiError> {
        let mut resp = get_with_redirects(&self.api, url, &[], None)?;
        let bytes = resp
            .body_mut()
            .with_config()
            .limit(JSON_LIMIT)
            .read_to_vec()
            .map_err(|e| ApiError::Network(e.to_string()))?;
        serde_json::from_slice(&bytes).map_err(|e| ApiError::Parse(e.to_string()))
    }

    /// The whole catalog (`GET /v2/mods` takes no parameters). Use
    /// [`crate::apply_query`] for search, sort and paging. Rows that fail to
    /// deserialize (missing id/remoteId/name, null in a required field) are skipped
    /// in a single streaming pass without buffering into an intermediate DOM.
    ///
    /// # Errors
    ///
    /// [`ApiError::Http`] when the server answers a non-2xx status,
    /// [`ApiError::Network`] when the transport fails, [`ApiError::Parse`]
    /// when the body is not the JSON shape expected.
    pub fn list_mods(&self) -> Result<Vec<ModDto>, ApiError> {
        let rows: CatalogRows = self.get_json(&format!("{}/v2/mods", self.base))?;
        Ok(rows.0)
    }

    /// Download files for a mod, with URLs exactly as the API returns them
    /// (possibly mirrored).
    ///
    /// # Errors
    ///
    /// [`ApiError::Http`], [`ApiError::Network`] or [`ApiError::Parse`], as
    /// [`Client::list_mods`].
    pub fn downloads(&self, id: &str) -> Result<Vec<DownloadFile>, ApiError> {
        let url = format!("{}/v2/mods/{}/downloads", self.base, encode_segment(id));
        Ok(self.get_json::<DownloadsResponse>(&url)?.downloads)
    }

    /// `Ok(None)` when the server answers 304 for `etag`; otherwise the new
    /// `ETag` (empty if the server sent none) and the raw body.
    ///
    /// # Errors
    ///
    /// [`ApiError::Http`] for a non-2xx answer other than 304,
    /// [`ApiError::Network`] when the transport or body read fails.
    pub fn perf_catalog(&self, etag: Option<&str>) -> Result<Option<(String, Vec<u8>)>, ApiError> {
        let url = format!("{}/v2/perf-catalog", self.base);
        let headers: Vec<(&str, &str)> =
            etag.map(|e| vec![("If-None-Match", e)]).unwrap_or_default();
        let mut resp = get_with_redirects(&self.api, &url, &headers, None)?;
        if resp.status().as_u16() == 304 {
            return Ok(None);
        }
        let new_etag = resp
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let body = resp
            .body_mut()
            .with_config()
            .limit(JSON_LIMIT)
            .read_to_vec()
            .map_err(|e| ApiError::Network(e.to_string()))?;
        Ok(Some((new_etag, body)))
    }

    /// Stream `url` to `dest` via `<dest>.part`, renaming on success. The
    /// partial file is removed on any error. `progress(done, total)` is called
    /// after each chunk; `total` is the Content-Length when known.
    ///
    /// # Errors
    ///
    /// As [`Client::fetch_cancellable`]; this variant never returns
    /// [`ApiError::Cancelled`].
    pub fn fetch(
        &self,
        url: &str,
        dest: &Path,
        progress: impl FnMut(u64, Option<u64>),
    ) -> Result<(), ApiError> {
        self.fetch_cancellable(url, dest, &AtomicBool::new(false), progress)
    }

    /// A small JSON GET, bounded like the API calls; a download resolver's
    /// answer is one URL.
    ///
    /// # Errors
    ///
    /// [`ApiError::Http`] when the server answers a non-2xx status,
    /// [`ApiError::Network`] when the transport or body read fails.
    pub fn fetch_json_bytes(&self, url: &str) -> Result<Vec<u8>, ApiError> {
        let mut resp = get_with_redirects(&self.api, url, &[], None)?;
        resp.body_mut()
            .with_config()
            .limit(JSON_LIMIT)
            .read_to_vec()
            .map_err(|e| ApiError::Network(e.to_string()))
    }

    /// [`Client::fetch`] that stops with [`ApiError::Cancelled`] once `cancel`
    /// is set. The flag is checked before each chunk; stalled reads time out
    /// after the per-read timeout so a dead socket cannot hang indefinitely.
    ///
    /// # Errors
    ///
    /// [`ApiError::Cancelled`] once the flag is seen; [`ApiError::Http`] for
    /// a non-2xx answer; [`ApiError::Network`] for a transport failure or size
    /// overflow; [`ApiError::Io`] when the partial file cannot be written or
    /// renamed; [`ApiError::EmptyDownload`] and [`ApiError::Incomplete`] when
    /// the body contradicts its Content-Length. Any error removes the partial
    /// file.
    pub fn fetch_cancellable(
        &self,
        url: &str,
        dest: &Path,
        cancel: &AtomicBool,
        progress: impl FnMut(u64, Option<u64>),
    ) -> Result<(), ApiError> {
        let part = part_path(dest);
        let result = self
            .fetch_to_part(url, &part, cancel, progress)
            .and_then(|()| fs::rename(&part, dest).map_err(ApiError::from));
        if result.is_err() {
            let _ = fs::remove_file(&part);
        }
        result
    }

    fn fetch_to_part(
        &self,
        url: &str,
        part: &Path,
        cancel: &AtomicBool,
        mut progress: impl FnMut(u64, Option<u64>),
    ) -> Result<(), ApiError> {
        let cancelled = || cancel.load(Ordering::Relaxed);
        if cancelled() {
            return Err(ApiError::Cancelled);
        }
        let mut resp = get_with_redirects(&self.download, url, &[], None)?;
        let total = resp
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        if let Some(expected) = total
            && expected > MAX_DOWNLOAD_BYTES
        {
            return Err(ApiError::Network(format!(
                "download exceeds {MAX_DOWNLOAD_BYTES} byte limit ({expected} bytes)"
            )));
        }
        let mut reader = resp.body_mut().as_reader();
        let mut file = File::create(part)?;
        let mut buf = [0u8; 16 * 1024];
        let mut done: u64 = 0;
        loop {
            if cancelled() {
                return Err(ApiError::Cancelled);
            }
            let n = match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(ApiError::Network(e.to_string())),
            };
            done = done.saturating_add(n as u64);
            if done > MAX_DOWNLOAD_BYTES {
                return Err(ApiError::Network(format!(
                    "download exceeded {MAX_DOWNLOAD_BYTES} byte limit"
                )));
            }
            if let Some(expected) = total
                && done > expected
            {
                return Err(ApiError::Incomplete {
                    received: done,
                    expected,
                });
            }
            file.write_all(&buf[..n])?;
            progress(done, total);
        }
        file.sync_all()?;
        if done == 0 {
            return Err(ApiError::EmptyDownload);
        }
        match total {
            Some(expected) if expected != done => Err(ApiError::Incomplete {
                received: done,
                expected,
            }),
            _ => Ok(()),
        }
    }
}
