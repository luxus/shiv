use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::thread;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use shiv_api::Client;
use std::hint::black_box;
use tempfile::TempDir;
use tiny_http::{Header, Response, Server};

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn pick<'a>(&mut self, choices: &[&'a str]) -> &'a str {
        choices[self.below(choices.len() as u64) as usize]
    }
}

fn catalog_row(rng: &mut Lcg, index: usize) -> String {
    let adjectives = [
        "clean", "neon", "retro", "sleek", "cyber", "gold", "shadow", "holy",
    ];
    let nouns = [
        "haze",
        "seven",
        "bebop",
        "silhouette",
        "crosshair",
        "hud",
        "sounds",
        "icons",
    ];
    let heroes = [
        "Haze", "Seven", "Bebop", "Abrams", "Vyper", "Ivy", "Shiv", "Wraith",
    ];
    let categories = ["Skins", "Audio", "Mods", "GUI"];
    let authors = [
        "lilotty",
        "megamodder",
        "SkinLord",
        "vpk_wizard",
        "yardEnjoyer",
    ];
    let name = format!(
        "{} {} {}",
        rng.pick(&adjectives),
        rng.pick(&nouns),
        rng.pick(&nouns)
    );
    let remote = 100_000 + index;
    let likes = rng.below(50_000);
    let downloads = rng.below(5_000_000);
    let description = if rng.below(4) == 0 {
        "null".to_owned()
    } else {
        format!(
            "\"A {} {} rework with custom textures, sounds and icons.\"",
            rng.pick(&adjectives),
            rng.pick(&nouns)
        )
    };
    let tags: Vec<String> = (0..rng.below(4))
        .map(|_| format!("\"{}\"", rng.pick(&nouns)))
        .collect();
    let images: Vec<String> = (0..1 + rng.below(4))
        .map(|i| format!("\"https://img.test/{remote}/shot-{i}.webp\""))
        .collect();
    let stamp = |field: u64| {
        format!(
            "2026-11-{:02}T{:02}:{:02}:{:02}.000Z",
            1 + field % 20,
            field % 24,
            field % 60,
            (field * 7) % 60
        )
    };
    format!(
        r#"{{"id":"m{index:05}","remoteId":"{remote}","name":"{name}","description":{description},"remoteUrl":"https://api.test/mods/{remote}","category":"{category}","author":"{author}","likes":{likes},"downloadCount":{downloads},"downloadable":true,"tags":[{tags}],"images":[{images}],"thumbnailUrl":"https://img.test/{remote}/thumb.webp","hero":"{hero}","isAudio":{audio},"isMap":false,"isNSFW":{nsfw},"remoteAddedAt":"{added}","remoteUpdatedAt":null,"filesUpdatedAt":"{updated}","modAuthorId":"author-{author_slot}","metadata":{{"donationLinks":[{{"url":"https://ko-fi.com/{author}","platform":"Ko-fi"}}]}},"someFutureField":{{"nested":[1,2,3,null,true]}}}}"#,
        category = rng.pick(&categories),
        author = rng.pick(&authors),
        hero = rng.pick(&heroes),
        audio = rng.below(8) == 0,
        nsfw = rng.below(20) == 0,
        added = stamp(rng.below(1_000_000)),
        updated = stamp(rng.below(1_000_000)),
        author_slot = rng.below(900),
        tags = tags.join(","),
        images = images.join(","),
    )
}

fn catalog_server(rows: usize) -> (String, u64, TempDir) {
    let mut rng = Lcg(0x5eed_c47a);
    let body = format!(
        "[{}]",
        (0..rows)
            .map(|i| catalog_row(&mut rng, i))
            .collect::<Vec<_>>()
            .join(",")
    );
    let bytes = body.len() as u64;
    let dir = tempfile::tempdir().unwrap();
    let fixture = dir.path().join("catalog.json");
    File::create(&fixture)
        .unwrap()
        .write_all(body.as_bytes())
        .unwrap();
    let server = Server::http("127.0.0.1:0").unwrap();
    let url = format!("http://{}", server.server_addr().to_ip().unwrap());
    let served: PathBuf = fixture.clone();
    thread::spawn(move || {
        for req in server.incoming_requests() {
            let Ok(file) = File::open(&served) else { break };
            let reply = Response::from_file(file)
                .with_header(Header::from_bytes("Content-Type", "application/json").unwrap());
            let _ = req.respond(reply);
        }
    });
    (url, bytes, dir)
}

fn catalog_bench(c: &mut Criterion) {
    let rows = 10_000usize;
    let (url, bytes, _fixture) = catalog_server(rows);
    let client = Client::new(&url);
    assert_eq!(client.list_mods().unwrap().len(), rows);
    let mut group = c.benchmark_group("catalog");
    group.throughput(Throughput::Bytes(bytes));
    group.bench_function(BenchmarkId::new("list_mods", rows), |b| {
        b.iter(|| {
            let mods = client.list_mods().unwrap();
            assert_eq!(mods.len(), rows);
            black_box(mods);
        });
    });
    group.finish();
}

criterion_group!(benches, catalog_bench);
criterion_main!(benches);
