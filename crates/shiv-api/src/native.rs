//! The native source protocol: a static catalog or JSON API a mod site
//! publishes so a manager can browse and install its mods
//! (`dmm-source.json`, protocol version 1.0).
//!
//! Everything a source serves is third-party data, so every document is
//! validated before it is used: the descriptor pins its identity and the
//! origins it may talk to, catalog pages must complete, and an artifact,
//! detail or history URL outside the declared origins is rejected.

use std::time::Duration;

use serde::Deserialize;
use ureq::Agent;

use crate::error::ApiError;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const BODY_TIMEOUT: Duration = Duration::from_secs(30);
const JSON_LIMIT: u64 = 10 * 1024 * 1024;
const MAX_CATALOG_PAGES: usize = 100;
const HISTORY_PAGE_LIMIT: usize = 20;
const HISTORY_RELEASE_LIMIT: usize = 500;
const API_PAGE_SIZE: u32 = 100;

pub struct NativeSourceClient {
    agent: Agent,
}

impl Default for NativeSourceClient {
    fn default() -> Self {
        Self::new()
    }
}

impl NativeSourceClient {
    pub fn new() -> Self {
        let agent: Agent = Agent::config_builder()
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_recv_response(Some(BODY_TIMEOUT))
            .timeout_recv_body(Some(BODY_TIMEOUT))
            .build()
            .into();
        Self { agent }
    }

    pub fn fetch_descriptor(&self, url: &str) -> Result<SourceDescriptor, ApiError> {
        let wire: DescriptorWire = self.get_json(url)?;
        let descriptor = wire.into_descriptor(url)?;
        descriptor.descriptor_within_its_origins()?;
        Ok(descriptor)
    }

    pub fn fetch_catalog(&self, source: &SourceDescriptor) -> Result<Vec<NativeListing>, ApiError> {
        match &source.catalog {
            CatalogConfig::Static { index_url } => self.fetch_static_catalog(source, index_url),
            CatalogConfig::Api { list_url } => self.fetch_api_catalog(source, list_url),
        }
    }

    pub fn fetch_release_history(
        &self,
        source: &SourceDescriptor,
        listing: &NativeListing,
    ) -> Result<Vec<NativeRelease>, ApiError> {
        let Some(first) = &listing.release_history_url else {
            return Ok(Vec::new());
        };
        let mut releases = Vec::new();
        let mut next = Some(first.clone());
        let mut pages = 0;
        while let Some(url) = next {
            pages += 1;
            if pages > HISTORY_PAGE_LIMIT {
                return Err(ApiError::Parse(format!(
                    "the release history of {} has more than {HISTORY_PAGE_LIMIT} pages",
                    listing.title
                )));
            }
            if releases.len() >= HISTORY_RELEASE_LIMIT {
                return Err(ApiError::Parse(format!(
                    "the release history of {} has more than {HISTORY_RELEASE_LIMIT} releases",
                    listing.title
                )));
            }
            if !origin_allowed(&url, &source.api_origins) {
                return Err(ApiError::Parse(format!(
                    "a release-history page is outside the source's API origins: {url}"
                )));
            }
            let page: HistoryPage = self.get_json(&url)?;
            for wire in page.items {
                releases.push(wire.into_release(&url, source)?);
            }
            next = page.next_cursor.filter(|c| !c.is_empty());
        }
        Ok(releases)
    }

    fn fetch_static_catalog(
        &self,
        source: &SourceDescriptor,
        index_url: &str,
    ) -> Result<Vec<NativeListing>, ApiError> {
        let mut listings = Vec::new();
        let mut seen_ids = std::collections::HashSet::new();
        let mut seen_pages = std::collections::HashSet::new();
        let mut snapshot = String::new();
        let mut next = Some(index_url.to_owned());
        while let Some(url) = next {
            if !seen_pages.insert(url.clone()) {
                return Err(ApiError::Parse(
                    "the static catalog cycles through the same pages".into(),
                ));
            }
            if seen_pages.len() > MAX_CATALOG_PAGES {
                return Err(ApiError::Parse(format!(
                    "the catalog has more than {MAX_CATALOG_PAGES} pages"
                )));
            }
            if !origin_allowed(&url, &source.api_origins) {
                return Err(ApiError::Parse(format!(
                    "a catalog page is outside the source's API origins: {url}"
                )));
            }
            let page: StaticPage = self.get_json(&url)?;
            let next_page = page.next_page();
            let page_snapshot = page.snapshot_id()?.to_owned();
            if snapshot.is_empty() {
                snapshot = page_snapshot;
            } else if snapshot != page_snapshot {
                return Err(ApiError::Parse(format!(
                    "the catalog pages describe different snapshots: {snapshot} and {page_snapshot}"
                )));
            }
            for item in page.items {
                let id = item.identity()?.to_owned();
                if !seen_ids.insert(id) {
                    return Err(ApiError::Parse(format!(
                        "the catalog lists {} twice",
                        item.identity()?
                    )));
                }
                listings.push(self.resolve_listing(item, &url, source)?);
            }
            next = next_page;
        }
        Ok(listings)
    }

    fn fetch_api_catalog(
        &self,
        source: &SourceDescriptor,
        list_url: &str,
    ) -> Result<Vec<NativeListing>, ApiError> {
        let mut listings = Vec::new();
        let mut seen_ids = std::collections::HashSet::new();
        let mut next = Some(list_url.to_owned());
        let mut pages = 0;
        while let Some(url) = next {
            pages += 1;
            if pages > MAX_CATALOG_PAGES {
                return Err(ApiError::Parse(format!(
                    "the catalog has more than {MAX_CATALOG_PAGES} pages"
                )));
            }
            if !origin_allowed(&url, &source.api_origins) {
                return Err(ApiError::Parse(format!(
                    "a catalog page is outside the source's API origins: {url}"
                )));
            }
            let page: ApiPage = self.get_json(&url)?;
            let next_cursor = page
                .next_cursor
                .clone()
                .filter(|c| !c.is_empty())
                .map(|c| with_cursor(&url, &c));
            for item in page.items {
                let id = item.identity()?.to_owned();
                if !seen_ids.insert(id) {
                    return Err(ApiError::Parse(format!(
                        "the catalog lists {} twice",
                        item.identity()?
                    )));
                }
                listings.push(self.resolve_listing(item, &url, source)?);
            }
            next = next_cursor;
        }
        Ok(listings)
    }

    fn resolve_listing(
        &self,
        item: PageItem,
        page_url: &str,
        source: &SourceDescriptor,
    ) -> Result<NativeListing, ApiError> {
        let Some(detail_href) = &item.detail_url else {
            return item.into_index_listing(page_url, source);
        };
        let detail_url = resolve_url(page_url, detail_href);
        if !origin_allowed(&detail_url, &source.api_origins) {
            return Err(ApiError::Parse(format!(
                "the detail of {} is outside the source's API origins: {detail_url}",
                item.identity()?
            )));
        }
        let detail: DetailWire = self.get_json(&detail_url)?;
        if detail.identity()? != item.identity()? {
            return Err(ApiError::Parse(format!(
                "the catalog asked for {} but its detail is {}",
                item.identity()?,
                detail.identity()?
            )));
        }
        let (mut listing, release_href) = detail.into_listing(&detail_url, source)?;
        if listing.release.is_none()
            && let Some(href) = release_href
        {
            listing.release = Some(self.fetch_current_release(&detail_url, &href, source)?);
        }
        Ok(listing)
    }

    fn fetch_current_release(
        &self,
        document_url: &str,
        release_href: &str,
        source: &SourceDescriptor,
    ) -> Result<NativeRelease, ApiError> {
        let release_url = resolve_url(document_url, release_href);
        if !origin_allowed(&release_url, &source.api_origins) {
            return Err(ApiError::Parse(format!(
                "the current release is outside the source's API origins: {release_url}"
            )));
        }
        let wire: ReleaseWire = self.get_json(&release_url)?;
        wire.into_release(&release_url, source)
    }

    fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T, ApiError> {
        let mut resp = self.agent.get(url).call().map_err(|e| map_error(e, url))?;
        let body = resp
            .body_mut()
            .with_config()
            .limit(JSON_LIMIT)
            .read_to_vec()
            .map_err(|e| ApiError::Network(e.to_string()))?;
        serde_json::from_slice(&body).map_err(|e| ApiError::Parse(e.to_string()))
    }
}

// ---- descriptor ----

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceDescriptor {
    pub descriptor_url: String,
    pub source_id: String,
    pub name: String,
    pub catalog: CatalogConfig,
    pub api_origins: Vec<String>,
    pub download_origins: Vec<String>,
    pub media_origins: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogConfig {
    Static { index_url: String },
    Api { list_url: String },
}

impl SourceDescriptor {
    fn descriptor_within_its_origins(&self) -> Result<(), ApiError> {
        if !origin_allowed(&self.descriptor_url, &self.api_origins) {
            return Err(ApiError::Parse(
                "the descriptor URL is outside the API origins it declares".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DescriptorWire {
    format: Option<String>,
    protocol_version: Option<String>,
    source_id: Option<String>,
    name: Option<String>,
    game_id: Option<String>,
    catalog: Option<CatalogWire>,
    network_origins: Option<OriginsWire>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CatalogWire {
    kind: Option<String>,
    index_url: Option<String>,
    list_url: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct OriginsWire {
    #[serde(default)]
    api: Vec<String>,
    #[serde(default)]
    downloads: Vec<String>,
    #[serde(default)]
    media: Vec<String>,
}

impl DescriptorWire {
    fn into_descriptor(self, url: &str) -> Result<SourceDescriptor, ApiError> {
        let format = required("format", self.format)?;
        let version = required("protocolVersion", self.protocol_version)?;
        let source_id = required("sourceId", self.source_id)?;
        let name = required("name", self.name)?;
        let game_id = required("gameId", self.game_id)?;
        if format != "dmm-source" {
            return Err(ApiError::Parse(format!(
                "the document is a {format} source, not dmm-source"
            )));
        }
        if version != "1.0" {
            return Err(ApiError::Parse(format!(
                "the source speaks protocol {version}, Shiv speaks 1.0"
            )));
        }
        if game_id != "deadlock" {
            return Err(ApiError::Parse(format!(
                "the source serves {game_id}, not deadlock"
            )));
        }
        let catalog = self
            .catalog
            .ok_or_else(|| ApiError::Parse("the source descriptor has no catalog".into()))?;
        let catalog = match catalog.kind.as_deref() {
            Some("static") => CatalogConfig::Static {
                index_url: resolve_url(url, &required("catalog.indexUrl", catalog.index_url)?),
            },
            Some("api") => CatalogConfig::Api {
                list_url: resolve_url(url, &required("catalog.listUrl", catalog.list_url)?),
            },
            other => {
                return Err(ApiError::Parse(format!(
                    "the catalog kind {other:?} is neither static nor api"
                )));
            }
        };
        let origins = self.network_origins.unwrap_or_default();
        if origins.api.is_empty() || origins.downloads.is_empty() {
            return Err(ApiError::Parse(
                "the source declares no API or download origins".into(),
            ));
        }
        Ok(SourceDescriptor {
            descriptor_url: url.to_owned(),
            source_id,
            name,
            catalog,
            api_origins: origins.api,
            download_origins: origins.downloads,
            media_origins: origins.media,
        })
    }
}

fn required(field: &str, value: Option<String>) -> Result<String, ApiError> {
    value
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| ApiError::Parse(format!("the source descriptor has no {field}")))
}

// ---- listings and releases ----

#[derive(Debug, Clone, PartialEq, Default)]
pub struct NativeListing {
    pub id: String,
    pub title: String,
    pub available: bool,
    pub removed: bool,
    pub nsfw: Option<bool>,
    pub description: Option<String>,
    pub author: Option<String>,
    pub hero: Option<String>,
    pub content_type: Option<String>,
    pub tags: Vec<String>,
    pub images: Vec<String>,
    pub updated_at: Option<String>,
    pub release: Option<NativeRelease>,
    pub release_history_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct NativeRelease {
    pub id: String,
    pub content_revision: String,
    pub version: Option<String>,
    pub published_at: Option<String>,
    pub changelog: Option<String>,
    pub artifacts: Vec<NativeArtifact>,
}

impl NativeRelease {
    pub fn manual(&self) -> bool {
        !self.artifacts.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct NativeArtifact {
    pub id: String,
    pub filename: String,
    pub url: String,
    pub size: Option<u64>,
    pub sha256: Option<String>,
}

impl NativeListing {
    pub fn installable(&self) -> bool {
        self.available
            && !self.removed
            && self
                .release
                .as_ref()
                .is_some_and(|r| r.manual() && !r.artifacts.is_empty())
    }

    pub fn thumbnail_url(&self) -> Option<&str> {
        self.images.first().map(String::as_str)
    }
}

#[derive(Deserialize)]
struct StaticPage {
    #[serde(rename = "snapshotId")]
    snapshot_id: Option<String>,
    #[serde(default)]
    items: Vec<PageItem>,
    #[serde(rename = "nextUrl", default)]
    next_url: Option<String>,
}

impl StaticPage {
    fn snapshot_id(&self) -> Result<&str, ApiError> {
        self.snapshot_id
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| ApiError::Parse("a catalog page has no snapshotId".into()))
    }

    fn next_page(&self) -> Option<String> {
        self.next_url
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .map(str::to_owned)
    }
}

#[derive(Deserialize)]
struct ApiPage {
    #[serde(default)]
    items: Vec<PageItem>,
    #[serde(rename = "nextCursor", default)]
    next_cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageItem {
    id: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    availability: Option<String>,
    #[serde(default)]
    is_nsfw: Option<bool>,
    detail_url: Option<String>,
}

impl PageItem {
    fn identity(&self) -> Result<&str, ApiError> {
        self.id
            .as_deref()
            .filter(|i| !i.trim().is_empty())
            .ok_or_else(|| ApiError::Parse("a catalog entry has no id".into()))
    }

    fn into_index_listing(
        self,
        page_url: &str,
        source: &SourceDescriptor,
    ) -> Result<NativeListing, ApiError> {
        let id = self.identity()?.to_owned();
        let (available, removed) = availability_of(&id, self.availability.as_deref())?;
        Ok(NativeListing {
            id,
            title: self.title.unwrap_or_default(),
            available,
            removed,
            nsfw: self.is_nsfw,
            release_history_url: None,
            images: Vec::new(),
            tags: Vec::new(),
            description: None,
            author: None,
            hero: None,
            content_type: None,
            updated_at: None,
            release: {
                let _ = (page_url, source);
                None
            },
        })
    }
}

fn availability_of(id: &str, availability: Option<&str>) -> Result<(bool, bool), ApiError> {
    match availability.unwrap_or_default() {
        "available" => Ok((true, false)),
        "unavailable" => Ok((false, false)),
        "removed" => Ok((false, true)),
        other => Err(ApiError::Parse(format!(
            "the mod {id} has availability {other:?}"
        ))),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DetailWire {
    id: Option<String>,
    title: Option<String>,
    #[serde(default)]
    availability: Option<String>,
    #[serde(default)]
    is_nsfw: Option<bool>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    hero: Option<String>,
    #[serde(default)]
    content_type: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    images: Vec<String>,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    release: Option<ReleaseWire>,
    #[serde(default)]
    release_url: Option<String>,
    #[serde(default, rename = "releaseHistoryUrl")]
    release_history_url: Option<String>,
}

impl DetailWire {
    fn identity(&self) -> Result<&str, ApiError> {
        self.id
            .as_deref()
            .filter(|i| !i.trim().is_empty())
            .ok_or_else(|| ApiError::Parse("the detail document has no id".into()))
    }

    fn into_listing(
        self,
        detail_url: &str,
        source: &SourceDescriptor,
    ) -> Result<(NativeListing, Option<String>), ApiError> {
        let id = self.identity()?.to_owned();
        let (available, removed) = availability_of(&id, self.availability.as_deref())?;
        let images: Vec<String> = self
            .images
            .into_iter()
            .take(4)
            .map(|href| resolve_url(detail_url, &href))
            .collect();
        if images
            .iter()
            .any(|url| !origin_allowed(url, &source.media_origins))
        {
            return Err(ApiError::Parse(format!(
                "a screenshot of {id} is outside the source's media origins"
            )));
        }
        let release = self
            .release
            .map(|wire| wire.into_release(detail_url, source))
            .transpose()?;
        Ok((
            NativeListing {
                id,
                title: self.title.unwrap_or_default(),
                available,
                removed,
                nsfw: self.is_nsfw,
                description: self.description,
                author: self.author,
                hero: self.hero,
                content_type: self.content_type,
                tags: self.tags,
                images,
                updated_at: self.updated_at,
                release,
                release_history_url: self
                    .release_history_url
                    .map(|href| resolve_url(detail_url, &href)),
            },
            self.release_url,
        ))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseWire {
    id: Option<String>,
    #[serde(default)]
    content_revision: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    changelog: Option<String>,
    #[serde(default)]
    artifacts: Vec<ArtifactWire>,
    #[serde(default)]
    installation: Option<InstallationWire>,
}

#[derive(Deserialize)]
struct InstallationWire {
    #[serde(default)]
    mode: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArtifactWire {
    id: Option<String>,
    #[serde(default)]
    filename: Option<String>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    download: Option<DownloadWire>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    hashes: Option<HashesWire>,
}

#[derive(Deserialize)]
struct DownloadWire {
    #[serde(default)]
    url: Option<String>,
}

#[derive(Deserialize)]
struct HashesWire {
    #[serde(default)]
    sha256: Option<String>,
}

#[derive(Deserialize)]
struct HistoryPage {
    #[serde(default)]
    items: Vec<ReleaseWire>,
    #[serde(default, rename = "nextCursor")]
    next_cursor: Option<String>,
}

impl ReleaseWire {
    fn into_release(
        self,
        document_url: &str,
        source: &SourceDescriptor,
    ) -> Result<NativeRelease, ApiError> {
        let id = self
            .id
            .filter(|i| !i.trim().is_empty())
            .ok_or_else(|| ApiError::Parse("a release has no id".into()))?;
        let content_revision = self
            .content_revision
            .filter(|r| !r.trim().is_empty())
            .ok_or_else(|| ApiError::Parse(format!("the release {id} has no contentRevision")))?;
        if self.installation.as_ref().and_then(|i| i.mode.as_deref()) == Some("components") {
            return Err(ApiError::Parse(format!(
                "the release {id} needs component choices, which Shiv cannot install yet"
            )));
        }
        let mut artifacts = Vec::new();
        for wire in self.artifacts {
            artifacts.push(wire.into_artifact(&id, document_url, source)?);
        }
        Ok(NativeRelease {
            id,
            content_revision,
            version: self.version,
            published_at: self.published_at,
            changelog: self.changelog,
            artifacts,
        })
    }
}

impl ArtifactWire {
    fn into_artifact(
        self,
        release_id: &str,
        document_url: &str,
        source: &SourceDescriptor,
    ) -> Result<NativeArtifact, ApiError> {
        let id = self
            .id
            .filter(|i| !i.trim().is_empty())
            .ok_or_else(|| ApiError::Parse(format!("a release {release_id} artifact has no id")))?;
        let Some(filename) = self.filename.filter(|f| is_safe_basename(f)) else {
            return Err(ApiError::Parse(format!(
                "the artifact {id} of release {release_id} has no safe file name"
            )));
        };
        if !matches!(
            self.format.as_deref(),
            Some("vpk") | Some("zip") | Some("rar") | Some("7z")
        ) {
            return Err(ApiError::Parse(format!(
                "the artifact {id} of release {release_id} is a {:?} file, which Shiv cannot install",
                self.format
            )));
        }
        let Some(download) = self.download.and_then(|d| d.url) else {
            return Err(ApiError::Parse(format!(
                "the artifact {id} of release {release_id} has no direct download URL"
            )));
        };
        let url = resolve_url(document_url, &download);
        if !origin_allowed(&url, &source.download_origins) {
            return Err(ApiError::Parse(format!(
                "the download of artifact {id} is outside the source's download origins: {url}"
            )));
        }
        Ok(NativeArtifact {
            id,
            filename,
            url,
            size: self.size,
            sha256: self
                .hashes
                .and_then(|h| h.sha256)
                .and_then(valid_sha256_hex),
        })
    }
}

fn valid_sha256_hex(hex: String) -> Option<String> {
    let hex = hex.to_ascii_lowercase();
    (hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit())).then_some(hex)
}

// ---- URLs ----

pub fn resolve_url(base: &str, href: &str) -> String {
    if href.starts_with("https://") || href.starts_with("http://") {
        return href.to_owned();
    }
    let Some((scheme, rest)) = base.split_once("://") else {
        return href.to_owned();
    };
    let without_fragment = rest.split(['?', '#']).next().unwrap_or(rest);
    let split = without_fragment.find('/');
    let authority = match split {
        Some(i) => &without_fragment[..i],
        None => without_fragment,
    };
    let path = match split {
        Some(i) => &without_fragment[i..],
        None => "",
    };
    if href.starts_with('/') {
        return format!("{scheme}://{authority}{}", normalize_path(href));
    }
    let dir = match path.rfind('/') {
        Some(i) => &path[..i],
        None => "",
    };
    format!(
        "{scheme}://{authority}{}",
        normalize_path(&format!("{dir}/{href}"))
    )
}

fn normalize_path(path: &str) -> String {
    let (bare, trailing) = match path.strip_suffix('/') {
        Some(bare) => (bare, "/"),
        None => (path, ""),
    };
    let mut segments: Vec<&str> = Vec::new();
    for segment in bare.split('/').filter(|s| !s.is_empty() && *s != ".") {
        match segment {
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    format!("/{}{trailing}", segments.join("/"))
}

pub fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    if !matches!(scheme, "http" | "https") {
        return None;
    }
    let host_port = rest.split(['/', '?', '#']).next()?;
    if host_port.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{host_port}"))
}

pub fn origin_allowed(url: &str, origins: &[String]) -> bool {
    origin_of(url).is_some_and(|origin| origins.iter().any(|o| o == &origin))
}

pub fn is_https_or_loopback(url: &str) -> bool {
    let Some(origin) = origin_of(url) else {
        return false;
    };
    if origin.starts_with("https://") {
        return true;
    }
    let host = origin
        .strip_prefix("http://")
        .unwrap_or(&origin)
        .split(':')
        .next()
        .unwrap_or_default();
    matches!(host, "localhost" | "127.0.0.1" | "[::1]")
}

pub fn is_safe_basename(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !name.contains(['/', '\\', ':', '\0'])
        && !name.ends_with(['.', ' '])
        && name.chars().all(|c| !c.is_control())
}

fn with_cursor(list_url: &str, cursor: &str) -> String {
    let separator = if list_url.contains('?') { '&' } else { '?' };
    format!("{list_url}{separator}cursor={cursor}&limit={API_PAGE_SIZE}")
}

fn map_error(e: ureq::Error, url: &str) -> ApiError {
    match e {
        ureq::Error::StatusCode(status) => ApiError::Http {
            status,
            url: url.to_owned(),
        },
        ureq::Error::Io(e) => ApiError::Io(e),
        other => ApiError::Network(other.to_string()),
    }
}
