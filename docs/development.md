# Development

How to work on this repository. Session conventions for AI-assisted work are in
[`15-ai-dev-workflow`](../.claude/skills/15-ai-dev-workflow/SKILL.md).

## Toolchains

| Tool | Version | Notes |
|---|---|---|
| Julia | 1.12 | Workspace at the repository root; one committed `Manifest.toml`. Run `julia +1.12`: the juliaup default may be newer, and its `Pkg` rewrites the manifest to format 2.1 |
| Rust | 1.98.1 | `rust-toolchain.toml` (honoured by rustup). With a distribution toolchain instead of rustup, install `rustfmt` and `clippy` separately; put the rustup `~/.cargo/bin` first on `PATH` |
| GNU make | any | Single entry point for the gates |
| DuckDB CLI | 1.5.6, pinned; CI installs it | Schema registry checks and ad-hoc queries |
| Bun | later | Dashboard |

No Python, in any form (constitution §4; the pre-commit hook rejects `*.py`).

## Commands

| Command | What it runs |
|---|---|
| `make check` | Every gate below; run before claiming anything is done |
| `make rust` | `cargo fmt --check`, `clippy -D warnings`, tests, rustdoc with warnings as errors |
| `make julia` | Instantiate the workspace, parse every `.jl`, golden lock and calendar fixtures, `Pkg.test` of every package under `julia/` |
| `make julia-test` | `Pkg.test` only (packages without `test/runtests.jl` are skipped) |
| `make md-check` | Markdown code fences and table column counts |
| `make hook-test` | The constitution pre-commit hook's own test suite |
| `make hook-install` | Install the hook into `.git/hooks` |
| `make schemas` | Verify the lake at `$QUANT_ROOT/lake` against `schemas/registry.json` (needs DuckDB) |
| `make schemas-check` | Part of `make check`: the registry regenerated from `SCHEMAS.md` and the Rust writer equals the committed one; verifier self-test |
| `make fmt` | Format Rust code |

`bash .claude/skills/15-ai-dev-workflow/scripts/session-start.sh` checks the environment.

## Conventions

- **The skills are the spec.** Read the constitution and the skill for your layer before
  writing code. If the skill is wrong, fix it in the same change.
- **Rust:** no `unwrap`, `expect` or `panic!` in library code (workspace lints); typed errors;
  `unsafe` forbidden; every public item documented.
- **Julia:** packages join the root workspace; never `Pkg.add` at run time; tests with
  `Test.jl` in `test/runtests.jl`, test-only dependencies in `test/Project.toml` (also a
  workspace member); docstrings on public functions. `Pkg.test` writes a throwaway
  package-level manifest, which is gitignored: the root `Manifest.toml` is the only one.
- **Contracts first:** golden fixtures and schema tests are written before the code that must
  satisfy them. Where an implementation and a fixture disagree, the fixture wins.
- **Commits:** `<layer>: <imperative summary>`, body explaining what and why, and
  `Constitution: §N` when an invariant is touched
  ([`15-ai-dev-workflow`](../.claude/skills/15-ai-dev-workflow/SKILL.md) §8).

## Changing the rules

- **A dataset or a column:** append-only schema change, registry update and migration note
  ([`13-parquet-lake/MIGRATIONS.md`](../.claude/skills/13-parquet-lake/MIGRATIONS.md)).
- **The constitution:** only by an owner-approved amendment recorded in
  [`AMENDMENTS.md`](../.claude/skills/00-construction/AMENDMENTS.md).
- **A decision not covered by the constitution:** a short record in [`adr/`](adr/README.md).

## Environment

Secrets and paths live in a gitignored `.env` at the repository root (`chmod 600`); never paste
them in chat or commit them. Nothing in the repository sets a default for a production path.

| Variable | Used by |
|---|---|
| `QUANT_ROOT`, `QUANT_STATE`, `QUANT_CONFIG` | Data root, mutable state, read-only config (`config/`) |
| `QUANT_REGISTRY_PATH` | Registry file; default `schemas/registry.json` |
| `EODHD_API_KEY`, `FRED_API_KEY`, `SEC_EDGAR_EMAIL` | `quant-ingest` (the email goes into the SEC User-Agent) |
| `BACKFILL_START` | First backfill date; default `2005-01-01` |

DuckDB and Bun may not be on a non-interactive shell's `PATH`:
`export PATH="$HOME/.duckdb/cli/latest:$HOME/.bun/bin:$PATH"`.

## Pitfalls

- **Merging worker branches:** each adds its own `[workspace.dependencies]`; merge by hand,
  keep the `[workspace.lints]` sections, and regenerate `Cargo.lock` with a build.
- **Lints in integration tests:** the allow-unwrap-in-tests setting does not cover helper
  functions; use a file-level `#![allow(clippy::unwrap_used, ..., reason = "...")]`.
- **`lake/` is a crate:** never add `/lake/` to `.gitignore`; the data lake lives under
  `$QUANT_ROOT`.
- **CI:** `gh run watch` exiting 0 does not prove the run passed; check the conclusion in
  `gh run list`.
- **Documentation:** `make md-check` validates code fences, table columns and relative links;
  update [`status.md`](status.md) when a component changes state.
