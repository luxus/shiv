use std::fs;

use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use shiv_core::addons::PAK_CAPACITY;
use shiv_core::sidecar::{self, InstalledMod, Move, Sidecar};
use tempfile::TempDir;

fn sidecar_claiming(id: &str, vpk: &str) -> Sidecar {
    Sidecar {
        mod_id: id.into(),
        name: format!("Mod {id}"),
        version: Some("1.0".into()),
        version_label: None,
        thumbnail_url: Some(format!("https://img.test/{id}.png")),
        vpks: vec![vpk.into()],
        nsfw: Some(false),
        hero: Some("Haze".into()),
        category: Some("Skins".into()),
        notes: None,
        source: None,
        stamps: None,
        unverified: None,
    }
}

fn addons_dir_with(movable_mods: usize, total_mods: usize) -> (TempDir, Vec<InstalledMod>) {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..total_mods {
        let vpk = if i < movable_mods {
            format!("pak{:02}_dir.vpk", i + 1)
        } else {
            format!("hand-added-{i:04}.vpk")
        };
        fs::write(dir.path().join(&vpk), vpk.as_bytes()).unwrap();
        sidecar::write(dir.path(), &sidecar_claiming(&format!("m{i:04}"), &vpk)).unwrap();
    }
    let mods = sidecar::group(dir.path());
    (dir, mods)
}

fn reorder_benches(c: &mut Criterion) {
    let mut group = c.benchmark_group("reorder");
    for n in [25usize, 50, PAK_CAPACITY as usize] {
        let (dir, _) = addons_dir_with(n, n);
        group.bench_with_input(BenchmarkId::new("move_mod", n), &n, |b, _| {
            b.iter_batched(
                || {
                    let shifted = sidecar::group(dir.path());
                    sidecar::move_mod(
                        dir.path(),
                        &shifted,
                        0,
                        Move::To(shifted.len() - 1),
                        shiv_core::install::InstallGuard::acquire(dir.path())
                            .unwrap()
                            .permit(),
                    )
                    .unwrap();
                    sidecar::group(dir.path())
                },
                |mods| {
                    sidecar::move_mod(
                        dir.path(),
                        &mods,
                        mods.len() - 1,
                        Move::Top,
                        shiv_core::install::InstallGuard::acquire(dir.path())
                            .unwrap()
                            .permit(),
                    )
                    .unwrap();
                },
                BatchSize::PerIteration,
            );
        });
    }
    for n in [25usize, 50, PAK_CAPACITY as usize] {
        let (dir, _) = addons_dir_with(n, n);
        group.bench_with_input(BenchmarkId::new("set_pak", n), &n, |b, _| {
            b.iter_batched(
                || {
                    let shifted = sidecar::group(dir.path());
                    sidecar::move_mod(
                        dir.path(),
                        &shifted,
                        0,
                        Move::To(shifted.len() - 1),
                        shiv_core::install::InstallGuard::acquire(dir.path())
                            .unwrap()
                            .permit(),
                    )
                    .unwrap();
                    sidecar::group(dir.path())
                },
                |mods| {
                    sidecar::set_pak(
                        dir.path(),
                        &mods,
                        mods.len() - 1,
                        1,
                        shiv_core::install::InstallGuard::acquire(dir.path())
                            .unwrap()
                            .permit(),
                    )
                    .unwrap();
                },
                BatchSize::PerIteration,
            );
        });
    }
    for n in [100usize, 500, 1000] {
        let (dir, mods) = addons_dir_with(PAK_CAPACITY as usize, n);
        assert_eq!(mods.len(), n);
        group.bench_with_input(BenchmarkId::new("group", n), &n, |b, _| {
            b.iter(|| sidecar::group(dir.path()));
        });
    }
    group.finish();
}

criterion_group!(benches, reorder_benches);
criterion_main!(benches);
