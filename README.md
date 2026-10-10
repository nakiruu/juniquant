# juniquant

A small-universe quant trading system for one owner's Schwab account. It forecasts
probabilistic price ranges for US growth stocks over 3-12 month horizons, builds a
downside-risk-aware portfolio, and (in a later phase) places orders under a contractual
5-day trading lock.

> **Status: phase I0 (repository foundation).** The system is specified but not built.
> The current goal is to collect 15+ years of data and train the forecasting models.
> Live trading comes last. See [`docs/INFRASTRUCTURE-PLAN.md`](docs/INFRASTRUCTURE-PLAN.md).

## What it does

1. **Data:** daily prices and corporate actions (Yahoo), fundamentals and filing events
   (SEC EDGAR), macro series with vintages (FRED/ALFRED) and historical index holdings
   (iShares IWF/IWP). Free sources only; the survivorship bias this causes is measured and
   reported with every result.
2. **Forecasts:** GSPQR (21 LightGBM quantile models), calibrated by TCP (conformal
   prediction) and extended into the tails by EVT.
3. **Portfolio:** a regime gate, a fundamental veto and conviction overlay, and a
   Sortino-maximising optimizer with position, sector and cash limits.
4. **Execution (later):** Schwab orders that obey the 5-day lock and the restricted list.

## Rules that never bend

- **The 5-day lock is a contractual obligation.** No opposite-side order within 5
  trading days, counting the day the order was placed (buy Monday, earliest sell the
  following Monday), across every account. It fails closed. See
  [`docs/compliance.md`](docs/compliance.md).
- **ICE is never traded**, in either direction (`config/restricted.toml`).
- **No lookahead:** every data point is used only from the date it was published.
- **No Python** in this repository: Julia for analytics, Rust for ingestion and
  execution, Bun/TypeScript for the dashboard.

These come from the constitution, [`.claude/skills/00-construction/SKILL.md`](.claude/skills/00-construction/SKILL.md),
which wins over every other document.

## Quick start

Requirements: Julia 1.12, Rust 1.98 with `rustfmt` and `clippy`, GNU make. Later phases
also need the DuckDB CLI and Bun. Details in [`docs/development.md`](docs/development.md).

```bash
make hook-install   # constitution pre-commit hook
make check          # every gate: Rust, Julia, fixtures, docs, hook tests
bash .claude/skills/15-ai-dev-workflow/scripts/session-start.sh   # environment check
```

## Repository layout

| Path | What |
|---|---|
| `.claude/skills/` | The specification: 19 skills, one per layer, plus the constitution |
| `docs/` | Readable overview: architecture, data, compliance, development, plans |
| `config/` | Restricted list, NYSE calendar overrides, quality gates, symbol map |
| `symbols/`, `calendar/`, `lake/`, `ingest/` | Rust crates (skeletons until phase I1/I2) |
| `tools/` | Julia repository checks (markdown, syntax) |
| `outline.md`, `scaffold.md` | Functional outline and target directory layout |
| `DECISIONS.md` | The owner's decisions and their status |

Data never lives in the repository: it goes under `$QUANT_ROOT` and `$QUANT_STATE`
([`13-parquet-lake/IO-CONTRACT.md`](.claude/skills/13-parquet-lake/IO-CONTRACT.md)).

## Documentation

| Document | For |
|---|---|
| [`docs/architecture.md`](docs/architecture.md) | How the pieces fit together |
| [`docs/data.md`](docs/data.md) | Sources, the backfill, survivorship, quality gates |
| [`docs/compliance.md`](docs/compliance.md) | The lock and the restricted list in plain language |
| [`docs/development.md`](docs/development.md) | Toolchains, commands, conventions |
| [`docs/INFRASTRUCTURE-PLAN.md`](docs/INFRASTRUCTURE-PLAN.md) | Build phases and how the work is split |
| [`.claude/skills/`](.claude/skills/) | The normative spec for implementers |
