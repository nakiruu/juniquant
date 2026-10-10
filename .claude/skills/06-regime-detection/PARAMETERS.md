# Regime Parameters

The exact parameter values for the Gaussian HMM.

## 1. Model Parameters

| Parameter | Value | Rationale |
|---|---|---|
| Number of states | 3 | Calm / turbulent / crisis |
| Covariance type | `full` | Each state has its own variance |
| Emission distribution | Gaussian | Baseline; Student-t is an alternative |
| Feature | Daily log return | Single feature, universe-level (point-in-time eligible set, `HMM-PROCEDURE.md` §1a) |
| Training window | 504 days (2 years) | Long enough for stable parameters |
| Refit cadence | Monthly, at month-end close `m`; the fit applies to dates in `(m, m_next]` | Regimes shift slowly; a row never uses a fit that saw its own date |
| Stored probabilities | Filtered (forward-only) | Smoothed values would use later returns |

## 2. Training Window

**Rolling 504 days.** The HMM is refit monthly (at the month-end close) on
the trailing 504 trading days of equal-weighted universe returns ending at
the fit date. This window is long
enough to observe each regime at least once in most market conditions,
but short enough to adapt to structural shifts.

**Why not expanding:** An expanding window weights old regimes
equally with recent ones. A 2008 crisis observation would influence
the model in 2026. The rolling window forgets old data.

**Why not shorter:** With fewer than ~400 days, the crisis state may
not appear in the training data, and the model cannot learn it.

## 3. Features

The HMM uses **one feature: daily log returns of the equal-weighted
universe.**

The equal-weighted universe is the mean daily log return across the tickers
that are **eligible on that date** (point-in-time, from the `universe`
dataset) and have a valid adjusted close on that date and the previous
trading day. Each return is a within-ticker log return; the mean is across
tickers per date. This is the market-level signal. It is the same series the
`vol_60d_res` feature regresses on. It inherits the free-data survivorship
bias of the universe (`01-data-ingestion`).

**Why not volatility directly:** Volatility is a function of returns.
Feeding raw volatility into the HMM adds no information and can
destabilize the fit. The HMM learns the volatility structure from the
return distribution.

**Why not multiple features:** Multivariate Gaussian HMMs with more
than one or two features are prone to overfitting on financial data.
The single-feature model is more stable. Research on HMM feature
selection notes that redundant features destabilize HMM states by
inflating covariance estimates.

## 4. Confirmation Rule

| Parameter | Value |
|---|---|
| Confirmation days | 2 |
| Applied to | `regime_factor` and `confirmed_state` |
| Not applied to | `viterbi_state` (stored raw, online) |

The raw online Viterbi state is stored for analysis. The confirmed state
(driving `regime_factor`) switches when the raw state has been the same on 2
consecutive days and differs from the confirmed state.

## 5. State Relabeling

After EM converges, states are relabeled by emission variance:

1. Compute each state's variance (diagonal of the covariance matrix).
2. Sort states by variance ascending.
3. Lowest → `low_vol`, middle → `high_vol`, highest → `crisis`.
4. Update the transition matrix and emission parameters to match the
   new labeling.

This ensures the `regime_factor` mapping is stable across refits. EM
can converge with states in any order; the relabeling fixes the
mapping.

## 6. What Is Not Tuned

- **3 states.** Fixed.
- **Gaussian emissions.** Fixed (Student-t is a skill amendment).
- **Single feature.** Fixed.
- **504-day rolling window.** Fixed.
- **2-day confirmation.** Fixed.

If a tuning pass wants to change one of these, it is a skill amendment.