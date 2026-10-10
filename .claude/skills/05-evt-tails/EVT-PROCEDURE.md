# EVT Procedure

The step-by-step algorithm for fitting the GPD and extrapolating to
extreme quantiles. The fit is **pooled across tickers, per horizon**: a
single ticker never has enough tail observations (§4).

## 1. Input: Standardized Calibrated Residuals

EVT operates on **calibrated residuals**, not raw returns, and pools them
only after removing each ticker's scale.

For a ticker `i`, a calibration date `s`, and horizon `H`, using the TCP
interval **as issued at `s`** (stored in `tcp_calibration.parquet`):

    calibrated_mid(i, s)       = (q_hat_lower + q_hat_upper) / 2
    calibrated_halfwidth(i, s) = (q_hat_upper - q_hat_lower) / 2
    residual(i, s)             = realized_return(i, s, H) - calibrated_mid(i, s)
    z(i, s)                    = residual(i, s) / calibrated_halfwidth(i, s)

`realized_return` is the same forward excess return TCP scores against
(`y_fwd_Hd`). The half-width is the scale at the date the interval was
issued, not today's. A row with `calibrated_halfwidth <= 0` or a missing
value is dropped and counted in `data_quality.parquet`.

`q_hat_lower` and `q_hat_upper` are the endpoints of a 95%-coverage
interval (`04-tcp-calibration/SKILL.md` §1), so `|z| = 1` is the interval
edge: for a well-calibrated TCP about 2.5% of `z` exceed `+1` and 2.5% fall
below `-1`. The EVT threshold (§3, the 95th percentile of `z`) therefore lies
**inside** the TCP interval, and the 99th percentile of `z` lies outside it.

**Maturity rule (same as TCP, constitution §3).** `z(i, s)` exists at fit
date `t` only if `s + H <= t`, with `s + H` counted on the global
trading-calendar index (`04` CALIBRATION §0). A residual whose forward
return has not been realized never enters a fit. `evt_residuals` rows are
written once the date has matured, in date order. A residual exists only for
`s >= t0(H)`, the first TCP origin of the horizon.

If TCP is well calibrated, `z` is roughly symmetric around zero with fat
tails, and its scale is comparable across tickers, which is what makes
pooling defensible.

## 2. Pooling Window (horizon-specific, with partial windows)

For each horizon `H` and fit date `t`, pool every ticker's `z(i, s)` over
the most recent matured dates, up to `W_full(H)`:

    s_hi    = t - H
    s_lo    = max( t0(H),  t - H - W_full(H) + 1 )
    W_avail = s_hi - s_lo + 1
    Z_H(t)  = { z(i, s) : s in [s_lo, s_hi], i in universe }

| Horizon | `W_min` (smallest window a GPD is fitted on) | `W_full` | blocks `W_full / H` |
|---|---|---|---|
| 63d | 252 | 504 | 8 |
| 126d | 378 | 756 | 6 |
| 252d | 756 | 1008 | 4 |

(*placeholders*; `W_min = max(3H, 252)`: at least three non-overlapping
label blocks.) The earlier single `W = 756` for every horizon gave 12 / 6 / 3
non-overlapping label blocks for 63 / 126 / 252; with the horizon-specific
`W_full` the 252-day horizon gets four. The window start is the later of
`t0(H)` and the nominal start, so a **partial window** is what exists
between the first fit date `t0 + H + W_min - 1` and the date the window
fills, `t0 + H + W_full - 1` (for a 15-year panel: day 1259 / 1637 / 2519
and 1511 / 2015 / 2771; `03-gspqr-model/PREDICTION-BACKFILL.md` §8). Every
`evt_tail_bounds` row records the window used (`window_days = W_avail`).
Pooling is per horizon only; never pool across horizons.

The window length does not grow past `W_full`: older residuals are dropped
(a rolling window), so the fit follows the recent tail.

## 3. Threshold and Exceedances

On the pooled standardized residuals `Z = Z_H(t)`, with `n = |Z|`:

    u_upper = quantile(Z, 0.95)
    u_lower = quantile(Z, 0.05)

The upper tail is fit as is. The **lower tail is fit on the negated
residuals** `Z~ = -Z`, whose 95th percentile is `u~ = -u_lower`:

    upper:  z_j - u_upper          for z_j > u_upper
    lower:  (-z_j) - u~            for -z_j > u~   (equivalently u_lower - z_j for z_j < u_lower)

Every exceedance is positive, and from here on both tails use the same
upper-tail code on their own sample (`Z` for upper, `Z~` for lower). The
negation back to the lower tail happens **once**, in §6.

`N_u` is the number of exceedances in the pooled sample (all of them, not
a declustered count). Because `u` is the 95th percentile of the same
sample, `N_u / n` is about 0.05.

## 4. Effective Sample: Tail Episodes, Not Counts

Why pooling alone is not enough: a ticker has about `W` residuals per
horizon, so only about 5% of that above its own 95th percentile, below any
usable GPD minimum. Pooling fixes the count, but the pooled residuals are not
independent. Residuals of different tickers on the same date share the market
move, and labels of nearby dates overlap by `(H-1)/H`. The raw count `N_u`
(about `0.05 × n`, in the tens of thousands for 450 tickers) is **not** an
effective sample size. The effective number of independent tail events is far
smaller and cannot be read off the count.

**The date-declustering gate is dropped.** With 450 to 600 tickers per date,
the chance that a date has no exceedance is about `0.95^450 ≈ 1e-10` under
independence (a common market move raises it only on extreme market days), so
"distinct dates with an exceedance" is essentially the number of dates in the
window and never fails. The earlier gate of 200 declustered dates was a window-length
check in disguise (and 200 dates is less than one 252-day label span).

**The criterion is distinct tail episodes in non-overlapping blocks.**
Partition the window `[s_lo, s_hi]` into non-overlapping blocks of `H`
consecutive dates, counted back from `s_hi`:

    N_blk = number of H-day blocks containing at least one exceedance of the tail

A block is the unit within which labels overlap, so blocks are the nearest
thing to independent trials. The fit requires `N_blk >= B_min = 3`
(*placeholder*). Honest reading: at this universe size essentially every
block contains an exceedance, so `N_blk` is in practice
`floor(W_avail / H)` and the criterion is a floor on the window length in label
blocks, equivalent to `W_avail >= W_min`. It is kept explicitly because it
is the right quantity if the universe is ever much smaller or the horizon
shorter. To say something about market-wide tail events as opposed to
idiosyncratic ones, the run also logs `n_market_tail_dates`: dates on which
more than three times the nominal share of tickers (15% for a 5% tail)
exceed the threshold, and the number of blocks that contain one. This is a
diagnostic, not a gate.

**Gates, per tail.** The GPD is fitted only if all hold:

    W_avail >= W_min(H)
    n >= 1000                       (pooled sample)
    N_u >= 50                       (raw exceedances, an MLE minimum)
    N_blk >= B_min

Otherwise the fallback cascade of §8 applies. The gates are applied to each
tail separately.

**What is fitted.** The GPD is fitted by MLE to all `N_u` pooled
exceedances (the point estimate is consistent under dependence), and the
exceedance rate in §6 uses `N_u / n`. `n_exceedances` (`SCHEMAS.md` #10) still
stores the number of distinct dates with an exceedance as information, and the
appended `n_episode_blocks` stores `N_blk`. Standard errors from the
likelihood are too optimistic under this dependence; do not report them as
confidence intervals.

**Known limitation.** Even blocks of `H` days are not independent (market
regimes persist across blocks), and the idiosyncratic tail of one ticker is
not independent of the market tail. `B_min = 3` is a floor on episodes
observed, not a claim of three independent events. Tail coverage in the
walk-forward track (`14-backtest-protocol`) is the safeguard.

## 5. GPD Fitting (native Julia MLE)

For each tail, fit a 2-parameter GPD with location 0 to the exceedances
`x_1..x_N` (all `x_j > 0`). The fit is a **native Julia routine**
(`models/src/evt/gpd_mle.jl`); no Python, no `scipy`, and no fallback through
interop. Whether `Distributions.jl` offers a usable `fit` for
`GeneralizedPareto` that fixes the location is unverified (verify); the
pipeline does not depend on it.

The routine maximizes the log-likelihood

    l(σ, ξ) = -N ln σ - (1 + 1/ξ) Σ ln(1 + ξ x_j / σ),   σ > 0,  1 + ξ x_j / σ > 0

by profiling out the scale (Grimshaw 1993, verify the reference). With
`θ = ξ / σ`,

    ξ(θ) = (1/N) Σ ln(1 + θ x_j),   σ(θ) = ξ(θ) / θ
    l_p(θ) = -N [ ln σ(θ) + ξ(θ) + 1 ]

and the exponential limit `θ -> 0` gives `l_p(0) = -N [ ln(mean(x)) + 1 ]`
(`ξ = 0`, `σ = mean(x)`). Procedure: evaluate `l_p` on a grid of `θ` over
`(-1/max(x), c/mean(x))`, keep the best point, refine in the bracketing
cell by golden-section search, then recover `(ξ, σ)`. A 1-D search on a grid
cannot hit a local boundary solution the way a 2-D optimizer can.

A scratch check of this exact procedure (grid of 400 + 400 points, 80
golden-section steps, 30 replications of `N = 400` GPD draws) recovered the
shape within 0.015 for `ξ ∈ {0.6, 0.3, 0.1, 0, -0.2}` on average (the
reference check for the test in §10; it was run once, in a scratch script
outside the repository, not in Julia).

If the search finds no interior maximum, if `σ(θ) <= 0`, or if the result
violates the guards of §6, the fit **failed** and the cascade of §8 applies.

## 6. Quantile Extrapolation and the Shape Guards

For a fitted tail with threshold `u`, scale `σ`, shape `ξ`, pooled sample
size `n`, and exceedance count `N_u`, the GPD quantile of the sample at
upper-tail probability `p` is

    x_p = u + (σ / ξ) × [ ((n / N_u) × (1 − p))^(−ξ) − 1 ]

The `−1` is **inside** the bracket. As `ξ → 0` the limit is

    x_p = u − σ × ln((n / N_u) × (1 − p))   =   u + σ × ln(N_u / (n (1 − p)))

(the argument of the log is below 1 for `p` above the threshold level, so
the extra term is positive). Write `a = (n / N_u)(1 − p)`.

**Numerically stable form.** Evaluate

    x_p = u + σ × expm1(−ξ × ln a) / ξ        for |ξ| >= 1e-8
    x_p = u − σ × ln a                         for |ξ| <  1e-8

The plain `a^(−ξ) − 1` loses digits for small `ξ` (scratch check with
`u = 1, σ = 0.5, a = 0.2`: the plain form's absolute error against the
`expm1` form is about `5e-11` at `|ξ| = 1e-6`, `5e-9` at `1e-8`, `4e-7` at
`1e-10` and `3e-5` at `1e-12`). The `expm1` form has no such loss, and the
limit branch used below `1e-8` differs from the true value by about
`σ ln²(a) ξ / 2`, under `1e-8` in absolute terms there. There is therefore
**one** switch, at `1e-8`, for numerical reasons only. It is unrelated to
the shape warning thresholds below. (The earlier `|ξ| < 1e-6` switch is
superseded.)

Checks run in a scratch script (400,000-point samples drawn from a
distribution with a known GPD tail above `u = 1`; formula evaluated at the
estimated threshold and the true `σ, ξ`): the formula reproduced the
empirical 99th percentile within sampling error for `ξ ∈ {0.8, 0.3, 0.1,
0.01, 1e-4, -0.2}` (for example `ξ = 0.3`: formula 2.0366, true 2.0344,
empirical 2.0299), and the `ξ → 0` limit is the correct branch for `|ξ|`
small.

Apply it with `p = 0.99` in **both** tails, each on its own sample:

    q_pooled(upper) =  x_0.99 computed on Z
    q_pooled(lower) = −x_0.99 computed on Z~ = −Z      ← the one negation back

`x_0.99` on `Z~` is the 99th percentile of the negated residuals, i.e. the
negative of the 1st percentile of `Z`. So `q_pooled(upper)` is the pooled
99th percentile of `z` (positive) and `q_pooled(lower)` is the pooled 1st
percentile of `z` (negative). Do not also negate `u` or the exceedances
again, and do not use `p = 0.01` for the lower tail.

**Shape guards (one consistent set).**

| Fitted `ξ` | Action |
|---|---|
| `ξ >= 1` | **Reject** the fit and use the fallback (§8), WARN `evt_xi_ge_1`. The tail has an infinite mean; the quantile is finite but grows as `a^(−ξ)` (for `σ = 1`, `x_p − u` is 2.5 at `ξ = 0.5`, 4.0 at `ξ = 1`, 6.8 at `ξ = 1.5`, 12.0 at `ξ = 2`, with `a = 0.2`), and an MLE this heavy on pooled, dependent residuals is more likely a symptom than a tail |
| `ξ <= -0.5` | **Reject**, WARN `evt_xi_le_neg_half`. The MLE is non-regular below `-0.5` (verify the Smith 1985 condition), and a finite-endpoint tail for return residuals is implausible |
| `0.5 < ξ < 1` | Accept, WARN `evt_xi_heavy` |
| `\|ξ\| < 0.01` | Accept and use the general stable formula, WARN `evt_xi_near_exponential` (near-exponential tail). No change of computation, no forced `ξ = 0` |
| otherwise | Accept |

Sanity: `q_pooled(upper) > u_upper` and `q_pooled(lower) < u_lower`. If the
result is not finite or violates this, the fit failed (§8).

## 7. Mapping Back to Each Ticker

At fit date `t`, ticker `i` has its current TCP interval
(`calibrated_mid_i`, `calibrated_halfwidth_i` from `tcp_calibration` at
`forecast_date = t`, same horizon):

    q_extreme_upper(i) = calibrated_mid_i + calibrated_halfwidth_i × q_pooled(upper)
    q_extreme_lower(i) = calibrated_mid_i + calibrated_halfwidth_i × q_pooled(lower)

That is `mid ± halfwidth × |q_pooled|` with the signs already carried by
`q_pooled(lower) < 0`. `q_extreme` is therefore a bound on the **forward
return** (the same units as `q_hat_lower` / `q_hat_upper`), not on the
residual. `evt_tail_bounds.parquet` has one row per
`(ticker, fit_date, horizon, tail)`; the pooled parameters (`gpd_shape_xi`,
`gpd_scale_sigma`, `threshold_u`, `n_exceedances`, `n_episode_blocks`,
`window_days`) are identical across tickers for a given
`(fit_date, horizon, tail)`. Only `q_extreme` varies by ticker.

The EVT layer must not narrow the TCP interval: `q_extreme_lower(i) <=
q_hat_lower(i)` and `q_extreme_upper(i) >= q_hat_upper(i)`. If a mapped
bound violates this (a fitted tail thinner than the interval edge),
clamp it to the TCP bound and log a `WARN`.

## 8. Fallback Cascade

Per `(fit_date, horizon, tail)`, the first row that applies:

| Condition | `fit_method` | Window used | Notes |
|---|---|---|---|
| Gates of §4 pass, MLE converges, guards of §6 pass | `gpd` | `W_avail` (partial if `< W_full`; flagged by `window_days`) | |
| Any of: `W_avail < W_min`, `N_blk < B_min`, `N_u < 50`, MLE failed, guard rejected, sanity failed; and `W_avail >= H` and `n >= 5000` | `pooled_empirical` | the same available window `[s_lo, s_hi]` | `q_pooled(upper) = quantile(Z, 0.99)`, `q_pooled(lower) = quantile(Z, 0.01)`; `gpd_shape_xi`, `gpd_scale_sigma` written NaN; WARN |
| `W_avail < H` (less than one label block) or `n < 5000` | none | n/a | **No row written.** A `data_quality` row (`evt_no_fit`, with the failed condition) and the run log record it; consumers use the TCP bounds |

The **pooled** empirical quantiles are used, never a per-ticker one (a single
ticker's 1% quantile rests on a handful of points). `n >= 5000` leaves at least
50 points beyond the 1% quantile. The mapping in §7 is unchanged. A horizon
with no row simply has no EVT bound on that date; this is the normal state
before `t0 + H + H - 1` and for a thin period, and it is not an error.

Consumers must read `fit_method` and `window_days` before treating a bound as
a GPD estimate.

## 9. Integration with TCP

The EVT bounds are **additional** to the TCP bounds, not replacements.

- TCP provides the 95%-coverage interval: `q_hat_lower`, `q_hat_upper`
- EVT provides the 1%/99% bounds: `q_extreme_lower`, `q_extreme_upper`

Downstream modules (portfolio optimizer, risk engine) use the TCP
interval for expected return and downside risk, and the EVT bounds for
extreme scenario analysis and stress testing.

## 10. Verification

- [ ] A test on synthetic Pareto data recovers the shape parameter within
      10% (native MLE; reference: the scratch check of §5)
- [ ] A test on exponential data (`ξ = 0`) and on Pareto data checks that
      the §6 formula reproduces the empirical 99th percentile (and that
      the `ξ → 0` branch has the right sign: its result exceeds `u`), and
      that the `expm1` form equals the plain form for `|ξ| >= 1e-3`
- [ ] A test asserts the guards: `ξ >= 1` and `ξ <= -0.5` take the fallback,
      `0.5 < ξ < 1` and `|ξ| < 0.01` warn and keep the GPD result
- [ ] A test asserts the threshold is the 95th percentile of the pooled
      standardized residuals
- [ ] A test asserts exceedances are positive
- [ ] A pooling test: tickers with different scales give the same
      `q_pooled`, and the §7 mapping recovers each ticker's quantile
- [ ] A block test: `N_blk` counts non-overlapping `H`-day blocks counted
      back from `s_hi` that contain an exceedance; duplicating a date's
      exceedances across many tickers leaves it unchanged
- [ ] A cascade test: partial window (`W_min <= W_avail < W_full`) fits a GPD
      and writes `window_days`; `W_avail < W_min` falls back to the pooled
      empirical over the same window; `W_avail < H` or `n < 5000` writes no
      row and one `data_quality` row; the fallback is never per-ticker
- [ ] A maturity test: truncating the data at `t` leaves the fit
      unchanged, and every residual used satisfies `s + H <= t` on the
      global calendar
- [ ] A history test: the available window is computed from the stored
      residuals (their first date and the horizon's `t0`), not from the length
      of any price series
- [ ] A test asserts the lower tail is fit on negated residuals and
      negated back exactly once (a symmetric sample gives
      `q_pooled(lower) ≈ −q_pooled(upper)`)
- [ ] A test asserts the quantile formula matches the GPD CDF inversion
- [ ] A test asserts `q_extreme` does not narrow the TCP interval

## 11. What Not to Do

- Do not fit the GPD to raw returns. Use calibrated residuals.
- Do not fit a GPD per ticker. Pool the standardized residuals per
  horizon.
- Do not pool unstandardized residuals; tickers differ in scale.
- Do not treat the raw exceedance count, or the number of distinct dates with
  an exceedance, as an effective sample size.
- Do not use an unmatured residual. The window is `[t-H-W+1, t-H]` on the
  global calendar.
- Do not standardize by today's TCP half-width; use the half-width issued
  on the residual's own date.
- Do not estimate the location parameter. Fix it at 0.
- Do not pool left and right tails. Fit separately.
- Do not write the extrapolation with the `−1` outside the bracket, or the
  `ξ → 0` limit with a plus sign.
- Do not accept `ξ >= 1` or `ξ <= -0.5`.
- Do not use a threshold other than the 95th percentile without a skill
  amendment.
- Do not fall back to a per-ticker empirical quantile.
- Do not call Python or `scipy` for any part of the fit.
- Do not extrapolate beyond the 1st/99th percentile.
- Do not infer the available history from `prices.parquet`.
