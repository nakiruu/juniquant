//! `quant-ingest`: data ingestion and the 15+ year backfill.
//!
//! Specification: `.claude/skills/01-data-ingestion/` (SKILL.md, BACKFILL.md, SOURCES.md).
//! Phase I2. Not implemented yet; the first subcommand will be `probe` (BACKFILL.md §2).

use std::process::ExitCode;

fn main() -> ExitCode {
    eprintln!("quant-ingest: not implemented yet (phase I2). See .claude/skills/01-data-ingestion/BACKFILL.md");
    ExitCode::FAILURE
}
