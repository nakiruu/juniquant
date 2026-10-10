# Infrastructure Plan

Status: draft for the owner, 2026-10-10. Turns the skills (`.claude/skills/`) and
`scaffold.md` into a working, high-quality codebase. The skills are the spec;
this plan is the build order, the quality bar, and how the work is split across
subagents.

**Critical path:** repository foundation, then shared contracts, then the 15+ year
data backfill, then the panel, then GSPQR + TCP + EVT training. Portfolio,
backtest, ledger, frontend and execution come after (`scaffold.md`, Build Order).

---

## 1. Starting point

| Item | State |
|---|---|
| Specs | 19 skills, constitution amended to 1.13, `DECISIONS.md` all resolved |
| Code | None yet. Skill `scripts/` hold sketches only (not normative) |
| Verified artifacts | Golden fixtures for the lock rule and NYSE calendar (`09-lock-state-machine/fixtures/`), Julia oracles that pass them, pre-commit hook with 52 passing tests |
| Toolchains present | Julia 1.12.1, Rust/cargo 1.98.1 with rustfmt and clippy, git, jq, DuckDB CLI 1.5.6, Bun 1.4.3 |
| Toolchains missing | SQLite CLI (backups, later) |
| Repository | Private GitHub repository with CI (`make check`) |

---

## 2. Quality bar (applies to every phase)

**Definition of done for any task:**
1. Behavior matches the skill; if the skill was wrong, the skill is fixed in the same change.
2. Tests written first for contracts (fixtures, schemas, pure functions), and passing.
3. Lint and format clean (below). No `unwrap`/`expect`/`panic!` in Rust library code.
4. No Python anywhere. No `Pkg.add` at run time (pinned environments only).
5. Docs updated: the module's docstrings, `docs/` page if user-visible, README if a command changes.
6. The constitution hook passes; no constitutional change without an owner-approved amendment.
7. An independent reviewer (fresh subagent, given the code, not the author's conclusions) found no blocking issue.

**Automated gates** (one command, `just check`, and the same in CI):

| Gate | Tool |
|---|---|
| Rust format / lint / test / docs | `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings` (workspace lints deny `unwrap_used`, `expect_used`, `panic` outside tests), `cargo test --workspace`, `cargo doc` without warnings |
| Rust dependency hygiene | `cargo deny` (licenses, advisories, duplicate versions) if installable; otherwise `Cargo.lock` review |
| Julia tests | `Pkg.test` for each package in the Julia workspace |
| Julia quality | Aqua.jl (ambiguities, stale deps, piracy), JET.jl on public entry points, JuliaFormatter check |
| Golden fixtures | Julia oracles and the Rust `calendar`/lock crates reproduce `09-lock-state-machine/fixtures/*.json` exactly |
| Constitution | `pre-commit-constitution.sh` and its test suite |
| Schemas | `schemas/registry.json` hashes match (`verify-schemas.sh`, needs DuckDB CLI) |
| Docs | Markdown fence/table checker (Julia, in `tools/`), link check, `cargo doc`, Documenter build (once packages exist) |

**Toolchain pins:** `rust-toolchain.toml` (1.98.1 with rustfmt and clippy),
Julia `compat` 1.12 in every `Project.toml` with committed `Manifest.toml`,
DuckDB CLI version recorded in `docs/development.md`.

---

## 3. Phases

### Phase I0 — Repository foundation (small, sequential, done by the coordinator)

**Done 2026-10-10** (commit `5d10e20`): git, Cargo workspace with strict lints and four crate skeletons, Julia workspace (`tools`), Makefile gates (`make check`), hook installed, session-start rewritten, README and `docs/`. Remaining: `rustfmt` and `clippy` must be installed (`sudo dnf install rustfmt clippy`) before `make check` passes end to end; no remote, so no CI workflow yet. A `Makefile` is used because `just` is not installed.

1. `git init`, `.gitignore` (data trees, `target/`, Julia depots), first commit of the specs as they are now. **Owner confirms** before the first commit, and whether a remote (GitHub or other) exists.
2. Cargo workspace at the root: members `symbols/`, `calendar/` (new, see I1), `lake/` (new), `ingest/`; later `execution/`. Shared `[workspace.lints]`, `rust-toolchain.toml`, `rustfmt.toml`, `clippy.toml`.
3. Julia workspace: root `Project.toml` with `[workspace] projects = ["features", "models", "portfolio", "regime", "conviction", "ledger", "backtest", "schemas", "tools"]` (Julia 1.12 workspaces; verify) and one shared `Manifest.toml`.
4. `justfile` with `check`, `test`, `fmt`, `fixtures`, `hook-install`, `docs` recipes (or a `Makefile` if `just` is not installed).
5. Pre-commit hook installed from `00-construction/scripts/`; `session-start.sh` updated to the real layout.
6. CI workflow (`.github/workflows/ci.yml`) that runs `just check` if a remote exists; otherwise `just check` is the local gate.
7. **Docs start here:** `README.md` and the `docs/` skeleton (section 6).

### Phase I1 — Shared contracts (test-first, parallel)

| Workstream | Deliverable | Contract it must satisfy |
|---|---|---|
| W1 `symbols` crate | Canonical ticker normalize/render, share-class issuer key (lock group, decision B1), `~` superseded rejection | `schemas/fixtures/symbol_normalization.csv` (written first) |
| W2 `calendar` crate + Julia `NYSECalendar` | Rules + overrides seed + runtime overrides; `add_trading_days`, `is_trading_day` | Golden calendar fixtures, identical in both languages |
| W3 `lake` crate + Julia `LakeIO` | `write_partition` / `read_dataset`, atomic temp + rename, idempotency | `13-parquet-lake/IO-CONTRACT.md`; property tests (kill mid-write, re-run) |
| W4 Schema registry | `schemas/registry.json`, `generate-registry.jl`, `verify-schemas.sh`, `migrations/2026-10-10-initial.md` | Canonical DuckDB `DESCRIBE` hash form (13 SKILL §6); needs DuckDB CLI |

W1-W3 own disjoint directories and run in parallel. W4 starts when DuckDB is installed.

**Status 2026-10-10:** W1 `symbols` and W2 `calendar` + `NYSECalendar` are merged. The workers hit the usage limit, so the coordinator finished, reviewed and merged them. Rust, Julia and the oracle agree on the 1990-2100 holiday list. Next: W3 + W4 together (lake crate and registry; the registry hash comes from the writer's output), then Julia `LakeIO`. Open question from W1: a ticker reused by a new issuer after a rename currently fails the symbol-map load as an alias collision (safe, but strict).

### Phase I2 — Ingestion and the 15+ year backfill (critical path)

1. **Probes first** (`quant-ingest probe P1..P8`, 01 `BACKFILL.md` §2): settle every "verify" item (Yahoo raw vs adjusted OHLCV and delisted symbols, EDGAR bulk URLs and sizes, ALFRED parameters, iShares `asOfDate` depth = decision D1, Form 25 timing, XBRL phase-in). Results are written to `docs/probes/` and fed back into the skills.
2. Common ingest infrastructure: raw landing zone with SHA-256 sidecars, `manifest.db` state machine, rate buckets, retry policy, fetch / parse / compact separation, gates G0-G10.
3. Per-source workers, in parallel after step 2, each owning one module:

| Worker | Sources | Datasets |
|---|---|---|
| W5 Yahoo | chart API | `prices` (raw as-traded), `splits`, `dividends` |
| W6 EDGAR | bulk companyfacts / submissions, 8-K items | `fundamentals`, `filing_events`, `delistings`, `security_master` (CIK side) |
| W7 FRED/ALFRED | series + vintages | `macro` (long form, `known_from`) |
| W8 iShares | holdings by as-of date (month-end + D2 extras) | `etf_holdings` |

4. Backfill run on the owner's machine (or Unraid), resumable; the coverage and survivorship reports (`price_coverage`, `survivorship_caveat.json`) are reviewed with the owner before Phase I3.

### Phase I3 — Universe and panel (Julia)

W9 universe builder + price coverage; W10 `tri`, raw features, ranks, targets (terminal values, decision D8), splits, panel; `validate-panel.jl` as a real test. Gate: panel validator passes and the survivorship caveat is attached.

### Phase I4 — Prediction

W11 GSPQR training + walk-forward generations (03 `PREDICTION-BACKFILL.md`, decisions C2, C4); W12 TCP (pooled, RM state); W13 EVT (native GPD MLE, windows); W14 regime (filtered HMM). Then the walk-forward track metrics (14). Gate: reproducible generations (fixed seeds, hashed inputs), coverage within tolerance on the walk-forward track.

### Later phases (unchanged order)

Portfolio (06, 07, 08) with neutral defaults, CPCV evaluation (14), ledger (16), frontend (17), then execution and live trading (09, 10, 11) last. The lock and compliance crates are built early only as far as I1 needs (calendar, symbols); the order path waits for the execution phase.

---

## 4. Subagent orchestration

- **Coordinator** (this session): writes contracts and fixtures first, assigns disjoint file ownership, merges cross-cutting edits, runs the gates, and reports to the owner. Never delegates a check it can run directly.
- **Workers**: one per workstream above, each briefed with the skill sections, the contract, the files it owns and the definition of done. Workers in the same wave never share files. With git in place, each worker can use its own worktree.
- **Reviewers**: for each completed workstream, an independent reviewer gets the code and the skill (not the worker's summary) and looks for bugs, contract violations, constitution breaches and missing tests. Findings go back to the same worker.
- **Verification**: the coordinator re-runs every gate after merging; nothing is reported as done on a worker's word.
- **Budget**: waves of 2-4 workers. Subagents hit the weekly usage limit on 2026-10-10 (reset 10am ET), so waves are sized to finish within a session and resumable.

---

## 5. Owner actions needed

| Action | Needed for |
|---|---|
| Approve `git init` and the first commit; say whether a remote exists | I0 |
| Install DuckDB CLI (and later Bun, SQLite CLI) | I1 W4, later phases |
| FRED API key in the environment (`FRED_API_KEY`) | I2 W7 |
| Contact email for the SEC User-Agent header (`SEC_EDGAR_EMAIL`) | I2 W6 |
| Local data location for development (`QUANT_ROOT`, `QUANT_STATE`) and disk budget (raw zone is the largest; estimates in 01 `BACKFILL.md`) | I2 |
| Review the probe results and the survivorship report | End of I2 |

---

## 6. Documentation

Docs start in Phase I0 and grow with every phase (part of the definition of done).

| File | Content |
|---|---|
| `README.md` | What the system is, current phase, quick start (toolchains, `just check`), repository layout, where the specs live, key rules (lock, ICE, no Python) |
| `docs/architecture.md` | Pipeline from data to orders, language boundaries, data lake, schedule; diagram |
| `docs/data.md` | Sources, backfill, survivorship caveat, quality gates, lake datasets (generated table from `schemas/registry.json` later) |
| `docs/compliance.md` | The 5-day lock in plain language with the owner's examples, ICE, how the audit works, how to read a compliance report |
| `docs/development.md` | Toolchains and pins, commands, conventions (from skill 15), how to add a dataset, a migration, an amendment |
| `docs/operations.md` | Later: Unraid deployment, schedule, backups, halts and how to clear them |
| `docs/probes/` | Results of probes P1-P8 |
| `docs/adr/` | Short records for decisions not in the constitution |
| API docs | `cargo doc` for crates; Documenter.jl for Julia packages once they exist |

The skills stay the normative spec for implementers; `docs/` is the readable
overview for the owner and anyone new, and links into the skills rather than
copying them.
