//! The data-release plan: one patch when embedded pack data changes, and
//! the same answer when the watch runs again.

use std::fs;
use std::path::Path;
use std::process::Command;

fn repo() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn script() -> std::path::PathBuf {
    repo().join("scripts/data_release.py")
}

fn run(args: &[&str]) -> (bool, String, String) {
    let output = Command::new("python3")
        .arg(script())
        .args(args)
        .output()
        .expect("python3 runs data_release.py");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

fn plan(args: &[&str]) -> String {
    let mut full = vec![
        "plan",
        "--assets-changed",
        "false",
        "--version",
        "0.4.0",
        "--tag-exists",
        "true",
        "--release-exists",
        "true",
        "--dispatch-settled",
        "false",
        "--head-pending",
        "false",
        "--tag-is-data-release",
        "false",
    ];
    full.extend_from_slice(args);
    let (ok, out, err) = run(&full);
    assert!(ok, "{err}");
    out.trim().to_owned()
}

#[test]
fn a_shipped_version_bumps_its_patch_once() {
    let (ok, out, err) = run(&["marker", "0.4.0"]);
    assert!(ok, "{err}");
    assert_eq!(out.trim(), "Data-Release-Version: 0.4.0");
    assert_eq!(
        plan(&[
            "--assets-changed",
            "true",
            "--tag-exists",
            "true",
            "--release-exists",
            "true",
        ]),
        "commit 0.4.1"
    );
    assert_eq!(
        plan(&[
            "--assets-changed",
            "true",
            "--version",
            "1.2.9",
            "--tag-exists",
            "true",
        ]),
        "commit 1.2.10"
    );
}

#[test]
fn an_unshipped_version_keeps_its_number() {
    assert_eq!(
        plan(&[
            "--assets-changed",
            "true",
            "--tag-exists",
            "false",
            "--release-exists",
            "false",
        ]),
        "commit 0.4.0"
    );
}

#[test]
fn a_second_run_does_not_bump_or_dispatch() {
    assert_eq!(plan(&[]), "skip");
    assert_eq!(
        plan(&[
            "--assets-changed",
            "false",
            "--tag-exists",
            "true",
            "--release-exists",
            "false",
            "--dispatch-settled",
            "true",
            "--tag-is-data-release",
            "true",
        ]),
        "skip",
        "a dispatch that is queued or already green is one release"
    );
    assert_eq!(
        plan(&[
            "--assets-changed",
            "false",
            "--tag-exists",
            "false",
            "--release-exists",
            "false",
            "--head-pending",
            "false",
        ]),
        "skip",
        "a human commit on main is not a data release"
    );
}

#[test]
fn a_rerun_finishes_a_release_that_stopped_early() {
    assert_eq!(
        plan(&[
            "--assets-changed",
            "false",
            "--tag-exists",
            "false",
            "--release-exists",
            "false",
            "--head-pending",
            "true",
        ]),
        "tag 0.4.0",
        "the version commit landed and the tag did not"
    );
    assert_eq!(
        plan(&[
            "--assets-changed",
            "false",
            "--tag-exists",
            "true",
            "--release-exists",
            "false",
            "--dispatch-settled",
            "false",
            "--tag-is-data-release",
            "true",
        ]),
        "dispatch 0.4.0",
        "the tag exists and the release workflow has not published it"
    );
}

#[test]
fn a_bad_version_is_refused() {
    for version in ["0.4", "1.2.3-rc", "01.2.3", ""] {
        let (ok, _out, err) = run(&["marker", version]);
        assert!(!ok, "{version} should be refused");
        assert!(err.contains("major.minor.patch"), "{err}");
    }
}

#[test]
fn apply_rewrites_only_the_app_version_and_is_idempotent() {
    let dir = std::env::temp_dir().join(format!(
        "shiv-data-release-{}-{}",
        std::process::id(),
        "apply"
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    let cargo = dir.join("Cargo.toml");
    let lock = dir.join("Cargo.lock");
    let meta = dir.join("metainfo.xml");
    fs::write(
        &cargo,
        "[package]\nname = \"shiv-ui\"\nversion = \"0.4.0\"\n\n[dependencies]\nimage = { version = \"0.4.0\" }\n",
    )
    .expect("cargo");
    fs::write(
        &lock,
        "[[package]]\nname = \"other\"\nversion = \"0.4.0\"\n\n[[package]]\nname = \"shiv-ui\"\nversion = \"0.4.0\"\ndependencies = [\n \"image\",\n]\n\n[[package]]\nname = \"winapi\"\nversion = \"0.4.0\"\n",
    )
    .expect("lock");
    fs::write(
        &meta,
        "<releases>\n    <release version=\"0.4.0\" date=\"2026-11-23\"/>\n</releases>\n",
    )
    .expect("metainfo");

    let args = [
        "apply",
        "--from",
        "0.4.0",
        "--to",
        "0.4.1",
        "--date",
        "2026-11-23",
        "--summary",
        "CompLock pack data from deadlockconfig.com, gameinfo build 6861.",
        "--cargo",
        cargo.to_str().expect("cargo path"),
        "--lock",
        lock.to_str().expect("lock path"),
        "--metainfo",
        meta.to_str().expect("meta path"),
    ];
    let (ok, _out, err) = run(&args);
    assert!(ok, "{err}");
    let cargo_text = fs::read_to_string(&cargo).expect("read cargo");
    let lock_text = fs::read_to_string(&lock).expect("read lock");
    let meta_text = fs::read_to_string(&meta).expect("read meta");
    assert!(cargo_text.contains("version = \"0.4.1\""));
    assert!(
        cargo_text.contains("image = { version = \"0.4.0\" }"),
        "a dependency version stays"
    );
    assert!(lock_text.contains("name = \"shiv-ui\"\nversion = \"0.4.1\"\n"));
    assert!(lock_text.contains("name = \"other\"\nversion = \"0.4.0\"\n"));
    assert!(lock_text.contains("name = \"winapi\"\nversion = \"0.4.0\"\n"));
    assert!(meta_text.contains("version=\"0.4.1\""));
    assert!(meta_text.contains("gameinfo build 6861"));
    assert!(meta_text.contains("version=\"0.4.0\""));

    let (ok, _out, err) = run(&args);
    assert!(ok, "{err}");
    assert_eq!(fs::read_to_string(&cargo).expect("cargo again"), cargo_text);
    assert_eq!(fs::read_to_string(&lock).expect("lock again"), lock_text);
    assert_eq!(fs::read_to_string(&meta).expect("meta again"), meta_text);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_watch_dispatches_release_on_the_version_tag() {
    let watch = fs::read_to_string(repo().join(".github/workflows/complock-watch.yml"))
        .expect("complock watch");
    let release =
        fs::read_to_string(repo().join(".github/workflows/release.yml")).expect("release workflow");
    assert!(
        watch.contains("gh workflow run release.yml --ref"),
        "the watch must start the release workflow itself"
    );
    assert!(
        watch.contains("scripts/data_release.py plan"),
        "the same plan decides a rerun"
    );
    assert!(watch.contains("scripts/data_release.py marker"));
    assert!(
        !watch.contains("gh release create"),
        "the watch must not publish a second way"
    );
    assert!(
        release.contains("startsWith(github.ref, 'refs/tags/v')"),
        "a dispatched v* tag publishes"
    );
    assert!(release.contains("refs/tags/v*"));
    let cargo = repo().join("crates/shiv-ui/Cargo.toml");
    let (ok, out, err) = run(&["read-version", cargo.to_str().expect("cargo path")]);
    assert!(ok, "{err}");
    let version = out.trim();
    let parts: Vec<&str> = version.split('.').collect();
    assert!(
        parts.len() == 3 && parts.iter().all(|p| p.parse::<u32>().is_ok()),
        "read-version gives the app version as x.y.z, got {version:?}"
    );
}
