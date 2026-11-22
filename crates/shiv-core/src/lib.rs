//! Shiv core: game location and the addons model.

// A library must not panic on the user's disk: every failure is a value.
// The few `expect`s that cannot fail carry a scoped `#[allow]` naming the
// reason (docs/code-quality.md).
#![deny(clippy::unwrap_used, clippy::expect_used)]
// Unit tests inside src may unwrap freely; the integration tests in
// tests/ are their own crates and are unaffected either way.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod addons;
pub mod alloc_count;
pub mod complock;
pub mod crosshair;
pub mod drift;
pub mod game;
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
