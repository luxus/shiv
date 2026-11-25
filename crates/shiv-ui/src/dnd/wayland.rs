//! File drops under native Wayland. winit 0.30 does not bind
//! `wl_data_device`, so Shiv binds its own on winit's connection (a second
//! data device per seat is allowed) and dispatches it on its own event queue
//! in a background thread. Only `text/uri-list` offers of local files are
//! accepted; everything else is left to the compositor's default (reject).

use std::collections::HashMap;
use std::io::Read;
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::ptr::NonNull;

use wayland_client::backend::{Backend, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
use wayland_client::protocol::wl_data_device_manager::{DndAction, WlDataDeviceManager};
use wayland_client::protocol::wl_data_offer::{self, WlDataOffer};
use wayland_client::protocol::{wl_registry, wl_seat::WlSeat};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, event_created_child};

const URI_LIST: &str = "text/uri-list";

/// What the drop thread reports to the UI thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DropEvent {
    Hover(bool),
    Files(Vec<PathBuf>),
}

/// Start listening for drops on the Wayland display `display` (winit's
/// `wl_display`). `emit` runs on the drop thread.
///
/// # Safety
/// `display` must stay a live `wl_display` for the rest of the process,
/// which holds for winit's display while the event loop exists.
pub(crate) unsafe fn start(
    display: NonNull<std::ffi::c_void>,
    emit: impl Fn(DropEvent) + Send + 'static,
) -> Result<(), String> {
    // SAFETY: the caller guarantees the display outlives the backend.
    let backend = unsafe { Backend::from_foreign_display(display.as_ptr().cast()) };
    let conn = Connection::from_backend(backend);
    let (globals, mut queue) =
        registry_queue_init::<State>(&conn).map_err(|e| format!("registry: {e}"))?;
    let qh = queue.handle();
    let manager: WlDataDeviceManager = globals
        .bind(&qh, 1..=3, ())
        .map_err(|e| format!("wl_data_device_manager: {e}"))?;
    let seat: WlSeat = globals
        .bind(&qh, 1..=1, ())
        .map_err(|e| format!("wl_seat: {e}"))?;
    let device = manager.get_data_device(&seat, &qh, ());
    let mut state = State {
        conn: conn.clone(),
        offers: HashMap::new(),
        current: None,
        emit: Box::new(emit),
        _device: device,
    };
    std::thread::Builder::new()
        .name("shiv-wayland-drop".into())
        .spawn(move || {
            loop {
                if let Err(e) = queue.blocking_dispatch(&mut state) {
                    crate::log::line(format_args!("Wayland drop listener stopped: {e}"));
                    return;
                }
            }
        })
        .map(|_| ())
        .map_err(|e| e.to_string())
}

struct State {
    conn: Connection,
    /// MIME types announced per offer.
    offers: HashMap<ObjectId, Vec<String>>,
    /// The offer under the pointer, when it carries files.
    current: Option<WlDataOffer>,
    emit: Box<dyn Fn(DropEvent) + Send>,
    _device: WlDataDevice,
}

impl State {
    fn uri_list_mime(&self, offer: &WlDataOffer) -> Option<&str> {
        self.offers
            .get(&offer.id())
            .and_then(|mimes| pick_uri_list_mime(mimes))
    }

    fn forget(&mut self, offer: &WlDataOffer) {
        self.offers.remove(&offer.id());
        offer.destroy();
    }

    /// Read the offer's uri-list. The source writes into the pipe from its
    /// own process, so reading here after a flush does not deadlock.
    fn read_uris(&self, offer: &WlDataOffer) -> Option<String> {
        const MAX_URI_LIST_BYTES: u64 = 1024 * 1024;
        let mime = self.uri_list_mime(offer)?.to_owned();
        let (reader, writer) = std::io::pipe().ok()?;
        offer.receive(mime, writer.as_fd());
        drop(writer);
        self.conn.flush().ok()?;
        let mut text = String::new();
        reader
            .take(MAX_URI_LIST_BYTES)
            .read_to_string(&mut text)
            .ok()?;
        Some(text)
    }
}

fn is_uri_list_mime(mime: &str) -> bool {
    mime == URI_LIST || mime.starts_with("text/uri-list;")
}

fn pick_uri_list_mime(mimes: &[String]) -> Option<&str> {
    mimes
        .iter()
        .find(|m| m.as_str() == URI_LIST)
        .or_else(|| mimes.iter().find(|m| is_uri_list_mime(m)))
        .map(String::as_str)
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlSeat,
        _: <WlSeat as Proxy>::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlDataDeviceManager, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlDataDeviceManager,
        _: <WlDataDeviceManager as Proxy>::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlDataOffer, ()> for State {
    fn event(
        state: &mut Self,
        offer: &WlDataOffer,
        event: wl_data_offer::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_offer::Event::Offer { mime_type } = event {
            state.offers.entry(offer.id()).or_default().push(mime_type);
        }
    }
}

impl Dispatch<WlDataDevice, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlDataDevice,
        event: wl_data_device::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_device::Event::DataOffer { id } => {
                state.offers.insert(id.id(), Vec::new());
            }
            wl_data_device::Event::Enter { serial, id, .. } => {
                if let Some(old) = state.current.take() {
                    state.forget(&old);
                }
                let Some(offer) = id else { return };
                let Some(mime) = state.uri_list_mime(&offer).map(str::to_owned) else {
                    offer.accept(serial, None);
                    state.forget(&offer);
                    return;
                };
                offer.accept(serial, Some(mime));
                if offer.version() >= 3 {
                    offer.set_actions(DndAction::Copy | DndAction::Move, DndAction::Copy);
                }
                state.current = Some(offer);
                (state.emit)(DropEvent::Hover(true));
            }
            wl_data_device::Event::Leave => {
                if let Some(offer) = state.current.take() {
                    state.forget(&offer);
                    (state.emit)(DropEvent::Hover(false));
                }
            }
            wl_data_device::Event::Drop => {
                let Some(offer) = state.current.take() else {
                    return;
                };
                let uris = state.read_uris(&offer);
                if offer.version() >= 3 {
                    offer.finish();
                }
                state.forget(&offer);
                (state.emit)(DropEvent::Hover(false));
                let files = uris.as_deref().map(file_paths).unwrap_or_default();
                if !files.is_empty() {
                    (state.emit)(DropEvent::Files(files));
                }
            }
            // The clipboard selection is not ours to handle.
            wl_data_device::Event::Selection { id: Some(offer) }
                if state.current.as_ref() != Some(&offer) =>
            {
                state.forget(&offer);
            }
            _ => {}
        }
    }

    event_created_child!(State, WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, ()),
    ]);
}

/// Local file paths in a `text/uri-list` body: `#` comments and non-`file:`
/// URIs are skipped, `%XX` escapes decoded.
pub(crate) fn file_paths(list: &str) -> Vec<PathBuf> {
    list.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.strip_prefix("file://"))
        // `file://host/path`: only the local host (empty or localhost).
        .filter_map(|rest| match rest.find('/') {
            Some(0) => Some(rest),
            Some(i) if &rest[..i] == "localhost" => Some(&rest[i..]),
            _ => None,
        })
        .filter_map(percent_decode)
        .map(PathBuf::from)
        .collect()
}

fn percent_decode(s: &str) -> Option<std::ffi::OsString> {
    use std::os::unix::ffi::OsStringExt;
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some(std::ffi::OsString::from_vec(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_lists_become_local_paths() {
        let list = "# from dolphin\r\nfile:///home/kai/Downloads/Haze%20Skin%20v2.zip\r\nfile://localhost/tmp/a.vpk\r\nhttps://example.com/x.vpk\r\nfile://otherhost/tmp/b.vpk\r\nfile:///tmp/%C3%A4%ZZ\r\n";
        assert_eq!(
            file_paths(list),
            [
                PathBuf::from("/home/kai/Downloads/Haze Skin v2.zip"),
                PathBuf::from("/tmp/a.vpk"),
            ]
        );
        assert_eq!(
            file_paths("file:///tmp/%C3%A4.vpk"),
            [PathBuf::from("/tmp/ä.vpk")]
        );
    }

    #[test]
    fn uri_list_mime_matches_exact_and_parameterized() {
        let exact = vec!["text/plain".into(), "text/uri-list".into()];
        assert_eq!(pick_uri_list_mime(&exact), Some("text/uri-list"));

        let charset = vec!["text/uri-list;charset=utf-8".into()];
        assert_eq!(
            pick_uri_list_mime(&charset),
            Some("text/uri-list;charset=utf-8")
        );

        let prefer_exact = vec!["text/uri-list;charset=utf-8".into(), "text/uri-list".into()];
        assert_eq!(pick_uri_list_mime(&prefer_exact), Some("text/uri-list"));

        let other = vec!["text/plain".into(), "application/octet-stream".into()];
        assert_eq!(pick_uri_list_mime(&other), None);
    }
}
