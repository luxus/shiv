//! The self-updater. A silent check on launch and the Check-for-updates
//! row both fetch the latest release; where this system allows replacing
//! the binary, Update downloads the asset, verifies the release's
//! published SHA-256 and swaps the file, then offers a restart. Where it
//! does not (Flatpak, Nix, a distro package, a binary placed by hand),
//! the row says how to update and opens the release page.

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;

use slint::ComponentHandle;

use crate::{App, UpdateState, spawn_worker};
use shiv_api::Client;
use shiv_api::{
    ASSET_ORIGINS, Release, asset_digest, digest_hex, latest_release, release_json, tag_is_newer,
};
use shiv_core::link::open_web_link;
use shiv_core::registry::check_download;

/// The running version, the thing a release tag is compared against.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The user agent the release check identifies as; GitHub refuses
/// requests without one.
const USER_AGENT: &str = concat!("shiv/", env!("CARGO_PKG_VERSION"));

/// The release page, opened when this install cannot update itself.
const RELEASES_URL: &str = "https://github.com/luxus/shiv/releases/latest";

/// The client the updater downloads through; its base is unused, the
/// asset fetch takes absolute URLs.
const DOWNLOADS_BASE: &str = "https://github.com";

// The file the swap moved in, waiting for the Restart row. Read and
// written on the UI thread only (the ready posting and the callback).
thread_local! {
    static PENDING: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Where an update can go on this system.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Target {
    /// Replace the file at this path (a portable `.exe`, an `AppImage`),
    /// then restart into it.
    Swap(PathBuf),
    /// An Inno Setup install: download the new setup exe, run it, quit.
    Installer,
    /// The package manager owns the binary; show `note` instead.
    Managed(&'static str),
    /// A cargo build output: no checks, no row.
    Dev,
}

/// The release file an update to a version needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Asset {
    /// Constructed off Windows: there the release's file is the portable
    /// `.exe`, and an `AppImage` needs the `APPIMAGE` runtime.
    #[cfg_attr(windows, allow(dead_code, reason = "constructed off Windows only"))]
    AppImage,
    /// Constructed on Windows only: the release's Linux file is the
    /// `AppImage`, and a bare Linux binary has nothing to swap.
    #[cfg_attr(not(windows), allow(dead_code, reason = "constructed on Windows only"))]
    Portable,
    Setup,
}

impl Asset {
    /// The published file name for `version`.
    fn file_name(self, version: &str) -> String {
        match self {
            Asset::AppImage => format!("Shiv-{version}-x86_64.AppImage"),
            Asset::Portable => format!("Shiv-{version}-x64-portable.exe"),
            Asset::Setup => format!("Shiv-{version}-x64-setup.exe"),
        }
    }
}

/// The runtime facts the update target is decided from: read once,
/// cheap, and every field is driveable in the tests.
struct Runtime {
    exe: PathBuf,
    appimage: Option<String>,
    /// `unins000.exe` beside the exe: an Inno Setup install.
    installed: bool,
    /// The exe is a cargo build output.
    dev: bool,
    /// The note shown instead of a swap, when the platform owns the
    /// binary (Flatpak, Nix, a distro package).
    managed: Option<&'static str>,
}

impl Runtime {
    fn detect() -> Runtime {
        let exe = std::env::current_exe().unwrap_or_default();
        let dev = exe.as_os_str().is_empty() || under_target_release(&exe);
        let managed = managed_note(
            std::env::var("FLATPAK_ID").is_ok(),
            exe.starts_with("/nix/store"),
            exe.parent().is_some_and(|dir| dir == Path::new("/usr/bin")),
        );
        Runtime {
            dev,
            managed,
            installed: sibling(&exe, "unins000.exe").try_exists().unwrap_or(false),
            appimage: std::env::var("APPIMAGE").ok().filter(|v| !v.is_empty()),
            exe,
        }
    }
}

/// How to update when the platform owns the binary; `None` when Shiv can
/// replace its own file. Flatpak, Nix and `/usr/bin` cannot occur
/// together, so the order is only for reading.
fn managed_note(flatpak: bool, nix: bool, usr_bin: bool) -> Option<&'static str> {
    if flatpak {
        return Some("Flatpak installs update through `flatpak update`.");
    }
    if nix {
        return Some("Nix installs update through the shiv flake input.");
    }
    if usr_bin {
        return Some("A distro package updates through your package manager.");
    }
    None
}

/// `.../target/release/shiv`, a build the release files never match.
fn under_target_release(exe: &Path) -> bool {
    exe.parent().is_some_and(|dir| {
        dir.file_name().is_some_and(|name| name == "release")
            && dir
                .parent()
                .is_some_and(|grand| grand.file_name().is_some_and(|name| name == "target"))
    })
}

/// Where an update goes, from the runtime facts.
fn target_from(runtime: &Runtime) -> Target {
    if runtime.dev {
        return Target::Dev;
    }
    if let Some(path) = &runtime.appimage {
        return Target::Swap(PathBuf::from(path));
    }
    if let Some(note) = runtime.managed {
        return Target::Managed(note);
    }
    if runtime.installed {
        return Target::Installer;
    }
    Target::Swap(runtime.exe.clone())
}

/// The release asset an update here can use; `None` when the release
/// publishes nothing this install can replace itself with.
fn asset_for(target: &Target) -> Option<Asset> {
    match target {
        Target::Swap(path) => asset_of_swap(path),
        Target::Installer => Some(Asset::Setup),
        Target::Managed(_) | Target::Dev => None,
    }
}

#[cfg(windows)]
// Every Windows arm returns Some; the Option keeps the one signature
// the non-Windows arm needs (a placed binary has no asset).
#[allow(clippy::unnecessary_wraps, reason = "the non-Windows arm returns None")]
fn asset_of_swap(_path: &Path) -> Option<Asset> {
    Some(Asset::Portable)
}

#[cfg(not(windows))]
fn asset_of_swap(path: &Path) -> Option<Asset> {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("AppImage"))
        .then_some(Asset::AppImage)
}

/// Wire the strip's callbacks and start the silent launch check.
pub(crate) fn attach(app: &App) {
    let state = app.global::<UpdateState>();
    let weak = app.as_weak();
    state.on_check(move || start_check(weak.clone(), false));
    let weak = app.as_weak();
    state.on_apply(move || start_apply(weak.clone()));
    state.on_restart(move || {
        let pending = PENDING.with(|cell| cell.borrow_mut().take());
        let Some(exe) = pending else {
            return;
        };
        // The new process starts on a worker; the quit is posted only
        // after the spawn returned, so the loop outlives the start.
        let _ = spawn_worker(
            "shiv-restart",
            move || Command::new(exe).spawn().is_ok(),
            move |started| {
                if started.unwrap_or(false) {
                    let _ = slint::invoke_from_event_loop(|| {
                        let _ = slint::quit_event_loop();
                    });
                }
            },
        );
    });
    state.on_open_release(move || {
        let _ = open_web_link(RELEASES_URL);
    });
    clean_stale_asides();
    // The launch check is silent: a failed or up-to-date check leaves
    // the strip as the quiet Check-for-updates row.
    start_check(app.as_weak(), true);
}

/// Start the release check on a worker. `silent` keeps a failed or
/// up-to-date check from showing anything (the launch check); the
/// row's own check reports what happened.
fn start_check(weak: slint::Weak<App>, silent: bool) {
    let target = target_from(&Runtime::detect());
    if matches!(target, Target::Dev) {
        if !silent {
            set_status(&weak, "This is a development build; releases do not apply.");
        }
        return;
    }
    if let Some(app) = weak.upgrade() {
        let state = app.global::<UpdateState>();
        state.set_busy(true);
        state.set_status("Checking for updates…".into());
    }
    let work_target = target.clone();
    let fallback = weak.clone();
    let started = spawn_worker(
        "shiv-update-check",
        move || check_release(&work_target),
        // `unwrap_or_else(Err)` folds a work panic into the same Err
        // arm as a failed check.
        move |result| match result.unwrap_or_else(Err) {
            Ok(outcome) => post_check(&weak, &target, outcome, silent),
            // Both arms clear `busy`; a silent failure leaves the
            // status empty so the strip falls back to the check row.
            Err(text) if !silent => set_status(&weak, &text),
            Err(_) => set_status(&weak, ""),
        },
    );
    if !started {
        set_status(&fallback, "The check could not start.");
    }
}

/// Worker body: fetch the latest release and compare its tag with the
/// running version.
fn check_release(target: &Target) -> Result<CheckOutcome, String> {
    let release = fetch_latest()?;
    if !tag_is_newer(&release.tag, VERSION) {
        return Ok(CheckOutcome::UpToDate);
    }
    let version = release
        .tag
        .strip_prefix('v')
        .unwrap_or(&release.tag)
        .to_owned();
    Ok(CheckOutcome::Newer {
        version,
        swappable: asset_for(target).is_some(),
    })
}

enum CheckOutcome {
    UpToDate,
    Newer { version: String, swappable: bool },
}

fn post_check(weak: &slint::Weak<App>, target: &Target, outcome: CheckOutcome, silent: bool) {
    match outcome {
        CheckOutcome::UpToDate => {
            let _ = weak.upgrade_in_event_loop(move |app| {
                let state = app.global::<UpdateState>();
                state.set_busy(false);
                if !silent {
                    state.set_status("Shiv is up to date.".into());
                }
            });
        }
        CheckOutcome::Newer { version, swappable } => {
            let managed = match (target, swappable) {
                (Target::Managed(note), _) => Some((*note).to_owned()),
                (_, false) => Some(
                    "This copy did not come from a release file; update it by hand from the \
                     release page."
                        .to_owned(),
                ),
                _ => None,
            };
            let _ = weak.upgrade_in_event_loop(move |app| {
                let state = app.global::<UpdateState>();
                state.set_version(version.into());
                if let Some(note) = managed {
                    state.set_managed(note.into());
                }
                state.set_status(String::new().into());
                state.set_busy(false);
            });
        }
    }
}

/// Start the download-and-apply on a worker. The release is re-fetched
/// there, so nothing about the check is carried as shared state.
fn start_apply(weak: slint::Weak<App>) {
    let target = target_from(&Runtime::detect());
    if !matches!(target, Target::Swap(_) | Target::Installer) {
        set_status(&weak, "This install cannot update itself.");
        return;
    }
    if let Some(app) = weak.upgrade() {
        let state = app.global::<UpdateState>();
        state.set_busy(true);
        state.set_status("Downloading…".into());
    }
    let work_weak = weak.clone();
    let fallback = weak.clone();
    let started = spawn_worker(
        "shiv-update",
        move || perform_update(&target, &work_weak),
        move |result| match result.unwrap_or_else(Err) {
            Ok(applied) => post_applied(&weak, applied),
            Err(text) => {
                let _ = weak.upgrade_in_event_loop(move |app| {
                    let state = app.global::<UpdateState>();
                    state.set_busy(false);
                    state.set_status(format!("The update did not finish: {text}").into());
                });
            }
        },
    );
    if !started {
        set_status(&fallback, "The update could not start.");
    }
}

/// Worker body: download the release asset beside the file it replaces,
/// verify the release's published digest against it, then swap or run
/// the installer. A digest mismatch deletes the download and refuses.
fn perform_update(target: &Target, weak: &slint::Weak<App>) -> Result<Applied, String> {
    let asset_kind = asset_for(target).ok_or("this install has no release file to update")?;
    let release = fetch_latest()?;
    if !tag_is_newer(&release.tag, VERSION) {
        return Err("the newer release is gone; check again".into());
    }
    let version = release.tag.strip_prefix('v').unwrap_or(&release.tag);
    let file = asset_kind.file_name(version);
    let asset = release
        .asset(&file)
        .ok_or_else(|| format!("the release publishes no {file}"))?;
    let digest = release
        .asset(&format!("{file}.sha256"))
        .ok_or_else(|| format!("the release publishes no {file}.sha256"))?;
    let hex = digest_hex(&asset_digest(&digest.url).map_err(|e| e.to_string())?)
        .ok_or("the release's sha256 file is malformed")?
        .to_owned();

    let staged = match target {
        Target::Swap(path) => sibling(path, ".new"),
        Target::Installer => std::env::temp_dir().join(&file),
        Target::Managed(_) | Target::Dev => return Err("this install cannot update itself".into()),
    };
    let client = Client::new(DOWNLOADS_BASE);
    let origins: Vec<String> = ASSET_ORIGINS.iter().map(|o| (*o).to_owned()).collect();
    let mut last_step = u64::MAX;
    let cancel = AtomicBool::new(false);
    let fetched = client.fetch_cancellable(
        &asset.url,
        &staged,
        &cancel,
        |done, total| {
            let step = match total {
                Some(t) if t > 0 => done.saturating_mul(100) / t,
                _ => done / 1_000_000,
            };
            if step == last_step {
                return;
            }
            last_step = step;
            let text = match total {
                Some(t) if t > 0 => format!("Downloading… {} %", step.min(100)),
                _ => format!("Downloading… {} MB", done / 1_000_000),
            };
            let weak = weak.clone();
            let _ = weak.upgrade_in_event_loop(move |app| {
                app.global::<UpdateState>().set_status(text.into());
            });
        },
        Some(&origins),
    );
    fetched.map_err(|e| format!("the download failed: {e}"))?;
    check_download(&staged, Some(&hex), None).map_err(|e| e.to_string())?;

    match target {
        Target::Swap(path) => {
            swap_new_in(&staged, path)?;
            Ok(Applied::Swapped(path.clone()))
        }
        Target::Installer => Ok(Applied::Installer(staged)),
        Target::Managed(_) | Target::Dev => Err("this install cannot update itself".into()),
    }
}

enum Applied {
    Swapped(PathBuf),
    Installer(PathBuf),
}

/// Worker body: run the applied update's effect, off the event loop. The
/// swap's file is posted to the strip for the Restart row; the installer
/// is started and the app quits so it can replace files.
fn post_applied(weak: &slint::Weak<App>, applied: Applied) {
    match applied {
        Applied::Swapped(path) => {
            let _ = weak.upgrade_in_event_loop(move |app| {
                PENDING.with(|cell| *cell.borrow_mut() = Some(path));
                let state = app.global::<UpdateState>();
                state.set_busy(false);
                state.set_ready(true);
                state.set_status(String::new().into());
            });
        }
        Applied::Installer(setup) => {
            // The installer replaces files this process keeps open; quit
            // so it can. Inno offers to start the new Shiv at the end.
            if Command::new(setup).spawn().is_ok() {
                let _ = slint::invoke_from_event_loop(|| {
                    let _ = slint::quit_event_loop();
                });
            } else {
                let _ = weak.upgrade_in_event_loop(|app| {
                    let state = app.global::<UpdateState>();
                    state.set_busy(false);
                    state.set_status("The installer could not start.".into());
                });
            }
        }
    }
}

/// Worker body: fetch the latest release.
fn fetch_latest() -> Result<Release, String> {
    latest_release(|url| release_json(url, USER_AGENT)).map_err(|e| e.to_string())
}

/// Worker body: move the verified `new_file` in over `dest`, keeping the
/// old file aside until the new one is in place. Windows cannot delete a
/// running exe, so the aside file is cleaned up at the next start.
fn swap_new_in(new_file: &Path, dest: &Path) -> Result<(), String> {
    let aside = sibling(dest, ".old");
    fs::rename(dest, &aside).map_err(|e| format!("the old file could not be moved aside: {e}"))?;
    if let Err(e) = fs::rename(new_file, dest) {
        let _ = fs::rename(&aside, dest);
        return Err(format!("the new file could not be moved in: {e}"));
    }
    let _ = fs::remove_file(&aside);
    Ok(())
}

/// Remove the aside file a previous Windows swap left behind (a running
/// exe cannot be deleted; by now it is not running).
fn clean_stale_asides() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let aside = sibling(&exe, ".old");
    let _ = spawn_worker("shiv-update-clean", move || fs::remove_file(aside), |_| {});
}

/// `dest`'s sibling with `suffix` appended to its file name.
fn sibling(dest: &Path, suffix: &str) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    dest.with_file_name(name)
}

/// Set the strip's status line and clear `busy`; an empty `text` leaves
/// the quiet Check-for-updates row.
fn set_status(weak: &slint::Weak<App>, text: &str) {
    let text = text.to_owned();
    let _ = weak.upgrade_in_event_loop(move |app| {
        let state = app.global::<UpdateState>();
        state.set_busy(false);
        state.set_status(text.into());
    });
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    fn runtime(exe: &Path, appimage: Option<&str>) -> Runtime {
        Runtime {
            exe: exe.to_path_buf(),
            appimage: appimage.map(str::to_owned),
            installed: false,
            dev: under_target_release(exe),
            managed: None,
        }
    }

    fn scratch(name: &str) -> PathBuf {
        static COUNT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "shiv-updater-{}-{}-{name}",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("{e}"));
        dir
    }

    fn write(path: &Path, bytes: &[u8]) {
        fs::write(path, bytes).unwrap_or_else(|e| panic!("{e}"));
    }

    fn read(path: &Path) -> Vec<u8> {
        fs::read(path).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn a_dev_build_a_package_and_a_flatpak_never_swap() {
        let dev = runtime(Path::new("/home/u/shiv/target/release/shiv"), None);
        assert_eq!(target_from(&dev), Target::Dev);

        let mut flatpak = runtime(Path::new("/app/bin/shiv"), None);
        flatpak.managed = managed_note(true, false, false);
        assert_eq!(
            target_from(&flatpak),
            Target::Managed("Flatpak installs update through `flatpak update`.")
        );

        let mut nix = runtime(Path::new("/nix/store/xyz-shiv/bin/shiv"), None);
        nix.managed = managed_note(false, true, false);
        assert_eq!(
            target_from(&nix),
            Target::Managed("Nix installs update through the shiv flake input.")
        );

        let mut aur = runtime(Path::new("/usr/bin/shiv"), None);
        aur.managed = managed_note(false, false, true);
        assert_eq!(
            target_from(&aur),
            Target::Managed("A distro package updates through your package manager.")
        );

        assert_eq!(managed_note(false, false, false), None);
    }

    #[test]
    fn an_appimage_swaps_its_own_file_and_a_placed_binary_cannot() {
        let image = runtime(
            Path::new("/tmp/x/shiv.AppImage"),
            Some("/home/u/Applications/Shiv.AppImage"),
        );
        assert_eq!(
            target_from(&image),
            Target::Swap(PathBuf::from("/home/u/Applications/Shiv.AppImage"))
        );
        assert_eq!(
            asset_for(&Target::Swap(image.exe.clone())),
            Some(Asset::AppImage)
        );

        let bare = runtime(Path::new("/home/u/bin/shiv"), None);
        assert_eq!(
            target_from(&bare),
            Target::Swap(PathBuf::from("/home/u/bin/shiv"))
        );
        assert_eq!(asset_for(&Target::Swap(bare.exe)), None);
    }

    #[cfg(windows)]
    #[test]
    fn a_windows_binary_swaps_the_portable_exe() {
        let portable = runtime(Path::new(r"C:\Tools\shiv\shiv.exe"), None);
        assert_eq!(target_from(&portable), Target::Swap(portable.exe.clone()));
        assert_eq!(
            asset_for(&Target::Swap(portable.exe)),
            Some(Asset::Portable)
        );
    }

    #[test]
    fn an_inno_install_runs_the_setup_exe() {
        let mut installed = runtime(
            Path::new(r"C:\Users\u\AppData\Local\Programs\Shiv\shiv.exe"),
            None,
        );
        installed.installed = true;
        assert_eq!(target_from(&installed), Target::Installer);
        assert_eq!(asset_for(&Target::Installer), Some(Asset::Setup));
    }

    #[test]
    fn asset_names_match_the_release_files() {
        assert_eq!(
            Asset::AppImage.file_name("0.6.1"),
            "Shiv-0.6.1-x86_64.AppImage"
        );
        assert_eq!(
            Asset::Portable.file_name("0.6.1"),
            "Shiv-0.6.1-x64-portable.exe"
        );
        assert_eq!(Asset::Setup.file_name("0.6.1"), "Shiv-0.6.1-x64-setup.exe");
    }

    #[test]
    fn a_verified_swap_moves_the_new_file_in_and_removes_the_old() {
        let dir = scratch("swap");
        let exe = dir.join("shiv.AppImage");
        let staged = dir.join("shiv.AppImage.new");
        write(&exe, b"old");
        write(&staged, b"new");

        super::swap_new_in(&staged, &exe).unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(read(&exe), b"new");
        assert!(!staged.try_exists().unwrap_or(false));
        assert!(!sibling(&exe, ".old").try_exists().unwrap_or(false));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_move_in_restores_the_old_file() {
        let dir = scratch("rollback");
        let exe = dir.join("shiv.AppImage");
        write(&exe, b"old");
        let missing = dir.join("shiv.AppImage.new");

        let result = super::swap_new_in(&missing, &exe);

        assert!(result.is_err());
        assert_eq!(read(&exe), b"old");
        assert!(!sibling(&exe, ".old").try_exists().unwrap_or(false));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn siblings_append_to_the_file_name() {
        assert_eq!(
            sibling(Path::new("/a/b/shiv.AppImage"), ".old"),
            PathBuf::from("/a/b/shiv.AppImage.old")
        );
        assert_eq!(
            sibling(Path::new("/a/b/shiv.AppImage"), ".new"),
            PathBuf::from("/a/b/shiv.AppImage.new")
        );
    }
}
