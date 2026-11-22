//! Drift (issue #21): the stamps an install records, and the state an
//! installed mod has against what its source publishes — over real
//! installs into a temp addons folder, never a hand-built sidecar where an
//! install can make one.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use shiv_core::drift::{self, Drift, Published};
use shiv_core::game::GamePaths;
use shiv_core::install::{self, ModMeta};
use shiv_core::{addons, complock, sidecar};
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

/// A real VPK at `out` holding `files` (path, contents).
fn vpk(out: &Path, files: &[(&str, &str)]) {
    let src = tempdir().unwrap();
    for (p, body) in files {
        let f = src.path().join(p);
        fs::create_dir_all(f.parent().unwrap()).unwrap();
        fs::write(f, body).unwrap();
    }
    vpkmanager::pack_directory(src.path(), out).unwrap();
}

/// Install `names` (dir VPKs with a chunk each when `chunked`) as mod `id`
/// at `version`, and return the installed mod as the addons scan sees it.
fn install(
    e: &Env,
    id: &str,
    version: &str,
    names: &[&str],
    chunked: bool,
) -> sidecar::InstalledMod {
    let mut files = Vec::new();
    for (n, name) in names.iter().enumerate() {
        let dir = e.work.join(format!("pak{:02}_dir.vpk", n + 1));
        vpk(&dir, &[(&format!("panorama/{name}.txt"), name)]);
        files.push(dir);
        if chunked {
            let chunk = e.work.join(format!("pak{:02}_000.vpk", n + 1));
            fs::write(&chunk, format!("chunk of {name}")).unwrap();
            files.push(chunk);
        }
    }
    let meta = ModMeta {
        mod_id: id.into(),
        name: id.into(),
        version: Some(version.into()),
        thumbnail_url: None,
        nsfw: false,
        hero: None,
        category: None,
    };
    install::install_files(
        &files,
        meta,
        &e.paths,
        || false,
        shiv_core::install::InstallGuard::acquire(&e.paths.addons)
            .unwrap()
            .permit(),
    )
    .unwrap();
    found(e, id)
}

fn found(e: &Env, id: &str) -> sidecar::InstalledMod {
    sidecar::group(&e.paths.addons)
        .into_iter()
        .find(|m| m.sidecar.as_ref().is_some_and(|s| s.mod_id == id))
        .expect("the installed mod")
}

fn current(revisions: &[&str]) -> Published {
    Published::Current(
        revisions
            .iter()
            .map(|r| (*r).to_owned())
            .collect::<BTreeSet<_>>(),
    )
}

#[test]
fn an_install_stamps_every_vpk_it_places_with_its_chunks() {
    let e = env();
    let m = install(&e, "two", "r1", &["a", "b"], true);
    let s = m.sidecar.as_ref().unwrap();
    let stamps = s.stamps.as_ref().expect("the install recorded stamps");
    assert_eq!(stamps.len(), s.vpks.len());
    for (name, stamp) in s.vpks.iter().zip(stamps) {
        let v = m
            .vpks
            .iter()
            .find(|v| v.file.file_name().unwrap().to_str() == Some(name.as_str()))
            .unwrap();
        assert!(!v.chunks.is_empty(), "the chunk belongs to the dir VPK");
        let on_disk: u64 = std::iter::once(&v.file)
            .chain(&v.chunks)
            .map(|f| fs::metadata(f).unwrap().len())
            .sum();
        assert_eq!(stamp.size, on_disk, "dir VPK and chunks together");
        assert_eq!(*stamp, sidecar::Stamp::of_vpk(v).unwrap());
    }
}

#[test]
fn untouched_files_are_pristine_or_outdated_by_the_published_revision() {
    let e = env();
    let m = install(&e, "m", "r1", &["a"], false);
    assert_eq!(drift::drift(&m, &current(&["r1"])), Drift::Pristine);
    assert_eq!(drift::drift(&m, &current(&["r0", "r1"])), Drift::Pristine);
    assert_eq!(drift::drift(&m, &current(&["r2"])), Drift::UpdateAvailable);
    assert_eq!(drift::drift(&m, &Published::Unknown), Drift::Pristine);
    assert_eq!(drift::drift(&m, &Published::Gone), Drift::Retired);
}

#[test]
fn renames_keep_a_mod_pristine() {
    let e = env();
    let m = install(&e, "m", "r1", &["a"], true);
    for v in &m.vpks {
        addons::set_enabled(v, false).unwrap();
    }
    let disabled = found(&e, "m");
    assert!(disabled.vpks.iter().all(|v| !v.enabled));
    assert_eq!(drift::drift(&disabled, &current(&["r1"])), Drift::Pristine);
    let mods = sidecar::group(&e.paths.addons);
    let index = mods
        .iter()
        .position(|m| m.sidecar.as_ref().is_some_and(|s| s.mod_id == "m"))
        .unwrap();
    sidecar::set_pak(
        &e.paths.addons,
        &mods,
        index,
        7,
        shiv_core::install::InstallGuard::acquire(&e.paths.addons)
            .unwrap()
            .permit(),
    )
    .unwrap();
    let moved = found(&e, "m");
    assert_eq!(moved.load_position(), 7);
    assert_eq!(drift::drift(&moved, &current(&["r1"])), Drift::Pristine);
}

#[test]
fn a_replaced_patched_or_missing_file_is_modified_whatever_the_source_says() {
    let replaced = {
        let e = env();
        let m = install(&e, "m", "r1", &["a"], false);
        vpk(
            &m.vpks[0].file,
            &[("panorama/other.txt", "someone else's build")],
        );
        (e, found_of(&m))
    };
    let (e, id) = replaced;
    let m = found(&e, &id);
    assert_eq!(drift::drift(&m, &current(&["r1"])), Drift::Modified);
    assert_eq!(drift::drift(&m, &current(&["r2"])), Drift::Modified);
    assert_eq!(drift::drift(&m, &Published::Unknown), Drift::Modified);

    let e = env();
    let m = install(&e, "m", "r1", &["a"], false);
    let file = &m.vpks[0].file;
    let same_size = fs::read(file).unwrap();
    fs::write(file, &same_size).unwrap();
    fs::File::options()
        .write(true)
        .open(file)
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000))
        .unwrap();
    assert_eq!(
        drift::drift(&found(&e, "m"), &current(&["r1"])),
        Drift::Modified,
        "same bytes count, rewritten: the time tells"
    );

    let e = env();
    let m = install(&e, "m", "r1", &["a"], true);
    fs::remove_file(&m.vpks[0].chunks[0]).unwrap();
    assert_eq!(
        drift::drift(&found(&e, "m"), &current(&["r1"])),
        Drift::Modified,
        "a lost chunk is a partial mod"
    );
}

fn found_of(m: &sidecar::InstalledMod) -> String {
    m.sidecar.as_ref().unwrap().mod_id.clone()
}

#[test]
fn mods_without_stamps_are_untracked_and_old_sidecars_still_read() {
    let e = env();
    vpk(
        &e.paths.addons.join("pak05_dir.vpk"),
        &[("panorama/hand.txt", "hand-added")],
    );
    let mods = sidecar::group(&e.paths.addons);
    let hand = mods.iter().find(|m| m.sidecar.is_none()).unwrap();
    assert_eq!(drift::drift(hand, &Published::Unknown), Drift::Untracked);
    assert_eq!(drift::drift(hand, &Published::Gone), Drift::Retired);
    let adopted = sidecar::adopt(&e.paths.addons, hand, "Hand", None).unwrap();
    assert_eq!(
        adopted.stamps, None,
        "Shiv did not place an adopted mod's files"
    );
    let raw = fs::read_to_string(
        e.paths
            .addons
            .join(format!(".shiv/{}.json", adopted.mod_id)),
    )
    .unwrap();
    assert!(!raw.contains("stamps"), "None is not written: {raw}");
    let back = found(&e, &adopted.mod_id);
    assert_eq!(drift::drift(&back, &current(&["r1"])), Drift::Untracked);

    let old = r#"{"mod_id":"old","name":"Old","version":"r1","thumbnail_url":null,"vpks":["pak06_dir.vpk"]}"#;
    vpk(
        &e.paths.addons.join("pak06_dir.vpk"),
        &[("panorama/old.txt", "old")],
    );
    fs::write(e.paths.addons.join(".shiv/old.json"), old).unwrap();
    let old_mod = found(&e, "old");
    assert_eq!(old_mod.sidecar.as_ref().unwrap().stamps, None);
    assert_eq!(drift::drift(&old_mod, &current(&["r1"])), Drift::Untracked);
}

#[test]
fn complock_paks_publish_the_crcs_of_their_parts() {
    let option = complock::options()
        .iter()
        .find(|o| o.pak.is_some())
        .expect("an option with a pak");
    let pak = option.pak.as_ref().unwrap();
    let revision = complock::revision(pak);
    assert_eq!(revision.matches("crc32:").count(), pak.parts.len());
    match complock::published(&format!("{}{}", complock::MOD_PREFIX, option.id)) {
        Published::Current(revisions) => assert!(revisions.contains(&revision)),
        other => panic!("{other:?}"),
    }
    let hud = complock::hud().expect("the HUD base");
    match complock::published(&format!("{}{}", complock::MOD_PREFIX, complock::HUD_ID)) {
        Published::Current(revisions) => {
            for i in 0..hud.choices.len() {
                let pak = hud.pak_of_choice(i).expect("a pak per choice");
                assert!(revisions.contains(&complock::revision(pak)));
            }
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        complock::published(&format!("{}retired-option", complock::MOD_PREFIX)),
        Published::Gone
    );
    assert_eq!(complock::published("mod_123"), Published::Unknown);
}

#[test]
fn a_complock_pak_is_known_by_its_addoninfo_and_retired_by_its_title() {
    let e = env();
    let addoninfo = |title: &str, author: &str| {
        format!(
            "\"AddonInfo\"\n{{\n    addonversion \"1.0\"\n    addontitle \"{title} - {author}\"\n    addonauthor \"{author}\"\n    buildDate \"2026-11-02T02:24:05.603Z\"\n}}\n"
        )
    };
    let retired = e.work.join("retired.vpk");
    vpk(
        &retired,
        &[(
            "addoninfo.txt",
            &addoninfo("Thicker outlines (Vulkan)", "HoppCX"),
        )],
    );
    let id = complock::identify(&retired).expect("a CompLock pak");
    assert_eq!(id.title, "Thicker outlines (Vulkan)");
    assert_eq!(id.build_date.as_deref(), Some("2026-11-02T02:24:05.603Z"));
    assert_eq!(complock::published_for_title(&id.title), Published::Gone);

    let still_there = complock::options()
        .iter()
        .find_map(|o| o.pak.as_ref())
        .expect("a pak")
        .title
        .clone();
    let older = e.work.join("older.vpk");
    vpk(
        &older,
        &[(
            "addoninfo.txt",
            &addoninfo(
                &format!(
                    "{} (an older variant)",
                    still_there.split('(').next().unwrap().trim()
                ),
                "HoppCX, jcStani",
            ),
        )],
    );
    let id = complock::identify(&older).expect("a CompLock pak");
    assert_eq!(complock::published_for_title(&id.title), Published::Unknown);

    let foreign = e.work.join("foreign.vpk");
    vpk(
        &foreign,
        &[("addoninfo.txt", &addoninfo("Better Builds", "luxus"))],
    );
    assert_eq!(complock::identify(&foreign), None);
    let bare = e.work.join("bare.vpk");
    vpk(&bare, &[("panorama/x.txt", "x")]);
    assert_eq!(complock::identify(&bare), None);
}

/// The proof run over a copy of a real addons folder (never the game's own):
/// every dir VPK in `SHIV_ADDONS_COPY` with its CompLock identity and what
/// the site publishes for its title.
#[test]
#[ignore = "needs a copy of a real addons folder in SHIV_ADDONS_COPY"]
fn real_paks_identify_against_the_pack_data() {
    let dir = PathBuf::from(std::env::var("SHIV_ADDONS_COPY").expect("SHIV_ADDONS_COPY"));
    let mut names: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().ends_with("_dir.vpk"))
        .collect();
    names.sort();
    for vpk in names {
        let name = vpk.file_name().unwrap().to_string_lossy().into_owned();
        match complock::identify(&vpk) {
            Some(id) => println!(
                "{name}: CompLock \"{}\" built {:?} -> {:?}",
                id.title,
                id.build_date,
                complock::published_for_title(&id.title)
            ),
            None => println!("{name}: not CompLock"),
        }
    }
}
