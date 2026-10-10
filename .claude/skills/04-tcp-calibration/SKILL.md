---
name: tcp-calibration
description: Temporal Conformal Prediction for GSPQR quantiles. Load when
  touching models/src/tcp/, conformity scores, rolling windows, or the
  Robbins-Monro offset.
---

# TCP Calibration

TCP wraps GSPQR. It does not change the base model. It reads the raw
quantiles from `gspqr_predictions.parquet` (the stored as-of predictions of
the walk-forward generations, `03-gspqr-model/PREDICTION-BACKFILL.md`),
computes conformity scores on a rolling window of matured dates pooled
across tickers, and produces calibrated interval bounds written to
`tcp_calibration.parquet`.

## 1. What TCP Produces

For each ticker, forecast date, and horizon:

| Output | Meaning |
|---|---|
| `q_hat_lower` | Lower endpoint of the calibrated interval (target coverage 95%) |
| `q_hat_upper` | Upper endpoint of the calibrated interval (target coverage 95%) |
| `rm_offset` | Robbins-Monro offset applied (0.0 if TCP-RM disabled) |
| `rm_n` | Number of matured Robbins-Monro updates behind `rm_offset` (state; appended) |
| `calib_width` | `q_hat_upper - q_hat_lower` |

The calibrated interval is:
- `q_hat_lower = q_05 - Q̂`
- `q_hat_upper = q_95 + Q̂`

Where `Q̂` is the conformal quantile of the calibration scores (or the
Robbins-Monro adjusted threshold in TCP-RM).

**Naming.** The raw `[q_05, q_95]` is a **90% base interval**. The conformal
step targets 95% coverage of the *calibrated* interval, so `q_hat_lower` and
`q_hat_upper` are the endpoints of a 95%-coverage interval (about the 2.5%
and 97.5% quantiles for a well-calibrated model). They are **not** the 5%
and 95% quantiles, even though the constitution (§9) and the schema notes
call them "5% and 95% bounds"; read those words as "the bounds of the
interval built on `q_05` and `q_95`" (a wording fix is proposed as a
constitutional amendment). Their metrics are coverage and the Winkler score
at `α = 0.05`, not pinball loss at `τ = 0.05 / 0.95` (pinball applies to the
raw quantiles `q_τ`).

## 2. The Two Variants

**TCP (standard).** Rolling split-conformal calibration. At each forecast
origin, the calibration window is a rolling set of the most recent `M_d`
matured dates, with the scores of all tickers pooled and normalized by the
raw interval width. The conformal threshold is the empirical quantile of the
pooled scores (default). A per-ticker window of `m = 60` observations is an
option (`CALIBRATION.md` §4).

**TCP-RM.** TCP plus an online Robbins-Monro offset. The offset adjusts
the conformal threshold based on recent coverage error, using a decaying
learning rate (`γ_0 / sqrt(n + 1)`, a slowly decaying tracking gain; see
`PARAMETERS.md` §4 for why this is not a strict Robbins-Monro schedule). This steers the actual coverage toward the
target in real time. The offset and its update count are persisted per
`(ticker, horizon)` (`rm_offset`, `rm_n`); the recursion is sequential in
date order (`CALIBRATION.md` §5).

The pipeline uses **TCP-RM by default**. See `PARAMETERS.md` for the
decay schedule.

## 3. Why TCP Over Static CQR

Static CQR (Conformalized Quantile Regression) assumes exchangeability.
Financial time series are not exchangeable — volatility clusters, regimes
shift, and the distribution of returns is not stationary.

TCP addresses this with a **rolling window**. At each forecast origin,
the calibration set is the most recent `M_d` matured dates, not a
fixed split from the beginning of the data. This means the conformal
threshold `Q̂` follows recent coverage failures: it grows when the raw
GSPQR interval has recently missed, and falls back toward zero when it
has not.

**What the adaptation can and cannot do.** Scores are zero-truncated
(§4), so `Q̂ >= 0` and the calibrated interval is **never narrower than
the raw GSPQR interval** `[q_05, q_95]`. TCP can only widen the raw
interval (or leave it unchanged when `Q̂ = 0`). Width can still fall over
time, but only because the raw GSPQR interval itself narrows and `Q̂`
returns to 0; it cannot fall below the raw width. Consequently TCP does
not repair a raw interval that over-covers; it only repairs under-coverage.
(With TCP-RM the applied offset `C_t` is added after the order statistic
and can be negative, so the final applied `Q̂ + C_t` can be below zero;
see `CALIBRATION.md` §3.) This is a deliberate one-sided choice, and the
diagnostic in §4a measures how often the correction is inactive.

Empirical results reported in the TCP paper: TCP attains 95.23% coverage
on the S&P 500 at a 95% nominal level. Those results are
for the paper's setup, not a measurement of this pipeline; this pipeline's
coverage is checked in `14-backtest-protocol`.

## 4. Conformity Scores

See `CALIBRATION.md` for the exact formulas. The summary:

For a calibration observation with realized return `y` and raw GSPQR
quantiles `q_05` and `q_95`:

    s_lower = q_05 - y        (if y < q_05, else 0)
    s_upper = y - q_95        (if y > q_95, else 0)
    score   = max(s_lower, s_upper)

In the default pooled method each score is divided by the raw interval width
`w = q_95 - q_05` (floored at a fraction of the date's median width) before
pooling across tickers, so `Q̂_i = q_norm × w_i`. The per-ticker option
keeps scores in return units.

The score is **zero-truncated**: observations inside the raw interval
contribute zero instead of a negative margin. The effect is that the
conformal quantile `Q̂` is `>= 0`, so the calibrated interval is never
narrower than the raw GSPQR interval. A signed (untruncated) CQR score
would allow `Q̂ < 0` and shrink an over-covering raw interval; this
pipeline does not. The TCP paper uses a zero-truncated CQR score
.

## 4a. Diagnostic: How Often Is TCP Inactive?

Because of the truncation, `Q̂ = 0` means the raw GSPQR interval already
covered at least `k` of the `n` pooled calibration scores and TCP changed
nothing. The fraction of origins with `Q̂ = 0` is logged by every
calibration run (per horizon) and reported in `evaluation_metrics`:

| Metric | Definition |
|---|---|
| `qhat_zero_frac` | Fraction of `(ticker, origin)` rows with the conformal `Q̂` (`CALIBRATION.md` §2 step 6, before the RM offset) exactly 0 |
| `qhat_applied_nonpos_frac` | Fraction with the applied `Q̂ + C_t <= 0` (differs from the above only with TCP-RM) |

A high `qhat_zero_frac` (for example above 0.5) says GSPQR is already
conservative and TCP is mostly a pass-through; a value near 0 says the
raw interval is regularly missing. Neither is an error, but a sudden
change is a regime or data signal. A positive `qhat_applied_nonpos_frac`
exceeding `qhat_zero_frac` means the RM offset is narrowing intervals.

## 5. The Rolling Window

At forecast origin `t` and horizon `H`, only dates whose forward return
has been realized may be used:

    Cal window:  dates s in [t - H - M_d + 1, t - H]     (so s + H <= t)

Where `M_d = H` in the default (placeholder, `PARAMETERS.md` §1) and `m = 60`
in the per-ticker option. The window is shifted back by `H`, so each horizon
has its own window, and the calibration set is the `M_d` most recent
*matured* dates. `t - H` and `s + H` count on the **global
trading-calendar index**, not on a ticker's own rows (`CALIBRATION.md` §0).
GSPQR is trained monthly (walk-forward generations) and TCP calibrates
daily, so TCP has no train window of its own; it reads the stored as-of
predictions.

**Critical:** no observation whose outcome is not yet known at `t`
enters the calibration for origin `t`. The window `[t-m, t-1]` is not
sufficient: for `H >= 63` none of its returns have matured. This is the
point-in-time requirement (constitution §3). See `CALIBRATION.md` §1 for
the maturity rule, the as-of prediction rule, and the dependence caveat.

**Why pooled by default.** Per ticker, `m = 60` labels at `H >= 63` overlap by
`(H-1)/H`, so the effective sample is about 1 to 2. The default pools the
scores of all tickers, each divided by its raw interval width, so the order
statistic rests on tens of thousands of scores; the market-wide dependence
remains, so the effective number of independent episodes is only about 2,
and coverage is checked empirically (`CALIBRATION.md` §1).

**Survivorship.** Rows flagged `survivorship_gap` (target NaN,
`02-panel-construction/TARGETS.md` §3) never enter a calibration window; the
excluded count is reported, and coverage measured on the survivor-tilted
universe carries the survivorship caveat (`01-data-ingestion/SURVIVORSHIP.md`,
`14-backtest-protocol` §6h) in every report built on `tcp_calibration`.

## 6. What Not to Do

- Do not use a static calibration split. TCP requires a rolling window.
- Do not include any date `s > t - H` in the calibration window. Its
  forward return is not realized. The window is `[t-H-M_d+1, t-H]` on the
  global calendar index.
- Do not build calibration scores from a re-run of the current GSPQR
  generation. Use the predictions stored on each date `s`.
- Do not use non-zero-truncated scores. The truncation keeps `Q̂ >= 0`:
  the calibrated interval is never narrower than raw GSPQR.
- Do not describe TCP as shrinking intervals in calm periods or as
  correcting over-coverage. It only widens (RM offset aside).
- Do not update the Robbins-Monro offset with a fixed learning rate.
  Use the decaying schedule (`γ_n = γ_0 / sqrt(n + 1)`) and update only
  with matured outcomes, in date order, resuming from the persisted
  `(rm_offset, rm_n)`.
- Do not default to per-ticker calibration at `H >= 63`. Use the pooled
  normalized default; per-ticker is an option with a defined pooled fallback
  (`CALIBRATION.md` §4).
- Do not call the calibrated endpoints the 5% / 95% quantiles, and do not
  score them with pinball loss.
- Do not overwrite `tcp_calibration.parquet`. Append to the current
  partition, atomic rename.

## 7. Verification

Before deploying a calibration change:

- [ ] Every calibration date satisfies `s + H <= t` (maturity rule, global
      calendar index)
- [ ] Calibration reads stored as-of predictions, not a re-run
- [ ] Conformity scores are zero-truncated, and the conformal `Q̂ >= 0`
      for every row (so before the RM offset the calibrated interval
      contains `[q_05, q_95]`)
- [ ] `qhat_zero_frac` and `qhat_applied_nonpos_frac` are logged per
      horizon by the calibration run and written to `evaluation_metrics`
- [ ] The Robbins-Monro offset uses the decaying schedule and `rm_n` is
      persisted; a resumed backfill equals a single run
- [ ] Empirical coverage on the walk-forward track is within ±2 percentage
      points of nominal (CPCV paths do not evaluate TCP,
      `14-backtest-protocol` §2)
- [ ] Interval width is reported against raw GSPQR and a static CQR
      baseline on the same data (no claim that TCP is narrower than
      either; it is at least as wide as raw GSPQR before the RM offset)
- [ ] `tcp_calibration.parquet` schema matches the registry

## 8. Related Skills

- `03-gspqr-model` — provides the raw quantiles and the walk-forward
  as-of history (`PREDICTION-BACKFILL.md`)
- `05-evt-tails` — consumes the calibrated residuals
- `13-parquet-lake` — schema for `tcp_calibration.parquet`
- `14-backtest-protocol` — evaluates coverage