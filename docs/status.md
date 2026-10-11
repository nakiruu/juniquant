# Project status

Where the build stands, what is verified and what is still open. This is the page to update
at the end of every work session. The build order and quality bar are in
[`INFRASTRUCTURE-PLAN.md`](INFRASTRUCTURE-PLAN.md); the specification is in
[`../.claude/skills/`](../.claude/skills/).

_Last updated: 2026-10-11, branch `i2-probes`._

**Current phase: I2, ingestion and the 15+ year backfill.** Foundation and shared contracts
(I0, I1) are done and merged. Probes P1-P6 are run, the common ingest infrastructure is built,
and the EODHD price worker (W5) is written and under test but not yet committed. Nothing
downstream of the lake (panel, models, portfolio, execution) exists yet.

Legend: **Done** = implemented, tested, merged to `main`. **WIP** = code exists but is not
merged, or is not yet exercised against the live source. **Spec only** = specified in a skill,
no implementation (skill `scripts/` hold non-normative sketches).

## By phase

| Phase | What | State |
|---|---|---|
| I0 | Repository foundation: Cargo and Julia workspaces, strict lints, `make check`, CI, pre-commit constitution hook | **Done** |
| I1 | Shared contracts: `symbols`, `calendar`, `lake`, schema registry, Julia `NYSECalendar` and `LakeIO` | **Done** (merged 2026-10-11) |
| I2 | Probes, common ingest infrastructure, per-source workers, the backfill | **In progress** (table below) |
| I3 | Universe and panel (Julia) | Spec only |
| I4 | GSPQR training, TCP, EVT, regime | Spec only |
| Later | Portfolio, CPCV backtest, ledger, dashboard, execution | Spec only |

## By component

| Component | Path | State | Notes |
|---|---|---|---|
| Canonical tickers, symbol map, lock groups | [`symbols/`](../symbols/README.md) | **Done** | Fixtures are the authority; dated ticker holdings (DECISIONS B5) |
| NYSE calendar (Rust) | [`calendar/`](../calendar/README.md) | **Done** | Reproduces the golden fixtures and the 1990-2100 holiday list |
| NYSE calendar (Julia) | [`julia/NYSECalendar`](../julia/NYSECalendar/README.md) | **Done** | Twin of the Rust crate; read-only |
| Lake I/O, schema registry | [`lake/`](../lake/README.md), [`schemas/`](../schemas/) | **Done** | 36 datasets registered; atomic writes, merge on primary key, date-filtered reads |
| Julia lake access | [`julia/LakeIO`](../julia/LakeIO/src/LakeIO.jl) | **Done** | Goes through the Rust `lake-io` binary over Arrow IPC |
| Probe harness, probes P1-P6 | [`ingest/src/probe/`](../ingest/src/probe/), [`probes/`](probes/README.md) | **Done** | Results recorded; open items below |
| Common ingest infrastructure | [`ingest/src/infra/`](../ingest/README.md) | **Done** | Landing zone, manifest, rate buckets, retry, stage and compact, quality log |
| W5 EODHD prices, splits, dividends | `ingest/src/eodhd.rs`, `quant-ingest eodhd backfill` | **WIP** | Written, unit and mock-server tests pass; uncommitted; no live backfill recorded yet |
| W6 EDGAR (fundamentals, filing events, delistings, CIK side of `security_master`) | | Spec only | Bulk file format settled by P5 |
| W7 FRED / ALFRED (`macro`) | | Spec only | Vintage behaviour settled by P6 |
| W8 iShares holdings (`etf_holdings`) | | Spec only | Blocked on probe P7 (history depth, DECISIONS D1) |
| W9 Listings (`listings`) | | Spec only | The daily Nasdaq Trader files should be archived from the first day the fetcher exists (DECISIONS D13) |
| Quality gates G0-G10 | `config/quality_gates.toml` | Spec only | Thresholds exist; the gate runner does not |
| Panel, models, regime, overlay, portfolio, backtest, ledger, trade summary | | Spec only | |
| Execution, lock monitor, audit journal, Schwab client | | Spec only | The calendar and symbol crates are the only parts built; the lock rule has golden fixtures and Julia oracles |
| Dashboard | | Spec only | |

## Quality gates that pass today

`make check` runs Rust format, clippy (`-D warnings`), tests and rustdoc; Julia parse checks,
golden fixtures and package tests; the schema registry check; markdown and link checks; and the
pre-commit hook's own tests. `cargo test --workspace` passes on this branch including the
uncommitted W5 code (checked 2026-10-11).

## Open items

**Probes**

- P7 (iShares history depth) and P8 (Form 25 timing) have not run. P7 gates the universe start
  and the holdings backfill (DECISIONS D1).
- P4: EODHD's publication time for a trading day is unmeasured; rerun on a weekday evening.
- P5: confirm the nightly regeneration of the EDGAR bulk zips (HEAD on two consecutive days).
- Details and decisions: [`probes/README.md`](probes/README.md).

**Owner decisions and actions**

- Shares outstanding for multi-class issuers such as BRK (P5 finding).
- ICE BofA credit spreads were deferred in P6; confirm.
- `.env` holds the variable names but `FRED_API_KEY` and `SEC_EDGAR_EMAIL` are still to be
  filled in; `QUANT_ROOT`, `QUANT_STATE` and the disk budget must be set before a backfill.
- `config/fractional_eligible.toml` is seeded by the owner (D9); until then the Bootstrap tier
  trades nothing.
- Check the ICE Global Personal Trading Policy for pre-clearance terms before live trading.

**Known shortcuts** (each is a `shortcut:` comment in the code)

- Manifest uses one SQLite connection behind a mutex (`ingest/src/infra/manifest.rs`).
- Compaction rebuilds every partition of a dataset and fails on a duplicate key across stage
  files; the daily bulk chunks need a "later stage file wins" rule (`ingest/src/infra/stage.rs`).
- Probes use blocking HTTP and a fixed interval instead of the production bucket.

**Documentation debt**

- `docs/operations.md` (Unraid deployment, schedule, backups, halts) is planned for the
  execution phase.
- `docs/data.md` will gain a generated dataset table from `schemas/registry.json`.
- `docs/adr/` is empty: no non-constitutional decision has needed a record beyond `DECISIONS.md`.

## Next steps

1. Commit and review W5; run a small live backfill (`--codes`, `--limit`), then the full one.
2. Run P7, then build W8 and W9; build W6 and W7 (independent of each other).
3. Implement the quality gate runner; review the coverage and survivorship reports with the owner.
4. Phase I3: universe and panel.

See also the 2026-10-10 [session handoff](../handoff101026.md) (historical).
