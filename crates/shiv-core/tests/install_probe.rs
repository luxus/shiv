//! The timed-install probe (docs/perf-results.md, issue #10): a local
//! ~100 MiB archive of real VPKs, installed end to end and timed against
//! a raw read of the same bytes.
//!
//! Not part of the default suite; run it on the machine being recorded,
//! in the profile that ships:
//!
//! ```sh
//! cargo test --release -p shiv-core --test install_probe -- --ignored --nocapture
//! ```
//!
//! What it measures is the install path only — format detection, staged
//! extraction, pak placement, sidecar — never the network: the archive is
//! local, the state the real flow reaches once a download finishes. The
//! VPK contents are random, so the zip's deflate stream degenerates to
//! stored blocks and the archive is about the size it extracts: the raw
//! read and the extraction move the same bytes, and their throughput
//! ratio is the proof plan's install gate (extraction above 60 % of
//! disk-read speed). Both sides run with a warm page cache, as after a
//! download. The running-game check is injected as `false`: a
//! process-table walk is not archive work. Wall-clock asserts are absent
//! on purpose — the numbers are recorded in docs/perf-results.md, where
//! the machine and date carry them; the asserts here keep the fixture and
//! the install path honest instead: the archive stays incompressible,
//! every archive byte is written to disk exactly once, and the mod lands
//! whole.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use shiv_core::game::GamePaths;
use shiv_core::install;
use tempfile::{TempDir, tempdir};
use zip::write::SimpleFileOptions;

const DIR_VPK_BYTES: usize = 64 * 1024;
const CHUNK_VPKS: usize = 20;
const CHUNK_VPK_BYTES: usize = 5 * 1024 * 1024;
const RUNS: usize = 5;

struct SplitMix64(u64);

impl SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

/// `len` pseudorandom bytes: VPK contents that deflate cannot shrink, so
/// the archive stays the size it extracts and the read baseline moves the
/// same bytes the install writes.
fn incompressible_bytes(seed: u64, len: usize) -> Vec<u8> {
    let mut rng = SplitMix64(seed);
    let mut out = Vec::with_capacity(len);
    while out.len() + 8 <= len {
        out.extend_from_slice(&rng.next_u64().to_le_bytes());
    }
    let tail = rng.next_u64().to_le_bytes();
    out.extend_from_slice(&tail[..len - out.len()]);
    out
}

/// A real VPK at `dest` holding one inner asset of `bytes` random bytes.
fn vpk_of_random_bytes(dest: &Path, bytes: usize, seed: u64) {
    let src = tempdir().unwrap();
    let asset = src.path().join("materials/models/hero/asset.vtex_c");
    fs::create_dir_all(asset.parent().unwrap()).unwrap();
    fs::write(&asset, incompressible_bytes(seed, bytes)).unwrap();
    vpkmanager::pack_directory(src.path(), dest).unwrap();
}

/// The bytes the fixture holds and the archive that carries them.
struct Fixture {
    vpk_bytes: u64,
    archive: PathBuf,
    archive_bytes: u64,
}

/// `probe.zip` holding one mod shaped like a large real one — a small
/// `pak01_dir.vpk` plus its `pakNN_<digits>.vpk` chunk VPKs, about
/// 100 MiB of VPK bytes in total.
fn build_fixture(dir: &Path) -> Fixture {
    let started = Instant::now();
    let entries: Vec<(String, usize)> =
        std::iter::once(("pak01_dir.vpk".to_owned(), DIR_VPK_BYTES))
            .chain((0..CHUNK_VPKS).map(|i| (format!("pak01_{i:03}.vpk"), CHUNK_VPK_BYTES)))
            .collect();

    let archive = dir.join("probe.zip");
    let mut zip = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
    let mut vpk_bytes = 0u64;
    for (i, (name, bytes)) in entries.iter().enumerate() {
        let staged = dir.join(format!("staged-{i}.vpk"));
        vpk_of_random_bytes(&staged, *bytes, 0x5eed_0000 + i as u64);
        vpk_bytes += fs::metadata(&staged).unwrap().len();
        zip.start_file(name.as_str(), SimpleFileOptions::default())
            .unwrap();
        let data = fs::read(&staged).unwrap();
        zip.write_all(&data).unwrap();
        fs::remove_file(&staged).unwrap();
    }
    zip.finish().unwrap();
    let archive_bytes = fs::metadata(&archive).unwrap().len();
    println!(
        "fixture: {vpk_bytes} vpk bytes, archive {archive_bytes} bytes, built in {:.2?}",
        started.elapsed()
    );
    Fixture {
        vpk_bytes,
        archive,
        archive_bytes,
    }
}

/// A throwaway `Deadlock` install with an empty `addons` folder.
fn fresh_addons() -> (TempDir, GamePaths) {
    let root = tempdir().unwrap();
    let paths = GamePaths::from_root(root.path().join("Deadlock"));
    fs::create_dir_all(&paths.addons).unwrap();
    (root, paths)
}

/// One full local install, returning its wall clock. Asserts the
/// invariants the number stands on: the mod lands whole, and every
/// archive byte was written to disk exactly once.
fn timed_install(fixture: &Fixture) -> Duration {
    let (_root, paths) = fresh_addons();
    install::reset_disk_bytes_written();
    let started = Instant::now();
    let sidecar =
        install::install_local_with(&fixture.archive, "probe mod", &paths, || false).unwrap();
    let elapsed = started.elapsed();

    assert_eq!(sidecar.vpks.len(), 1, "one dir vpk in the sidecar");
    let placed = fs::read_dir(&paths.addons)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name() != ".shiv")
        .count();
    assert_eq!(
        placed,
        CHUNK_VPKS + 1,
        "the dir vpk and every chunk landed in addons"
    );
    assert_eq!(
        install::disk_bytes_written(),
        fixture.vpk_bytes,
        "the install wrote each archive byte exactly once"
    );
    elapsed
}

fn median_of(mut runs: Vec<Duration>) -> Duration {
    runs.sort_unstable();
    runs[runs.len() / 2]
}

fn mb_per_s(bytes: u64, elapsed: Duration) -> f64 {
    bytes as f64 / elapsed.as_secs_f64() / 1e6
}

/// The runs as whole milliseconds, the row `docs/perf-results.md` records.
fn milliseconds(runs: &[Duration]) -> String {
    runs.iter()
        .map(|d| format!("{} ms", d.as_millis()))
        .collect::<Vec<_>>()
        .join(", ")
}

#[test]
#[ignore = "a timing probe: run with --release --nocapture, record in docs/perf-results.md"]
fn extraction_throughput_against_raw_read() {
    let fixture_dir = tempdir().unwrap();
    let fixture = build_fixture(fixture_dir.path());

    assert!(
        fixture.archive_bytes * 10 >= fixture.vpk_bytes * 9
            && fixture.archive_bytes * 10 <= fixture.vpk_bytes * 11,
        "the fixture must stay incompressible: archive {} bytes against {} vpk bytes",
        fixture.archive_bytes,
        fixture.vpk_bytes
    );

    assert_eq!(
        fs::read(&fixture.archive).unwrap().len() as u64,
        fixture.archive_bytes
    );
    println!(
        "warmup: one read and one install discarded ({})",
        milliseconds(&[timed_install(&fixture)])
    );

    let mut reads = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let started = Instant::now();
        let bytes = fs::read(&fixture.archive).unwrap();
        reads.push(started.elapsed());
        assert_eq!(bytes.len() as u64, fixture.archive_bytes);
    }
    let mut installs = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        installs.push(timed_install(&fixture));
    }

    let read_median = median_of(reads.clone());
    let install_median = median_of(installs.clone());
    println!(
        "raw read, {RUNS} runs: {} → median {} → {:.0} MB/s",
        milliseconds(&reads),
        read_median.as_millis(),
        mb_per_s(fixture.archive_bytes, read_median),
    );
    println!(
        "install, {RUNS} runs: {} → median {} → {:.0} MB/s ({} bytes extracted)",
        milliseconds(&installs),
        install_median.as_millis(),
        mb_per_s(fixture.vpk_bytes, install_median),
        fixture.vpk_bytes
    );
    let ratio =
        mb_per_s(fixture.vpk_bytes, install_median) / mb_per_s(fixture.archive_bytes, read_median);
    println!(
        "extraction throughput is {:.1} % of the raw-read throughput (gate: above 60 %)",
        ratio * 100.0
    );
}
