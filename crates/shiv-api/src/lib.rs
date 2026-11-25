//! HTTP client for the Deadlock Mods API (blocking, `ureq`).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::pedantic)]
#![cfg_attr(
    test,
    allow(clippy::unwrap_used, clippy::expect_used, clippy::pedantic)
)]

mod client;
mod deadlocker;
mod dto;
mod error;
mod gamebanana;
mod github;
mod http;
mod native;
mod query;

pub use client::Client;
pub use deadlocker::DeadlockerClient;
pub use dto::{DonationLink, DownloadFile, ModDto, ModMetadata, is_donation_url};
pub use error::ApiError;
pub use gamebanana::{ChangelogEntry, Comment, Comments, GameBananaClient, ModFile, Page};
pub use github::{
    API_ORIGIN, ASSET_ORIGINS, Release, ReleaseAsset, UPDATES_REPO, asset_digest, digest_hex,
    latest_release, release_asset_text, release_json, tag_is_at_least, tag_is_newer,
};
pub use native::{
    CatalogConfig, NativeArtifact, NativeListing, NativeRelease, NativeSourceClient,
    ResolvedDownload, SourceDescriptor, from_json_bytes, is_https_or_loopback, is_safe_basename,
    origin_allowed, origin_of, resolve_url,
};
pub use query::{ModPage, ModQuery, SortKey, apply_query};
