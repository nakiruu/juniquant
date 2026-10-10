# Development

How to work on this repository. Session conventions for AI-assisted work are in
[`15-ai-dev-workflow`](../.claude/skills/15-ai-dev-workflow/SKILL.md).

## Toolchains

| Tool | Version | Notes |
|---|---|---|
| Julia | 1.12 | Workspace at the repository root; one committed `Manifest.toml` |
| Rust | 1.98.1 | `rust-toolchain.toml` (honoured by rustup). On Fedora's system toolchain install `rustfmt` and `clippy`: `sudo dnf install rustfmt clippy` |
| GNU make | any | Single entry point for the gates |
| DuckDB CLI | to be pinned in phase I1 | Schema registry checks and ad-hoc queries |
| Bun | later | Dashboard |

No Python, in any form (constitution §4; the pre-commit hook rejects `*.py`).

## Commands

| Command | What it runs |
|---|---|
| `make check` | Every gate below; run before claiming anything is done |
| `make rust` | `cargo fmt --check`, `clippy -D warnings`, tests, rustdoc with warnings as errors |
| `make julia` | Instantiate the workspace, parse every `.jl`, golden lock and calendar fixtures |
| `make md-check` | Markdown code fences and table column counts |
| `make hook-test` | The constitution pre-commit hook's own test suite |
| `make hook-install` | Install the hook into `.git/hooks` |
| `make schemas` | Verify lake schemas against `schemas/registry.json` (needs DuckDB) |
| `make fmt` | Format Rust code |

`bash .claude/skills/15-ai-dev-workflow/scripts/session-start.sh` checks the environment.

## Conventions

- **The skills are the spec.** Read the constitution and the skill for your layer before
  writing code. If the skill is wrong, fix it in the same change.
- **Rust:** no `unwrap`, `expect` or `panic!` in library code (workspace lints); typed errors;
  `unsafe` forbidden; every public item documented.
- **Julia:** packages join the root workspace; never `Pkg.add` at run time; tests with
  `Test.jl`; docstrings on public functions.
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
