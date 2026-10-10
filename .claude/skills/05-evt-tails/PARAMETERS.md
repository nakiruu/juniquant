# EVT Parameters

The exact parameter values for the Peaks-Over-Threshold procedure. Values
marked *placeholder* are to be checked on the walk-forward track.

## 1. Pooling, Threshold, and Gates

| Parameter | Value | Rationale |
|---|---|---|
| Pooling | Per horizon, across all tickers | One ticker has about 5% of its few hundred residuals in the tail; a per-ticker GPD needs about 1000 residuals for 50 exceedances |
| Standardization | `z = residual / calibrated_halfwidth` at the issue date | Removes per-ticker scale so residuals can be pooled |
| Full window `W_full` | 504 / 756 / 1008 matured dates for 63d / 126d / 252d (*placeholder*) | 8 / 6 / 4 non-overlapping label blocks; window `s in [max(t0, t-H-W_full+1), t-H]`, same maturity rule as TCP |
| Minimum window `W_min` | 252 / 378 / 756 (*placeholder*; `max(3H, 252)`) | Smallest window a GPD is fitted on; a shorter one is a partial window below this, handled by the fallback |
| Threshold quantile | 95th / 5th percentile of the pooled `z` | Balances bias and variance; inside the TCP interval (`\|z\| = 1` is the 95%-interval edge) |
| Minimum tail episodes `B_min` | 3 non-overlapping `H`-day blocks containing an exceedance (*placeholder*) | Replaces the vacuous date-declustering gate (at 450 to 600 tickers every date has an exceedance); in practice a window-length floor |
| Minimum raw exceedances | 50 per tail | MLE minimum |
| Minimum pooled sample (GPD) | 1000 | Below this no GPD is fitted |
| Minimum pooled sample (empirical fallback) | 5000, and `W_avail >= H` | At least 50 points beyond the 1% quantile; one full label block |

The threshold is **always** the 95th percentile of the pooled standardized
residuals for that horizon (5th for the lower tail). It is a number in
standardized units, not a return. Raw exceedances are all used in the fit;
the block count only gates it.

With about 450 tickers and the smallest windows, `n` is about 113 thousand at
`W_min = 252`; the sample-size gates are not binding. The binding constraint
is the number of label blocks of history, which is why the timeline in
`03-gspqr-model/PREDICTION-BACKFILL.md` §8 is dominated by `t0 + H + W - 1`.

## 2. GPD Fitting

| Parameter | Value |
|---|---|
| Estimation method | Maximum likelihood (MLE), native Julia (`models/src/evt/gpd_mle.jl`); profile likelihood in `θ = ξ/σ`, grid plus golden-section search (`EVT-PROCEDURE.md` §5) |
| Location | Fixed at 0 (exceedances are already `x - u`) |
| Shape prior | None |
| Scale prior | None |

The GPD is the 2-parameter version: scale `σ` and shape `ξ`. The
location is 0 because we model exceedances above the threshold, not the
raw residual. No Python or `scipy` fallback exists or is allowed.

## 3. Exceedance Direction

| Tail | Sample fit | Threshold | Exceedances |
|---|---|---|---|
| Upper | `Z` | 95th percentile of `Z` | `z - u` for `z > u` |
| Lower | `Z~ = -Z` | 95th percentile of `Z~` (= `-u_lower`) | `(-z) - u~` for `-z > u~` |

Both tails are modeled as positive exceedances of their own sample, so the
same GPD machinery applies. The lower-tail result is negated back once, in
§4.

## 4. Quantile Extrapolation

For a target upper-tail probability `p` (`p = 0.99` for both tails), with
`a = (n / N_u) × (1 - p)`:

    x_p = u + (σ / ξ) × [ a^(-ξ) - 1 ]                      reference form
    x_p = u + σ × expm1(-ξ × ln a) / ξ     for |ξ| >= 1e-8  implementation form
    x_p = u - σ × ln a                      for |ξ| <  1e-8

Where:
- `u` is the threshold (standardized units)
- `σ` is the GPD scale
- `ξ` is the GPD shape
- `n` is the number of pooled residuals
- `N_u` is the number of pooled exceedances (raw)
- `p` is the target probability

The single numerical switch at `1e-8` is unrelated to the `|ξ| < 0.01`
warning below (`EVT-PROCEDURE.md` §6).

Upper tail: `q_pooled(upper) = x_0.99` computed on `Z`.

Lower tail (the single negation): fit on `Z~ = -Z`, compute `x_0.99` with
the same formula, and set `q_pooled(lower) = -x_0.99`.

Per ticker `i`, with its current TCP `mid_i` and `halfwidth_i`:

    q_extreme(i, upper) = mid_i + halfwidth_i × q_pooled(upper)
    q_extreme(i, lower) = mid_i + halfwidth_i × q_pooled(lower)

(`q_pooled(lower) < 0`.) Full derivation and checks: `EVT-PROCEDURE.md` §6–§7.

## 5. Shape Guards and Fallback Rules

| Condition | Action |
|---|---|
| `W_avail >= W_min`, `N_blk >= B_min`, `N_u >= 50`, `n >= 1000` | GPD fit (a partial window is allowed down to `W_min`; `window_days` records it) |
| Any gate fails, `W_avail >= H` and `n >= 5000` | Pooled empirical 1%/99% of `Z` over the same available window, NaN shape/scale, `fit_method = pooled_empirical`, WARN |
| `W_avail < H` or `n < 5000` | No row for that `(fit_date, horizon, tail)`; one `data_quality` row (`evt_no_fit`); consumers use the TCP bounds |
| `ξ >= 1` | Reject the GPD, fallback as above, WARN |
| `ξ <= -0.5` | Reject the GPD, fallback as above, WARN |
| `0.5 < ξ < 1` | Accept, WARN (heavy tail) |
| `\|ξ\| < 0.01` | Accept with the general stable formula, WARN (near-exponential). Not forced to `ξ = 0` |
| MLE does not converge | Fallback as above, log FAIL |
| Extrapolated bound narrower than the TCP bound | Clamp to the TCP bound, log WARN |

The fallback is never per-ticker.

## 6. What Is Not Tuned

- **The 95th percentile threshold.** Fixed.
- **The shape-guard bounds `[-0.5, 1)`.** Fixed.
- **The MLE estimator.** Fixed (PWM/L-moments are alternatives but not
  the default).
- **The GPD family.** Fixed (2-parameter GPD, location = 0).

`W_full`, `W_min` and `B_min` are placeholders to be checked by walk-forward
tail-coverage results, not tuned on CPCV paths or held-out test rows. Any
other change is a skill amendment.
