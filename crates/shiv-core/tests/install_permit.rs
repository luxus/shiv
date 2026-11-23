//! The calls that shipped without the install lock, against the current
//! signatures. `6c5beb5` added the lock at the edit dialog's `set_pak`.
//! `8b37a9b` added it at the interchange import's `install_files`. Both
//! calls, as they were, must fail naming `InstallPermit`.

use std::path::Path;
use std::process::Command;

const FORGOTTEN: &str = r#"
fn interchange_import_before_8b37a9b(
    files: &[std::path::PathBuf],
    meta: shiv_core::install::ModMeta,
    paths: &shiv_core::game::GamePaths,
    game_running: &dyn Fn() -> bool,
) -> Result<shiv_core::sidecar::Sidecar, shiv_core::install::InstallError> {
    shiv_core::install::install_files(files, meta, paths, game_running)
}

fn set_pak_before_6c5beb5(
    addons: &std::path::Path,
    mods: &[shiv_core::sidecar::InstalledMod],
    index: usize,
    pak: u32,
) -> Result<(), shiv_core::sidecar::SidecarError> {
    shiv_core::sidecar::set_pak(addons, mods, index, pak)
}

fn next_pak_from_outside(addons: &std::path::Path) -> Result<u32, shiv_core::addons::AddonsError> {
    shiv_core::addons::next_pak(addons)
}

fn claim_reusable_without_permit(
    addons: &std::path::Path,
    part: &shiv_core::complock::Part,
) -> std::io::Result<Option<shiv_core::complock::ReusedPart>> {
    shiv_core::complock::claim_reusable(addons, part)
}
"#;

#[test]
fn forgotten_pak_change_does_not_compile() {
    let root = tempfile::tempdir().expect("tempdir");
    let manifest = root.path().join("Cargo.toml");
    let src = root.path().join("src");
    std::fs::create_dir_all(&src).expect("src");
    let core = Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::write(
        &manifest,
        format!(
            "[package]\nname = \"forgotten-lock\"\nversion = \"0.0.0\"\nedition = \"2021\"\npublish = false\n\n[dependencies]\nshiv-core = {{ path = \"{}\" }}\n",
            core.display()
        ),
    )
    .expect("manifest");
    std::fs::write(src.join("lib.rs"), FORGOTTEN).expect("lib");
    let output = Command::new("cargo")
        .args(["check", "--offline", "--manifest-path"])
        .arg(&manifest)
        .env("CARGO_TARGET_DIR", root.path().join("target"))
        .output()
        .expect("cargo check");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "the pre-lock calls compiled. install_files, set_pak, and claim_reusable must take InstallPermit.\n{text}"
    );
    assert!(
        text.contains("InstallPermit"),
        "the error must name InstallPermit, the type that is the lock.\n{text}"
    );
    assert!(
        text.contains("install_files"),
        "8b37a9b's interchange import called install_files with no lock.\n{text}"
    );
    assert!(
        text.contains("set_pak"),
        "6c5beb5's edit dialog called set_pak with no lock.\n{text}"
    );
    assert!(
        text.contains("claim_reusable"),
        "claim_reusable renames paks and must require InstallPermit.\n{text}"
    );
    assert!(
        text.contains("next_pak") && text.contains("private"),
        "an outside crate must not call next_pak.\n{text}"
    );
}
