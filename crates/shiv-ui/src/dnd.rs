//! Dropping mod files on the window installs them, or replaces the local
//! mod they are a newer version of (see `installed::files_dropped`); several
//! files dropped at once are queued and processed one by one.
//!
//! winit 0.30 reports file drops on X11, Windows and macOS but not on
//! Wayland, so under Wayland Shiv listens itself (`wayland.rs`). Shiv runs
//! natively under Wayland when `WAYLAND_DISPLAY` is set; `SHIV_X11=1` forces
//! X11 (`XWayland`) instead, where winit's own drop events are used.

#[cfg(all(unix, not(target_vendor = "apple")))]
mod wayland;

use std::ffi::OsStr;

use slint::ComponentHandle;
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};

use crate::{App, installed};

/// Whether to ask winit for X11: only when requested with `SHIV_X11`.
#[cfg_attr(
    target_vendor = "apple",
    allow(
        dead_code,
        reason = "called only from Linux backend selection and tests"
    )
)]
pub(crate) fn force_x11(shiv_x11: Option<&OsStr>) -> bool {
    shiv_x11.is_some_and(|v| !v.is_empty() && v != "0")
}

/// Select the winit backend, on X11 when [`force_x11`] says so. Must run
/// before the first window is created.
pub fn select_backend() -> Result<(), slint::PlatformError> {
    #[cfg(all(unix, not(target_vendor = "apple")))]
    if force_x11(std::env::var_os("SHIV_X11").as_deref()) {
        use winit::platform::x11::EventLoopBuilderExtX11;
        let mut builder = winit::event_loop::EventLoop::with_user_event();
        builder.with_x11();
        return slint::BackendSelector::new()
            .backend_name("winit".into())
            .with_winit_event_loop_builder(builder)
            .select();
    }
    Ok(())
}

/// Route file drag and drop of the main window to the Installed screen:
/// winit's events (X11, Windows, macOS) and, under Wayland, Shiv's own
/// data device once the window exists.
pub fn attach(app: &App) {
    app.window().on_winit_window_event(|_, event| {
        handle(event);
        EventResult::Propagate
    });
    #[cfg(all(unix, not(target_vendor = "apple")))]
    {
        let weak = app.as_weak();
        let _ = slint::spawn_local(async move {
            let Some(app) = weak.upgrade() else { return };
            let Ok(window) = app.window().winit_window().await else {
                return;
            };
            if let Err(e) = start_wayland(&window) {
                crate::log::line(format_args!("dropping files is off: {e}"));
            }
        });
    }
}

#[cfg(all(unix, not(target_vendor = "apple")))]
fn start_wayland(window: &winit::window::Window) -> Result<(), String> {
    use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
    let handle = window.display_handle().map_err(|e| e.to_string())?;
    let RawDisplayHandle::Wayland(wl) = handle.as_raw() else {
        // X11: winit's own drop events apply.
        return Ok(());
    };
    // SAFETY: winit's wl_display lives as long as its event loop, which
    // runs until the process exits.
    unsafe {
        wayland::start(wl.display, |event| {
            let _ = slint::invoke_from_event_loop(move || match event {
                wayland::DropEvent::Hover(on) => installed::drop_hover(on),
                wayland::DropEvent::Files(files) => installed::files_dropped(files),
            });
        })
    }
}

fn handle(event: &winit::event::WindowEvent) {
    use winit::event::WindowEvent;
    match event {
        WindowEvent::HoveredFile(_) => installed::drop_hover(true),
        WindowEvent::HoveredFileCancelled => installed::drop_hover(false),
        WindowEvent::DroppedFile(path) => installed::file_dropped(path.clone()),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x11_only_on_request() {
        assert!(!force_x11(None), "native Wayland by default");
        assert!(!force_x11(Some(OsStr::new(""))));
        assert!(!force_x11(Some(OsStr::new("0"))));
        assert!(force_x11(Some(OsStr::new("1"))));
    }
}
