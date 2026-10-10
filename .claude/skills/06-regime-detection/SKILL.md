---
name: regime-detection
description: Gaussian HMM regime detection for the portfolio gate. Load
  when touching regime/src/, HMM fitting, Viterbi decoding, or regime factors.
---

# Regime Detection

The regime gate answers one question: is the market currently in a
state where the model's predictions should be trusted at full size, at
half size, or not at all? It does not predict returns. It classifies
the current volatility environment.

## 1. What the Regime Gate Produces

For each trading date, one row in `regime_states.parquet`:

| Output | Meaning |
|---|---|
| `regime_low_vol_prob` | P(low volatility regime), **filtered** (forward-only) |
| `regime_high_vol_prob` | P(high volatility regime), filtered |
| `regime_crisis_prob` | P(crisis regime), filtered |
| `viterbi_state` | Online Viterbi state at that date (low_vol / high_vol / crisis) |
| `regime_factor` | Multiplier applied to **new capital** (1.0 / 0.5 / 0.0) |
| `confirmed_state`, `fit_date` | Appended: the 2-day-confirmed state; the month-end of the fit used |

The `regime_factor` is what the portfolio layer consumes. See §5.

**No lookahead.** A row for date `t` is a function of returns up to `t` and
of an HMM fitted on returns up to a month-end `m < t`. The probabilities are
filtered (forward recursion only) and the state is the last element of the
Viterbi path on the data through `t`. Smoothed (forward-backward) values are
never stored for past dates, and a row is never rewritten when the model is
refitted (`HMM-PROCEDURE.md` §3). A backfill of 15 years is a walk-forward:
refit at each month-end, write the rows for the next month with that fit.

## 2. Why a Regime Gate

GSPQR produces calibrated intervals. Those intervals are valid
**conditional on the training distribution**. When the market shifts
regime, the training distribution no longer describes the current
environment, and the intervals become unreliable until the model
adapts.

The regime gate is a blunt instrument for this problem. It does not
fix the model. It reduces exposure when the model's assumptions are
most likely to be violated. Three regimes are enough to capture the
distinction that matters: calm, turbulent, and crisis.

## 3. Model Specification

**3-state Gaussian HMM** fit to daily log returns of the equal-weighted
universe.

| Component | Specification |
|---|---|
| States | 3 |
| Emissions | Gaussian, full covariance |
| Features | Daily log return of the equal-weighted point-in-time eligible universe (single feature; `HMM-PROCEDURE.md` §1a) |
| Training window | Rolling 504 days (2 years) ending at the fit date, refit monthly |
| Decoding | Causal only: online Viterbi for the state; filtered (forward) probabilities |

The three states are labeled post-fit by their volatility:

| State | Label | Typical volatility |
|---|---|---|
| Lowest variance | `low_vol` | ~10-15% annualized |
| Middle variance | `high_vol` | ~20-30% annualized |
| Highest variance | `crisis` | ~40%+ annualized |

**Label switching:** The EM algorithm can converge with states swapped.
The fit procedure relabels states by their emission variance after
convergence. See `HMM-PROCEDURE.md` §5.

## 4. Confirmation Rule

A single day's Viterbi state does not trigger a regime change. The
system requires **2 consecutive days** in the new state before the
`regime_factor` updates.

**Why:** HMM state transitions flicker. A single bad day can flip the
Viterbi path without indicating a real regime change. The confirmation
rule trades a one-day lag for a large reduction in false transitions.
Research on regime-switching strategies notes that excessive regime
shifts increase transaction costs, and controlling false alarms is a
practical concern.

The `regime_states.parquet` stores the raw (online) Viterbi state in
`viterbi_state`. The `regime_factor` column (and `confirmed_state`) stores the
confirmed state after the 2-day rule: the confirmed state changes when the
raw state has been the same on 2 consecutive days and differs from the
confirmed state.

## 5. Regime Factor Mapping

| Confirmed state | `regime_factor` | Effect on sizing |
|---|---|---|
| `low_vol` | 1.0 | Full size |
| `high_vol` | 0.5 | Half size |
| `crisis` | 0.0 | No new positions |

The factor scales the **increase** in a position's weight, and is applied
**last**, after the optimizer, conviction scaling, renormalization and
caps (`08-portfolio-optimization` §5):

    w_target = min(w_current, w_final) + regime_factor * max(w_final - w_current, 0)

- `regime_factor = 1.0`: `w_target = w_final`.
- `regime_factor = 0.5`: additions are half-sized.
- `regime_factor = 0.0`: no additions at all. Holdings are kept, but a
  position the optimizer itself wants smaller is still reduced.

The regime never forces a sale. Sales come only from the optimizer or the
veto and remain subject to the lock. The factor is applied after
renormalization on purpose: a factor applied before a renormalize-to-0.98
step is cancelled by it (and `0.0` divides by zero). The weight not
deployed stays in cash; it is not redistributed.

## 6. What Not to Do

- Do not use more than 3 states. The economic labels become
  ambiguous beyond three, and the model overfits.
- Do not use a single day's Viterbi state. Apply the 2-day
  confirmation rule.
- Do not label states by their index. Label by emission variance
  after fit. See `HMM-PROCEDURE.md` §5.
- Do not fit the HMM on the full history and evaluate in-sample.
  Use a rolling window ending before the dates it is applied to
  (`HMM-PROCEDURE.md` §2).
- Do not store smoothed (forward-backward) probabilities or the interior of
  a Viterbi path for past dates. Historical rows are filtered values from a
  model fitted on earlier data.
- Do not compute the universe return by differencing prices across tickers.
  It is the mean of within-ticker log returns over the point-in-time
  eligible set (`HMM-PROCEDURE.md` §1a).
- Do not assume the HMM captures directional regimes. It captures
  volatility regimes. A `high_vol` state can occur in a bull or bear
  market.
- Do not use the regime gate as a return predictor. It is a risk gate.

## 7. Verification

Before deploying a regime change:

- [ ] A test asserts 3 states with distinct variances after fit
- [ ] A test asserts state relabeling by variance
- [ ] A test asserts the 2-day confirmation rule
- [ ] A lookahead test: truncating returns at `t` leaves the row for `t`
      unchanged, and every row's `fit_date < date`
- [ ] A test asserts `regime_factor` mapping (1.0 / 0.5 / 0.0)
- [ ] A test asserts `regime_factor = 0.0` produces no target above the
      current weight, and no division by zero
- [ ] A test asserts the regime gate is applied after renormalization
      and caps, and that weights sum to <= 0.98
- [ ] `regime_states.parquet` schema matches the registry
- [ ] The HMM's log-likelihood is monotonically increasing during EM

## 8. Related Skills

- `08-portfolio-optimization` — consumes `regime_factor`
- `11-rust-event-bus` — `RegimeFlip` event is emitted on confirmed changes
- `13-parquet-lake` — schema for `regime_states.parquet`
- `14-backtest-protocol` — evaluates coverage per regime