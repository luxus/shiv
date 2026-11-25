//! App release version tooling and CompLock watch workflow guard.

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

#[test]
fn a_bad_version_is_refused() {
    for version in ["0.4", "1.2.3-rc", "01.2.3", ""] {
        let (ok, _out, err) = run(&["cmp-version", "--current", version, "--tag", "v0.6.0"]);
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
fn the_watch_publishes_a_prerelease_artifact_without_releasing_shiv() {
    let watch = fs::read_to_string(repo().join(".github/workflows/complock-watch.yml"))
        .expect("complock watch");
    assert!(watch.contains("--prerelease"));
    assert!(watch.contains("--latest=false"));
    assert!(watch.contains("complock-data"));
    assert!(watch.contains("scripts/complock_artifact.py"));
    for sim in ["needs-shiv", "bad-crc", "missing-pak"] {
        assert!(watch.contains(sim), "missing simulate option {sim}");
    }
    assert!(
        !watch.contains("scripts/data_release.py"),
        "the watch must never compute or apply an app release"
    );
    assert!(
        !watch.contains("gh workflow run release.yml"),
        "the watch must not dispatch release.yml"
    );
    assert!(!watch.contains("git tag"), "the watch must not tag v*");
    assert!(
        !watch.contains("actions: write"),
        "the watch must not hold actions: write permission"
    );

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

#[test]
fn release_tag_has_permission_to_dispatch_release() {
    let workflow =
        fs::read_to_string(repo().join(".github/workflows/release-tag.yml")).expect("release-tag");
    assert!(workflow.contains("gh workflow run release.yml"));
    assert!(
        workflow.contains("actions: write"),
        "release-tag needs actions: write to workflow_dispatch release.yml"
    );
}
