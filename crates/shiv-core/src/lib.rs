//! Shiv core: game location and the addons model.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::pedantic)]
#![cfg_attr(
    test,
    allow(clippy::unwrap_used, clippy::expect_used, clippy::pedantic)
)]

pub mod addons;
pub mod alloc_count;
pub mod complock;
pub mod crosshair;
pub mod drift;
mod fs_util;
pub mod game;
pub mod generated;
pub mod health;
mod hero_settings;
pub mod install;
pub mod interchange;
pub mod launch;
pub mod link;
pub mod mount;
mod pak_slot;
pub mod perf;
mod platform;
pub mod profiles;
pub mod registry;
pub mod reset;
pub mod sidecar;
pub mod timestamp;
pub mod vanilla;

/// Shiv's settings folder: `shiv` under the user's settings folder
/// (`$XDG_CONFIG_HOME` or `~/.config`, `%APPDATA%`, `~/Library/Application
/// Support`); `None` where there is none.
#[must_use]
pub fn settings_dir() -> Option<std::path::PathBuf> {
    platform::config_dir().map(|d| d.join("shiv"))
}
