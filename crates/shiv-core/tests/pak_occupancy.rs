use std::process::Command;

#[test]
fn choosing_a_pak_number_from_files_alone_fails_the_check() {
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Command::new("python3")
        .arg(repo.join("scripts/check_pak_occupancy.py"))
        .arg(&repo)
        .output()
        .expect("python3 runs the occupancy check");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "file_pak_numbers counts files only. Call pak_slot::occupied, or pak_slot::free_slots to take a number.\n{text}"
    );
}
