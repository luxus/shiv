//! HTTP client for the Deadlock Mods API (blocking, `ureq`).

// A network library must not panic on someone else's server: every
// failure is a value (docs/code-quality.md).
#![deny(clippy::unwrap_used, clippy::expect_used)]
// Unit tests inside src may unwrap freely.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod client;
mod dto;
mod error;
mod gamebanana;
mod native;
mod query;

pub use client::Client;
pub use dto::{DonationLink, DownloadFile, ModDto, ModMetadata, is_donation_url};
pub use error::ApiError;
pub use gamebanana::{ChangelogEntry, Comment, Comments, GameBananaClient, ModFile, Page};
pub use native::{
    CatalogConfig, NativeArtifact, NativeListing, NativeRelease, NativeSourceClient,
    SourceDescriptor, is_https_or_loopback, is_safe_basename, origin_allowed, origin_of,
    resolve_url,
};
pub use query::{ModPage, ModQuery, SortKey, apply_query};
