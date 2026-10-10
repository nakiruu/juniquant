# Metrics

The bps metrics in the trade summary, their formulas, and their edge
cases.

## 1. Gross Return Bps

    gross_return_bps = (exit_price − entry_price) / entry_price × 10000

The raw return of the trade, before cost and benchmark. For a long-only
system, this is the only direction the trade can go.

Edge cases:
- If `entry_price == 0`, the metric is undefined. This cannot happen in
  practice; the entry price is a positive fill.
- If `exit_price == entry_price`, the metric is 0.

## 2. Slippage Bps

Slippage is taken from `orders.parquet.slippage_bps` for each fill in the
round-trip.

For a trade with one entry fill and one exit fill:

    slippage_bps = entry_slippage_bps + exit_slippage_bps

For a trade with multiple fills of equal size:

    slippage_bps = mean(entry_slippages) + mean(exit_slippages)

For a trade with multiple fills of unequal size, quantity-weighted:

    entry_slippage_bps = sum(qty_i × slip_i) / sum(qty_i)
    exit_slippage_bps  = sum(qty_j × slip_j) / sum(qty_j)
    slippage_bps = entry_slippage_bps + exit_slippage_bps

The quantity-weighted formula is the correct one for unequal fills. The
simpler formulas are approximations and should not be used when fills are
unequal.

**Which slippage column.** `orders.slippage_bps` is measured against the
mid at that order's *own* submission. For a replaced or laddered order that
hides drift accumulated while the ladder worked, so when
`orders.slippage_vs_arrival_bps` is present (arrival mid of the whole
sequence, `10-execution-schwab/EXECUTION-WINDOW.md` §10.1) it is the
benchmark and is used instead.

**Sign convention:** positive slippage means the fill was worse than the
mid-price at submission. A buy at the ask has positive slippage of half
the spread. A buy at the bid has negative slippage of half the spread
(favorable). The sign is preserved in the sum.

## 3. Net Return Bps

    net_return_bps = gross_return_bps − slippage_bps

The trade's return after execution cost.

Note that `slippage_bps` is a percentage of the price at each fill, and
`gross_return_bps` is a percentage of the entry price. The two are not
perfectly commensurable if entry and exit prices differ significantly.
For short holding periods and small price moves, the approximation is
close. For long holding periods and large moves, the slippage should be
scaled to entry price:

    slippage_bps_adjusted = slippage_bps × (fill_price / entry_price)

This adjustment is not applied by default. It is documented here so that
anyone inspecting the metric knows the approximation.

## 4. Benchmark Return Bps

    benchmark_return_bps = (universe_value[exit_date] / universe_value[entry_date] − 1) × 10000

Where `universe_value[t]` is the equal-weighted universe index on date `t`,
computed as the cumulative product of daily equal-weighted returns from
`prices.parquet`.

The index is recomputed daily in `06-regime-detection` and stored in
`regime_states.parquet` if not separately. If not available, compute it
from `prices.parquet` directly.

## 5. Alpha Bps

    alpha_bps = net_return_bps − benchmark_return_bps

The trade's return above what the universe delivered. This is the metric
that answers "was this a good trade?"

A positive `alpha_bps` means the trade beat the universe after costs.
A negative one means it did not. The mean across trades is the strategy's
per-trade alpha.

## 6. Holding Days

    holding_days = exit_date − entry_date (in trading days)

Trading days, not calendar days. A Friday-to-Monday holding period is 1
trading day, not 3.

## 7. Return Per Day Bps

    return_per_day_bps = net_return_bps / holding_days

Makes short and long holds comparable. A trade that returns 500 bps over
5 days (100 bps/day) is different from a trade that returns 500 bps over
100 days (5 bps/day), even though both have the same total return.

## 8. Trade Sortino

    downside_deviation_bps = sqrt(mean(min(0, r_t)²)) × 10000

Where `r_t` is the daily return of the ticker over the holding period,
below the risk-free rate. The trailing window is the holding period.

    trade_sortino = net_return_bps / downside_deviation_bps

This is the trade-level analogue of the strategy-level Sortino. It rewards
trades with positive return per unit of downside risk.

If `downside_deviation_bps == 0` (no down days in the holding period),
`trade_sortino` is undefined. Store it as null.

## 9. Reporting

The trade summary is not itself a report. It is a table. Reports are built
from it by grouping:

- **By regime at entry:** mean `alpha_bps` for `low_vol`, `high_vol`, `crisis`.
- **By conviction bucket:** mean `alpha_bps` for conviction [0.50, 0.75), [0.75, 1.00), [1.00, 1.25], (1.25, ...].
- **By holding period:** mean `alpha_bps` for 5–20 days, 20–60 days, 60–120 days, 120+ days.
- **By tier:** mean `slippage_bps` for Bootstrap, Transition, Standard.
- **By exit reason:** mean `alpha_bps` for normal exits versus veto-driven exits (the exit was a sale of a name that had an active veto; derived from `veto_flags`, not from `lock_overridden`).

These groupings are the diagnostic. A strategy with positive overall alpha
but negative alpha in the `high_vol` regime has a regime-conditional
problem. A strategy with positive alpha in the Standard tier and negative
alpha in the Bootstrap tier has an execution-cost problem. The trade
summary makes these visible.

## 10. What Not to Do

- Do not report a mean `alpha_bps` without the distribution. A mean of
  50 bps with a standard deviation of 400 bps is not the same as a mean
  of 50 bps with a standard deviation of 20 bps.
- Do not compare `alpha_bps` across different benchmarks. The benchmark
  is fixed at the equal-weighted universe.
- Do not use `gross_return_bps` as the trade's quality. It ignores cost
  and benchmark.
- Do not omit the quantity weighting for slippage on unequal fills.
- Do not compute `trade_sortino` when the downside deviation is zero.
  Store null.