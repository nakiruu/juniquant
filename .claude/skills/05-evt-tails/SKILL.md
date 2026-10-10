---
name: evt-tails
description: Extreme Value Theory tail extension for calibrated residuals.
  Load when touching models/src/evt/, GPD fitting, tail quantiles, or extreme bounds.
---

# EVT Tail Extension

TCP produces a calibrated interval with 95% target coverage (about the
2.5% / 97.5% quantiles; the raw GSPQR `[q_05, q_95]` is its 90% base). EVT
extends the tails beyond it to 1%/99% using Extreme Value Theory. This skill documents the Peaks-Over-Threshold
(POT) procedure, the Generalized Pareto Distribution (GPD) fit, and the
extreme quantile extrapolation.

## 1. What EVT Produces

For each ticker, fit date, and horizon, per tail:

| Output | Meaning |
|---|---|
| `gpd_shape_xi` | GPD shape parameter (tail heaviness); pooled, same for all tickers |
| `gpd_scale_sigma` | GPD scale parameter; pooled, in standardized units |
| `q_extreme` | The 1st or 99th percentile bound, in forward-return units, mapped to the ticker |

The `evt_tail_bounds.parquet` schema (see `13-parquet-lake/SCHEMAS.md`
#10) stores these with one row per `(ticker, fit_date, horizon, tail)`.
The GPD is fitted once per `(fit_date, horizon, tail)` on residuals pooled
across tickers; only `q_extreme` differs by ticker. Early in the history a
horizon has a partial window, a pooled-empirical fallback, or no row at all
(`EVT-PROCEDURE.md` §8); the timeline is in
`03-gspqr-model/PREDICTION-BACKFILL.md` §8.

## 2. Why EVT Over Just Using More Quantiles

You could ask GSPQR for the 1st and 99th percentiles directly. LightGBM
supports any quantile level. Why add EVT?

**Reason 1: Data scarcity.** The 1st percentile is defined by 1% of the
sample. The GPD fit uses **all exceedances above the 95th percentile**,
5x more points, and estimates the tail shape from the exceedance
distribution rather than from the extreme quantile alone. (A single
ticker still has too few: see §3.)

**Reason 2: Extrapolation.** EVT provides a principled way to extrapolate
beyond observed quantiles. The GPD quantile formula extrapolates into the
tail where empirical quantiles become unstable. LightGBM's quantile
regression is non-parametric and cannot extrapolate.

**Reason 3: Theoretical grounding.** The Pickands-Balkema-de Haan theorem
guarantees that exceedances above a high threshold converge to a GPD for
a broad class of distributions. This is the
EVT analogue of the Central Limit Theorem.

## 3. Why the Fit Is Pooled

A GPD fit wants at least 50 exceedances above the 95th percentile, which
is about 1000 residuals. A ticker has a few hundred matured residuals per
horizon over the window (504 to 1008 dates), so about 25 to 50
exceedances, and those residuals overlap (labels share `(H-1)/H`). Per-ticker
fitting therefore sits below the minimum for every ticker and horizon.

The fit is pooled instead. Each residual is standardized by its ticker's
TCP half-width at the date the interval was issued, the standardized
residuals of all tickers are pooled per horizon over a trailing window of
matured dates (horizon-specific, `W_full` = 504 / 756 / 1008), one GPD is
fitted per tail, and the pooled quantile is mapped back to each ticker with
its own current mid and half-width.

**Raw counts are not an effective sample.** Residuals of different tickers
on the same date are correlated (a market move hits all of them), and
labels of nearby dates overlap. With 450 to 600 tickers the pooled
exceedance count is in the tens of thousands, but the number of independent
tail events is far smaller. Date-declustering does not help: under
independence the chance a date has no exceedance is about 1e-10 (0.95^450);
common market moves raise it only on extreme market days, so the old gate of
200 declustered dates never bit. The criterion is distinct tail **episodes**: the number of
non-overlapping `H`-day blocks of the window that contain an exceedance,
with a minimum (placeholder 3), plus a minimum window (`EVT-PROCEDURE.md` §4).
The fallback is the pooled empirical 1%/99% quantile over the available
window, not a per-ticker one.

## 4. The POT Procedure

See `EVT-PROCEDURE.md` for the full algorithm. The summary:

1. **Compute standardized calibrated residuals** from the TCP layer, for
   matured dates only (`s + H <= t`):
   `z = (realized_return - calibrated_mid) / calibrated_halfwidth`,
   with mid and half-width as issued on `s`.
2. **Pool** `z` across tickers per horizon over the available matured
   dates, up to `W_full(H)` (partial window allowed down to `W_min(H)`).
3. **Select a threshold** at the 95th percentile of the pooled `z`
   (5th for the lower tail, handled by negating `z`).
4. **Extract exceedances** above the threshold; require at least 3 tail
   episode blocks, 50 raw exceedances and the minimum window per tail.
5. **Fit a GPD** to the pooled exceedances via native Julia maximum
   likelihood, then apply the shape guards (`ξ` in `(-0.5, 1)`).
6. **Extrapolate** to the 1st/99th percentile with the GPD quantile
   formula (`EVT-PROCEDURE.md` §6), then **map back** per ticker:
   `q_extreme(i) = mid_i + halfwidth_i × q_pooled`.

## 5. Threshold Selection and Gate

The threshold `u` is the 95th percentile of the pooled standardized
residuals. This is not tuned. The rationale:

- 95% gives enough exceedances (5% of the pooled sample) for a stable GPD
  fit.
- 95% is high enough that the GPD approximation holds.
- The 95th percentile of `z` lies **inside** the TCP interval: `|z| = 1` is
  the edge of the 95%-coverage interval (about 2.5% of `z` beyond each
  edge), so the threshold is a one-sided 5% tail, not "the same level as the
  TCP bounds". The 1% / 99% target lies beyond the interval edge, which is
  why the EVT bound normally does not need the clamp.

**If a gate fails** (window shorter than `W_min`, fewer than 3 tail episode
blocks, fewer than 50 raw exceedances, a failed or rejected MLE): the GPD
fit is not trusted. The pipeline logs a `WARN` and falls back to the
**pooled** empirical 1st/99th percentile of the standardized residuals over
the same available window, mapped back per ticker, if the window is at least
one label block `H` and the pooled sample at least 5000. The `q_extreme` is
still produced, but flagged (`fit_method = pooled_empirical`, NaN shape and
scale). Below that, no row is written and a `data_quality` row records it.
The fallback is never a per-ticker empirical quantile.

## 6. GPD Parameter Estimation

Maximum likelihood (MLE) is the default, as a **native Julia routine**
(profile likelihood in `θ = ξ/σ`; `EVT-PROCEDURE.md` §5). There is no Python
or `scipy` fallback. Whether `Distributions.jl` provides a usable `fit` for
`GeneralizedPareto` that fixes the location is unverified (verify) and is
not relied on.

**Why MLE despite known issues:** MLE is the standard estimator. It has
theoretical efficiency guarantees. Alternatives (PWM, L-moments) can
outperform MLE in small samples, but the pooled sample and the episode and
exceedance gates mitigate the small-sample problem.

**Known limitation:** MLE for GPD can be numerically difficult when the
shape parameter is near 0 or when exceedances are sparse.
The pipeline monitors the fitted shape parameter with one consistent set of
guards (`EVT-PROCEDURE.md` §6): `xi >= 1` or `xi <= -0.5` is **rejected**
(fallback); `0.5 < xi < 1` and `|xi| < 0.01` are accepted with a `WARN` (the
general formula is evaluated in a numerically stable form and is not
replaced by the exponential one); the only formula switch, at `|xi| < 1e-8`,
is numerical.

**Known limitation (dependence):** cross-sectional dependence and
overlapping labels make the effective number of independent tail events far
smaller than any raw count; the episode-block floor is a minimum, not a
claim of independence. See `EVT-PROCEDURE.md` §4.

## 7. What Not to Do

- Do not fit the GPD to raw returns. It is fit to **calibrated residuals**
  from TCP, standardized by the TCP half-width. See `EVT-PROCEDURE.md` §1.
- Do not fit a GPD per ticker. A ticker has far too few tail points.
- Do not pool unstandardized residuals.
- Do not use a residual whose forward return has not matured
  (`s + H > t`).
- Do not use a threshold below the 95th percentile. The GPD approximation
  degrades for lower thresholds.
- Do not fit a single GPD to both tails. Left and right tails are fit
  separately with their own thresholds (the lower on negated residuals).
- Do not extrapolate beyond the 1st/99th percentile. The GPD fit is valid
  for the exceedance region, not for the extreme far tail (0.1%, 0.01%).
- Do not count raw pooled exceedances, or distinct dates with an
  exceedance, as an effective sample. Gate on tail episode blocks and the
  window length.
- Do not accept `xi >= 1` or `xi <= -0.5`.
- Do not use Python or `scipy` for the fit.
- Do not fall back to a per-ticker empirical quantile. Fall back to the
  pooled one.
- Do not ignore the shape parameter. A shape near 0 or above 0.5 indicates
  the tail model may not fit well (WARN), and above 1 it is rejected.

## 8. Verification

Before deploying an EVT change:

- [ ] A test on synthetic Pareto data recovers the known shape parameter
      to within 10%
- [ ] A test checks the corrected quantile formula (including the
      `xi -> 0` branch) against the empirical 99th percentile of simulated
      exponential and Pareto data
- [ ] A test asserts the threshold is the 95th percentile of the pooled
      standardized residuals
- [ ] A pooling test: the fit pools across tickers per horizon after
      standardizing by the issue-date TCP half-width, and the per-ticker
      mapping recovers each ticker's quantile
- [ ] A block test: `N_blk` counts non-overlapping `H`-day blocks with an
      exceedance; the fit requires `N_blk >= 3`, a window of at least
      `W_min`, `N_u >= 50`, and a partial window records `window_days`
- [ ] A guard test: `xi >= 1` and `xi <= -0.5` take the fallback; the
      `xi -> 0` form is the numerically stable one
- [ ] A maturity test: every residual used satisfies `s + H <= t`, and
      truncating the data at `t` leaves the fit unchanged
- [ ] A test asserts the fallback cascade: pooled empirical 1%/99% over the
      available window when a gate fails (never per-ticker), and no row when
      the window is shorter than `H` or `n < 5000`
- [ ] `evt_tail_bounds.parquet` and `evt_residuals` schemas match the
      registry
- [ ] A test asserts left and right tails are fit separately, the lower on
      negated residuals and negated back once
- [ ] A test asserts `q_extreme` never narrows the TCP interval

## 9. Related Skills

- `04-tcp-calibration` — provides calibrated intervals (mid, half-width)
  and the maturity rule
- `03-gspqr-model/PREDICTION-BACKFILL.md` — the history timeline: first TCP
  origin and the first partial and full EVT windows per horizon
- `03-gspqr-model` — provides the raw quantiles that TCP calibrates
- `13-parquet-lake` — schema for `evt_residuals` and `evt_tail_bounds`
- `14-backtest-protocol` — evaluates tail coverage
