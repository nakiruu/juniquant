//! Canonical ticker symbols shared by `ingest` and `execution`.
//!
//! Specification: `.claude/skills/01-data-ingestion/UNIVERSE.md` §3 (normalization grammar,
//! test vectors in `schemas/fixtures/symbol_normalization.csv`) and
//! `.claude/skills/09-lock-state-machine/SKILL.md` §3.8 (share classes of one issuer form
//! one lock group, owner decision B1; superseded `SYMBOL~CIK` tickers are rejected).
//!
//! Phase I1, workstream W1. Not implemented yet.
