# Handoff 2026-10-10

Session handoff for juniquant. Repository: `/home/nico/juniquant`, branch `main`, remote
`github.com/nakiruu/juniquant` (private). Last pushed commit: `c6d7aed`
(CI run 38082954760 green). Start a new session by reading this file, then
[`docs/INFRASTRUCTURE-PLAN.md`](docs/INFRASTRUCTURE-PLAN.md).

## Ground rules from the owner

- **Compliance outranks latency, slippage and profit.** The owner works for NYSE's parent and
  is contractually bound by the 5-trading-day opposite-side lock:
  - It applies across every account (Fidelity included).
  - Share classes of one issuer are one lock group.
  - A cancel with zero fills does not count.
  - The block runs through D+4 trading days after the last fill.
  - There are no exceptions. On any doubt, it fails closed.
- **ICE is never traded.** It is held only in Fidelity.
- **No Python anywhere.** Rust, Julia and Bun/TypeScript only; the pre-commit hook rejects `*.py`.
- **Project phase:**
  1. Infrastructure.
  2. The 15+ year free-data backfill, with survivorship bias documented.
  3. GSPQR + TCP + EVT training.
  4. Live trading, which is far off.
- **Ways of working:**
  - Use subagent orchestration.
  - Check your work and reasoning before claiming anything is done.
  - Keep the README and `docs/` current.
- The skills in [`.claude/skills/`](.claude/skills/) are the spec. The constitution is
  [`00-construction/SKILL.md`](.claude/skills/00-construction/SKILL.md); amendments 1.0-1.13 are
  all approved. [`DECISIONS.md`](DECISIONS.md) is fully resolved.

## Done

| Phase | State |
|---|---|
| I0 foundation | Done: git, Cargo workspace with strict lints, Julia workspace, Makefile gates, hook, CI, README and `docs/` |
| SIC change | Done: `security_master.sic`, `sic_description` and `sic_asof` (EDGAR, current value only). It is a fallback sector through `config/sic_to_gics.toml` (flagged `sic_fallback`) and identifies financial issuers for the Altman Z veto. GICS from the iShares snapshot stays the source for the 25% sector cap |
| I1 contracts | Done: `schemas/fixtures/symbol_normalization.csv` (41 rows), `symbol_render.csv` (11 rows) |
| I1 W1 `symbols` crate | Merged: `Canon`, `normalize`, `is_tradable`, `Cik`, `SupersededTicker`, `render`, `SymbolMap` (fail closed, B1 issuer check) |
| I1 W2 `calendar` crate + `julia/NYSECalendar` | Merged: both reproduce the golden fixtures; a 1990-2100 cross-language holiday fixture is checked by Rust, Julia and the oracle |

`make check` passes locally and in CI. It runs:

- `cargo fmt`, `clippy -D warnings`, tests and rustdoc;
- Julia parse checks, oracle fixtures and `Pkg.test` for `julia/*`;
- markdown and link checks;
- the hook's 54 tests.

## Next

1. **I1 W3 + W4, the `lake` crate and the schema registry, done together.** The design is in
   the plan and in the lost W3 brief:
   - The registry entries gain `fields`, `primary_key`, `merge`, `layout` and
     `partition_keys`, parsed from `13-parquet-lake/SCHEMAS.md` by
     `schemas/generate-registry.jl`.
   - A Rust bin `lake-schema-probe` writes an empty leaf through the real writer, and DuckDB
     `DESCRIBE` on that file gives the `columns`/`hash`.
   - `schemas/verify-schemas.sh` gets a lake mode and a `--self-test` mode for CI.
   - CI installs the pinned DuckDB CLI (1.5.6).
   - The writer follows `IO-CONTRACT.md` §3 exactly: validate, merge (`overwrite` or
     `append_only`), sort by primary key, write a ZSTD-3 temp file, fsync, rename, fsync the
     directory.
   - The W3 worker had picked `arrow`/`parquet` 60.0.0 and `sha2` 0.11.
   - Add `migrations/2026-10-10-initial.md`, and replace the sketch scripts in
     `13-parquet-lake/scripts/`.
2. **Julia `LakeIO`** (`julia/LakeIO`, a placeholder for now). It must produce files whose
   DuckDB `DESCRIBE` hash equals the registry's.
3. Run an independent review of each workstream, then rerun `make check`, merge and push.
4. Then I2: run probes P1-P8 first ([`01-data-ingestion/BACKFILL.md`](.claude/skills/01-data-ingestion/BACKFILL.md)
   §2), then the common ingest infrastructure, then the per-source workers.

## Open questions for the owner

- ~~Ticker reuse after a rename~~: decided by the owner (DECISIONS B5), implemented in `symbols`.
- ~~Altman Z sector exclusion~~: confirmed by the owner (DECISIONS D11).

## Owner setup pending

- Put `FRED_API_KEY` and `SEC_EDGAR_EMAIL` in a gitignored `.env` at the repo root
  (`chmod 600`). Never paste them in chat. Nothing reads them until I2.
- Local `QUANT_ROOT` / `QUANT_STATE` locations and the disk budget, needed for the I2 backfill.

## Gotchas

- **Tool paths.** DuckDB and Bun are not on the tool shell's PATH. Use
  `export PATH="$HOME/.duckdb/cli/latest:$HOME/.bun/bin:$PATH"`.
- **Subagent limits.** Subagents hit the weekly usage limit on 2026-10-10 (it resets at 10am ET).
  Brief workers to commit early. If a worker dies, salvage its worktree under
  `.claude/worktrees/`; that path is gitignored and skipped by the checks.
- **Merging worker branches.** Each worker adds its own `[workspace.dependencies]` block.
  Merge them by hand and keep the `[workspace.lints]` sections (one regex edit deleted them
  once). Regenerate `Cargo.lock` with a build.
- **Lints in integration tests.** Clippy's allow-unwrap-in-tests does not cover helper
  functions in integration tests. Use a file-level
  `#![allow(clippy::unwrap_used, ..., reason = "...")]`.
- **Julia test dependencies.** Put them in `test/Project.toml`, listed in the root
  `[workspace]`. `Pkg.test` writes a throwaway `julia/*/Manifest.toml`, which is gitignored.
- **No `/lake/` in `.gitignore`.** An earlier `.gitignore` entry `/lake/` hid the `lake`
  crate and broke CI. The data lake lives under `$QUANT_ROOT`, never in the repo.
- **CI status.** `gh run watch` exiting 0 is not proof the run passed. Confirm with
  `gh run list` (the conclusion column).
- **Commit trailers.** Every commit message ends with:

```
Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01E9Wcr9pYGhQwgPgXSgV3m3
```
