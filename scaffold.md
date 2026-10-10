# Codebase Scaffold (Updated)

The complete directory layout, now including the ledger layer. Every file the skills reference has a home.

Paths at run time come from three environment variables defined once in `13-parquet-lake/IO-CONTRACT.md` §1: `QUANT_ROOT` (data root; container `/data`, host `/mnt/cache/quant`; the lake is `$QUANT_ROOT/lake`), `QUANT_STATE` (mutable service state; container `/state`, host `/mnt/cache/appdata/quant`) and `QUANT_CONFIG` (the repository's `config/`, mounted read-only at `/config`). Everything below that lives in state sits **directly** under `$QUANT_STATE` (there is no `state/` subdirectory). Code and scripts never hard-code a host path.

```
quant/
├── .claude/
│   └── skills/                          # All 19 skills (00-18)
│       ├── 00-construction/
│       │   └── scripts/
│       │       ├── pre-commit-constitution.sh
│       │       └── test-pre-commit-constitution.sh   # runs the hook against a scratch repo
│       ├── 01-data-ingestion/
│       ├── 02-panel-construction/
│       ├── 03-gspqr-model/
│       ├── 04-tcp-calibration/
│       ├── 05-evt-tails/
│       ├── 06-regime-detection/
│       ├── 07-fundamental-overlay/
│       ├── 08-portfolio-optimization/
│       ├── 09-lock-state-machine/
│       │   ├── fixtures/                # golden fixtures: lock_cases.json, nyse_calendar_cases.json
│       │   └── scripts/                 # Julia oracles: lock_rule_oracle.jl, nyse_calendar_oracle.jl, run_fixtures.jl
│       ├── 10-execution-schwab/
│       ├── 11-rust-event-bus/
│       ├── 12-julia-rust-bridge/
│       ├── 13-parquet-lake/
│       ├── 14-backtest-protocol/
│       ├── 15-ai-dev-workflow/
│       ├── 16-portfolio-ledger/
│       ├── 17-frontend/
│       └── 18-trade-summary/
│
├── models/                              # Julia: GSPQR, TCP, EVT training & inference (code; artifacts live in $QUANT_ROOT/models/)
│   ├── Project.toml
│   ├── Manifest.toml
│   ├── src/
│   │   ├── QuantModels.jl
│   │   ├── gspqr/
│   │   │   ├── train.jl
│   │   │   ├── features.jl
│   │   │   ├── params.jl
│   │   │   ├── backfill.jl              # monthly walk-forward generations over history (03 PREDICTION-BACKFILL.md)
│   │   │   └── export.jl
│   │   ├── tcp/
│   │   │   ├── calibrate.jl
│   │   │   ├── scores.jl
│   │   │   ├── pooled.jl                # pooled-by-date scores normalized by raw width, M_d = H matured dates
│   │   │   └── robbins_monro.jl         # persisted RM state (rm_offset, rm_n)
│   │   ├── evt/
│   │   │   ├── fit.jl
│   │   │   ├── gpd_mle.jl               # native Julia GPD maximum likelihood (no Python, no scipy)
│   │   │   ├── threshold.jl
│   │   │   └── extrapolate.jl
│   │   └── inference/
│   │       └── predict.jl               # daily inference; Julia is the sole writer of gspqr_predictions
│   └── test/
│       ├── runtests.jl
│       ├── test_gspqr.jl
│       ├── test_tcp.jl
│       └── test_evt.jl
│
├── features/                            # Julia: universe builder, price coverage, panel construction
│   ├── Project.toml
│   ├── Manifest.toml
│   ├── src/
│   │   ├── Features.jl
│   │   ├── universe.jl                  # point-in-time daily membership + trailing eligibility (01 UNIVERSE.md)
│   │   ├── coverage.jl                  # price_coverage: survivorship coverage per ticker (01 SURVIVORSHIP.md)
│   │   ├── raw.jl                       # raw features; total-return index `tri` (02 FEATURES.md §0)
│   │   ├── ranks.jl
│   │   ├── targets.jl                   # the only file with forward indexing (constitution §3)
│   │   ├── splits.jl                    # approved chronological splits (constitution §3)
│   │   └── panel.jl
│   └── test/
│       ├── runtests.jl
│       ├── test_universe.jl
│       ├── test_coverage.jl
│       ├── test_raw.jl
│       ├── test_ranks.jl
│       └── test_targets.jl
│
├── portfolio/                           # Julia: optimization
│   ├── Project.toml
│   ├── src/
│   │   ├── Portfolio.jl
│   │   ├── pmpt.jl
│   │   ├── covariance.jl
│   │   ├── constraints.jl
│   │   └── conviction_apply.jl
│   └── test/
│       └── runtests.jl
│
├── regime/                              # Julia: HMM
│   ├── Project.toml
│   ├── src/
│   │   ├── Regime.jl
│   │   ├── fit.jl                       # monthly walk-forward refits; parameters to $QUANT_ROOT/models/regime/hmm_<fit_date>.json
│   │   ├── decode.jl                    # filtered (forward-only) probabilities; never smoothed
│   │   ├── online_viterbi.jl            # causal online Viterbi state
│   │   ├── relabel.jl
│   │   └── confirm.jl
│   └── test/
│       └── runtests.jl
│
├── conviction/                          # Julia: fundamental overlay
│   ├── Project.toml
│   ├── src/
│   │   ├── Conviction.jl
│   │   ├── veto.jl
│   │   ├── f_score.jl
│   │   ├── altman_z.jl
│   │   ├── components.jl
│   │   └── standardize.jl
│   └── test/
│       └── runtests.jl
│
├── ledger/                              # Julia: portfolio ledger & tax lots
│   ├── Project.toml
│   ├── src/
│   │   ├── Ledger.jl
│   │   ├── daily.jl                     # write-ledger-row
│   │   ├── metrics.jl                   # drawdown, Sortino, cumulative
│   │   ├── realized.jl                  # YTD realized P&L
│   │   └── query.jl                     # common ledger queries
│   └── test/
│       ├── runtests.jl
│       ├── test_daily.jl
│       └── test_realized.jl
│
├── backtest/                            # Julia: walk-forward track + CPCV evaluation
│   ├── Project.toml
│   ├── src/
│   │   ├── Backtest.jl
│   │   ├── cpcv.jl                      # approved purged, embargoed CPCV (constitution §3)
│   │   ├── metrics.jl
│   │   └── paths.jl                     # run artifacts under $QUANT_ROOT/reports/backtest/<run_id>/cpcv/split=<k>/
│   └── test/
│       └── runtests.jl
│
├── symbols/                             # Rust: shared canonical-ticker crate (normalize, render, is_tradable)
│   ├── Cargo.toml                       # used by ingest/ and execution/ (lock, orders, risk); 01 UNIVERSE.md §3
│   ├── src/
│   │   └── lib.rs
│   └── tests/
│       └── normalization.rs             # loads schemas/fixtures/symbol_normalization.csv
│
├── execution/                           # Rust: production hot path
│   ├── Cargo.toml
│   ├── Cargo.lock
│   ├── src/
│   │   ├── main.rs
│   │   ├── lib.rs
│   │   ├── symbols.rs                   # canonical symbol map over config/symbol_map.toml; thin layer over the symbols crate; rejects SYMBOL~CIK10
│   │   ├── locks/
│   │   │   ├── mod.rs
│   │   │   ├── order_history.rs         # OrderHistory: per-account REST read before every authorize (N = 0), Streamer = dirty trigger only, moka single-flight; fail-closed
│   │   │   ├── calendar.rs              # NYSE holiday rules + overrides, add_trading_days (seed config/nyse_calendar_overrides.toml + runtime $QUANT_STATE copy)
│   │   │   ├── authorize.rs             # authorize() (pure verdict), authorize_and_reserve(), BuyPermit/SellPermit, Denied (no cached lock row)
│   │   │   ├── gate.rs                  # per-ticker gate: check-then-send atomic (09 §6a)
│   │   │   ├── halt.rs                  # $QUANT_STATE/TRADING_HALTED: set / clear / auto-clear INCOMPLETE (09 §8b)
│   │   │   ├── audit.rs                 # daily post-close compliance audit (16:40) + dated report
│   │   │   ├── pending.rs
│   │   │   └── monitor.rs               # fires pending orders on the first permitted day; derived events
│   │   ├── schwab/
│   │   │   ├── mod.rs
│   │   │   ├── oauth.rs
│   │   │   ├── orders.rs
│   │   │   ├── client.rs
│   │   │   ├── streamer.rs              # Streamer session: LOGIN/SUBS/ADD, ACCT_ACTIVITY notifier, LEVELONE_EQUITIES, reconnect, dirty/gap signals
│   │   │   ├── quotes.rs                # quote cache, REST quote polling, 1-minute aggregation, quote_spread_1m/session=YYYY-MM-DD/data.parquet writer
│   │   │   ├── fractional.rs            # fractional_eligible() hook (config/fractional_eligible.toml + previewOrder results)
│   │   │   └── shadow.rs
│   │   ├── eventbus/
│   │   │   ├── mod.rs
│   │   │   ├── channels.rs
│   │   │   ├── events.rs
│   │   │   ├── dedup.rs
│   │   │   └── handlers.rs
│   │   ├── audit/
│   │   │   ├── mod.rs
│   │   │   ├── writer.rs                # SQLite journal, group commit ($QUANT_STATE/audit.db)
│   │   │   ├── spool.rs                 # ingests Julia-originated events ($QUANT_STATE/audit-spool/)
│   │   │   ├── export.rs                # nightly Parquet export of closed days (audit_log/date=YYYY-MM-DD/data.parquet)
│   │   │   └── types.rs
│   │   ├── risk/
│   │   │   ├── mod.rs
│   │   │   ├── pre_trade.rs             # restricted-list check first, then caps, tier
│   │   │   ├── restricted.rs            # loads config/restricted.toml; unreadable => reject all
│   │   │   └── reconciliation.rs        # daily position reconciliation (16:15); mismatch halts
│   │   ├── lots/                        # NEW: tax lot tracking
│   │   │   ├── mod.rs
│   │   │   ├── updater.rs               # FIFO matching
│   │   │   ├── types.rs                 # TaxLot struct
│   │   │   └── split.rs                 # partial-close logic
│   │   ├── inference/
│   │   │   ├── mod.rs
│   │   │   └── bosk_loader.rs           # reads models/registry.json + model files for parity tests and on-demand use; never writes gspqr_predictions
│   │   └── data/
│   │       ├── mod.rs
│   │       ├── parquet.rs
│   │       └── arrow_ipc.rs
│   ├── tests/
│   │   ├── lock_fixtures.rs             # consumes the 09 golden fixtures (09-lock-state-machine/fixtures/*.json)
│   │   ├── lock_rule.rs                 # property tests vs the fixtures/oracle, fail-closed, multi-account, tz/DST
│   │   ├── restricted.rs                # config/restricted.toml contains ICE; pre-trade rejects
│   │   ├── compliance_audit.rs          # violation => halt + CRITICAL + dated report
│   │   ├── execution_guards.rs
│   │   ├── event_bus.rs
│   │   ├── audit_journal.rs             # durability, triggers, spool, export
│   │   ├── export_roundtrip.rs
│   │   ├── lot_fifo.rs                  # NEW: FIFO correctness
│   │   └── lot_partial_close.rs         # NEW: split correctness
│   └── benches/
│       └── order_submit.rs
│
├── ingest/                              # Rust: data fetching over direct HTTP (no Python)
│   ├── Cargo.toml
│   ├── src/
│   │   ├── main.rs
│   │   ├── lib.rs
│   │   ├── backfill.rs                  # 15+ year resumable backfill driver (01 BACKFILL.md)
│   │   ├── manifest.rs                  # $QUANT_STATE/ingest/manifest.db (SQLite)
│   │   ├── landing.rs                   # immutable raw landing zone $QUANT_ROOT/raw/ (fetch timestamp + SHA-256)
│   │   ├── yahoo.rs                     # chart API: raw as-traded OHLCV, split and dividend events
│   │   ├── edgar.rs                     # bulk XBRL fundamentals, submissions
│   │   ├── fred.rs                      # FRED + ALFRED vintages
│   │   ├── ishares.rs                   # holdings by as-of date
│   │   ├── parse/
│   │   │   ├── mod.rs
│   │   │   ├── yahoo.rs
│   │   │   ├── edgar.rs
│   │   │   ├── fred.rs
│   │   │   └── ishares.rs
│   │   ├── gates.rs                     # quality gates written by Rust (01 QUALITY-GATES.md)
│   │   ├── holdings.rs                  # etf_holdings writer
│   │   ├── identifiers.rs               # security_master writer (uses the symbols crate)
│   │   ├── filing_events.rs             # 8-K items, Form 25/15
│   │   ├── delistings.rs                # EDGAR-derived delisting events
│   │   ├── rate_limit.rs
│   │   ├── retry.rs
│   │   ├── atomic_write.rs
│   │   └── quality_log.rs
│   └── tests/
│       ├── test_yahoo.rs
│       ├── test_edgar.rs
│       └── test_atomic.rs
│
├── frontend/                            # Bun + TypeScript
│   ├── package.json
│   ├── bun.lockb
│   ├── tsconfig.json
│   ├── src/
│   │   ├── index.ts
│   │   ├── api/
│   │   │   ├── routes.ts
│   │   │   ├── websocket.ts
│   │   │   └── schemas.ts
│   │   ├── db/
│   │   │   ├── duckdb.ts
│   │   │   └── queries.ts
│   │   ├── dashboard/
│   │   │   ├── app.tsx
│   │   │   ├── components/
│   │   │   │   ├── Portfolio.tsx
│   │   │   │   ├── Ledger.tsx           # NEW: NAV and P&L chart
│   │   │   │   ├── TaxLots.tsx          # NEW: lot viewer
│   │   │   │   ├── Predictions.tsx
│   │   │   │   ├── Regime.tsx
│   │   │   │   ├── Locks.tsx
│   │   │   │   └── Orders.tsx
│   │   │   └── charts/
│   │   │       ├── IntervalChart.tsx
│   │   │       ├── NavChart.tsx         # NEW: cumulative return
│   │   │       └── Performance.tsx
│   │   └── lib/
│   │       └── format.ts
│   └── tests/
│       └── api.test.ts
│
├── schemas/                             # Schema registry (the authoritative registry is this repository copy)
│   ├── registry.json
│   ├── Project.toml                     # Julia env for registry/validation tooling: DuckDB, Tables, JSON3, OrderedCollections, Parquet2, DataFrames
│   ├── Manifest.toml
│   ├── fixtures/
│   │   └── symbol_normalization.csv     # test vectors of the symbols crate (01 UNIVERSE.md §3)
│   └── definitions/
│       ├── panel.md
│       ├── portfolio_ledger.md          # NEW
│       ├── tax_lots.md                  # NEW
│       └── ...
│
├── migrations/                          # Schema change history
│   ├── 2026-10-10-initial.md            # includes the pre-first-write corrections (13-parquet-lake SKILL §1)
│   └── 2026-10-10-tax-lots-partial-close.md   # NEW
│
├── data/                                # Local dev data (gitignored)
│   └── .gitkeep
│
├── scripts/                             # Cross-language tooling (copies of the skill scripts; the skill copies are the specs)
│   ├── session-start.sh
│   ├── pre-commit-constitution.sh
│   ├── test-pre-commit-constitution.sh  # 00-construction/scripts: hook tests in a scratch repository
│   ├── verify-schemas.sh
│   ├── generate-registry.jl
│   ├── validate-filings.jl
│   ├── validate-panel.jl
│   ├── train-gspqr.jl
│   ├── calibrate.jl
│   ├── fit-tails.jl
│   ├── fit-regime.jl
│   ├── compute-overlay.jl
│   ├── optimize-portfolio.jl
│   ├── run-backtest.jl
│   ├── roundtrip-test.jl
│   ├── build-trade-summary.jl
│   ├── audit-lock-compliance.rs         # daily lock-compliance audit + report (sketch)
│   ├── lock-monitor.rs                  # pending-order timer (sketch)
│   ├── write-ledger.jl                  # NEW
│   └── update-lots.rs                   # NEW
│
├── docker/                              # Unraid deployment
│   ├── quant-core/
│   │   └── Dockerfile                   # Julia + Rust
│   ├── quant-api/
│   │   └── Dockerfile                   # Bun
│   ├── duckdb/
│   │   └── Dockerfile                   # read-only debug container
│   └── compose.yaml
│
├── config/                              # Runtime configuration (mounted read-only as $QUANT_CONFIG)
│   ├── default.toml
│   ├── production.toml                  # gitignored
│   ├── schwab.example.toml
│   ├── restricted.toml                  # tickers the system never orders (ICE); amendment 1.5
│   ├── fractional_eligible.toml         # owner-approved Stock Slices-eligible tickers (Bootstrap tier only trades these)
│   ├── symbol_map.toml                  # canonical symbol map (09 §3.8)
│   ├── nyse_calendar_overrides.toml     # seed of ad-hoc NYSE closures (2007-01-02, 2012-10-29, 2012-10-30, 2018-12-05, 2025-01-09); runtime copy maintained by calendar-sync (never derived from prices)
│   ├── quality_gates.toml               # ingestion quality-gate thresholds (01 QUALITY-GATES.md)
│   ├── macro_series.toml                # FRED/ALFRED series list (01 SOURCES.md)
│   ├── identifier_overrides.csv         # identifier and vendor-ticker alias overrides (01 UNIVERSE.md)
│   └── price_sources.toml               # optional: extra price sources for delisted names (01 SURVIVORSHIP.md)
│
├── .github/
│   └── workflows/
│       ├── ci.yml
│       ├── constitution.yml
│       └── backtest.yml
│
├── .gitignore
├── .editorconfig
├── README.md
└── CLAUDE.md
```

The skill-local Julia oracles of the lock (`09-lock-state-machine/scripts/lock_rule_oracle.jl`, `nyse_calendar_oracle.jl`, `run_fixtures.jl`) stay with their fixtures in the skill directory; they are the executable definition of the rule and are not copied to `scripts/`. There is no Python anywhere (the former `lock_rule_reference.py` was removed; the fixtures it generated are frozen goldens). Position reconciliation is Rust (`execution/src/risk/reconciliation.rs`); there is no Julia reconciliation script.

### Data and state trees (not in the repository)

```
$QUANT_ROOT/                             # container /data, host /mnt/cache/quant
├── lake/                                # the 34 datasets (13-parquet-lake SKILL §3), table below
├── raw/                                 # immutable landing zone: every raw response, fetch timestamp + SHA-256 (01 BACKFILL.md)
├── stage/                               # parsed per-chunk intermediates, rebuildable
├── reports/                             # coverage / caveat / gate reports; backtest run artifacts (reports/backtest/<run_id>/cpcv/split=<k>/)
└── models/                              # model artifacts written by Julia
    ├── registry.json                    # live generation + one entry per generation
    ├── backfill_manifest.json           # resumability of the generation backfill (03 PREDICTION-BACKFILL.md)
    ├── generations/<generation>/        # 21 model files per monthly generation
    └── regime/hmm_<fit_date>.json       # HMM parameters per walk-forward refit (06 HMM-PROCEDURE.md)

$QUANT_STATE/                            # container /state, host /mnt/cache/appdata/quant
├── audit.db                             # SQLite audit journal (system of record), + -wal / -shm
├── audit-spool/                         # Julia-originated events, ingested by Rust
├── ingest/manifest.db                   # ingestion manifest (Rust ingest only)
├── locks/<name>.lock                    # advisory single-writer lock files
├── schwab_refresh_token                 # Schwab OAuth refresh token, 0600 (Rust only; 10 OAUTH.md §3)
├── lock_external.csv
├── lock_snapshot.json
├── TRADING_HALTED
├── acknowledged_violations.json
├── nyse_calendar_overrides.toml         # runtime overrides (calendar-sync, append-only)
├── compliance/                          # dated lock-compliance reports
└── logs/
```

### Lake datasets (`$QUANT_ROOT/lake/`)

`S` = single file `<name>.parquet`; `P(keys)` = hive directory `<name>/<k>=<v>/.../data.parquet` (every leaf is exactly one `data.parquet`). The authoritative table is `13-parquet-lake/SKILL.md` §3.

| # | Dataset | Path | Writer |
|---|---|---|---|
| 1 | `universe` | `universe/year=YYYY/data.parquet` | Julia (universe builder) |
| 2 | `prices` (raw as-traded OHLCV) | `prices/year=YYYY/month=MM/data.parquet` | Rust (ingest) |
| 3 | `fundamentals` | `fundamentals.parquet` | Rust (ingest) |
| 4 | `macro` | `macro.parquet` | Rust (ingest) |
| 5 | `panel` | `panel/year=YYYY/month=MM/data.parquet` | Julia (features) |
| 6 | `regime_states` | `regime_states.parquet` | Julia (regime) |
| 7 | `gspqr_predictions` | `gspqr_predictions/year=YYYY/month=MM/data.parquet` | Julia (GSPQR; sole writer) |
| 8 | `tcp_calibration` | `tcp_calibration/year=YYYY/month=MM/data.parquet` | Julia (TCP) |
| 9 | `evt_residuals` | `evt_residuals/year=YYYY/month=MM/data.parquet` | Julia (EVT) |
| 10 | `evt_tail_bounds` | `evt_tail_bounds/year=YYYY/data.parquet` | Julia (EVT) |
| 11 | `conviction_scores` | `conviction_scores.parquet` | Julia (conviction) |
| 12 | `veto_flags` | `veto_flags.parquet` | Julia (conviction) |
| 13 | `portfolio_targets` | `portfolio_targets.parquet` | Julia (portfolio) |
| 14 | `position_locks` (derived view, not authoritative) | `position_locks.parquet` | Rust (lock monitor) |
| 15 | `pending_orders` | `pending_orders.parquet` | Rust (order manager) |
| 16 | `orders` | `orders.parquet` | Rust (execution) |
| 17 | `positions` | `positions.parquet` | Rust (execution) |
| 18 | `fills` | `fills.parquet` | Rust (execution) |
| 19 | `evaluation_metrics` | `evaluation_metrics.parquet` | Julia (backtest) |
| 20 | `data_quality` | `data_quality.parquet` | Rust (ingest) |
| 21 | `audit_log` | `audit_log/date=YYYY-MM-DD/data.parquet` | Rust (event bus; nightly export of `audit.db`) |
| 22 | `portfolio_ledger` | `portfolio_ledger/year=YYYY/data.parquet` | Julia (ledger) |
| 23 | `tax_lots` | `tax_lots.parquet` | Rust (execution) |
| 24 | `trade_summary` | `trade_summary/year=YYYY/data.parquet` | Julia (ledger) |
| 25 | `order_history` (evidence snapshot) | `order_history.parquet` | Rust (compliance audit) |
| 26 | `execution_costs` | `execution_costs/year=YYYY/data.parquet` | Rust (execution) |
| 27 | `quote_spread_1m` | `quote_spread_1m/session=YYYY-MM-DD/data.parquet` | Rust (quote recorder) |
| 28 | `splits` | `splits.parquet` | Rust (ingest) |
| 29 | `dividends` | `dividends.parquet` | Rust (ingest) |
| 30 | `etf_holdings` | `etf_holdings/year=YYYY/data.parquet` | Rust (ingest) |
| 31 | `security_master` | `security_master.parquet` | Rust (ingest) |
| 32 | `delistings` | `delistings.parquet` | Rust (ingest) |
| 33 | `filing_events` | `filing_events.parquet` | Rust (ingest) |
| 34 | `price_coverage` | `price_coverage.parquet` | Julia (universe builder) |

---

## What Changed From the Previous Scaffold

Five additions, all driven by the ledger skill:

| Addition | Location | Purpose |
|---|---|---|
| `ledger/` Julia module | Top-level | Daily ledger writes, drawdown, Sortino, YTD realized P&L |
| `execution/src/lots/` | Inside Rust execution crate | FIFO lot matching on fills |
| `Ledger.tsx` and `TaxLots.tsx` | `frontend/src/dashboard/components/` | Dashboard views for NAV and lot history |
| `NavChart.tsx` | `frontend/src/dashboard/charts/` | Cumulative return visualization |
| Two new migration files | `migrations/` | Initial and tax-lot-partial-close |

Plus the two test files (`lot_fifo.rs`, `lot_partial_close.rs`) and the two scripts (`write-ledger.jl`, `update-lots.rs`).

Later additions (2026-10-10 integration): the shared `symbols/` crate; the ingestion backfill modules (`backfill.rs`, `manifest.rs`, `landing.rs`, `parse/`, `gates.rs`, `holdings.rs`, `identifiers.rs`, `filing_events.rs`, `delistings.rs`); the Julia universe builder and price coverage (`features/src/universe.jl`, `coverage.jl`); the walk-forward model modules (`gspqr/backfill.jl`, `tcp/pooled.jl`, `evt/gpd_mle.jl`, `regime/src/online_viterbi.jl`); `execution/src/symbols.rs`, `locks/gate.rs`, `locks/halt.rs`; the `schemas/` Julia environment and symbol fixtures; the data and state trees and the 34-dataset table above.

---

## Language Boundaries (Updated)

| Directory | Language | Purpose |
|---|---|---|
| `models/`, `features/`, `portfolio/`, `regime/`, `conviction/`, `ledger/`, `backtest/` | Julia | Analytics, universe builder and price coverage, model training and daily inference, calibration, portfolio construction, ledger |
| `execution/` (incl. `execution/src/risk/`), `ingest/`, `symbols/` | Rust | Execution hot path, data ingestion, shared symbol normalization, event bus, risk checks, lot tracking |
| `frontend/` | Bun/TypeScript | Dashboard, API, visualization |
| `schemas/`, `migrations/`, `scripts/` | Mixed | Cross-language tooling and data contracts (`schemas/` has its own Julia environment) |
| `docker/` | YAML/Dockerfile | Unraid deployment |

The `ledger/` module is Julia because it computes metrics (drawdown, Sortino, cumulative return) and is not on the hot path. The `execution/src/lots/` module is Rust because lot matching happens on every fill, which is in the execution path. There is no Python anywhere (the pre-commit hook rejects `*.py`, `*.pyi`, `*.ipynb`).

---

## Build Order (Updated)

**Current phase: data collection** (Ingestion: 15+ years of every dataset from free sources, survivorship bias documented and quantified), then training GSPQR + TCP + EVT (Prediction). Execution and live trading come last and are out of scope for now (`outline.md`, "Project phase and build order"). The table below was reordered on 2026-10-10: the earlier version put Bridge and Execution before Portfolio, which contradicted the owner's stated order (portfolio/regime/conviction, ledger, frontend, then execution and live). The phases themselves are unchanged.

| Phase | Directories Created | Skills Implemented |
|---|---|---|
| **Foundation** | `.claude/skills/`, `schemas/`, `migrations/`, `scripts/` | `00`, `15`, `13` |
| **Ingestion** | `ingest/`, `symbols/` | `01` |
| **Panel** | `features/` (universe builder, price coverage, panel) | `02` |
| **Prediction** | `models/` | `03`, `04`, `05` |
| **Portfolio** | `portfolio/`, `regime/`, `conviction/` | `06`, `07`, `08`. Runs before Execution exists with neutral defaults: no lock snapshot and an empty book mean nothing is locked, zero cost vectors, no positions (`08-portfolio-optimization` SKILL §5f) |
| **Evaluation** | `backtest/` | `14` |
| **Ledger** | `ledger/`, `execution/src/lots/` | `16` |
| **Frontend** | `frontend/` | (dashboard only) |
| **Bridge** *(later; optional)* | `execution/src/inference/`, `execution/src/data/` | `12` (in-process FFI via RustCall.jl/jlrs is a later-phase option; the current exchange is Parquet) |
| **Execution** *(later; live trading)* | `execution/src/schwab/` (incl. `streamer.rs`, `quotes.rs`), `execution/src/locks/`, `execution/src/risk/`, `execution/src/eventbus/`, `execution/src/symbols.rs`, `config/restricted.toml`, `config/nyse_calendar_overrides.toml` | `10`, `11`, `09` |
| **Deployment** | `docker/`, `config/` | Unraid |

The ledger phase comes before the frontend because the dashboard displays ledger data. Before execution exists, the ledger is fed by simulated or imported fills (it normally consumes real fills, which arrive only in the later Execution phase); this is an open point for `16-portfolio-ledger`, not resolved here. Evaluation moved ahead of the ledger because the backtest needs only the portfolio layer.

---

## Files That Cross Boundaries (Updated)

These files cross boundaries and deserve explicit placement:

| File | Location | Purpose |
|---|---|---|
| `schemas/registry.json` | Top-level (repository) | Every writer reads it; CI verifies it |
| `models/registry.json` | `$QUANT_ROOT/models/` | Julia writes it; the Rust loader reads it for parity tests and on-demand use |
| `gspqr_predictions` | `$QUANT_ROOT/lake/` | Julia is the sole writer; Rust reads it |
| `schemas/fixtures/symbol_normalization.csv` | `schemas/fixtures/` | Test vectors shared by the `symbols` crate and every consumer |
| `09-lock-state-machine/fixtures/*.json` | Skill directory | Golden lock and calendar fixtures, read by the Julia oracles and `execution/tests/lock_fixtures.rs` |
| `execution/src/data/arrow_ipc.rs` | Rust | The Arrow schema shared with Julia |
| `execution/src/lots/types.rs` | Rust | The `TaxLot` struct, mirrored by Julia's `ledger/src/` readers |

The `TaxLot` struct definition must match the Arrow schema in `schemas/definitions/tax_lots.md`. Both sides read the schema before serializing or deserializing. A schema mismatch is a hard failure.

---

## First Files to Write

The order remains the same, with two additions at the end:

1. `schemas/registry.json` — initial empty registry.
2. `schemas/definitions/panel.md` — first schema, matches `13-parquet-lake/SCHEMAS.md` §5.
3. `schemas/definitions/portfolio_ledger.md` — ledger schema, matches `16-portfolio-ledger` §schema.
4. `schemas/definitions/tax_lots.md` — lot schema.
5. `scripts/pre-commit-constitution.sh`.
6. `scripts/session-start.sh`.
7. `migrations/2026-10-10-initial.md`.
8. `ingest/Cargo.toml` and `ingest/src/main.rs` — first production crate (with `symbols/` as its dependency).

---

## Lock and Restricted-Securities Files (amendments 1.4, 1.5)

| File | Purpose |
|---|---|
| `config/restricted.toml` | Tickers the system never orders (initial: `ICE`). Changes are commits approved by the account owner |
| `config/nyse_calendar_overrides.toml` | Seed of ad-hoc NYSE closures (2007-01-02, 2012-10-29, 2012-10-30, 2018-12-05, 2025-01-09), unioned with the computed rules and the runtime overrides written by `calendar-sync`; trading days are never derived from the `prices` dataset |
| `$QUANT_STATE/nyse_calendar_overrides.toml` | Runtime overrides: append-only, written by `calendar-sync` on first sighting, by the same-day signal and by the owner's `quant-exec calendar add` |
| `$QUANT_STATE/lock_external.csv` | Owner-maintained orders from accounts the API cannot see (columns `account,ticker,side,placed_date,status,filled_qty`); must exist, header-only is valid; not in the repo or the lake |
| `$QUANT_STATE/lock_snapshot.json` | Advisory derived snapshot (blocked side, first permitted day per ticker) written by Rust at about 16:50 ET (after the 16:40 audit) for the 17:00 optimizer, optionally rewritten at 09:00; Julia fails closed if missing or stale once there are holdings; never an input to `authorize` |
| `$QUANT_STATE/TRADING_HALTED` | Halt flag, written only by the Rust halt module (`execution/src/locks/halt.rs`); present, unreadable or unparseable halts every order |
| `$QUANT_STATE/acknowledged_violations.json` | Append-only owner acknowledgements of reported violations (reported, not re-halted) |
| `$QUANT_STATE/compliance/` | Dated daily lock-compliance reports |
| `$QUANT_ROOT/lake/order_history.parquet` | Append-only evidence snapshot of the audited history (`13-parquet-lake` #25) |
| `$QUANT_ROOT/lake/quote_spread_1m/session=YYYY-MM-DD/data.parquet` | 1-minute quoted-spread history built forward by the quote recorder (`13-parquet-lake` #27; `10-execution-schwab/EXECUTION-WINDOW.md` §2.1) |
| `config/fractional_eligible.toml` | Owner-approved fractional-eligible tickers (Stock Slices: S&P 500 stocks); `Unknown` is not tradable in Bootstrap |
| `$QUANT_ROOT/lake/execution_costs/year=YYYY/data.parquet` | Per-name execution cost estimates written after each session, read by the 17:00 optimizer (`13-parquet-lake` #26) |
