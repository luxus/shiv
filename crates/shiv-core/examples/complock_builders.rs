//! Classify the builder files a complock-watch run downloaded.
//!
//! `cargo run --release -p shiv-core --example complock_builders -- FETCH_DIR DECISION`
//!
//! `FETCH_DIR` holds `hit.html` (or `hit.js`), `lame.min.js`, `hp_hpbars.js`
//! and `hpbars_base.vpk`. `DECISION` receives one word on the first line:
//! `current`, `template`, `review` or `failed`, and the reasons under it.
//! A template decision also writes `out/hit_builder.js` and, when the base
//! pak moved, `out/hpbars_base.vpk`. Exit 2 on bad arguments. Exit 0
//! otherwise: a failed fetch is a decision, not a crash.

use std::fs;
use std::path::Path;
use std::process::ExitCode;

use shiv_core::generated::{self, RemoteBuilders, Watch};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [fetch, decision] = args.as_slice() else {
        eprintln!("usage: complock_builders FETCH_DIR DECISION");
        return ExitCode::from(2);
    };
    let fetch = Path::new(fetch);
    let read = |name: &str| fs::read(fetch.join(name));
    let hit = fs::read_to_string(fetch.join("hit.html"))
        .or_else(|_| fs::read_to_string(fetch.join("hit.js")));
    let (Ok(hit), Ok(lame), Ok(hp), Ok(base)) = (
        hit,
        read("lame.min.js"),
        read("hp_hpbars.js"),
        read("hpbars_base.vpk"),
    ) else {
        return write_decision(decision, "failed\n- a builder file did not download\n");
    };
    let hp = String::from_utf8_lossy(&hp);
    let watch = generated::classify(&RemoteBuilders {
        hit_page: &hit,
        lame: &lame,
        hpbars_js: &hp,
        base_pak: &base,
    });
    let text = match watch {
        Watch::Current => "current\n".to_owned(),
        Watch::Review { changed } => {
            let mut text = String::from("review\n");
            for name in changed {
                text.push_str(&format!("- {name}\n"));
            }
            text
        }
        Watch::Failed { reason } => format!("failed\n- {reason}\n"),
        Watch::Template {
            hit_builder,
            base_pak,
        } => {
            let out = fetch.join("out");
            if fs::create_dir_all(&out).is_err()
                || fs::write(out.join("hit_builder.js"), hit_builder).is_err()
            {
                return write_decision(
                    decision,
                    "failed\n- could not write the retargeted script\n",
                );
            }
            if let Some(pak) = base_pak
                && fs::write(out.join("hpbars_base.vpk"), pak).is_err()
            {
                return write_decision(decision, "failed\n- could not write the base pak\n");
            }
            "template\n".to_owned()
        }
    };
    write_decision(decision, &text)
}

fn write_decision(path: &str, text: &str) -> ExitCode {
    print!("{text}");
    match fs::write(path, text) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cannot write {path}: {e}");
            ExitCode::from(2)
        }
    }
}
