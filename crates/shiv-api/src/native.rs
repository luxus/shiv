//! The native source protocol: a static catalog or JSON API a mod site
//! publishes so a manager can browse and install its mods
//! (`dmm-source.json`, protocol version 1.0).
//!
//! Everything a source serves is third-party data, so every document is
//! validated before it is used: the descriptor pins its identity and the
//! origins it may talk to, catalog pages must complete, and an artifact,
//! detail or history URL outside the declared origins is rejected.

use std::collections::HashSet;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use ureq::Agent;

use crate::error::ApiError;
use crate::http::{
    READ_TIMEOUT, build_agent, deserialize_null_default, encode_segment, get_with_redirects,
};

const GLOBAL_TIMEOUT: Duration = Duration::from_secs(60);
const JSON_LIMIT: u64 = 10 * 1024 * 1024;
const MAX_CATALOG_PAGES: usize = 100;
const HISTORY_PAGE_LIMIT: usize = 20;
const HISTORY_RELEASE_LIMIT: usize = 500;
const API_PAGE_SIZE: u32 = 100;

/// Client for native `dmm-source` (protocol 1.0) mod registries.
pub struct NativeSourceClient {
    agent: Agent,
}

impl Default for NativeSourceClient {
    fn default() -> Self {
        Self::new()
    }
}

impl NativeSourceClient {
    /// Construct a client with strict redirect and timeout limits.
    #[must_use]
    pub fn new() -> Self {
        Self {
            agent: build_agent(READ_TIMEOUT, Some(GLOBAL_TIMEOUT)),
        }
    }

    /// Fetch and validate a registry descriptor: it must parse, and its URLs
    /// must stay inside the origins it declares.
    ///
    /// # Errors
    ///
    /// [`ApiError::Http`] for a non-2xx answer, [`ApiError::Network`] on a
    /// transport failure, [`ApiError::Parse`] when the descriptor is not the
    /// JSON shape expected or declares itself outside its own origins.
    pub fn fetch_descriptor(&self, url: &str) -> Result<SourceDescriptor, ApiError> {
        let initial_origin = origin_of(url).ok_or_else(|| {
            ApiError::Parse(format!("the descriptor URL has no valid origin: {url}"))
        })?;
        let pinned = [initial_origin];
        let wire: DescriptorWire = self.get_json_in(url, &pinned)?;
        let descriptor = wire.into_descriptor(url)?;
        descriptor.descriptor_within_its_origins()?;
        Ok(descriptor)
    }

    /// The source's listings, through whichever catalog kind its descriptor
    /// names. Entries whose individual documents fail validation are returned
    /// uninstallable with [`NativeListing::problem`] set; catalog-level errors
    /// (missing entry id, duplicate id, snapshot mismatch, page overflow)
    /// fail the whole fetch.
    ///
    /// # Errors
    ///
    /// [`ApiError::Http`], [`ApiError::Network`] or [`ApiError::Parse`] on a
    /// catalog-level failure.
    pub fn fetch_catalog(&self, source: &SourceDescriptor) -> Result<Vec<NativeListing>, ApiError> {
        match &source.catalog {
            CatalogConfig::Static { index_url } => self.fetch_static_catalog(source, index_url),
            CatalogConfig::Api { list_url } => self.fetch_api_catalog(source, list_url),
        }
    }

    /// Where the artifact downloads from: its direct URL, or the fresh one
    /// its resolver serves — which must still be on a declared download
    /// origin, or the registry is trying to send Shiv elsewhere.
    ///
    /// # Errors
    ///
    /// [`ApiError::Parse`] when the resolver sits outside the source's API
    /// origins, serves no URL, or resolves to a URL outside the source's
    /// download origins; [`ApiError::Http`]/[`ApiError::Network`] when the
    /// resolver request itself fails.
    pub fn resolve_artifact_url(
        &self,
        source: &SourceDescriptor,
        artifact: &NativeArtifact,
    ) -> Result<String, ApiError> {
        let Some(resolver) = &artifact.resolver_url else {
            return Ok(artifact.url.clone());
        };
        let resolver_url = resolve_url(&source.descriptor_url, resolver);
        if !origin_allowed(&resolver_url, &source.api_origins) {
            return Err(ApiError::Parse(format!(
                "the download resolver is outside the source's API origins: {resolver_url}"
            )));
        }
        let served: ResolvedUrl = self.get_json_in(&resolver_url, &source.api_origins)?;
        let Some(fresh) = served.url.filter(|u| !u.trim().is_empty()) else {
            return Err(ApiError::Parse(
                "the download resolver served no URL".into(),
            ));
        };
        let download_url = resolve_url(&resolver_url, &fresh);
        if !origin_allowed(&download_url, &source.download_origins) {
            return Err(ApiError::Parse(format!(
                "the resolved download is outside the source's download origins: {download_url}"
            )));
        }
        Ok(download_url)
    }

    /// The listing's release history, following the source's cursor pages.
    /// A listing without a history URL has none.
    ///
    /// # Errors
    ///
    /// [`ApiError::Parse`] when a history page sits outside the source's API
    /// origins, cycles, exceeds its page or release limit, or a release row
    /// is not in the shape Shiv can install; [`ApiError::Http`] and
    /// [`ApiError::Network`] when a page request fails.
    pub fn fetch_release_history(
        &self,
        source: &SourceDescriptor,
        listing: &NativeListing,
    ) -> Result<Vec<NativeRelease>, ApiError> {
        let Some(first) = &listing.release_history_url else {
            return Ok(Vec::new());
        };
        let mut releases = Vec::new();
        let mut seen_pages = HashSet::new();
        let mut next = Some(first.clone());
        while let Some(url) = next {
            if !seen_pages.insert(url.clone()) {
                return Err(ApiError::Parse(format!(
                    "the release history of {} cycles through the same pages",
                    listing.title
                )));
            }
            if seen_pages.len() > HISTORY_PAGE_LIMIT {
                return Err(ApiError::Parse(format!(
                    "the release history of {} has more than {HISTORY_PAGE_LIMIT} pages",
                    listing.title
                )));
            }
            if !origin_allowed(&url, &source.api_origins) {
                return Err(ApiError::Parse(format!(
                    "a release-history page is outside the source's API origins: {url}"
                )));
            }
            let page: HistoryPage = self.get_json_in(&url, &source.api_origins)?;
            for wire in page.items {
                if releases.len() >= HISTORY_RELEASE_LIMIT {
                    return Err(ApiError::Parse(format!(
                        "the release history of {} has more than {HISTORY_RELEASE_LIMIT} releases",
                        listing.title
                    )));
                }
                releases.push(wire.into_release(&url, source)?);
            }
            next = page
                .next_cursor
                .as_deref()
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .map(|c| resolve_history_next(first, &url, c));
        }
        Ok(releases)
    }

    fn fetch_static_catalog(
        &self,
        source: &SourceDescriptor,
        index_url: &str,
    ) -> Result<Vec<NativeListing>, ApiError> {
        let mut listings = Vec::new();
        let mut seen_ids = HashSet::new();
        let mut seen_pages = HashSet::new();
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
            let page: StaticPage = self.get_json_in(&url, &source.api_origins)?;
            let next_page = page.next_page().map(|href| resolve_url(&url, &href));
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
                listings.push(self.resolve_or_refuse(item, &url, source)?);
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
        let mut seen_ids = HashSet::new();
        let mut seen_pages = HashSet::new();
        let mut next = Some(list_url.to_owned());
        while let Some(url) = next {
            if !seen_pages.insert(url.clone()) {
                return Err(ApiError::Parse(
                    "the API catalog cycles through the same cursor".into(),
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
            let page: ApiPage = self.get_json_in(&url, &source.api_origins)?;
            let next_cursor = page
                .next_cursor
                .as_deref()
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .map(|c| with_cursor(list_url, c));
            for item in page.items {
                let id = item.identity()?.to_owned();
                if !seen_ids.insert(id) {
                    return Err(ApiError::Parse(format!(
                        "the catalog lists {} twice",
                        item.identity()?
                    )));
                }
                listings.push(self.resolve_or_refuse(item, &url, source)?);
            }
            next = next_cursor;
        }
        Ok(listings)
    }

    fn resolve_or_refuse(
        &self,
        item: PageItem,
        page_url: &str,
        source: &SourceDescriptor,
    ) -> Result<NativeListing, ApiError> {
        let id = item.identity()?.to_owned();
        let title = item.title.clone().unwrap_or_default();
        let nsfw = item.is_nsfw;
        let removed = item.availability.as_deref() == Some("removed");
        Ok(self
            .resolve_listing(item, page_url, source)
            .unwrap_or_else(|e| NativeListing {
                id,
                title,
                nsfw,
                removed,
                problem: Some(e.to_string()),
                ..NativeListing::default()
            }))
    }

    fn resolve_listing(
        &self,
        item: PageItem,
        page_url: &str,
        source: &SourceDescriptor,
    ) -> Result<NativeListing, ApiError> {
        let Some(detail_href) = &item.detail_url else {
            return item.into_index_listing();
        };
        let detail_url = resolve_url(page_url, detail_href);
        if !origin_allowed(&detail_url, &source.api_origins) {
            return Err(ApiError::Parse(format!(
                "the detail of {} is outside the source's API origins: {detail_url}",
                item.identity()?
            )));
        }
        let detail: DetailWire = self.get_json_in(&detail_url, &source.api_origins)?;
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
        let wire: ReleaseWire = self.get_json_in(&release_url, &source.api_origins)?;
        wire.into_release(&release_url, source)
    }

    fn get_json_in<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        allowed_origins: &[String],
    ) -> Result<T, ApiError> {
        let mut resp = get_with_redirects(&self.agent, url, &[], Some(allowed_origins))?;
        let body = resp
            .body_mut()
            .with_config()
            .limit(JSON_LIMIT)
            .read_to_vec()
            .map_err(|e| ApiError::Network(e.to_string()))?;
        serde_json::from_slice(&body).map_err(|e| ApiError::Parse(e.to_string()))
    }
}

fn resolve_history_next(base_history_url: &str, current_url: &str, next: &str) -> String {
    if next.starts_with("http://")
        || next.starts_with("https://")
        || next.starts_with('/')
        || next.starts_with("./")
        || next.starts_with("../")
        || next.starts_with('?')
    {
        resolve_url(current_url, next)
    } else {
        with_cursor(base_history_url, next)
    }
}

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
    #[serde(default, deserialize_with = "deserialize_null_default")]
    api: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    downloads: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    media: Vec<String>,
}

fn validate_origins(kind: &str, raw: Vec<String>) -> Result<Vec<String>, ApiError> {
    let mut normalized = Vec::with_capacity(raw.len());
    for entry in raw {
        let Some(origin) = origin_of(&entry) else {
            return Err(ApiError::Parse(format!(
                "the source declares an invalid {kind} origin: {entry}"
            )));
        };
        if !entry.eq_ignore_ascii_case(&origin) || !is_https_or_loopback(&origin) {
            return Err(ApiError::Parse(format!(
                "the source declares an unsafe {kind} origin: {entry}"
            )));
        }
        normalized.push(origin);
    }
    Ok(normalized)
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
        let api_origins = validate_origins("API", origins.api)?;
        let download_origins = validate_origins("download", origins.downloads)?;
        let media_origins = validate_origins("media", origins.media)?;
        Ok(SourceDescriptor {
            descriptor_url: url.to_owned(),
            source_id,
            name,
            catalog,
            api_origins,
            download_origins,
            media_origins,
        })
    }
}

fn required(field: &str, value: Option<String>) -> Result<String, ApiError> {
    value
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| ApiError::Parse(format!("the source descriptor has no {field}")))
}

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
    /// Why this entry was refused; such a listing is never installable.
    pub problem: Option<String>,
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
    #[must_use]
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
    /// A registry that rotates its download URLs serves a fresh one from
    /// here (a JSON `{"url": ...}` document on an API origin).
    pub resolver_url: Option<String>,
}

impl NativeArtifact {
    /// The URL to try first: the direct download when there is one, else
    /// the resolver's.
    #[must_use]
    pub fn download_url(&self) -> String {
        if self.url.is_empty() {
            self.resolver_url.clone().unwrap_or_default()
        } else {
            self.url.clone()
        }
    }
}

impl NativeListing {
    #[must_use]
    pub fn installable(&self) -> bool {
        self.problem.is_none()
            && self.available
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
    #[serde(default, deserialize_with = "deserialize_null_default")]
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
    #[serde(default, deserialize_with = "deserialize_null_default")]
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

    fn into_index_listing(self) -> Result<NativeListing, ApiError> {
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
            release: None,
            problem: None,
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
    #[serde(default, deserialize_with = "deserialize_null_default")]
    tags: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
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
            .filter(|href| !href.trim().is_empty())
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
        let release_history_url = self
            .release_history_url
            .as_deref()
            .map(str::trim)
            .filter(|href| !href.is_empty())
            .map(|href| resolve_url(detail_url, href));
        if let Some(history_url) = &release_history_url
            && !origin_allowed(history_url, &source.api_origins)
        {
            return Err(ApiError::Parse(format!(
                "the release history of {id} is outside the source's API origins: {history_url}"
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
                release_history_url,
                problem: None,
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
    #[serde(default, deserialize_with = "deserialize_null_default")]
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
    #[serde(default, rename = "resolverUrl")]
    resolver_url: Option<String>,
}

#[derive(Deserialize)]
struct HashesWire {
    #[serde(default)]
    sha256: Option<String>,
}

#[derive(Deserialize)]
struct HistoryPage {
    #[serde(default, deserialize_with = "deserialize_null_default")]
    items: Vec<ReleaseWire>,
    #[serde(default, rename = "nextCursor")]
    next_cursor: Option<String>,
}

#[derive(Deserialize)]
struct ResolvedUrl {
    #[serde(default)]
    url: Option<String>,
}

/// A download resolver's answer: `{"url": "..."}`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ResolvedDownload {
    pub url: Option<String>,
}

/// Parse a resolver answer from its bytes.
///
/// # Errors
///
/// A one-line message saying the bytes are not the JSON shape a resolver
/// answer must have.
pub fn from_json_bytes(bytes: &[u8]) -> Result<ResolvedDownload, String> {
    serde_json::from_slice(bytes)
        .map_err(|e| format!("the resolver response could not be read: {e}"))
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
        if let Some(mode) = self.installation.as_ref().and_then(|i| i.mode.as_deref())
            && mode != "manual"
        {
            return Err(ApiError::Parse(format!(
                "the release {id} uses installation mode {mode:?}, which Shiv cannot install yet"
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
        if !matches!(self.format.as_deref(), Some("vpk" | "zip" | "rar" | "7z")) {
            return Err(ApiError::Parse(format!(
                "the artifact {id} of release {release_id} is a {:?} file, which Shiv cannot install",
                self.format
            )));
        }
        let Some(download) = self.download else {
            return Err(ApiError::Parse(format!(
                "the artifact {id} of release {release_id} has no download"
            )));
        };
        let (url, resolver_url) = match (
            download.url.filter(|u| !u.trim().is_empty()),
            download.resolver_url.filter(|u| !u.trim().is_empty()),
        ) {
            (Some(direct), None) => {
                let resolved = resolve_url(document_url, direct.trim());
                if !origin_allowed(&resolved, &source.download_origins) {
                    return Err(ApiError::Parse(format!(
                        "the download of artifact {id} is outside the source's download origins: {resolved}"
                    )));
                }
                (resolved, None)
            }
            (None, Some(resolver)) => {
                let resolved_url = resolve_url(document_url, resolver.trim());
                if !origin_allowed(&resolved_url, &source.api_origins) {
                    return Err(ApiError::Parse(format!(
                        "the download resolver of artifact {id} is outside the source's API origins: {resolved_url}"
                    )));
                }
                (String::new(), Some(resolved_url))
            }
            _ => {
                return Err(ApiError::Parse(format!(
                    "the artifact {id} of release {release_id} must have exactly one of a direct download URL or a resolver URL"
                )));
            }
        };
        Ok(NativeArtifact {
            id,
            filename,
            url,
            size: self.size,
            sha256: self
                .hashes
                .as_ref()
                .and_then(|h| h.sha256.as_deref())
                .and_then(valid_sha256_hex),
            resolver_url,
        })
    }
}

fn valid_sha256_hex(hex: &str) -> Option<String> {
    let hex = hex.to_ascii_lowercase();
    (hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit())).then_some(hex)
}

/// Resolve `href` relative to `base`, preserving query strings and fragments.
#[must_use]
pub fn resolve_url(base: &str, href: &str) -> String {
    if has_http_scheme(href) {
        return href.to_owned();
    }
    let Some((scheme, rest)) = base.split_once("://") else {
        return href.to_owned();
    };
    let scheme = scheme.to_ascii_lowercase();
    if let Some(after_slashes) = href.strip_prefix("//") {
        return format!("{scheme}://{after_slashes}");
    }
    let without_fragment = rest.split(['?', '#']).next().unwrap_or(rest);
    let split = without_fragment.find('/');
    let authority = match split {
        Some(i) => &without_fragment[..i],
        None => without_fragment,
    };
    let base_path = match split {
        Some(i) => &without_fragment[i..],
        None => "/",
    };
    if href.starts_with('?') || href.starts_with('#') {
        return format!("{scheme}://{authority}{base_path}{href}");
    }
    let (href_path, suffix) = split_path_suffix(href);
    let resolved_path = if href_path.starts_with('/') {
        normalize_path(href_path)
    } else {
        let dir = match base_path.rfind('/') {
            Some(i) => &base_path[..i],
            None => "",
        };
        normalize_path(&format!("{dir}/{href_path}"))
    };
    format!("{scheme}://{authority}{resolved_path}{suffix}")
}

fn has_http_scheme(url: &str) -> bool {
    url.get(..8)
        .is_some_and(|p| p.eq_ignore_ascii_case("https://"))
        || url
            .get(..7)
            .is_some_and(|p| p.eq_ignore_ascii_case("http://"))
}

fn split_path_suffix(href: &str) -> (&str, &str) {
    match href.find(['?', '#']) {
        Some(i) => (&href[..i], &href[i..]),
        None => (href, ""),
    }
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
    if segments.is_empty() {
        return "/".to_owned();
    }
    format!("/{}{trailing}", segments.join("/"))
}

/// Canonical `scheme://host[:port]` origin of an `http`/`https` URL.
/// Rejects userinfo (`@`), backslashes, whitespace, control characters, and
/// malformed host/port authorities.
#[must_use]
pub fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https") {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.is_empty()
        || authority.contains(['@', '\\'])
        || authority
            .chars()
            .any(|c| c.is_control() || c.is_whitespace())
    {
        return None;
    }
    let (host, port) = split_authority(authority)?;
    if host.is_empty() {
        return None;
    }
    let host = host.to_ascii_lowercase();
    match port {
        Some(p) => Some(format!("{scheme}://{host}:{p}")),
        None => Some(format!("{scheme}://{host}")),
    }
}

fn split_authority(authority: &str) -> Option<(&str, Option<&str>)> {
    if let Some(after_open) = authority.strip_prefix('[') {
        let close = after_open.find(']')?;
        let host = &authority[..=close + 1];
        let rest = &after_open[close + 1..];
        if rest.is_empty() {
            return Some((host, None));
        }
        let port = rest.strip_prefix(':')?;
        if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        return Some((host, Some(port)));
    }
    match authority.split_once(':') {
        None => Some((authority, None)),
        Some((host, port)) => {
            if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            Some((host, Some(port)))
        }
    }
}

/// True when `url`'s canonical origin matches one of `origins`.
#[must_use]
pub fn origin_allowed(url: &str, origins: &[String]) -> bool {
    origin_of(url).is_some_and(|origin| {
        origins
            .iter()
            .any(|o| origin_of(o).as_deref() == Some(origin.as_str()))
    })
}

/// True for any valid `https://` URL, or an `http://` URL pointing strictly at
/// loopback (`localhost`, `127.0.0.1`, `[::1]`).
#[must_use]
pub fn is_https_or_loopback(url: &str) -> bool {
    let Some(origin) = origin_of(url) else {
        return false;
    };
    if origin.starts_with("https://") {
        return true;
    }
    let Some(authority) = origin.strip_prefix("http://") else {
        return false;
    };
    let Some((host, _)) = split_authority(authority) else {
        return false;
    };
    matches!(host, "localhost" | "127.0.0.1" | "[::1]")
}

/// True for a single safe cross-platform filename without path separators,
/// control characters, or Windows-illegal characters.
#[must_use]
pub fn is_safe_basename(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !name.starts_with(' ')
        && !name.ends_with(['.', ' '])
        && !matches!(name, "." | "..")
        && !name.contains(['/', '\\', ':', '\0', '<', '>', '"', '|', '?', '*'])
        && name.chars().all(|c| !c.is_control())
}

fn with_cursor(list_url: &str, cursor: &str) -> String {
    let separator = if list_url.contains('?') { '&' } else { '?' };
    format!(
        "{list_url}{separator}cursor={}&limit={API_PAGE_SIZE}",
        encode_segment(cursor)
    )
}
