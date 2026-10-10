---
name: trade-summary
description: Per-trade diagnostic in basis points. Load when touching
  trade_summary, alpha_bps, trade-level Sortino, or any code that scores
  completed round-trips.
---

# Trade Summary

The trade summary answers one question: did this trade beat holding the
benchmark, after execution costs? It produces one row per closed
round-trip, in basis points, with the slippage and the benchmark return
subtracted.

It is not a strategy metric. `evaluation_metrics.parquet` and
`portfolio_ledger.parquet` handle strategy-level measurement. This layer
is between them — it tells you *why* the strategy worked, not whether it
did.

## 1. What the Trade Summary Contains

One file: `trade_summary.parquet`. One row per closed round-trip.

| Column | Type | Meaning |
|---|---|---|
| `trade_id` | `utf8` | UUID |
| `ticker` | `utf8` | |
| `entry_date` | `date32` | Earliest open date in the closed lots |
| `exit_date` | `date32` | Latest close date in the closed lots |
| `entry_price` | `float64` | Weighted-average open price |
| `exit_price` | `float64` | Weighted-average close price |
| `quantity` | `float64` | Total closed shares (fractional permitted) |
| `gross_return_bps` | `float64` | `(exit − entry) / entry × 10000` |
| `slippage_bps` | `float64` | Sum of entry and exit slippage from `orders.parquet` |
| `net_return_bps` | `float64` | `gross_return_bps − slippage_bps` |
| `benchmark_return_bps` | `float64` | Universe return over the same period |
| `alpha_bps` | `float64` | `net_return_bps − benchmark_return_bps` |
| `holding_days` | `int32` | `exit_date − entry_date` in trading days |
| `return_per_day_bps` | `float64` | `net_return_bps / holding_days` |
| `downside_deviation_bps` | `float64` | Trailing downside over the holding period |
| `trade_sortino` | `float64` | `net_return_bps / downside_deviation_bps` |
| `regime_at_entry` | `utf8` | From `regime_states.parquet` |
| `regime_at_exit` | `utf8` | From `regime_states.parquet` |
| `conviction_at_entry` | `float64` | From `conviction_scores.parquet` |
| `tier_at_entry` | `utf8` | `Bootstrap` / `Transition` / `Standard`, from `orders.tier` of the earliest entry order |
| `lock_overridden` | `bool` | Reserved; constant `false`. No lock override exists (amendment 1.4); the column is kept for schema stability until removed by migration |

Twenty columns. The trade is fully described by these.

## 2. The Three Bps Metrics

There are three bps numbers, and they are not interchangeable:

- **`gross_return_bps`** — the raw return of the trade. Ignores cost and benchmark.
- **`net_return_bps`** — gross minus execution slippage. The trade's own return after cost.
- **`alpha_bps`** — net minus benchmark. The trade's contribution above what the universe delivered.

`alpha_bps` is the number that answers "was this a good trade?" A positive
mean `alpha_bps` across trades means the prediction engine is adding value
beyond the universe. A negative one means it is not.

## 3. Trade Definition

See `TRADE-DEFINITION.md`. A "trade" is a **closed round-trip**, not a fill
and not a lot. It is the aggregation of all lots opened at entry and all
lots closed at exit.

The matching rule is FIFO, consistent with `16-portfolio-ledger/TAX-LOTS.md`.
When a position is fully closed, all lots belonging to that round-trip are
grouped and a single trade row is written. Partial exits produce partial
trades: the closed portion is one trade, the remaining open portion is not.

## 4. Benchmark

The benchmark is the **equal-weighted universe return** over the trade's
holding period, sourced from `prices.parquet`. This matches the universe
used by the regime HMM (see `06-regime-detection`).

Using the universe as the benchmark, rather than the S&P 500 or the ETF
itself, means `alpha_bps` measures the trade against the actual opportunity
set. If the trade is AAPL and the universe returned 3%, the trade needs to
beat 3% to have positive alpha.

**Alternative benchmarks are not used.** SPY is not used because it
excludes the mid-cap and small-cap names in the universe. The ETF itself
(IWF, IWP) is not used because the trade may be a name that is in neither.
The equal-weighted universe is the correct benchmark.

## 5. Slippage Attribution

Slippage is taken from `orders.parquet.slippage_bps` (vs the mid at the
order's own submission), or from `slippage_vs_arrival_bps` when present,
which is the benchmark for replaced or laddered orders
(`10-execution-schwab/EXECUTION-WINDOW.md` §10.1). For a trade with
multiple entry fills and multiple exit fills, the slippage is:

    slippage_bps = sum(entry fill slippage) + sum(exit fill slippage)

This assumes equal weighting across fills. If the fills are unequal in
size, the slippage should be quantity-weighted. See `METRICS.md` §3 for
the weighted formula.

Slippage is positive when the fill was worse than mid, negative when
better. A limit order that fills at the bid on a buy has negative slippage
relative to mid. A market order that fills at the ask has positive
slippage of half the spread.

## 6. Regime and Conviction Context

The trade row carries `regime_at_entry`, `regime_at_exit`, and
`conviction_at_entry`. These are context, not metrics. They allow
grouping:

- Trades entered in `low_vol` versus `high_vol` regime.
- Trades entered with conviction above 1.10 versus below 0.90.
- Trades entered in the Bootstrap tier versus the Standard tier.

These groupings are how the diagnostic becomes useful. The raw
`alpha_bps` distribution tells you whether the engine works. The grouped
distribution tells you *where* it works.

## 7. Write Protocol

The trade summary is written after the tax lots are updated, on the day
a round-trip closes. If a round-trip closes on 2024-06-20, the trade row
for that round-trip is written on 2024-06-20, after the lot update and
before the ledger write.

Order of the daily sequence:

    16:15  Reconcile positions
    16:20  Update tax lots
    16:22  Build trade summary rows for round-trips closed today
    16:25  Write portfolio_ledger row
    16:30  Model inference
    ...

The trade summary sits between the lot update and the ledger write. It
depends on the lots being current and is a prerequisite for the ledger's
realized P&L figures.

## 7a. The Script

`scripts/build-trade-summary.jl` is the SKETCH of the builder described above
(not compiled, not normative; `ledger/src/` is its real home). It builds the
equal-weighted universe index from per-ticker simple returns, groups the lots
closed today by ticker, and appends the rows. `downside_deviation_bps` and
`trade_sortino` are left as placeholders in it.

## 8. What Not to Do

- Do not write a trade row for a partial exit as if it were a full trade.
  Partial exits produce partial trades; the remaining open portion is not
  a trade.
- Do not use SPY or IWF as the benchmark. Use the equal-weighted universe.
- Do not compute slippage as an average across unequal fills. Use the
  quantity-weighted formula (see `METRICS.md` §3).
- Do not omit `regime_at_entry` and `conviction_at_entry`. They are the
  reason the table exists.
- Do not recompute `realized_pnl` from `tax_lots.parquet`. The lot table
  is the source of truth for dollars; the trade summary is the source of
  truth for basis points.
- Do not write a trade row when only the entry has occurred. A trade
  requires an exit.
- Do not include open positions in the alpha aggregation. Only closed
  trades have measurable alpha.

## 9. Verification

- [ ] A test asserts one row per closed round-trip
- [ ] A test asserts `net_return_bps = gross_return_bps − slippage_bps`
- [ ] A test asserts `alpha_bps = net_return_bps − benchmark_return_bps`
- [ ] A test asserts slippage is quantity-weighted for unequal fills
- [ ] A test asserts partial exits produce partial trades
- [ ] A test asserts the benchmark is the equal-weighted universe
- [ ] A test asserts the trade row is written after lot update and before
      ledger write

## 10. Related Skills

- `16-portfolio-ledger` — the FIFO lot matching that defines round-trips
- `10-execution-schwab` — the `slippage_bps` source
- `06-regime-detection` — the `regime_at_entry` source
- `07-fundamental-overlay` — the `conviction_at_entry` source
- `13-parquet-lake` — schema for `trade_summary.parquet`
- `14-backtest-protocol` — the simulated equivalent