//! The complock-watch gate: the fresh pack-data, its gameinfo and the
//! downloaded parts, checked with the same typed model Shiv uses.
//!
//! `cargo run --release -p shiv-core --example complock_watch -- FRESH_JSON
//! GAMEINFO LAST_GOOD_JSON PARTS_DIR REPORT_MD`
//!
//! Exit 0 and no report when the update is safe to embed; exit 1 with the
//! problems written to REPORT_MD, one line each, when it is not; exit 2 on
//! bad arguments or unreadable inputs.

use std::process::ExitCode;

use shiv_core::complock;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [fresh, gameinfo, last_good, parts, report] = args.as_slice() else {
        eprintln!("usage: complock_watch FRESH_JSON GAMEINFO LAST_GOOD_JSON PARTS_DIR REPORT_MD");
        return ExitCode::from(2);
    };
    let read = |path: &str| {
        std::fs::read_to_string(path).map_err(|e| eprintln!("cannot read {path}: {e}"))
    };
    let (Ok(fresh), Ok(gameinfo), Ok(last_good)) = (read(fresh), read(gameinfo), read(last_good))
    else {
        return ExitCode::from(2);
    };
    let mut problems = complock::check_pack(&fresh, &gameinfo, &last_good);
    if !problems
        .iter()
        .any(|p| matches!(p, complock::PackProblem::Unparsed(_)))
    {
        problems.extend(complock::check_parts(&fresh, std::path::Path::new(parts)));
    }
    if problems.is_empty() {
        println!("pack-data, gameinfo and every part check clean");
        return ExitCode::SUCCESS;
    }
    let lines: String = problems.iter().map(|p| format!("- {p}\n")).collect();
    print!("{lines}");
    if let Err(e) = std::fs::write(report, &lines) {
        eprintln!("cannot write {report}: {e}");
        return ExitCode::from(2);
    }
    ExitCode::FAILURE
}
