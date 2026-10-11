# HMM Procedure

The step-by-step algorithm for fitting the Gaussian HMM and decoding
regimes **without lookahead**. A regime row for date `t` is a function of
returns up to `t` and of a model fitted on returns up to a fit date `m < t`.
Nothing about `t` is ever revised by later data.

## 1. Inputs

For a given fit date `m` (a month-end close):

- Daily log returns of the equal-weighted **point-in-time eligible
  universe** for the trailing 504 trading days ending at `m` (§1a).
- The previous fit's parameters (for warm-start, optional).

### 1a. The universe return series

For each trading date `t` (global trading-calendar index):

    r_u(t) = mean over tickers i in E(t) of  ln( adj_close_i(t) / adj_close_i(t-1) )

where `E(t)` is the set of tickers that are **eligible on `t`** in the
universe dataset (`13-parquet-lake/SCHEMAS.md` #1, `eligible = true` at
`date = t`, using only information available at `t`) and that have a valid
adjusted close on both `t` and the previous trading day on the global
calendar. The return is computed **within each ticker's own series** and then
averaged across tickers per date. It is never a difference taken across rows of
different tickers, and a gap in a ticker's history gives no return for that
ticker (not a return across the gap). The same series is the "equal-weighted
universe return" of the `vol_60d_res` feature (`02-panel-construction`); there
is one definition. The universe is point-in-time (the membership and
eligibility known at `t`), not today's constituents. It still inherits the
remaining survivorship bias (`01-data-ingestion`, coverage report): delisted
names that the price source (EODHD, `DECISIONS.md` D15) does not cover are
missing, which understates the volatility of the universe, and the regime thresholds should be read with
that in mind.

Adjusted prices are computed in Julia from the raw prices, splits and
dividends (`02-panel-construction/FEATURES.md`; verify the helper name).

## 2. Fitting (monthly, at month-end `m`)

A fit uses only `r_u` on the 504 trading days ending at `m`. The fit date
`m` is recorded with the parameters and with every regime row it produces.

```julia
# Own implementation of a 1-D, 3-state Gaussian HMM (scripts/fit-regime.jl).
# HiddenMarkovModels.jl is not required for fitting or filtering; its API
# (baum_welch/forward/viterbi names and signatures) is unverified (verify) and
# is not relied on.
hmm = em_fit(returns_504; n_states = 3, max_iter = 100, tol = 1e-6)
hmm = relabel_by_variance(hmm)
```

**Initialization.** Split the 504 returns into three groups by terciles of
`|r - mean(r)|` (large moves, middle, small moves) and use each group's mean
and variance, with a persistent transition matrix (0.95 on the diagonal).
(An initialization that splits by the *return level* separates down days from
up days, which is not a volatility split; do not use it.) For a refit,
warm-start from the previous parameters if the previous fit converged.

**Convergence.** EM stops when the log-likelihood improves by less than
`1e-6` or after 100 iterations. The log-likelihood must be monotonically
non-decreasing (up to floating-point noise). If it decreases, the fit is
invalid and the previous parameters stay in force (WARN).

**Parameters persisted.** `models/regime/hmm_<fit_date>.json` (log record,
JSON is allowed): `fit_date`, window start, initial probabilities, transition
matrix, state means and variances in the **relabeled** order, final
log-likelihood, convergence flag, iterations.

## 3. Decoding: Filtered and Online, Never Smoothed

For a date `t` with `m` the latest fit date strictly before `t`
(dates in `(m, m_next]` use the fit of `m`), take the observations from the
start of the fit window through `t`: `r_u(w0..t)`, with `w0 = m - 503`.
Two causal quantities are computed by forward recursions only:

**Filtered state probabilities** (the `regime_*_prob` columns):

    alpha_t(j) ∝ ( sum_i alpha_{t-1}(i) A(i,j) ) · b_j(r_t),   alpha_{w0}(j) ∝ pi(j) b_j(r_{w0})

stored as `P(S_t = j | r_{w0..t}, θ_m)`. These use no observation after `t`.

**Online Viterbi state** (the `viterbi_state` column):

    delta_t(j) = max_i [ delta_{t-1}(i) + ln A(i,j) ] + ln b_j(r_t)
    viterbi_state(t) = argmax_j delta_t(j)

the last element of the Viterbi path decoded on the data through `t`. It is
causal. It is **not** the element at `t` of a path decoded over a longer
window, which would use later observations and is a smoothed value. It can
differ from the argmax of the filtered probabilities; both are stored.

**Smoothed (forward-backward) probabilities are never stored as regime
rows.** A forward-backward pass over a window gives, for an early date in the
window, probabilities that depend on later returns. Writing those into
`regime_states` for past dates is lookahead: historical rows would look
cleaner than the signal the live system actually had. Smoothed values are
allowed only for the fit diagnostics of §4 on the fit window.

**Historical rows.** A historical regime row for date `t` is exactly what the
procedure above produces at `t` with the latest fit `m < t`. The backfill
therefore walks the dates forward, refitting at each month-end `m`, and
writes the rows for `(m, m_next]` with that fit. Past rows are never
rewritten. A re-run reproduces them (same inputs, deterministic EM).

**First rows.** The first fit needs 504 returns; returns start on day 2, so
the first fit date is the first month-end on or after day 505 and the first
regime row is the next day (about day 526 of a 15-year panel). Before that the
regime is undefined and no row exists. The first GSPQR prediction is later
(day 820), so portfolio backtests are unaffected.

## 4. State Analysis

After each fit, compute per-state statistics on the training window (this
may use smoothed state assignments of that window; it is diagnostic and not
stored in `regime_states`):

| Statistic | Computation |
|---|---|
| Mean return | `mean(returns[state == i])` |
| Variance | `var(returns[state == i])` |
| Annualized volatility | `sqrt(variance × 252)` |
| Occupancy | `count(state == i) / length(returns)` |
| Persistence | Diagonal of transition matrix |

These confirm the states are economically distinct and the labels sensible.

**Identification is weak at 504 observations.** A scratch simulation (one
504-day draw from a chain with daily volatilities 0.6% / 1.2% / 3.0%, a port
of the EM, filter and online Viterbi of `scripts/fit-regime.jl` run outside the
repository) gave a monotone log-likelihood, rows summing to 1 and a causal
filter and Viterbi state, but the fitted states matched the true ones
loosely (the true low-volatility state was split across the fitted low and
middle states). The labels name fitted variance buckets, not known regimes;
the occupancy, volatility and persistence statistics above are what to read
before trusting the gate, and a fit with a state of occupancy below 3% should
raise a `WARN`.

## 5. Relabeling

After the fit, relabel states by emission variance:

```julia
function relabel_by_variance(h)
    order = sortperm(h.var)                 # ascending: low_vol, high_vol, crisis
    # permute pi, rows and columns of A, means and variances by `order`
    # and RETURN the permuted model. Computing `order` and discarding it is
    # a no-op relabel, which is the bug the earlier script had.
end
```

**Why this matters:** EM can converge with the states in any order.
Without relabeling, `state 1` might be `crisis` in one fit and
`low_vol` in the next. The `regime_factor` mapping would silently
invert. Relabeling by variance prevents this, and it is applied before
any decoding.

## 6. The Confirmation Rule

The raw (online Viterbi) state is stored. The confirmed state requires **2
consecutive days** in the new state:

```julia
# raw[t], confirmed[t-1] known (the stored values of the previous row)
confirmed[t] = raw[t] == raw[t-1] ? raw[t] : confirmed[t-1]
confirmed[first] = raw[first]
```

(Two equal consecutive raw states switch the confirmed state; the earlier
text required three equal states, which contradicts "2 consecutive days".)
The confirmed state drives `regime_factor` (and the stored
`confirmed_state`). The raw state drives `viterbi_state`. The recursion uses
the **stored** previous row, so it is resumable and never revised.

## 7. Warm-Start and Refit

On monthly refit:

1. Load the previous fit's parameters.
2. Initialize EM from the terciles of §2 (as a baseline).
3. Fit EM from that init.
4. Fit EM with the previous parameters as init.
5. Choose the fit with the higher log-likelihood.
6. Relabel by variance.
7. Compare to the previous fit's state occupancy. If the occupancy
   shifted by more than 30%, log a `WARN` (possible structural shift).

The new parameters apply to dates after `m` only. A refit never changes a
row already written.

## 8. What Not to Do

- Do not skip the relabeling step, and do not discard its permutation.
- Do not store smoothed (forward-backward) probabilities, or the Viterbi path
  of a window that extends past the date being written, for past dates.
- Do not use a single day's Viterbi state for the regime factor.
- Do not fit on the full history and evaluate in-sample; a fit at `m` is for
  dates after `m`.
- Do not take the universe return as a difference across rows of different
  tickers.
- Do not use more than 3 states.
- Do not assume the HMM captures direction. It captures volatility.
- Do not ignore a decreasing log-likelihood. It indicates a bad fit.
- Do not overwrite `regime_states.parquet` with only the latest window.
  Append rows for new dates.

## 9. Verification

- [ ] A test with synthetic 3-state data recovers the state sequence
- [ ] A test asserts states are relabeled by variance (the permutation is
      applied to `pi`, `A`, means and variances)
- [ ] A test asserts the 2-day confirmation rule (two equal raw days switch
      the confirmed state; one does not)
- [ ] A test asserts the log-likelihood is non-decreasing
- [ ] A test asserts the transition matrix rows sum to 1
- [ ] A test asserts the filtered probabilities sum to 1 per date
- [ ] A lookahead test: truncating the returns at `t` leaves the stored row for
      `t` (filtered probabilities, online Viterbi state) unchanged, and
      appending later returns does not change any past row
- [ ] A test asserts the row for `t` uses a fit whose window ends before `t`
      (`fit_date < date`)
- [ ] A test asserts the universe return of a date is the mean over the
      eligible set of within-ticker log returns (a ticker with a gap
      contributes nothing for the gap days), and is invariant to ticker order
- [ ] A test asserts a re-run reproduces every stored row
