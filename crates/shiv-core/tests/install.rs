use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use shiv_core::game::GamePaths;
use shiv_core::install::{self, InstallError, Limits, ModMeta};
use shiv_core::reset;
use shiv_core::sidecar::{self, Sidecar};
use tempfile::{TempDir, tempdir};
use zip::write::SimpleFileOptions;

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
    meta_version(id, "1.0")
}

fn meta_version(id: &str, version: &str) -> ModMeta {
    ModMeta {
        mod_id: id.into(),
        name: format!("Mod {id}"),
        version: Some(version.into()),
        thumbnail_url: None,
        nsfw: false,
        hero: None,
        category: None,
    }
}

fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
    let mut z = zip::ZipWriter::new(fs::File::create(path).unwrap());
    for (name, data) in entries {
        z.start_file(*name, SimpleFileOptions::default()).unwrap();
        z.write_all(data).unwrap();
    }
    z.finish().unwrap();
}

fn zip_in(e: &Env, entries: &[(&str, &[u8])]) -> PathBuf {
    let p = e.work.join("mod.zip");
    write_zip(&p, entries);
    p
}

/// Every file and directory under `root` with contents (dirs map to None).
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                out.insert(p.clone(), None);
                walk(&p, out);
            } else {
                out.insert(p.clone(), Some(fs::read(&p).unwrap()));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, &mut out);
    out.retain(|path, _| !is_install_lock(path));
    let empty_shiv: Vec<PathBuf> = out
        .iter()
        .filter(|(path, bytes)| bytes.is_none() && path.file_name().is_some_and(|n| n == ".shiv"))
        .map(|(path, _)| path.clone())
        .filter(|dir| !out.keys().any(|path| path.starts_with(dir) && path != dir))
        .collect();
    for dir in empty_shiv {
        out.remove(&dir);
    }
    out
}

fn is_install_lock(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == "lock")
        && path
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == ".shiv")
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n != ".shiv")
        .collect();
    v.sort();
    v
}

fn not_running() -> bool {
    false
}

fn run(e: &Env, archive: &Path, id: &str) -> Result<Sidecar, InstallError> {
    install::install_with(
        archive,
        meta(id),
        &e.paths,
        not_running,
        shiv_core::install::InstallGuard::acquire(&e.paths.addons)
            .unwrap()
            .permit(),
    )
}

#[test]
fn installs_zip_with_one_vpk() {
    let e = env();
    let a = zip_in(&e, &[("readme.txt", b"hi"), ("skin.vpk", b"VPKDATA")]);
    let sc = run(&e, &a, "m1").unwrap();
    assert_eq!(sc.vpks, vec!["pak01_dir.vpk"]);
    assert_eq!(sc.mod_id, "m1");
    assert_eq!(sc.name, "Mod m1");
    assert_eq!(
        fs::read(e.paths.addons.join("pak01_dir.vpk")).unwrap(),
        b"VPKDATA"
    );
    assert_eq!(names(&e.paths.addons), vec!["pak01_dir.vpk"]);
    assert_eq!(sidecar::read(&e.paths.addons, "m1").unwrap(), Some(sc));
    assert!(!e.paths.addons.join(".shiv/tmp").exists());
}

#[test]
fn install_records_the_nsfw_flag() {
    let e = env();
    let a = zip_in(&e, &[("skin.vpk", b"VPKDATA")]);
    let mut m = meta("spicy");
    m.nsfw = true;
    let sc = install::install_with(
        &a,
        m,
        &e.paths,
        not_running,
        shiv_core::install::InstallGuard::acquire(&e.paths.addons)
            .unwrap()
            .permit(),
    )
    .unwrap();
    assert_eq!(sc.nsfw, Some(true));
    let read = sidecar::read(&e.paths.addons, "spicy").unwrap().unwrap();
    assert_eq!(read.nsfw, Some(true));
    // Not NSFW is recorded too, distinct from unknown.
    let b = zip_in(&e, &[("other.vpk", b"VPKDATA")]);
    let plain = install::install_with(
        &b,
        meta("plain"),
        &e.paths,
        not_running,
        shiv_core::install::InstallGuard::acquire(&e.paths.addons)
            .unwrap()
            .permit(),
    )
    .unwrap();
    assert_eq!(plain.nsfw, Some(false));
}

#[test]
fn installs_multi_vpk_with_lowest_free_paks() {
    let e = env();
    fs::write(e.paths.addons.join("pak01_dir.vpk"), b"other").unwrap();
    let a = zip_in(
        &e,
        &[
            ("sub/pak07_dir.vpk", b"B"),
            ("sub/pak03_dir.vpk", b"A"),
            ("x/more.vpk", b"C"),
        ],
    );
    let sc = run(&e, &a, "m1").unwrap();
    assert_eq!(
        sc.vpks,
        vec!["pak02_dir.vpk", "pak03_dir.vpk", "pak04_dir.vpk"]
    );
    assert_eq!(
        fs::read(e.paths.addons.join("pak02_dir.vpk")).unwrap(),
        b"A"
    );
    assert_eq!(
        fs::read(e.paths.addons.join("pak03_dir.vpk")).unwrap(),
        b"B"
    );
    assert_eq!(
        fs::read(e.paths.addons.join("pak04_dir.vpk")).unwrap(),
        b"C"
    );
    assert_eq!(
        fs::read(e.paths.addons.join("pak01_dir.vpk")).unwrap(),
        b"other"
    );
}

#[test]
fn moves_chunk_files_with_their_dir_vpk() {
    let e = env();
    fs::write(e.paths.addons.join("pak01_dir.vpk"), b"other").unwrap();
    let a = zip_in(
        &e,
        &[
            ("pak05_dir.vpk", b"D"),
            ("pak05_000.vpk", b"C0"),
            ("pak05_001.vpk", b"C1"),
        ],
    );
    let sc = run(&e, &a, "m1").unwrap();
    assert_eq!(sc.vpks, vec!["pak02_dir.vpk"]);
    assert_eq!(
        names(&e.paths.addons),
        vec![
            "pak01_dir.vpk",
            "pak02_000.vpk",
            "pak02_001.vpk",
            "pak02_dir.vpk"
        ]
    );
    assert_eq!(
        fs::read(e.paths.addons.join("pak02_001.vpk")).unwrap(),
        b"C1"
    );
}

#[test]
fn renames_non_pak_vpk_to_pak_dir() {
    let e = env();
    let a = zip_in(&e, &[("Cool Skin v2.VPK", b"S")]);
    let sc = run(&e, &a, "m1").unwrap();
    assert_eq!(sc.vpks, vec!["pak01_dir.vpk"]);
    assert_eq!(names(&e.paths.addons), vec!["pak01_dir.vpk"]);
}

#[test]
fn rejects_path_traversal_entry() {
    for bad in [
        "../evil.vpk",
        "a/../../evil.vpk",
        "/tmp/shiv-evil-test.vpk",
        "C:\\evil.vpk",
    ] {
        let e = env();
        let before = snapshot(e._root.path());
        let a = zip_in(&e, &[("ok.vpk", b"fine"), (bad, b"evil")]);
        let before_with_zip = {
            let mut b = before.clone();
            b.extend(snapshot(&e.work));
            b
        };
        let err = run(&e, &a, "m1").unwrap_err();
        assert!(matches!(err, InstallError::UnsafePath(_)), "{bad}: {err:?}");
        assert_eq!(snapshot(e._root.path()), before_with_zip, "{bad}");
        assert!(!Path::new("/tmp/shiv-evil-test.vpk").exists());
    }
}

#[test]
fn rejects_symlink_entry() {
    let e = env();
    let p = e.work.join("link.zip");
    let mut z = zip::ZipWriter::new(fs::File::create(&p).unwrap());
    z.start_file("skin.vpk", SimpleFileOptions::default())
        .unwrap();
    z.write_all(b"x").unwrap();
    z.add_symlink("evil.vpk", "/etc/passwd", SimpleFileOptions::default())
        .unwrap();
    z.finish().unwrap();
    let err = run(&e, &p, "m1").unwrap_err();
    assert!(matches!(err, InstallError::UnsafePath(_)), "{err:?}");
    assert!(names(&e.paths.addons).is_empty());
    assert_only_the_lock(&e.paths.addons);
}

#[test]
fn archive_without_vpk_leaves_addons_unchanged() {
    let e = env();
    fs::write(e.paths.addons.join("pak01_dir.vpk"), b"keep").unwrap();
    let a = zip_in(&e, &[("readme.txt", b"hi"), ("img/a.png", b"png")]);
    let before = snapshot(&e.paths.addons);
    let err = run(&e, &a, "m1").unwrap_err();
    assert!(matches!(err, InstallError::NoVpk), "{err:?}");
    assert_eq!(snapshot(&e.paths.addons), before);
}

#[test]
fn refuses_while_game_running() {
    let e = env();
    let a = zip_in(&e, &[("skin.vpk", b"x")]);
    let before = snapshot(&e.paths.addons);
    let err = install::install_with(
        &a,
        meta("m1"),
        &e.paths,
        || true,
        shiv_core::install::InstallGuard::acquire(&e.paths.addons)
            .unwrap()
            .permit(),
    )
    .unwrap_err();
    assert!(matches!(err, InstallError::GameRunning), "{err:?}");
    assert_eq!(snapshot(&e.paths.addons), before);
    assert_only_the_lock(&e.paths.addons);
}

fn assert_only_the_lock(addons: &Path) {
    let shiv = addons.join(".shiv");
    if !shiv.exists() {
        return;
    }
    let names: Vec<_> = fs::read_dir(&shiv)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert!(names.iter().all(|name| name == "lock"), "{names:?}");
}

#[test]
fn addons_full_leaves_addons_unchanged() {
    let e = env();
    for n in 1..=99 {
        fs::write(e.paths.addons.join(format!("pak{n:02}_dir.vpk")), b"x").unwrap();
    }
    let a = zip_in(&e, &[("skin.vpk", b"x")]);
    let before = snapshot(&e.paths.addons);
    let err = run(&e, &a, "m1").unwrap_err();
    assert!(matches!(err, InstallError::AddonsFull), "{err:?}");
    assert_eq!(snapshot(&e.paths.addons), before);
}

#[test]
fn reinstall_replaces_old_files_and_keeps_old_on_failure() {
    let e = env();
    let a1 = zip_in(&e, &[("pak01_dir.vpk", b"v1"), ("pak01_000.vpk", b"c1")]);
    let first = run(&e, &a1, "m1").unwrap();
    assert_eq!(first.vpks, vec!["pak01_dir.vpk"]);

    // A failing reinstall leaves the old mod intact.
    let bad = e.work.join("bad.zip");
    write_zip(&bad, &[("readme.txt", b"no vpk")]);
    let before = snapshot(&e.paths.addons);
    assert!(run(&e, &bad, "m1").is_err());
    assert_eq!(snapshot(&e.paths.addons), before);
    let trav = e.work.join("trav.zip");
    write_zip(&trav, &[("a.vpk", b"n"), ("../x.vpk", b"n")]);
    assert!(run(&e, &trav, "m1").is_err());
    assert_eq!(snapshot(&e.paths.addons), before);

    // A successful reinstall replaces the files and the sidecar.
    let a2 = e.work.join("v2.zip");
    write_zip(&a2, &[("new.vpk", b"v2")]);
    let second = run(&e, &a2, "m1").unwrap();
    assert_eq!(second.vpks.len(), 1);
    let name = &second.vpks[0];
    assert_eq!(fs::read(e.paths.addons.join(name)).unwrap(), b"v2");
    assert_eq!(names(&e.paths.addons), vec![name.clone()]);
    assert_eq!(sidecar::read(&e.paths.addons, "m1").unwrap(), Some(second));
}

/// A catalog or GameBanana update is a second `install_with` of the same
/// mod. The previous pak bytes have to survive in the reset backup, on the
/// slot they already occupied, and Restore previous has to swap them back.
#[test]
fn a_catalog_update_keeps_the_previous_files_restorable() {
    let e = env();
    fs::write(e.paths.addons.join("pak50_dir.vpk"), b"neighbor").unwrap();
    let v1 = e.work.join("v1.zip");
    write_zip(
        &v1,
        &[
            ("pak01_dir.vpk", b"version-one"),
            ("pak01_000.vpk", b"chunk-one"),
        ],
    );
    let first = install::install_with(
        &v1,
        meta_version("m1", "v1"),
        &e.paths,
        not_running,
        install::InstallGuard::acquire(&e.paths.addons)
            .unwrap()
            .permit(),
    )
    .unwrap();
    let old_name = first.vpks[0].clone();
    let old_bytes = fs::read(e.paths.addons.join(&old_name)).unwrap();
    assert_eq!(old_bytes, b"version-one");

    let v2 = e.work.join("v2.zip");
    write_zip(&v2, &[("fresh.vpk", b"version-two")]);
    let second = install::install_with(
        &v2,
        meta_version("m1", "v2"),
        &e.paths,
        not_running,
        install::InstallGuard::acquire(&e.paths.addons)
            .unwrap()
            .permit(),
    )
    .unwrap();
    assert_ne!(
        second.vpks, first.vpks,
        "the new pak takes a free slot while the old one still occupies its own"
    );
    assert_eq!(
        fs::read(e.paths.addons.join(&second.vpks[0])).unwrap(),
        b"version-two"
    );
    assert_eq!(
        fs::read(e.paths.addons.join("pak50_dir.vpk")).unwrap(),
        b"neighbor"
    );
    assert!(
        !e.paths.addons.join(&old_name).exists(),
        "the replaced pak leaves the addons folder"
    );

    let backup = reset::latest_backup(&e.paths.addons, "m1").expect("previous files kept");
    assert_eq!(backup.sidecar.version.as_deref(), Some("v1"));
    assert_eq!(fs::read(backup.dir.join(&old_name)).unwrap(), old_bytes);
    assert!(
        backup
            .files
            .iter()
            .any(|name| fs::read(backup.dir.join(name)).unwrap() == b"chunk-one"),
        "the chunk is kept with the dir pak"
    );

    let restored = reset::restore_previous(
        &e.paths,
        "m1",
        not_running,
        install::InstallGuard::acquire(&e.paths.addons)
            .unwrap()
            .permit(),
    )
    .unwrap();
    assert_eq!(restored.version.as_deref(), Some("v1"));
    assert_eq!(fs::read(e.paths.addons.join(&old_name)).unwrap(), old_bytes);
    assert_eq!(
        fs::read(e.paths.addons.join("pak50_dir.vpk")).unwrap(),
        b"neighbor"
    );

    let again = reset::restore_previous(
        &e.paths,
        "m1",
        not_running,
        install::InstallGuard::acquire(&e.paths.addons)
            .unwrap()
            .permit(),
    )
    .unwrap();
    assert_eq!(again.version.as_deref(), Some("v2"));
    assert_eq!(
        fs::read(e.paths.addons.join(&second.vpks[0])).unwrap(),
        b"version-two"
    );
}

#[test]
fn uninstall_removes_vpks_chunks_and_sidecar() {
    let e = env();
    let a = zip_in(
        &e,
        &[
            ("pak02_dir.vpk", b"D"),
            ("pak02_000.vpk", b"C"),
            ("b.vpk", b"E"),
        ],
    );
    run(&e, &a, "m1").unwrap();
    fs::write(e.paths.addons.join("pak09_dir.vpk"), b"other").unwrap();
    // Disable one of the mod's dir files; uninstall must still find it.
    fs::rename(
        e.paths.addons.join("pak02_dir.vpk"),
        e.paths.addons.join("pak02_dir.vpk.disabled"),
    )
    .unwrap();
    let mods = sidecar::group(&e.paths.addons);
    let m = mods
        .iter()
        .find(|m| m.sidecar.as_ref().is_some_and(|s| s.mod_id == "m1"))
        .unwrap();
    install::uninstall_with(
        m,
        || false,
        shiv_core::install::InstallGuard::acquire(&e.paths.addons)
            .unwrap()
            .permit(),
    )
    .unwrap();
    assert_eq!(names(&e.paths.addons), vec!["pak09_dir.vpk"]);
    assert_eq!(sidecar::read(&e.paths.addons, "m1").unwrap(), None);

    // Unknown mod: only its own VPK goes.
    let mods = sidecar::group(&e.paths.addons);
    assert_eq!(mods.len(), 1);
    assert!(mods[0].sidecar.is_none());
    install::uninstall_with(
        &mods[0],
        || false,
        shiv_core::install::InstallGuard::acquire(&e.paths.addons)
            .unwrap()
            .permit(),
    )
    .unwrap();
    assert!(names(&e.paths.addons).is_empty());
}

#[test]
fn unsupported_magic_is_rejected() {
    let e = env();
    // A .zip extension must not matter; magic does.
    let p = e.work.join("fake.zip");
    fs::write(&p, b"this is not an archive at all").unwrap();
    let err = run(&e, &p, "m1").unwrap_err();
    assert!(matches!(err, InstallError::UnsupportedArchive), "{err:?}");
    assert!(names(&e.paths.addons).is_empty());
    // A real zip with a wrong extension is accepted.
    let z = e.work.join("real.bin");
    write_zip(&z, &[("a.vpk", b"x")]);
    assert!(run(&e, &z, "m2").is_ok());
}

fn write_7z(path: &Path, files: &[(&str, &[u8], u32)]) {
    let mut w = sevenz_rust2::ArchiveWriter::create(path).unwrap();
    for (name, data, attrs) in files {
        let mut entry = sevenz_rust2::ArchiveEntry::new_file(name);
        if *attrs != 0 {
            entry.has_windows_attributes = true;
            entry.windows_attributes = *attrs;
        }
        w.push_archive_entry(entry, Some(*data)).unwrap();
    }
    w.finish().unwrap();
}

#[test]
fn sevenz_extracts() {
    let e = env();
    let p = e.work.join("mod.7z");
    write_7z(
        &p,
        &[
            ("docs/readme.txt", b"hi", 0),
            ("m/pak04_dir.vpk", b"SEVEN", 0),
        ],
    );
    let sc = run(&e, &p, "m7").unwrap();
    assert_eq!(sc.vpks, vec!["pak01_dir.vpk"]);
    assert_eq!(
        fs::read(e.paths.addons.join("pak01_dir.vpk")).unwrap(),
        b"SEVEN"
    );

    // Traversal and symlinks are rejected before anything is written.
    let e = env();
    let before = snapshot(e._root.path());
    let p = e.work.join("bad.7z");
    write_7z(&p, &[("ok.vpk", b"x", 0), ("../evil.vpk", b"x", 0)]);
    let before = {
        let mut b = before;
        b.extend(snapshot(&e.work));
        b
    };
    assert!(matches!(
        run(&e, &p, "m7"),
        Err(InstallError::UnsafePath(_))
    ));
    assert_eq!(snapshot(e._root.path()), before);
    let p = e.work.join("link.7z");
    let unix_symlink = (0o120777u32 << 16) | 0x8000;
    write_7z(
        &p,
        &[("ok.vpk", b"x", 0), ("l.vpk", b"/etc/passwd", unix_symlink)],
    );
    assert!(matches!(
        run(&e, &p, "m8"),
        Err(InstallError::UnsafePath(_))
    ));
    assert!(names(&e.paths.addons).is_empty());
}

fn rar_crc16(bytes: &[u8]) -> u16 {
    (crc32fast::hash(bytes) & 0xFFFF) as u16
}

/// Hand-built RAR 4.x archive holding stored (method 0x30) files.
fn rar4_stored(files: &[(&str, &[u8])]) -> Vec<u8> {
    let entries: Vec<(&str, &[u8], u8, u32)> =
        files.iter().map(|(n, d)| (*n, *d, 3, 0o100644)).collect();
    rar4_entries(&entries)
}

/// `(name, data, host_os, attrs)` entries.
fn rar4_entries(files: &[(&str, &[u8], u8, u32)]) -> Vec<u8> {
    let mut out = b"Rar!\x1a\x07\x00".to_vec();
    // Main header.
    let mut main = vec![0x73, 0, 0, 13, 0, 0, 0, 0, 0, 0, 0];
    let crc = rar_crc16(&main);
    let mut hdr = crc.to_le_bytes().to_vec();
    hdr.append(&mut main);
    out.extend(hdr);
    for (name, data, host, attr) in files {
        let mut h = vec![0x74];
        h.extend(0x8000u16.to_le_bytes()); // flags: ADD_SIZE present
        h.extend((32 + name.len() as u16).to_le_bytes());
        h.extend((data.len() as u32).to_le_bytes()); // packed
        h.extend((data.len() as u32).to_le_bytes()); // unpacked
        h.push(*host);
        h.extend(crc32fast::hash(data).to_le_bytes());
        h.extend(0x5A21_0000u32.to_le_bytes()); // dos time
        h.push(20); // unpack version
        h.push(0x30); // stored
        h.extend((name.len() as u16).to_le_bytes());
        h.extend(attr.to_le_bytes());
        h.extend(name.as_bytes());
        out.extend(rar_crc16(&h).to_le_bytes());
        out.extend(h);
        out.extend(*data);
    }
    out.extend([0xC4, 0x3D, 0x7B, 0x00, 0x40, 0x07, 0x00]);
    out
}

#[test]
fn rar_extracts() {
    let e = env();
    let p = e.work.join("mod.rar");
    fs::write(
        &p,
        rar4_stored(&[("readme.txt", b"hi"), ("pak06_dir.vpk", b"RARDATA")]),
    )
    .unwrap();
    let sc = run(&e, &p, "mr").unwrap();
    assert_eq!(sc.vpks, vec!["pak01_dir.vpk"]);
    assert_eq!(
        fs::read(e.paths.addons.join("pak01_dir.vpk")).unwrap(),
        b"RARDATA"
    );

    let e = env();
    let p = e.work.join("bad.rar");
    fs::write(&p, rar4_stored(&[("ok.vpk", b"x"), ("../evil.vpk", b"x")])).unwrap();
    assert!(matches!(
        run(&e, &p, "mr"),
        Err(InstallError::UnsafePath(_))
    ));
    assert!(names(&e.paths.addons).is_empty());
}

#[test]
fn size_cap_is_enforced() {
    let e = env();
    let a = zip_in(&e, &[("skin.vpk", &[7u8; 4096])]);
    let before = snapshot(&e.paths.addons);
    let small = Limits {
        max_bytes: 1000,
        max_entries: 100,
    };
    let err = install::install_with_limits(
        &a,
        meta("m1"),
        &e.paths,
        not_running,
        small,
        shiv_core::install::InstallGuard::acquire(&e.paths.addons)
            .unwrap()
            .permit(),
    )
    .unwrap_err();
    assert!(matches!(err, InstallError::TooLarge), "{err:?}");
    assert_eq!(snapshot(&e.paths.addons), before);

    let many = zip_in(&e, &[("a.txt", b"1"), ("b.txt", b"2"), ("skin.vpk", b"3")]);
    let few = Limits {
        max_bytes: 1 << 20,
        max_entries: 2,
    };
    let err = install::install_with_limits(
        &many,
        meta("m1"),
        &e.paths,
        not_running,
        few,
        shiv_core::install::InstallGuard::acquire(&e.paths.addons)
            .unwrap()
            .permit(),
    )
    .unwrap_err();
    assert!(matches!(err, InstallError::TooLarge), "{err:?}");
    assert_eq!(snapshot(&e.paths.addons), before);
}

#[test]
fn multi_vpk_fills_gaps_in_pak_numbers() {
    let e = env();
    fs::write(e.paths.addons.join("pak02_dir.vpk"), b"o").unwrap();
    fs::write(e.paths.addons.join("pak04_dir.vpk.disabled"), b"o").unwrap();
    let a = zip_in(&e, &[("a.vpk", b"A"), ("b.vpk", b"B"), ("c.vpk", b"C")]);
    let sc = run(&e, &a, "m1").unwrap();
    assert_eq!(
        sc.vpks,
        vec!["pak01_dir.vpk", "pak03_dir.vpk", "pak05_dir.vpk"]
    );
}

#[test]
fn rar_rejects_symlink_and_reparse_attrs() {
    for (host, attr) in [(3u8, 0o120777u32), (2u8, 0x20 | 0x400)] {
        let e = env();
        let p = e.work.join("l.rar");
        fs::write(
            &p,
            rar4_entries(&[
                ("ok.vpk", b"x", 3, 0o100644),
                ("l.vpk", b"/etc/passwd", host, attr),
            ]),
        )
        .unwrap();
        let err = run(&e, &p, "m1").unwrap_err();
        assert!(
            matches!(err, InstallError::UnsafePath(_)),
            "{attr:o}: {err:?}"
        );
        assert!(names(&e.paths.addons).is_empty());
    }
}

#[test]
fn uninstall_refuses_while_game_running() {
    let e = env();
    let a = zip_in(&e, &[("skin.vpk", b"x")]);
    run(&e, &a, "m1").unwrap();
    let m = sidecar::group(&e.paths.addons).remove(0);
    let before = snapshot(&e.paths.addons);
    let err = install::uninstall_with(
        &m,
        || true,
        shiv_core::install::InstallGuard::acquire(&e.paths.addons)
            .unwrap()
            .permit(),
    )
    .unwrap_err();
    assert!(matches!(err, InstallError::GameRunning), "{err:?}");
    assert_eq!(snapshot(&e.paths.addons), before);
    install::uninstall_with(
        &m,
        not_running,
        shiv_core::install::InstallGuard::acquire(&e.paths.addons)
            .unwrap()
            .permit(),
    )
    .unwrap();
}

#[test]
fn uninstall_unknown_vpk_removes_its_chunks() {
    let e = env();
    let d = &e.paths.addons;
    fs::write(d.join("pak03_dir.vpk"), b"x").unwrap();
    fs::write(d.join("pak03_000.vpk"), b"x").unwrap();
    fs::write(d.join("pak03_001.vpk"), b"x").unwrap();
    fs::write(d.join("pak04_000.vpk"), b"x").unwrap();
    let m = sidecar::group(d)
        .into_iter()
        .find(|m| m.vpks[0].pak == Some(3))
        .unwrap();
    assert!(m.sidecar.is_none());
    install::uninstall_with(
        &m,
        not_running,
        shiv_core::install::InstallGuard::acquire(&e.paths.addons)
            .unwrap()
            .permit(),
    )
    .unwrap();
    assert_eq!(names(d), vec!["pak04_000.vpk"]);
}

#[test]
fn sidecar_with_traversal_vpk_name_deletes_nothing_outside_addons() {
    let e = env();
    let victim = e.paths.addons.parent().unwrap().join("victim.vpk");
    fs::write(&victim, b"precious").unwrap();
    let abs = e.work.join("abs_victim.vpk");
    fs::write(&abs, b"precious").unwrap();
    let evil = Sidecar {
        mod_id: "m1".into(),
        name: "x".into(),
        version: None,
        thumbnail_url: None,
        vpks: vec![
            "pak01_dir.vpk".into(),
            "../victim.vpk".into(),
            abs.to_string_lossy().into_owned(),
        ],
        nsfw: None,
        hero: None,
        category: None,
        notes: None,
        source: None,
        stamps: None,
    };
    fs::write(e.paths.addons.join("pak01_dir.vpk"), b"x").unwrap();
    sidecar::write(&e.paths.addons, &evil).unwrap();

    // Reinstall must not touch the victims.
    let a = zip_in(&e, &[("new.vpk", b"n")]);
    run(&e, &a, "m1").unwrap();
    assert!(victim.exists() && abs.exists());

    // Uninstall with a crafted InstalledMod must not either.
    let mut m = sidecar::group(&e.paths.addons).remove(0);
    m.sidecar = Some(evil);
    install::uninstall_with(
        &m,
        not_running,
        shiv_core::install::InstallGuard::acquire(&e.paths.addons)
            .unwrap()
            .permit(),
    )
    .unwrap();
    assert!(victim.exists() && abs.exists());
}

fn set_age(path: &Path, secs: u64) {
    let t = std::time::SystemTime::now() - std::time::Duration::from_secs(secs);
    fs::File::open(path).unwrap().set_modified(t).unwrap();
}

fn plant_staging(e: &Env, name: &str, age_secs: u64) -> PathBuf {
    let d = e.paths.addons.join(".shiv/tmp").join(name);
    fs::create_dir_all(&d).unwrap();
    fs::write(d.join("junk.vpk"), b"x").unwrap();
    set_age(&d, age_secs);
    d
}

#[test]
fn stale_staging_dirs_are_cleaned_up() {
    let e = env();
    let stale = plant_staging(&e, "99999-1-0", 2 * 3600);
    let a = zip_in(&e, &[("skin.vpk", b"x")]);
    run(&e, &a, "m1").unwrap();
    assert!(!stale.exists());
    assert!(!e.paths.addons.join(".shiv/tmp").exists());
}

#[test]
fn fresh_foreign_staging_is_left_alone() {
    let e = env();
    let fresh = plant_staging(&e, "99999-1-0", 10);
    let own_old = plant_staging(&e, &format!("{}-1-0", std::process::id()), 2 * 3600);
    let a = zip_in(&e, &[("skin.vpk", b"x")]);
    run(&e, &a, "m1").unwrap();
    assert!(fresh.exists());
    assert!(own_old.exists());
}

#[test]
fn install_writes_each_archive_byte_once() {
    let e = env();
    let skin = vec![7u8; 512 * 1024];
    let chunk = vec![11u8; 128 * 1024];
    let a = zip_in(
        &e,
        &[
            ("pak07_dir.vpk", &skin),
            ("pak07_000.vpk", &chunk),
            ("readme.txt", b"not a vpk, never extracted"),
        ],
    );
    install::reset_disk_bytes_written();
    let sc = run(&e, &a, "m1").unwrap();
    assert_eq!(sc.vpks, vec!["pak01_dir.vpk"]);
    let expected = (skin.len() + chunk.len()) as u64;
    assert_eq!(install::disk_bytes_written(), expected);
    let installed: u64 = ["pak01_dir.vpk", "pak01_000.vpk"]
        .iter()
        .map(|n| fs::metadata(e.paths.addons.join(n)).unwrap().len())
        .sum();
    assert_eq!(installed, expected);
}

fn install_module_source() -> String {
    fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/install.rs")).unwrap()
}

#[test]
fn install_writes_are_counted_and_placement_renames() {
    let source = install_module_source();
    let lines: Vec<&str> = source.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let writes = [
            "fs::copy(",
            "io::copy(",
            "extract_to(",
            "fs::write(",
            "write_all(",
        ];
        if !writes.into_iter().any(|w| line.contains(w)) {
            continue;
        }
        let following = &lines[i + 1..(i + 13).min(lines.len())];
        assert!(
            following.iter().any(|l| l.contains("tally_disk_write")),
            "src/install.rs:{} writes to disk outside tally_disk_write: {line}",
            i + 1
        );
    }
    let placement = lines
        .iter()
        .position(|l| l.starts_with("fn move_into_place"))
        .unwrap();
    let body: String = lines[placement..]
        .iter()
        .copied()
        .take_while(|l| *l != "}")
        .collect();
    assert!(body.contains("fs::rename"), "{body}");
    assert!(!body.contains("fs::copy"), "{body}");
}
