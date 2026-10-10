# Evaluation Metrics

The metrics reported by the backtest protocol, their definitions, and
their interpretation. Each metric comes from one track: interval, tail and
TCP-inactivity metrics from the walk-forward (WF) track, portfolio metrics
from CPCV, raw-quantile metrics from both (`SKILL.md` §2a). Every report
opens with the survivorship and source header (`SKILL.md` §6h).

## 1. Coverage

    coverage_95 = mean(1{y_t ∈ [q_hat_lower_t, q_hat_upper_t]})
    coverage_90 = mean(1{y_t ∈ [q_05_t, q_95_t]})                (raw base interval)

The fraction of observations where the realized return falls within the
TCP-calibrated interval (95% target) and within the raw GSPQR interval (90%
nominal). `coverage_95` is a WF-track metric.

**Target:** 0.95 ± 0.02 for the calibrated interval; 0.90 for the raw one.
Deviation beyond ±2 percentage points indicates miscalibration. Report with
the interval width (`avg_width`) and by calendar year, regime and raw-width
tercile, with HAC standard errors (lag >= H), since overlapping labels make
the observations dependent.

## 2. Winkler Score

    Winkler = width + (2/α)·(lower − y)·1{y < lower} + (2/α)·(y − upper)·1{y > upper}

Where `width = upper − lower` and `α = 0.05` (the interval is the 95%
calibrated one). Lower is better. The Winkler score rewards narrow intervals
and penalizes violations. WF-track metric.

## 3. Pinball Loss

    pinball(y, q̂_τ, τ) = τ·(y − q̂_τ) if y ≥ q̂_τ
                        (1−τ)·(q̂_τ − y) if y < q̂_τ

Applied to the **raw** GSPQR quantile `q_τ` for each τ ∈ {0.01, 0.05, 0.25,
0.50, 0.75, 0.95, 0.99}, reported per τ and averaged. Lower is better.
Measures quantile forecast accuracy.

It is **not** applied to `q_hat_lower` / `q_hat_upper`: those are the
endpoints of a 95%-coverage interval, not the 5% and 95% quantiles, so the
pinball loss at τ = 0.05 / 0.95 scores them against the wrong target; their
metrics are coverage and Winkler. It is also never applied to the midpoint of
the TCP interval as if it were the median: the median forecast is `q_50`
(constitution §9). The EVT bounds `q_extreme` are scored by the exceedance
rate at 1% / 99% (WF track), not by pinball, unless a pinball at τ = 0.01 / 0.99
on `q_extreme` is stated explicitly.

## 4. CRPS

Continuous Ranked Probability Score. Measures the distance between the
predicted distribution and the realized value. Lower is better. CRPS
generalizes pinball loss to the full distribution:
`CRPS = 2 ∫_0^1 pinball_τ dτ`. With only the seven stored quantiles it is
**approximated** by `2 Σ_k w_k pinball_{τ_k}` with trapezoid weights `w_k`
over the grid {0.01, 0.05, 0.25, 0.50, 0.75, 0.95, 0.99}; the region outside
[0.01, 0.99] is ignored (the rule and this limitation are an approximation to
verify, and the number is labelled "approximate CRPS").

## 5. Sortino Ratio

    Sortino = (mean(R_p) − R_target) / σ_downside

Where `σ_downside` is the lower semi-deviation below `R_target`. Higher
is better. This is the primary return metric because the optimizer
maximizes it.

## 6. Max Drawdown

    MDD = max_t (peak_t − trough_t) / peak_t

The worst peak-to-trough decline. Lower is better. Reported as a
negative percentage.

## 7. Sharpe Ratio

    Sharpe = (mean(R_p) − R_f) / std(R_p)

Higher is better. Reported alongside the Deflated Sharpe Ratio when the
trial count `N` is known.

## 7a. Implementation Shortfall

Per order, against the **arrival mid** (`10-execution-schwab/EXECUTION-WINDOW.md`
§10.1), positive is worse:

    IS_bps = dir × (avg_fill − arrival_mid) / arrival_mid × 10000,   dir = +1 buy, −1 sell

Quantity-weighted over the fills of an order (not a simple average of
unequal fills). Unfilled quantity carries an opportunity cost
`dir × (close_of_session − arrival_mid) / arrival_mid × 10000`; the
fill-rate-aware shortfall is
`fill_rate × IS_bps + (1 − fill_rate) × opp_cost_bps`.

Portfolio level: total shortfall cost in bps of average NAV,
`Σ_i notional_i × IS_i / mean(NAV)`, per year (`× 252 / n_days`), reported
per tier, per ladder arm, per spread bucket and per spread source
(`measured_1m` vs `proxy`, §8). Secondary benchmark:
interval VWAP.

In the backtest the arrival mid is a proxy (`SKILL.md` §6e): the *spread*
part of the shortfall is modelled and meaningful; the *drift* part is set
by the proxy and is not a measurement. Live, both parts are measured
(`orders.slippage_vs_arrival_bps`).

## 7b. Turnover

    turnover = ( Σ_t Σ_i |Δ notional_{i,t}| / 2 ) / mean(NAV) × 252 / n_days

Annualized one-way turnover, with the number of **counted orders per name
per year** alongside it (every counted order opens a lock window, so the
count is the lock exposure). Reported with the no-trade band on and off to
show what the band removes.

## 7c. Lock-Binding Frequency

Fraction of wanted trades that were blocked by the lock. A second solve
without the lock bounds (a diagnostic, backtest and live) gives the
lock-free weights `w_free`. A **wanted trade** is a name-date with
`|w_free − w_cur| > τ_i` (the same no-trade band). It is **blocked** if the
lock status forbids its direction (a sell on a name with a counted buy, a
buy on a name with a counted sell).

    LBF   = #blocked wanted trades / #wanted trades
    LBF_w = Σ |w_free − w_locked| / Σ |w_free − w_cur|     (notional-weighted)

where `w_locked` is the lock-aware target. Both are distributions across
CPCV paths. LBF is a *frequency*; the cost of those blocks is the return
difference of the "cost of the lock" experiment (`SKILL.md` §6f), not a
number derived from LBF.

## 7d. Spread-Cost Attribution

For each fill, with `mid_fill` the mid at the fill time:

    slippage = dir × (fill − arrival_mid)
             = dir × (fill − mid_fill)          spread / crossing component (negative if a passive fill earns it)
             + dir × (mid_fill − arrival_mid)   drift while working (adverse selection)

Aggregate by tier, ladder arm and spread bucket as bps of average NAV per
year, next to gross alpha, to show how much of the gross return the spread
consumes. The opportunity cost of unfilled quantity is reported as its own
line. Trades skipped by the no-trade band are not a cost line: the band is
a choice evaluated by net return. In the backtest only the spread component is
modelled; the drift component is proxy-dependent (§7a).

## 8. Reporting Format

**Split by spread source.** Every cost-dependent metric (net return,
Sortino, max drawdown, implementation shortfall, turnover cost, spread-cost
attribution) is reported for the whole backtest and **split by
`spread_source`**: `measured_1m` (fills whose spread comes from
`quote_spread_1m.parquet`, which exists only from the recording start date)
and `proxy` (OHLC-proxy / spread-bucket estimate, labelled `proxy spread`,
for all earlier periods). The share of fills and of notional in each source
is printed next to the table. A split is by fill, so a path that straddles
the recording start date contributes to both rows; the per-source rows are
cost attributions, not separate return series. Measured rows exist only in
paths whose test blocks overlap the recorded period, and may be empty.

CPCV-track metrics are reported as a **distribution** across CPCV paths
(the table shows the layout; WF-track metrics have no path distribution and are
reported by year and regime):

| Metric (CPCV track) | Mean | Std | 5th pctile | 50th pctile | 95th pctile |
|---|---|---|---|---|---|
| Coverage of the raw 90% interval | 0.897 | 0.012 | 0.878 | 0.898 | 0.915 |
| Pinball (raw `q_τ`, mean over τ) | 0.041 | 0.005 | 0.034 | 0.041 | 0.049 |
| Rank IC of `q_50` | 0.03 | 0.01 | 0.01 | 0.03 | 0.05 |
| Sortino | 1.12 | 0.24 | 0.78 | 1.10 | 1.48 |
| MDD | −0.18 | 0.04 | −0.24 | −0.17 | −0.12 |

WF-track layout (single path, by calendar year and regime, HAC standard
errors with lag >= H; no path distribution):

| Metric (WF track) | Year | Horizon | Value | HAC s.e. |
|---|---|---|---|---|
| Coverage of the calibrated 95% interval | 2021 | 63d | 0.95 | 0.01 |
| Winkler (alpha = 0.05) | 2021 | 63d | 0.51 | 0.04 |
| `qhat_zero_frac` | 2021 | 63d | 0.40 | n/a |

The numbers in the tables above are layout illustrations, not results.
The header of `SKILL.md` §6h (survivorship caveat, data window, track,
spread-source split) precedes every such table.
Implementation shortfall, turnover, lock-binding frequency and the
spread-cost components (§7a-7d) are reported in the same format, per
CPCV path, next to Sortino, and the "cost of the lock" differences
(`SKILL.md` §6f) as distributions of per-path differences.

A single mean without the distribution is not reported for CPCV-track
metrics. The distribution is the result.

## 9. What Not to Do

- Do not report a single Sharpe ratio from a single path.
- Do not report coverage without the interval width. Wide intervals
  trivially achieve coverage.
- Do not report TCP/EVT interval metrics as a CPCV path distribution; they
  are single-path WF-track numbers.
- Do not score `q_hat_lower` / `q_hat_upper` with pinball, and do not
  use the TCP midpoint as the median forecast.
- Do not publish any table without the survivorship and source header
  (`SKILL.md` §6h).
- Do not compare metrics across different CPCV configurations. The
  parameters are fixed.
- Do not report the DSR without the trial count `N`.
- Do not report shortfall as a single average over tiers; the cost
  structures differ (`10-execution-schwab/SHADOW-ENGINE.md` §6).
- Do not read the drift part of a backtest shortfall as a measurement.
- Do not report cost-dependent metrics without the `measured_1m` / `proxy`
  split, and do not describe a `proxy spread` result as a measured cost.