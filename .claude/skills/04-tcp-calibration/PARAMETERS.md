# TCP Parameters

The exact parameter values, their rationale, and their tuning ranges.
Values marked *placeholder* are starting points to be checked on the
walk-forward track, not results.

## 1. Window Sizes

| Parameter | Symbol | Value | Rationale |
|---|---|---|---|
| Calibration method | — | `pooled_norm` (default); `per_ticker` (option) | Per-ticker windows have an effective sample of about 1 to 2 at `H >= 63` (`CALIBRATION.md` §1, §4) |
| Pooled calibration window | `M_d` | `H` dates (63 / 126 / 252), *placeholder* | One full label span, so the window holds at least one block of non-overlapping labels. Longer (`2H`) adds a second market episode at the price of a later first origin |
| Per-ticker window (option) | `m` | 60 matured dates | The earlier default; kept only for comparison |
| Maturity lag | `H` | 63 / 126 / 252 | A date `s` is usable at origin `t` only if `s + H <= t`, counted on the global trading-calendar index |
| Required history | `(m_first(H) + 1) + H + M_d - 1` | day 945 / 1134 / 1512 of a 15-year panel | First origin with every window date holding a stored as-of prediction (`03-gspqr-model/PREDICTION-BACKFILL.md` §8) |

The calibration dates at origin `t` are `s in [t - H - M_d + 1, t - H]`,
which is a different window for each horizon. See `CALIBRATION.md` §1.

**Counts gate (placeholders).** The pooled step needs `n >= n_min = 5000`
scores and at least `ceil(0.8 × M_d)` window dates each holding at least
50 tickers. Below that, no TCP row is written for `(t, H)` and a `WARN` goes
to `data_quality.parquet`. For about 450 tickers and `M_d = 63` the pool is
about 28 thousand scores; `n_min` only bites at the start of the history and
in thin periods.

**Width floor.** `ω = 0.1` (*placeholder*): the raw interval width used for
normalization is floored at `ω` times the cross-sectional median width of the
same date, so a degenerate (near-zero) raw interval cannot produce an
unbounded score.

**Why `M_d = H` and not 60.** With `m = 60` and `H >= 63`, the 60 labels of
one ticker overlap almost completely. Pooling across tickers raises the
number of scores to tens of thousands, but the effective number of
independent market episodes stays about `M_d / H + 1`, about 2. Both facts
go in every report. The pooled order statistic `k = ceil((n+1)(1-α))` is of
a large sample, so its resolution is fine; its reliability is limited by the
episodes.

## 2. Miscoverage Rate

| Parameter | Symbol | Value |
|---|---|---|
| Nominal coverage of the calibrated interval | `1 - α` | 0.95 |
| Miscoverage | `α` | 0.05 |

`q_hat_lower` and `q_hat_upper` are the endpoints of an interval with 95%
target coverage. The raw `[q_05, q_95]` is a 90% base interval; the
calibrated endpoints are not the 5% and 95% quantiles (`SKILL.md` §1).
A separate 80% interval (α = 0.20) can be produced by running TCP with a
different α and writing to a different output file, but the pipeline does
not do this by default.

## 3. Order Statistic

    k = ceil((n + 1) × (1 - α))

with `n` the number of pooled scores (default) or `m` (per-ticker option).
For the per-ticker option with `m = 60`: `k = ceil(61 × 0.95) = 58`. For a
pooled `n = 28,000`: `k = ceil(28001 × 0.95) = 26,601`. The conformal
quantile is the `k`-th smallest score. If `k > n`, use the largest score. If
`n = 0`, no row is written (the consumer sees the raw interval).

## 4. Robbins-Monro Schedule

TCP-RM adds an online offset `C_t` to the conformal threshold. The
update rule:

    C_{t+1} = C_t + γ_t × e_t

Where:
- `e_t = 1{y ∉ [q_hat_lower, q_hat_upper]} - α` (coverage error of the
  interval issued at origin `t - H`, using the outcome that matured at `t`)
- `γ_n = γ_0 / sqrt(n + 1)` (decaying learning rate, `n` = matured updates so
  far for this `(ticker, horizon)`, persisted as `rm_n`)

Parameters:

| Parameter | Symbol | Value | Rationale |
|---|---|---|---|
| Initial learning rate | `γ_0` | 0.01 | Small enough to avoid oscillation |
| Decay | `1 / sqrt(n + 1)` | — | Slowly decaying tracking gain (see below) |
| Initial offset | `C_0` | 0.0 | Start with no adjustment |
| Offset clip | — | `[-0.5, 0.5]` | Prevent runaway in extreme regimes |
| Inversion floor | — | applied correction `>= -w/2` | `w = q_95 - q_05`; keeps `q_hat_lower <= q_hat_upper` |

The state `(C, n)` is per `(ticker, horizon)` and sequential: see
`CALIBRATION.md` §5 for persistence, ordering and resume.

The schedule `γ_n = γ_0 / sqrt(n + 1)` satisfies `∑γ_n = ∞` but **not**
`∑γ_n² < ∞` (the sum of `1/n` diverges), so it does not meet the strict
Robbins-Monro conditions and the offset is not guaranteed to converge.
This is acceptable for non-stationary data: the gain decays slowly
enough that the offset keeps tracking drift. A schedule that does meet
both conditions is `γ_0 / (n + 1)^a` with `a` in `(0.5, 1]`, which would
freeze the offset over time. The pipeline keeps `a = 0.5` and makes no
convergence claim; coverage is checked empirically instead.

**When to disable TCP-RM:** If the coverage error is already near zero
on the validation set, the offset adds noise without benefit. Disable
by setting `γ_0 = 0.0`. The pipeline defaults to enabled because
financial time series are non-stationary by nature.

## 5. Tuning Ranges

Coverage is the empirical fraction of realized returns inside the
calibrated interval. Nominal coverage is `1 − α = 0.95` (the miss rate is
`α = 0.05`). If coverage deviates from `1 − α` by more than 2 percentage
points:

| Symptom | Adjust |
|---|---|
| Over-coverage (coverage > 1 − α + 0.02, i.e. above 0.97) | Check `qhat_zero_frac` first. If it is near 1, the raw GSPQR interval is already conservative and zero-truncated TCP cannot narrow it; fix it in `03-gspqr-model`, not here. Otherwise `Q̂` is conservative: increase `γ_0` so a negative RM offset can pull it down, or reduce `M_d` |
| Under-coverage (coverage < 1 − α − 0.02, i.e. below 0.93) | Increase `γ_0` (the offset reacts faster to recent misses) and/or increase `M_d` (a longer window includes more past large misses, so the order statistic is less often 0). Heuristics; confirm on the walk-forward track |
| Wide intervals, correct coverage | Intervals cannot be narrower than raw GSPQR; if the width comes from the raw interval, address it in `03-gspqr-model`. If `qhat_zero_frac` is low, reduce `M_d` so `Q̂` forgets old misses sooner |
| Oscillating offset | Reduce `γ_0` by half |

**Do not tune `α`.** It is the target coverage, not a free parameter.
Changing it changes what the interval means.

Tuning uses the walk-forward track only, never the CPCV paths and never the
`split == test` rows (`14-backtest-protocol`).

## 6. What Is Not Tuned

- **The zero-truncation of scores.** Always truncated (`Q̂ >= 0`).
- **The rolling window structure.** Always rolling, never static.
- **The Robbins-Monro update form.** Always `C_{t+1} = C_t + γ_t e_t`.
- **The maturity rule.** Calibration dates are always `s <= t - H` on the
  global calendar.
- **Width normalization of pooled scores.** Always on in the default.

If a tuning pass wants to change one of these, it is a skill amendment.
