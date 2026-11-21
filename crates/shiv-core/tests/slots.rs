//! CompLock slots: every pak of the site's pack goes on the slot its part
//! name gives (HUD++ pak01, ...), whatever order the installs finish in,
//! and whatever sat on those slots moves above the pack with its sidecar.
//! Real installs of real VPKs into a temp addons folder.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use shiv_core::game::GamePaths;
use shiv_core::install::{self, INSTALL_LOCK, ModMeta};
use shiv_core::{complock, sidecar};
use tempfile::{TempDir, tempdir};

struct Env {
    _root: TempDir,
    paths: GamePaths,
    work: PathBuf,
}

fn env() -> Env {
    let root = tempdir().unwrap();
    let paths = GamePaths::from_root(root.path().join("Deadlock"));
    fs::create_dir_all(&paths.addons).unwrap();
    let work = root.path().join("work");
    fs::create_dir_all(&work).unwrap();
    Env {
        _root: root,
        paths,
        work,
    }
}

fn meta(id: &str) -> ModMeta {
    ModMeta {
        mod_id: id.into(),
        name: id.into(),
        version: Some("1".into()),
        thumbnail_url: None,
        nsfw: false,
        hero: None,
        category: None,
    }
}

/// A real VPK named `name` in its own folder under `work`, holding one
/// file that names `tag`; with a `_000` chunk when `chunked`.
fn vpk(work: &Path, tag: &str, name: &str, chunked: bool) -> Vec<PathBuf> {
    let dir = work.join(tag);
    fs::create_dir_all(&dir).unwrap();
    let src = tempdir().unwrap();
    fs::create_dir_all(src.path().join("panorama")).unwrap();
    fs::write(src.path().join(format!("panorama/{tag}.txt")), tag).unwrap();
    let out = dir.join(name);
    vpkmanager::pack_directory(src.path(), &out).unwrap();
    let mut files = vec![out];
    if chunked {
        let chunk = dir.join(name.replace("_dir.vpk", "_000.vpk"));
        fs::write(&chunk, format!("chunk of {tag}")).unwrap();
        files.push(chunk);
    }
    files
}

fn vpks_of(e: &Env, id: &str) -> Vec<String> {
    sidecar::read(&e.paths.addons, id).unwrap().unwrap().vpks
}

/// The pack's paks as the site names them: `(option id, part name)`.
fn pack() -> Vec<(String, String)> {
    let hud = complock::hud().unwrap();
    let mut out = vec![(
        "hud".to_owned(),
        hud.pak_of_choice(0).unwrap().parts[0].name.clone(),
    )];
    for o in complock::options() {
        let pak = o
            .pak
            .as_ref()
            .or_else(|| o.choices.iter().find_map(|c| c.pak.as_ref()));
        if let Some(pak) = pak {
            out.push((o.id.clone(), pak.parts[0].name.clone()));
        }
    }
    out
}

#[test]
fn the_site_names_slots_one_to_seven_and_hud_is_pak01() {
    let pack = pack();
    assert_eq!(complock::last_slot(), 7);
    assert_eq!(pack[0], ("hud".to_owned(), "pak01_dir.vpk".to_owned()));
    let mut slots: Vec<u32> = pack
        .iter()
        .map(|(_, n)| complock::slot_of(n).unwrap())
        .collect();
    slots.sort_unstable();
    assert_eq!(slots, (1..=7).collect::<Vec<_>>(), "one pak per slot");
    assert_eq!(complock::slot_of("pak1_dir.vpk"), None);
    assert_eq!(complock::slot_of("pak01_000.vpk"), None);
}

/// The race the UI runs: every pak installs on its own thread, HUD++ last,
/// all through the one install lock. Before slots, HUD++ took whatever was
/// free when its download finished (pak07 here).
#[test]
fn concurrent_installs_land_on_the_sites_slots_whatever_finishes_first() {
    let e = env();
    let paths = Arc::new(e.paths.clone());
    let mut pack = pack();
    pack.reverse();
    let mut threads = Vec::new();
    for (n, (id, name)) in pack.iter().enumerate() {
        let files = vpk(&e.work, id, name, id == "cloth");
        let (paths, id) = (paths.clone(), format!("complock-{id}"));
        threads.push(std::thread::spawn(move || {
            // Later in the list finishes later: HUD++ comes last.
            std::thread::sleep(std::time::Duration::from_millis(30 * n as u64));
            let _guard = INSTALL_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            install::install_files_in_slots(
                &files,
                meta(&id),
                &paths,
                || false,
                complock::last_slot(),
            )
            .unwrap();
        }));
    }
    for t in threads {
        t.join().unwrap();
    }
    for (id, name) in &pack {
        assert_eq!(
            vpks_of(&e, &format!("complock-{id}")),
            std::slice::from_ref(name),
            "{id}"
        );
        assert!(e.paths.addons.join(name).is_file());
    }
    assert!(
        e.paths.addons.join("pak07_000.vpk").is_file(),
        "the chunk followed"
    );
}

#[test]
fn a_mod_on_a_pack_slot_moves_above_the_pack_with_its_chunks() {
    let e = env();
    // Two of the user's mods on pak01 and pak02 (one chunked).
    let a = vpk(&e.work, "mine-a", "pak01_dir.vpk", true);
    install::install_files(&a, meta("mine-a"), &e.paths, || false).unwrap();
    let b = vpk(&e.work, "mine-b", "pak01_dir.vpk", false);
    install::install_files(&b, meta("mine-b"), &e.paths, || false).unwrap();
    assert_eq!(vpks_of(&e, "mine-a"), ["pak01_dir.vpk"]);
    assert_eq!(vpks_of(&e, "mine-b"), ["pak02_dir.vpk"]);

    let hud = vpk(&e.work, "hud", "pak01_dir.vpk", false);
    install::install_files_in_slots(&hud, meta("complock-hud"), &e.paths, || false, 7).unwrap();
    assert_eq!(vpks_of(&e, "complock-hud"), ["pak01_dir.vpk"]);
    assert_eq!(vpks_of(&e, "mine-a"), ["pak08_dir.vpk"]);
    assert!(e.paths.addons.join("pak08_000.vpk").is_file());
    assert!(!e.paths.addons.join("pak01_000.vpk").exists());
    assert_eq!(
        vpks_of(&e, "mine-b"),
        ["pak02_dir.vpk"],
        "not on a wanted slot"
    );

    let particles = vpk(&e.work, "particles", "pak02_dir.vpk", false);
    install::install_files_in_slots(
        &particles,
        meta("complock-particles"),
        &e.paths,
        || false,
        7,
    )
    .unwrap();
    assert_eq!(vpks_of(&e, "complock-particles"), ["pak02_dir.vpk"]);
    assert_eq!(vpks_of(&e, "mine-b"), ["pak09_dir.vpk"]);
    // The moved mods still read as themselves.
    let mods = sidecar::group(&e.paths.addons);
    assert_eq!(mods.len(), 4);
    assert!(mods.iter().all(|m| m.sidecar.is_some()));
}

#[test]
fn an_update_or_an_old_shivs_slot_converges_on_the_named_slot() {
    let e = env();
    // 0.3.x put HUD++ wherever was free: pak05 here.
    for n in 1..=4 {
        let f = vpk(&e.work, &format!("filler{n}"), "pak01_dir.vpk", false);
        install::install_files(&f, meta(&format!("filler{n}")), &e.paths, || false).unwrap();
    }
    let old = vpk(&e.work, "hud-old", "pak01_dir.vpk", false);
    install::install_files(&old, meta("complock-hud"), &e.paths, || false).unwrap();
    assert_eq!(vpks_of(&e, "complock-hud"), ["pak05_dir.vpk"]);

    let new = vpk(&e.work, "hud-new", "pak01_dir.vpk", false);
    install::install_files_in_slots(&new, meta("complock-hud"), &e.paths, || false, 7).unwrap();
    assert_eq!(vpks_of(&e, "complock-hud"), ["pak01_dir.vpk"]);
    assert!(
        !e.paths.addons.join("pak05_dir.vpk").exists(),
        "the old file is gone"
    );
    assert_eq!(vpks_of(&e, "filler1"), ["pak08_dir.vpk"]);

    // Updating again in place keeps pak01 and leaves nothing behind.
    let again = vpk(&e.work, "hud-again", "pak01_dir.vpk", false);
    install::install_files_in_slots(&again, meta("complock-hud"), &e.paths, || false, 7).unwrap();
    assert_eq!(vpks_of(&e, "complock-hud"), ["pak01_dir.vpk"]);
    let mut names: Vec<String> = fs::read_dir(&e.paths.addons)
        .unwrap()
        .filter_map(|d| d.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with(".vpk"))
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "pak01_dir.vpk",
            "pak02_dir.vpk",
            "pak03_dir.vpk",
            "pak04_dir.vpk",
            "pak08_dir.vpk"
        ]
    );
}
