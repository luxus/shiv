//! The real binary answers `--version` and `--help` and exits, without a
//! display and without ever starting the window (0.3.2 opened it and so
//! "hung" on `shiv --version`).

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn shiv(arg: &str) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_shiv"))
        .arg(arg)
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("WAYLAND_SOCKET")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn shiv");
    let start = Instant::now();
    while child.try_wait().expect("wait").is_none() {
        if start.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            panic!("shiv {arg} did not exit");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    child.wait_with_output().expect("output")
}

#[test]
fn version_and_help_print_and_exit() {
    let out = shiv("--version");
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        format!("shiv {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert!(out.stderr.is_empty(), "no backend was started: {out:?}");
    let out = shiv("--help");
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("Usage: shiv"));
}

#[test]
fn an_unknown_flag_fails_without_opening_anything() {
    let out = shiv("--frobnicate");
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown option '--frobnicate'"));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("backend"));
}
