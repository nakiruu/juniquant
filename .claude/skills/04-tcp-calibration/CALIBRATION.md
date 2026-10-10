# Calibration Procedure

The step-by-step TCP calibration algorithm. The **default** is pooled across
tickers by date with width-normalized scores (§2); per-ticker calibration is
an option (§4).

## 0. Calendar Index

Every `t - H`, `s + H` and window bound in this skill counts on the **global
trading-calendar index**: the sorted distinct dates of the panel, which must
equal the computed NYSE trading days over the panel range. It never counts
rows of one ticker's own history (a ticker with missing days would then get
a shifted window) and never counts the dates present in `prices.parquet` for
that ticker. `t - H` is the date `H` entries before `t` on that index.


**Eligible names only (owner decision C3).** Conformity scores, the pooled
order statistic and the Robbins-Monro updates use only `(date, ticker)` rows that
are point-in-time `eligible` in `universe` at the calibration date and are not
flagged `survivorship_gap`: calibrate on what you would trade.

## 1. Inputs and the Maturity Rule

For a given forecast origin `t`, ticker `i`, and horizon `H` (in trading
days):

- `gspqr_predictions.parquet` — the raw quantiles `q_05`, `q_95` for `i`
  **as written on each calibration date `s`**. These are never recomputed
  with the current model generation (see below). They are the stored,
  monotone-rearranged values (`03-gspqr-model` §1), so `q_05 <= q_95`
  holds by construction; scores are never computed on raw crossed output.
  Each comes from one walk-forward generation trained on rows matured at its
  cutoff only (`03-gspqr-model/PREDICTION-BACKFILL.md`).
- `panel.parquet` — the realized target `y_fwd_Hd(i, s)`, the same
  forward excess return GSPQR was trained to predict. Conformity scores
  must be computed on the same quantity as the quantiles. `split` is never
  read.

**Maturity rule.** The realized value for calibration date `s` is the
return from `s` to `s + H`. It exists at origin `t` only if
`s + H <= t`, so only dates `s <= t - H` may enter the calibration set.
For every horizon in this system (`H >= 63`) a `[t-m, t-1]` window would
contain no matured observations when `m = 60`. Using unmatured returns is
lookahead (constitution §3).

The calibration dates for origin `t` are the `M_d` most recent matured
dates:

    cal dates:  s in [t - H - M_d + 1, t - H]

with `M_d = H` in the default (`PARAMETERS.md` §1) and `m = 60` in the
per-ticker option. The window is shifted back by `H` for each horizon; each
horizon has its own window and its own scores.

**As-of predictions.** The `q_05(i, s, H)` and `q_95(i, s, H)` used for a
calibration date are the values the generation current on date `s`
produced. That generation was trained only on rows whose targets had matured
by its cutoff, which precedes `s`, so the score is a genuine out-of-sample
residual. Recomputing history with today's model produces in-sample
residuals and a threshold that is too small. The history is the walk-forward
backfill (`03-gspqr-model/PREDICTION-BACKFILL.md`), never the current
generation.

**What a "5%/95%" label means.** The raw `[q_05, q_95]` is a **90% base
interval**. The conformal step targets `1 - α = 95%` coverage of the
*calibrated* interval, so `q_hat_lower` and `q_hat_upper` are the endpoints
of a 95%-coverage interval, roughly the 2.5% and 97.5% quantiles when the
model is well calibrated. They are not the 5% and 95% quantiles, and the
pinball loss at `τ = 0.05 / 0.95` is not their metric: coverage and the
Winkler score at `α = 0.05` are (`14-backtest-protocol/METRICS.md`).

**Known limitation (dependence).** The labels of consecutive calibration
dates overlap by `(H-1)/H`, and tickers on one date share the market move.
The pooled sample is large (tens of thousands of scores) but the number of
effectively independent market episodes in it is only about `M_d / H + 1`,
that is about 2. The conformal guarantee is therefore approximate, and
empirical coverage in the walk-forward track (`14-backtest-protocol`), by
date block and by volatility bucket, is the safeguard, not the nominal order
statistic. There is no proxy-horizon scaling (an earlier `sqrt(H/63)` rule
was dropped: it assumes independent increments).

## 2. The Algorithm: Pooled Across Tickers by Date (default)

```
Inputs: origin t, horizon H, parameters (M_d = H, α, ω, n_min, d_min, γ_0)

POOLED QUANTILE (once per (t, H), shared by all tickers)
1. Calibration dates: s in [t - H - M_d + 1, t - H].
2. For every ticker j with a stored prediction at s and a non-NaN
   y_fwd_Hd(j, s):
   a. w(j,s)   = q_95(j,s,H) - q_05(j,s,H)             (stored, rearranged)
   b. w_eff    = max(w(j,s), ω * median_k w(k,s))       (floor, same date s)
   c. score    = max(q_05 - y, y - q_95, 0) / w_eff     (zero-truncated, unit-free)
3. Pool the scores of all (j, s): n scores. Counts gate: need n >= n_min
   and at least ceil(0.8 * M_d) window dates with >= 50 tickers each;
   otherwise write no TCP row for (t, H) and log a WARN (data_quality).
4. Sort. k = ceil((n + 1) * (1 - α));  q_norm = score_(k) if k <= n else
   score_(n).  (q_norm >= 0; log whether q_norm == 0.)

PER TICKER (for each ticker i with a stored prediction at t)
5. RM update (only if TCP-RM enabled and a calibrated interval was stored for
   s = t - H): see §5. State is (C, n) per (ticker, horizon).
6. w_eff(i,t) = max(q_95(i,t,H) - q_05(i,t,H), ω * median_k w(k,t))
   Q_hat       = q_norm * w_eff(i,t)                    (return units; >= 0)
7. applied     = Q_hat + C_t, floored at -w(i,t)/2 so the interval cannot invert
8. q_hat_lower = q_05(i,t,H) - applied
   q_hat_upper = q_95(i,t,H) + applied
9. Write to tcp_calibration.parquet with calib_method = 'pooled_norm',
   n_cal_scores = n, rm_offset = C_t, rm_n = n_t.
```

Normalizing by the raw interval width makes scores comparable across
tickers (a volatile name's miss in return units would otherwise dominate the
pooled order statistic); `Q̂_i` then scales back by the ticker's own width, so
a wide raw interval gets a proportionally larger correction. The pooled
quantile is one number per `(t, H)`, so the cost per origin is one
partial sort, not one per ticker.

Nothing in steps 1-7 reads a value that is not realized by `t`. The outcome
`y_fwd_Hd(i, t)` for the interval written at `t` is not used until origin
`t + H`.

## 3. Zero-Truncation

The `max(..., 0)` in step 2c is the zero-truncation. Without it, a
calibration observation where `y` is inside the raw interval would
contribute a negative score (the margin by which it is inside), which
could pull the conformal quantile below zero.

With truncation every score is `>= 0`, so the order statistic satisfies
`q_norm >= 0`, `Q̂ >= 0`, and the calibrated interval `[q_05 - Q̂, q_95 + Q̂]`
always contains the raw interval. TCP can therefore widen the raw GSPQR
interval but never narrow it. If the raw interval over-covers, `Q̂` is 0 and
the over-coverage is left uncorrected; this is accepted. The TCP paper uses
zero-truncated CQR scores.

**Interaction with the RM offset.** Step 7 adds `C_t` (clipped to
`[-0.5, 0.5]`) after the order statistic. The "never narrower than raw"
property holds for `Q̂` of step 6; with TCP-RM enabled the applied
`Q̂ + C_t` can be negative, and then the interval is narrower than raw
(never inverted: step 7 floors at `-w/2`). Whether to floor `Q̂ + C_t` at 0
is an open design decision (flooring removes the only mechanism that can
correct over-coverage); until it is decided the pipeline does not floor at
0, and the diagnostic below exposes it.

**Diagnostic.** Each run logs, per horizon, the fraction of rows with
`Q̂ == 0` (step 6) and the fraction with `Q̂ + C_t <= 0` (applied). The
backtest writes them to `evaluation_metrics` as `qhat_zero_frac` and
`qhat_applied_nonpos_frac`. They are recoverable from stored columns:
applied `Q̂ = q_05 - q_hat_lower`, and `Q̂ = applied Q̂ - rm_offset`.

## 4. Per-Ticker Option and the Pooled Fallback

**Option: `per_ticker`** (`calib_method = 'per_ticker'`). Ticker `i`'s own
`m = 60` most recent matured dates, un-normalized scores in return units,
`k = ceil((m + 1)(1 - α))`. Use it only as a comparison: for `H >= 63` the
60 labels overlap almost entirely, the effective sample is about 1 to 2
observations, and the order statistic `k = 58` of 60 is essentially the
largest recent miss. It is not the default for that reason, and the earlier
argument that per-ticker "adds a ticker-specific residual correction" is
dropped: the width normalization of §2 already does that.

**Pooled fallback, defined.** Under the `per_ticker` option, a ticker with
fewer than `m_min = 30` (placeholder) matured stored scores in its window
(recently listed, or fewer than `H + m` days of stored predictions) uses

    Q_hat_i = q_norm(t, H) * w_eff(i, t)

where `q_norm(t, H)` is the pooled normalized quantile of §2 step 4 for the
same origin and horizon, and the row is written with `calib_method =
'per_ticker_fallback_pooled'`. It is never a raw-unit pooled `Q̂` from other
tickers (the units would not match) and never a value from another date. If
the pooled quantile itself fails the counts gate of §2 step 3, no row is
written. Each fallback is logged as a `WARN` in `data_quality.parquet`.

## 5. Robbins-Monro State and Sequencing

The realized outcome for the interval issued at `s` is known at `s + H`.
The offset used at origin `t` therefore reflects coverage errors only
from origins up to `t - H`, and step 5 applies exactly one matured update
per origin per ticker:

    if a row (i, s = t - H, H) exists in tcp_calibration and y_fwd_Hd(i, s) is non-NaN:
        e   = 1{ y not in [q_hat_lower(i,s), q_hat_upper(i,s)] } - α
        n_t = n_{t-1} + 1
        C_t = clip( C_{t-1} + γ_{n_{t-1}} * e , -0.5, 0.5 ),  γ_n = γ_0 / sqrt(n + 1)
    else:
        n_t = n_{t-1};  C_t = C_{t-1}

**State is persisted.** `rm_offset` is `C_t` and `rm_n` is the update count
`n_t`, both written on every `tcp_calibration` row (`SCHEMAS.md` #8,
`rm_n` appended). The state for `(i, H)` at origin `t` is the last row for
that pair with `forecast_date < t`. A ticker with no row at `t - H`
(not in the universe then) or whose target is NaN (delisted before
`s + H`) gets no update.

**The recursion is sequential per `(ticker, horizon)`.** `C_t` depends on
`C_{t-1}` through the whole history, so the TCP backfill runs strictly in
date order within a horizon and cannot be parallelized across dates (it can
across horizons and tickers). The pooled quantile `q_norm(t, H)` is
stateless and can be recomputed in any order. A backfill is resumable: start
at the first month partition not yet written, reading `(rm_offset, rm_n)`
from the last row of each pair in the previous partition. The first RM
update for horizon `H` happens at `t0 + H` (`t0` = first TCP origin,
`03-gspqr-model/PREDICTION-BACKFILL.md` §8); before it `C = 0, n = 0`. The
same rule applies to all horizons; there is no special case for long
horizons.

The offset is per ticker and horizon, in return units. A horizon-level
offset driven by the cross-sectional mean miss is a possible simplification
with a less noisy update; it is not adopted here.

## 6. What Not to Do

- Do not include any date `s > t - H` in the calibration set. Its
  forward return is not realized.
- Do not count `t - H` on a ticker's own rows. Use the global trading
  calendar index (§0).
- Do not use a static calibration set from the beginning of the data.
- Do not recompute historical predictions with the current model to
  build calibration scores. Use the stored as-of values.
- Do not skip the zero-truncation.
- Do not claim the truncation lets TCP shrink intervals or correct
  over-coverage. It makes `Q̂ >= 0`.
- Do not use a fixed learning rate for Robbins-Monro.
- Do not pool the calibration across horizons. Each horizon has its own
  calibration scores and its own window.
- Do not pool un-normalized scores across tickers. Divide by the raw interval
  width first.
- Do not update the Robbins-Monro offset with an outcome that has not
  matured.
- Do not forget to clip the Robbins-Monro offset.
- Do not run the Robbins-Monro backfill out of date order, and do not restart
  it from `n = 0` when stored state exists.
- Do not call `q_hat_lower` / `q_hat_upper` the 5% / 95% quantiles, and do
  not score them with pinball loss at `τ = 0.05 / 0.95`.

## 7. Verification

- [ ] A test with synthetic data and known coverage produces the
      expected `q_norm` and `Q̂`
- [ ] A test asserts every calibration date `s` satisfies `s + H <= t`
      for each horizon, with `t - H` taken on the global calendar (a ticker
      with missing days gets the same window as a complete one)
- [ ] A property test truncates the price series at `t` and asserts the
      interval written for origin `t` is unchanged (no unmatured data used)
- [ ] A test asserts scores are zero-truncated and `Q̂ >= 0` (the
      calibrated interval contains the raw interval when RM is off)
- [ ] A test asserts the pooled quantile is invariant to ticker order and that
      scaling one ticker's returns and quantiles by a constant leaves its
      normalized scores unchanged
- [ ] A test asserts the fraction of rows with `Q̂ == 0` is logged per
      horizon and equals the fraction computed from stored columns
      (`q_05 - q_hat_lower - rm_offset == 0`)
- [ ] A test asserts the Robbins-Monro update uses `γ_0 / sqrt(n + 1)`, only
      matured outcomes, and persists `rm_n`; a backfill split into two runs
      (resumed from the stored state) is identical to one run
- [ ] A test asserts the offset is clipped to `[-0.5, 0.5]` and the applied
      correction never inverts the interval
- [ ] A test asserts calibration reads stored as-of predictions, not a
      re-run of the current generation, and never reads `split`
- [ ] A test asserts the pooled fallback uses `q_norm(t, H)` times the
      ticker's own width and sets `calib_method` accordingly
- [ ] Empirical coverage on the walk-forward track is within ±2pp of nominal,
      reported by date block and by raw-width tercile
