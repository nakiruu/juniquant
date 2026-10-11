# Architecture

Overview of how the system fits together. Details live in the skills
([`../.claude/skills/`](../.claude/skills/)); this page links to them rather than repeating them.

This page describes the target design. Which stages exist today (the lake, symbols, calendar
and the ingest infrastructure; everything after ingestion is still a specification) is in
[`status.md`](status.md).

## Pipeline

```
 EODHD / EDGAR / FRED+ALFRED / iShares
        │  Rust ingest (direct HTTP, raw landing zone, resumable manifest)      01
        ▼
 Parquet lake ($QUANT_ROOT/lake, 36 datasets, schema registry)                13
        │  Julia: universe (point-in-time), coverage, total-return index, panel  02
        ▼
 GSPQR: 21 LightGBM quantile models, monthly walk-forward generations           03
        │  TCP: conformal calibration → 95%-coverage interval                    04
        │  EVT: generalized Pareto tails → 1% / 99% bounds                       05
        ▼
 Regime gate (filtered HMM)  ·  Veto + conviction (fundamentals)                06, 07
        ▼
 Portfolio: Sortino-maximising SOCP, 5% name / 25% sector caps, 2% cash         08
        ▼
 (later) Rust execution: lock check, tier rules, Schwab orders, audit journal    09-11
        ▼
 Ledger and tax lots · trade summary · dashboard                                16-18
```

The numbers are skill directories. Backtesting (14) replays the same pipeline over history on
two tracks: a walk-forward track (the only source of interval and tail metrics) and CPCV for
the raw model and the portfolio.

## Language boundaries

| Layer | Language |
|---|---|
| Ingestion, execution, lock, risk, audit journal, shared crates (`symbols`, `calendar`, `lake`) | Rust |
| Universe, panel, models, calibration, tails, regime, conviction, portfolio, ledger, backtest | Julia |
| Dashboard and API | Bun / TypeScript |
| Storage | Parquet + DuckDB |

Julia and Rust exchange data through the lake (Parquet). Julia is the sole writer of
`gspqr_predictions`; Rust reads it. In-process FFI is a later-phase option (skill 12).
No Python anywhere (constitution §4).

## Data and state

- `$QUANT_ROOT` (container `/data`): the lake, the raw landing zone, staging, reports, model
  artifacts.
- `$QUANT_STATE` (container `/state`): mutable service state: manifest, audit journal, halt
  flag, lock snapshot, compliance reports, tokens.
- `$QUANT_CONFIG` (container `/config`, read-only): the repository's `config/`.

Defined once in [`13-parquet-lake/IO-CONTRACT.md`](../.claude/skills/13-parquet-lake/IO-CONTRACT.md).

## Daily schedule (later, when live)

Decisions are made after the close; orders are placed in the next session between 10:00 and
15:30 ET. The full schedule (prices 16:10, reconciliation 16:15, tax lots 16:20, trade summary
16:22, ledger 16:25, inference 16:30, compliance audit 16:40, lock snapshot 16:50,
optimization 17:00, audit export and backup 18:00) is in [`../outline.md`](../outline.md).

## Deployment

Unraid server, Docker containers: `quant-core` (Julia + Rust, the only writer of the lake),
`quant-api` (Bun, read-only) and an optional DuckDB debug container
([`13-parquet-lake/UNRAID-STORAGE.md`](../.claude/skills/13-parquet-lake/UNRAID-STORAGE.md)).
