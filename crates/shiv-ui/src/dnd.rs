//! Dropping a mod file on the window installs it, or replaces the local mod
//! it is a newer version of (see `installed::file_dropped`).
//!
//! winit 0.30 reports file drops on X11, Windows and macOS, but its Wayland
//! backend has no drag and drop. On Linux Shiv therefore runs through X11
//! (XWayland on a Wayland desktop) when an X display is available. Set
//! `SHIV_WAYLAND=1` to keep native Wayland; dropping files then does nothing
//! and Install from file / Update from file still work.

use std::ffi::OsStr;

use slint::ComponentHandle;
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};

use crate::{App, installed};

/// Whether to ask winit for X11: an X display exists and native Wayland was
/// not requested.
pub(crate) fn prefer_x11(display: Option<&OsStr>, shiv_wayland: Option<&OsStr>) -> bool {
    let display = display.is_some_and(|d| !d.is_empty());
    let wayland = shiv_wayland.is_some_and(|v| !v.is_empty() && v != "0");
    display && !wayland
}

/// Select the winit backend, on X11 when [`prefer_x11`] says so. Must run
/// before the first window is created.
pub fn select_backend() -> Result<(), slint::PlatformError> {
    #[cfg(all(unix, not(target_vendor = "apple")))]
    if prefer_x11(
        std::env::var_os("DISPLAY").as_deref(),
        std::env::var_os("SHIV_WAYLAND").as_deref(),
    ) {
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

/// Route file drag and drop events of the main window to the Installed screen.
pub fn attach(app: &App) {
    app.window().on_winit_window_event(|_, event| {
        handle(event);
        EventResult::Propagate
    });
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
    fn x11_only_with_a_display_and_without_the_opt_out() {
        let d = Some(OsStr::new(":0"));
        assert!(prefer_x11(d, None));
        assert!(prefer_x11(d, Some(OsStr::new("0"))));
        assert!(!prefer_x11(d, Some(OsStr::new("1"))));
        assert!(!prefer_x11(None, None), "no X display: native Wayland");
        assert!(!prefer_x11(Some(OsStr::new("")), None));
    }
}
