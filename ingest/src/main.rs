//! `quant-ingest`: data ingestion and the 15+ year backfill.
//!
//! Specification: `.claude/skills/01-data-ingestion/` (SKILL.md, BACKFILL.md, SOURCES.md).
//! Phase I2. Implemented so far: `quant-ingest probe <p1..p6>` (BACKFILL.md §2).

mod probe;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [cmd, name] if cmd == "probe" => match probe::run(name) {
            Ok(path) => {
                println!("{name}: evidence in {}", path.display());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("quant-ingest probe {name}: {e}");
                ExitCode::FAILURE
            }
        },
        _ => {
            eprintln!("usage: quant-ingest probe <p1..p6>");
            ExitCode::from(2)
        }
    }
}
