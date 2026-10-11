---
name: backtest-protocol
description: Walk-forward evaluation with purged cross-validation.
  Load when touching backtest/src/ (cpcv.jl, metrics.jl, paths.jl), performance
  metrics, or any code that scores the system on historical data.
---

# Backtest Protocol

The backtest is the only honest measure of whether the system works.
Every other skill builds the system. This skill evaluates it.

## 1. The Core Problem

Financial time series are not exchangeable. Standard k-fold
cross-validation — shuffling observations into random folds — leaks
future information into the training set and produces inflated
performance estimates.

Walk-forward analysis addresses this by training on the past and testing
on the future. But simple walk-forward has its own problem: the
estimated Sharpe ratio has high variance because a large portion of the
decisions are based on a small portion of the dataset, and a few
observations carry outsized weight.

## 2. The Protocol

The pipeline uses **Combinatorial Purged Cross-Validation (CPCV)**,
introduced by López de Prado. CPCV:

1. Partitions the timeline into `N` contiguous groups.
2. For each combination of `k` test groups, trains on the remaining
   `N-k` groups.
3. Applies **purging** on both sides of each test block (removes training
   observations whose label interval overlaps a test label interval,
   before and after the block) and **embargoing** (removes a further
   `h` training observations after the purged zone that follows each test
   block, to cover residual serial correlation). López de Prado, *Advances
   in Financial Machine Learning*, ch. 7 (verify section numbers).
4. Generates multiple backtest paths by reconstructing the full
   timeline from each combination.

The result is a **distribution** of performance metrics across paths,
not a single point estimate. This directly addresses the high variance
of walk-forward.

### 2a. Two Tracks (which metric comes from which)

CPCV trains a model per split. TCP and EVT cannot be evaluated that way:
they need *as-of* predictions from models trained only on rows matured at each
date, and a monthly walk-forward inside each of 120 splits would be about
about 7,200 monthly generations (the audit's figure: 120 splits x about 60
month-ends), each of up to 21 fits. In-fold TCP/EVT on
that split's own training predictions would be in-sample. So the evaluation
has **two tracks**:

| Track | What runs | Models | Output |
|---|---|---|---|
| **Walk-forward (WF)** | The production pipeline over the whole history: monthly generations -> stored as-of predictions -> TCP -> EVT residuals/tails, plus the walk-forward regime (`03-gspqr-model/PREDICTION-BACKFILL.md`). One path | about 414 horizon-generations, 2,898 fits (an estimate of cost is in that file) | Interval and tail metrics of TCP/EVT, the raw-quantile metrics of the as-of predictions |
| **CPCV** | The raw GSPQR quantile model, **one model per split**, plus the portfolio, lock, cost and regime gate simulation | 120 splits x 21 fits = 2,520 fits (estimate below), fixed `n_estimators`, no early stopping | Distribution over 36 paths of portfolio metrics and raw-model metrics |

**Metrics by track:**

| Metric | Track | Note |
|---|---|---|
| `coverage_95`, `avg_width`, Winkler (alpha = 0.05) of `q_hat_lower/upper` | WF only | The calibrated interval targets 95% coverage; these are its metrics (not pinball) |
| `qhat_zero_frac`, `qhat_applied_nonpos_frac` | WF only | TCP inactivity (`04` §4a) |
| EVT tail metrics (exceedance rates of `q_extreme` at 1% / 99%, `fit_method` and `window_days` mix) | WF only | `05-evt-tails` |
| `coverage_90` (the raw `[q_05, q_95]` interval) | WF and CPCV | Raw model, 90% nominal |
| Pinball at each raw `tau` in the seven quantiles, `rank_ic` of `q_50` | WF and CPCV | CPCV gives the per-path distribution |
| CRPS (approximate, from the seven quantiles) | WF and CPCV | `METRICS.md` §4 |
| Sortino, Sharpe, max drawdown, DSR, turnover, implementation shortfall, lock-binding frequency, spread-cost attribution, cost of the lock | CPCV only | Portfolio simulation; WF has no portfolio |

CPCV evaluates the **raw** quantile model; TCP and EVT outputs never enter a
CPCV path (the optimizer uses `q_50` only, `08-portfolio-optimization`; the
regime gate, conviction and veto need no training on the CPCV folds and use
the walk-forward regime rows, which are forward-only, plus PIT fundamentals).
Interval coverage, Winkler and the tail metrics are therefore **single-path**
numbers: report them by calendar year and by regime with HAC standard errors
(lag >= H, constitution §3), not as a 36-path distribution.

**No double use of the WF track.** The TCP/EVT placeholders
(`04-tcp-calibration/PARAMETERS.md`, `05-evt-tails/PARAMETERS.md`) may be
checked on the WF track. Reserve the last 20% of its origins (placeholder) as a
confirmation window that is not used to choose them, and say which window each
reported number comes from.

## 3. Parameters

| Parameter | Value | Rationale |
|---|---|---|
| `N` (total groups) | 10 | Contiguous blocks of the timeline |
| `k` (test groups per fold) | 3 | `C(10,3) = 120` splits, each training on 7 of 10 groups |
| Backtest paths | 36 | `φ = C(N−1, k−1) = C(9,2) = 36` |
| Purge | `H` trading days on **both** sides of each test block, where `H` is the label horizon of the model being trained (63, 126 or 252) | A training row at `t` has the label interval `[t, t+H]`; it is removed if that interval overlaps any test label interval `[s, s+H]`, which is exactly the rows within `H` days before or after the block (owner decision C1, 2026-10-10) |
| Embargo | `h = max(21, ceil(0.01 * T))` trading days after each test block, beyond the post-block purge; `T` is the panel length in trading days (about 3,780 for 15 years, so `h = 38`) | Residual serial correlation (owner decision C1). See the note below the table |
| Train/validation split within train | 80/20, purged by `H` | For early stopping (`03-gspqr-model/TRAINING.md` §2) |

The path count follows from the split, not the other way round: the
number of full backtest paths is `φ = k/N × C(N,k) = C(N−1, k−1)`. For
`N = 10`: `k = 2` gives 9 paths (45 splits), `k = 3` gives 36 paths
(120 splits), and `k = 8` gives 36 paths from 45 splits but trains each
split on only 2 of 10 groups. Earlier drafts listed "k = 8, 45
combinations, 36 paths"; that trains on 20% of history and was replaced.

**Purge and embargo (decided, owner C1, 2026-10-10; AFML ch. 7).** The two
remove different things:

- **Purge removes label overlap.** A test row at `s` has the label interval
  `[s, s + H]`; a training row at `t` has `[t, t + H]`. They share part of the
  same price path whenever `|t - s| <= H`. For a test block `[a, b]` the
  training rows that overlap some test label are therefore those in
  `[a - H, a - 1]` (front side) and `[b + 1, b + H]` (back side); both are
  removed. `H` is the horizon of the model being trained, so a 63d model is
  purged by 63 days and a 252d model by 252. The old one-sided 252-day purge
  over-purged the short horizons on the front and left the back side to the
  embargo.
- **The embargo covers residual serial correlation** beyond the label
  overlap (volatility clustering, slow-moving features). AFML's rule of thumb
  is `h ≈ 0.01 T`; here `h = max(21, ceil(0.01 * T))`, 38 days for a 15-year
  panel (3,780 x 0.01 = 37.8, rounded up), applied after the back-side purge.
- **Feature lookback is not leakage by itself.** `mom_12m_1m` looks back 252
  days, so a training row shortly after a block has a feature built from
  prices inside the block. That price was public at the row's date and the
  row's label lies after the block, so no test label enters the fit. The
  earlier argument for a 252-day embargo on this ground is withdrawn.

Cost, by simulation (`D` = 3,528 evaluable days, `N` = 10, `k` = 3, group size
352, `h` = 38; estimates). Arithmetic: 7 training groups hold 7 x 352 = 2,464
dates. Of the 9 group boundaries, each is a train-to-test boundary in
C(8,2) = 28 of the 120 splits, so a split has on average 9 x 28 / 120 = 2.1
front boundaries and 2.1 back boundaries. Expected exclusion is
2.1 x `H` + 2.1 x (`H` + `h`) = 2.1 x (2`H` + 38), as long as the front and back
zones of a training group sandwiched between two test blocks do not overlap
(2`H` + 38 <= 352):

| Model horizon `H` | Arithmetic: 2,464 - 2.1 x (2H + 38) | Simulated mean (min) training dates | Share of `D` |
|---|---|---|---|
| 63 | 2,464 - 344.4 = 2,119.6 | 2,120 (1,972) | 60% |
| 126 | 2,464 - 609.0 = 1,855.0 | 1,855 (1,594) | 53% |
| 252 | 2,464 - 1,138.2 = 1,325.8 (zones overlap in sandwiched groups, so this undercounts) | 1,414 (838) | 40% |

For comparison, the withdrawn setting (252-day front purge, 252-day embargo, all
horizons) gave about 1,477 dates for every horizon; the portfolio-horizon (63d)
model now trains on about 640 more dates.

Purging and embargoing apply to training rows only; the test blocks are never
trimmed. Training rows are also restricted to names that are point-in-time
eligible at their date (`03-gspqr-model/TRAINING.md`, owner decision C3).

## 4. Evaluation Metrics

See `METRICS.md`. The pipeline reports:

- **Coverage**: empirical coverage of the TCP-calibrated interval
  (95% target; WF track) and of the raw 90% base interval (both tracks)
- **TCP inactivity**: `qhat_zero_frac` and `qhat_applied_nonpos_frac`
  (`04-tcp-calibration` §4a), written to `evaluation_metrics` (WF track)
- **Winkler Score** at alpha = 0.05: sharpness of the 95% calibrated interval
  (WF track)
- **Pinball Loss**: accuracy of the raw quantiles `q_tau` (not of the
  calibrated endpoints, which are not the 5%/95% quantiles)
- **CRPS**: distributional forecast accuracy (approximate, `METRICS.md` §4)
- **Sortino Ratio**: downside-adjusted return
- **Max Drawdown**: worst peak-to-trough decline
- **Sharpe Ratio**: risk-adjusted return
- **Implementation shortfall**, **turnover**, **lock-binding frequency**
  and **spread-cost attribution** (`METRICS.md` §7a-7d)

Each CPCV-track metric is reported as a **distribution** across the CPCV
paths (mean, standard deviation, percentiles), not a single number. The
WF-track metrics are single-path numbers, reported by year and regime with
HAC standard errors (§2a). Every report carries the header of §6h.

## 5. Walk-Forward vs CPCV

See `WALK-FORWARD.md`. Walk-forward appears in two roles here. As the
**walk-forward track** it is the production pipeline replayed over history
(monthly generations, as-of predictions, TCP, EVT, regime): the only valid
source of TCP/EVT evaluation, and the source of the stored predictions those
layers read. Inside a single GSPQR fit, the 80/20 chronological split is used
for early stopping. CPCV is used for the **portfolio and raw-model
evaluation** because it produces multiple paths and a distribution of
metrics.

The `WalkForwardEstimator` in `PortfolioOptimisers.jl` supports a
`purged_size` parameter (default 5) for the pre-test boundary (unverified,
verify). The pipeline does not rely on it: the walk-forward track is the
generation procedure of `03-gspqr-model/PREDICTION-BACKFILL.md`.

## 6. Implementation

See `scripts/run-backtest.jl`. The backtest:

1. Loads the panel. The `split` column is **not** used by the CPCV track
   (it is for the one-shot held-out research evaluation only,
   `03-gspqr-model/PREDICTION-BACKFILL.md` §4).
2. Builds the CPCV groups with purge and embargo (`DataSplits.PurgedKFold` or
   equivalent; the package and its CPCV support are unverified, verify; a
   hand-written splitter with the tests of §9 is the fallback). The groups
   partition the evaluable dates, from the first date with a feature row and a
   regime row (about day 526 of a 15-year panel) to the last date with a
   63-day label; rows before the first group are always training-eligible,
   subject to the purge.
3. For each of the 120 splits, trains **one** raw GSPQR quantile model per
   (horizon, quantile) on the split's training dates (fixed `n_estimators`
   from `03-gspqr-model/HYPERPARAMETERS.md` §2, no early stopping, the fixed
   seed): 21 fits per split, 2,520 in all (the fits are independent jobs and
   resumable per split). It predicts the split's test blocks and stores them
   for the portfolio horizon (63d by default) under the run directory
   (`$QUANT_ROOT/reports/backtest/<run_id>/cpcv/split=<k>/`, a run artifact,
   not a lake dataset, so it lives under `reports/` and never under `lake/`,
   `13-parquet-lake/IO-CONTRACT.md` §1); the pinball, `coverage_90` and `rank_ic` of every horizon are
   computed in the split job.
4. TCP and EVT are **not** run in CPCV. Their evaluation is the WF track
   (§2a), computed once over the whole history.
5. Runs the portfolio optimizer on the test portion, with the restricted
   list excluded, the lock bounds computed from the simulated order
   history, and the cost-aware formulation (`08-portfolio-optimization`
   §5a-5d). The optimizer's expected return is the raw GSPQR `q_50`
   (constitution §9: never the TCP midpoint).
7. Simulates execution with the tier-aware, spread-based cost model (§6a,
   §6e) and applies the lock rule to every simulated order (§6d). The lock
   is a pure function of the simulated order history, not a state that is
   mutated on fills.
8. Computes returns for the test portion.
9. Joins test portions into backtest paths.
10. Computes metrics per path (CPCV track) and writes the WF-track metrics
    from the stored as-of pipeline; every `evaluation_metrics` row carries
    `run_id`, `track`, `path_id`, `fold` (`13-parquet-lake/SCHEMAS.md` #19).

**Compute (an estimate, not a measurement).** 120 x 21 = 2,520 fits. With the
mean training set (§3) of about 2,120 / 1,855 / 1,414 evaluable dates for the
63d / 126d / 252d models x about 450 eligible tickers, that is about 0.95 /
0.83 / 0.64 million rows per fit (mean about 0.81 million), and an assumed 10 to 60 s per fit on an
8-core machine, that is about 7 to 42 hours. Fitting only the 63d models
(for the portfolio) would be 840 fits; the other horizons exist for their
pinball. Measure five fits and extrapolate before committing the machine.

**Critical:** The portfolio optimizer and the lock rule must run
**inside** each fold. They cannot be pre-computed on the full dataset,
because the lock status at any date depends on prior simulated orders
within that path, and the optimizer's bounds depend on the lock status.

**Timing convention.** A decision at the close of day `t` is executed in
session `t+1`. The placement date is `D = t+1` (the next trading day), the
same convention as live (`10-execution-schwab/EXECUTION-WINDOW.md` §1). The
optimizer at the close of `t` receives the lock status for session `t+1`.

### 6a. Tier-Aware Execution Simulator

The backtest's execution simulator reads the execution tier from the
trailing 5-day mean of the simulated NAV, as the live order generator
does. The tier is not a fixed parameter of the backtest; it evolves with
the simulated account.

The simulator implements the same three tiers as the live order
generator (see `10-execution-schwab/SKILL.md` §2a). Fills are priced
against a **reference mid** for session `t+1` (§6e) and pay the cost model
of `10-execution-schwab/EXECUTION-WINDOW.md` §5:

| Tier | Trailing 5-day mean simulated NAV | Execution model |
|---|---|---|
| Bootstrap | < $2,500 | Fractional market order (eligible names only, §6g; quantity rounded down to 2 decimals): fill at reference mid plus (buy) or minus (sell) the half-spread |
| Transition | $2,500 ≤ NAV < $10,000 | Whole-share limit (quantity rounded down), ladder arm below; for eligible names a fractional market remainder (rounded down to 2 decimals), as in Bootstrap; for ineligible names no remainder, which stays in cash (§6g) |
| Standard | ≥ $10,000 | Whole-share limit (quantity rounded down), ladder arm below |

Fractional quantities are market orders in regular hours only; whole-share
limits are fine in every tier. The simulator never turns a fractional
quantity into a limit order and never rounds a quantity up. Exits follow
amendment 1.13: in Standard a legacy fractional remainder of a full exit is
sold at market (fill at the bid); in Bootstrap any exit of a held position
fills as a market sell.

Ladder arms for the limit tiers:

| Arm | Simulated fill |
|---|---|
| `cross` (headline) | Reference mid ± (half-spread + calibrated adjustment), the capped marketable limit crossing the touch |
| `post`, pessimistic (`p = 0`) | Same as `cross`: the passive stage never fills |
| `post`, optimistic (`p = 1`) | Fill at the passive price (mid or one tick inside), no adverse selection |
| `post`, `p ∈ (0, 1)` | Mixture with an assumed fill probability |

**The mid fill is optimistic.** A limit order resting at or near the mid
may not fill, and a fill at the mid assumes zero spread cost. The earlier
version of this protocol filled limit orders at the mid; that
understated cost and is now only the optimistic bound of the `post` arm.
The `post` arm is modelled with a **fill-probability assumption, which is
an assumption until the live experiment** (`EXECUTION-WINDOW.md` §10)
**produces fills**: it is flagged in every report, run at `p ∈ {0, 0.5,
1}` as a sensitivity bracket, and the **headline numbers use `p = 0`**
(equivalently the `cross` arm) so no unmeasured benefit of passive posting
is in them.

**Spread source.** Every fill carries `spread_source`, one of:

| `spread_source` | Used when | Estimate |
|---|---|---|
| `measured_1m` | `quote_spread_1m.parquet` (`13-parquet-lake/SCHEMAS.md` #27, built forward by the quote recorder, `10-execution-schwab/EXECUTION-WINDOW.md` §2.1) has valid rows for that name and date | Median of `spread_bps_median` over the valid minutes of the execution window; carried or null rows are ignored; no valid minute falls back to `proxy` |
| `proxy` | No recorded quotes for that name and date | The OHLC-proxy / spread-bucket estimate (quintile of the universe's median spread, by dollar-volume proxy), `proxy_method = bucket`; the earlier fixed 5 bps is the last resort, `proxy_method = fixed` |

**The quote history starts at the recording start date.** The Market Data
API has no historical bid/ask, so `quote_spread_1m` exists only from the
day the recorder first ran. A 10-year backtest therefore cannot use
measured spreads for the earlier years: those fills are labelled
**`proxy spread`** and their costs are an estimate, not a measurement.
This is a limitation of the data, not a finding. Results are never pooled
across the two sources without saying so: every metric is also reported
**split by spread source** (`METRICS.md` §8), together with the share of
fills (and of notional) in each source. The `measured_1m` period is the
only part of the backtest whose cost model rests on observed spreads, and
it is short at first; read the headline of a long backtest as mostly
proxy-based. The recording start date and the number of recorded sessions
are written into every report.

Live and simulated tiers agree by construction. Live reads the trailing
5-day mean from the daily ledger, which changes once per day, so every
order in one rebalance sees the same tier. The simulator therefore
computes the tier once per rebalance from the same trailing mean and uses
it for every order in that rebalance. A NAV that crosses a boundary
mid-rebalance does not change the tier until the next rebalance.

### 6b. Starting NAV

The backtest requires a starting NAV. Two configurations are supported:

| Configuration | Starting NAV | Purpose |
|---|---|---|
| Bootstrap-realistic | $1,000 | Reproduces the live account's starting conditions |
| Standard-research | $100,000 | Evaluates the strategy at the scale it was designed for |

Both configurations are run. The Bootstrap-realistic backtest shows what
the system would have done with the actual account. The Standard-research
backtest shows what the strategy does when the tier constraint is removed.

The two backtests are reported separately. A strategy that works at
$100,000 but not at $1,000 is not the same strategy. The Bootstrap tier
exists because the account is small, not because the strategy is different.

### 6c. Fractional Quantity in the Backtest

The backtest's simulated positions are stored as `float64` quantities,
not integers. This is required for the Bootstrap and Transition tiers.
The metrics are unaffected; a position of 0.30 shares contributes 0.30 ×
price to market value, same as any other quantity.

Fractional quantities are rounded **down** to 2 decimal places
(`floor_2dp(x) = floor(100 x) / 100`), the whole-share part of a
Transition order is floored as before, and Standard rounds down to whole
shares. A rounded quantity never exceeds `weight × NAV / price`, so the
cap and the cash buffer survive rounding; the residual is cash. A quantity
of 0.00 is no order.

The ledger in the backtest is simulated using the same
`portfolio_ledger.parquet` schema as the live system. The `n_positions`
column counts nonzero fractional positions. A 0.05-share position counts
as a position.

### 6d. The Lock Rule Inside Every Fold

The same rule as the constitution (`00-construction` §1) and the live
lock module (`09-lock-state-machine`), as a pure function of the
**simulated order history** of the path:

- `D(order)` is the order's **last fill date** (the placement date when it
  fills the same day). A decision at the close of `t` is placed and, for a
  `Day` order, filled in session `t+1`, so `D = t+1`, the next trading day.
  The block runs from the placement day through `D+4`; an order still open
  blocks with no end date (simulated `Day` orders are terminal each
  session). A simulated order that never fills is not counted.
- A counted order blocks the opposite side on the trading days
  `D … D+4`. The first permitted opposite order is on `D + 5` trading
  days. Buy on a Monday: the first sell is the following Monday.
- Only **counted** orders matter: an order with at least one fill (or open
  at a session end, which never happens for `Day` orders). A simulated
  order that does not fill (a `post` arm assumed not to fill) is not
  counted and creates no block.
- A same-direction order is always allowed and extends the block (buy
  Monday and Wednesday: first sell the following Wednesday).
- Trading days come from the computed NYSE calendar (rules plus the
  overrides seed), never from the dates present in `prices.parquet`. The
  rules cover any backtest year from 2022 for Juneteenth and earlier years
  without it; historical ad-hoc closures are in the overrides seed
  (2007-01-02, 2012-10-29, 2012-10-30, 2018-12-05, 2025-01-09; the seed is
  not exhaustive before 2007, `09-lock-state-machine` §4).
- Per day: (1) compute each name's lock status for session `t+1` from the
  history through `t`; (2) the optimizer receives the bounds; (3) orders for
  session `t+1` are generated and each is checked against the **full**
  history, including orders already simulated that session (the optimizer's
  bounds are only a pre-filter); (4) fills are recorded and the history
  extended.
- State per **contiguous test block**: each block of a CPCV path starts
  flat (cash) with an empty lock history, because the model for each block
  comes from a different fold and no portfolio exists before the first
  block. This slightly understates the lock's effect at block starts.
  The last decision of a block (executed after the block ends) is dropped;
  the position is marked to market at the block's last close, with no
  liquidation cost charged.
- The restricted list (`config/restricted.toml`) is excluded from the
  universe in every fold for all dates (the owner's restriction is treated
  as persistent; a start date per entry would need a schema addition).

A bug here is a **compliance** bug, not a modelling nuance: the simulator
must never "cheat" by letting an optimizer output stand that the live lock
would have refused.

### 6e. Cost Model and Optimizer Settings in the Simulator

Everything below follows `10-execution-schwab/EXECUTION-WINDOW.md`; it
is not restated here.

- **Costs.** Tier-aware, spread-based, per name: half-spread plus the
  calibrated adjustment, from `quote_spread_1m` where it exists
  (`spread_source = measured_1m`), else the OHLC-proxy / spread-bucket
  estimate (`spread_source = proxy`) (§6a).
- **Reference mid.** Without intraday bars the simulator uses a proxy for
  the session-`t+1` arrival mid. The base case is the OHLC average of
  session `t+1`; the open and the close are run as sensitivities (they
  bracket the timing assumption). The arrival-mid benchmark of
  `METRICS.md` §7a is this proxy, so the *drift* part of the shortfall is
  determined by the proxy and is not a measurement; only the *spread* part
  (modelled) is meaningful in the backtest.
- **Optimizer.** Cost-aware (`08` §5c), lock-aware bounds on (`08` §5a),
  restricted list excluded (`08` §5b), fractional-eligibility filter in the
  Bootstrap tier (`08` §5b-1), post-optimization no-trade band on
  (`08` §5d), conviction and regime gate as in `08` §5.
- **Band multiplier `c`.** Not tuned on the test paths. It is fixed a
  priori or chosen on the *training* groups of each fold by running the same
  simulator there; the grid over `c ∈ {0, 0.5, 1, 2, 4}` is reported on the
  test paths as a sensitivity, not a selection.
- **Costs applied on the same dates** as the fills: no cost in a day with
  no order; whole-share rounding residuals and 2-decimal rounding residuals
  return to cash.
- **Fractional eligibility** (§6g): in the Bootstrap tier the optimizer's
  universe excludes non-eligible names (`08` §5b-1); in the Transition tier
  ineligible names trade whole shares only.
- **Regime gate and veto** as before.

### 6f. The "Cost of the Lock" Experiment

The lock is contractual and has no override, so its cost is a fact to be
measured, not a parameter to be tuned. The experiment quantifies it by
re-running the same path with the lock switched on and off, and with the
two placement timings.

| Run | Lock | Placement |
|---|---|---|
| R0 | off (analysis only) | in-session |
| R1 | on | in-session (production) |
| R2 | on | evening (placed at the close of `t`, filled in session `t+1`); a counterfactual |

The block is dated by the rule itself (`00-construction` §1): it runs from
placement while the order is open and through 4 trading days after `D`, the
**last fill date**; the first opposite order is `D + 5` trading days. The
simulator dates blocks from the **simulated last fill day**; a simulated
order that never fills is not counted; a partially filled order counts from
its last fill day. There is no placement-day versus fill-day clock choice:
the rule is the later of the two by definition.

R2 differs from R1 precisely in two respects: the execution prices and
slippage of the evening plan (an order filling at the open rather than
inside the window), and the open-order blocking (an evening order is open,
hence blocking, from the evening of `t`, one session before its fill, so a
sell wanted in session `t+1` is refused by the open buy; the block's end
date `D + 5` with `D = t+1` is the same as R1). It is a cost/benefit view
of the evening plan, not a different lock rule.

- R0 exists **only** in the Julia analysis harness. It is a counterfactual
  and must not be reachable from any live path (Julia never places orders;
  the Rust lock module has no off mode; the constitution allows no
  override).
- **Report** the difference per path, `Δ_p = metric(R1, p) − metric(R0, p)`
  for net return, Sortino, max drawdown, turnover
  and lock-binding frequency, as a distribution across the 36 CPCV paths
  (mean, standard deviation, 5th/50th/95th percentiles). CPCV paths share
  test groups, so the 36 differences are not independent; do not report a
  p-value that treats them as i.i.d. The distribution is descriptive.
- Pair the runs by path and by the same trained models: the same
  predictions, only the lock differs.

### 6g. Fractional Eligibility in the Simulator

The live rule (`08` §5b-1, `10-execution-schwab/SKILL.md` §2a): in the
Bootstrap tier a ticker that is not fractional-eligible (or `Unknown`) is
removed from the eligible set before the optimizer, because a whole share is
unaffordable at that NAV; in the Transition tier eligible names may use the
fractional market remainder, ineligible names trade whole shares only (the
sub-share remainder stays in cash and a weight below one share yields no
trade); the Standard tier is unaffected. The simulator applies the same
rule per rebalance, using the tier computed from the trailing 5-day mean
simulated NAV (§6a).

**Assumption, flagged in every report: historical eligibility is unknown.**
Schwab's fractional list has no recorded history, so the simulator applies
the **current** `config/fractional_eligible.toml` to **all** history. This
is a survivorship-like assumption: names that are eligible today (large,
surviving S&P 500 members) are treated as always eligible, and names that
left or never qualified are treated as never eligible. It can bias the
Bootstrap and Transition results toward today's large caps and away from
names that were tradable or untradable at the time. The simulated universe
is also restricted by the list, so Bootstrap-realistic results describe the
eligible subset, not the full IWF/IWP universe. The Standard-research
configuration ($100,000, §6b) is not affected by the filter.

### 6h. Mandatory Report Header: Survivorship and Source Caveats

Every metric report (CPCV tables, WF tables, any figure derived from them)
opens with a header that cannot be omitted:

1. **Survivorship caveat (measured EODHD coverage gaps).** Prices are from
   EODHD, which keeps delisted tickers' history but not every old delisting
   (`DECISIONS.md` D15), and the panel is a point-in-time rebuild from
   holdings snapshots, so names that were delisted, acquired or otherwise
   dropped, and whose prices EODHD does not provide, are missing. Survivorship bias is **documented, not
   fixed**: returns, volatility, interval coverage and Sortino are measured on a
   survivor-tilted universe, usually optimistically, by an amount that is not
   known. The header quotes the measured size of the gap from the data
   coverage report (`01-data-ingestion/SURVIVORSHIP.md`, which now
   exists; the share of universe rows with `exclusion_reason = no_price_data`
   by year, `13-parquet-lake/SCHEMAS.md` #1) and the report's date. If that
   report is missing, the header says "survivorship gap not measured" and the
   report is marked incomplete.
2. **Data window.** First and last date, and the horizon-specific start of
   each layer from the timeline (`03-gspqr-model/PREDICTION-BACKFILL.md` §8).
3. **Track.** Which metrics are WF-track and which CPCV-track (§2a).
4. **Spread-source split.** The share of fills and notional in
   `measured_1m` versus `proxy` (§6a), with the recording start date, as
   already required (`METRICS.md` §8).
5. **Eligibility assumption** (§6g) when the Bootstrap or Transition tier is used.
6. **Trials.** The trial count `N` for the DSR (§7), or "DSR omitted".

A report without items 1 to 4 is not a result.

## 7. The Deflated Sharpe Ratio

The pipeline reports the Deflated Sharpe Ratio (DSR) alongside the raw
Sharpe. The DSR corrects for the number of trials:

    DSR = Φ((SR_max − E[SR_max]) × √(T−1) / σ_SR)

Where `E[SR_max] ≈ √(2 ln N) / √T` is the expected maximum Sharpe under
the null of zero skill, given `N` trials.

**Caveat:** The DSR requires the number of trials `N`. In this pipeline,
`N` is the number of model configurations evaluated during hyperparameter
tuning. The DSR is reported **only when `N` is known and logged**. If
`N` is unknown, the DSR is omitted and the raw Sharpe is reported with
an explicit warning.

## 8. What Not to Do

- Do not use standard k-fold cross-validation. It leaks future data.
- Do not shuffle observations. Chronological order is mandatory.
- Do not report a single Sharpe ratio. Report the distribution across
  CPCV paths.
- Do not tune hyperparameters on the test set. Use walk-forward within
  the training portion.
- Do not report TCP or EVT interval metrics from CPCV paths. They come from
  the walk-forward track.
- Do not score the calibrated endpoints `q_hat_lower/upper` with pinball
  loss at 0.05/0.95, and do not use the TCP midpoint as the median. The
  median is `q_50` (constitution §9).
- Do not use the `split` column in CPCV or in the WF track.
- Do not publish a metric report without the survivorship and source header
  (§6h).
- Do not skip purging. Overlapping label horizons leak information.
- Do not skip embargoing. Serial correlation bleeds across the boundary.
- Do not purge one side only, and do not purge every model by the longest
  horizon: the purge is `H` of the model being trained, on both sides.
- Do not justify a longer embargo by feature lookback (`mom_12m_1m`): a
  feature built from public past prices is not leakage by itself.
- Do not simulate the lock as a state mutated on fills, with `D` taken from
  the fill date in the production run, or from `prices.parquet` dates.
- Do not fill limit orders at the mid in the headline numbers, and do not
  put an assumed fill probability for passive orders into the headline.
- Do not include a restricted ticker in any fold's universe.
- Do not label a proxy-spread fill `measured_1m`, do not pool the two
  spread sources without the split, and do not present proxy-period costs as
  measured.
- Do not model a fractional quantity as a limit order, round it up, or round
  it to nearest; fractional is a market order in regular hours, rounded down
  to 2 decimals.
- Do not hide the eligibility assumption: the current list applied to all
  history must be stated in every report that uses the Bootstrap or
  Transition tier.
- Do not tune the band multiplier `c` (or any cost parameter) on the test
  paths.
- Do not leave the lock-off run reachable from any live path.
- Do not report the DSR without stating `N`. A DSR without `N` is a
  number with a free parameter.

## 9. Verification

- [ ] A test asserts CPCV produces the expected number of paths
- [ ] A test asserts the CPCV job fits exactly 21 models per split, with no
      early stopping and fixed `n_estimators`, and calls no TCP or EVT code
- [ ] A test asserts interval coverage, Winkler and tail metrics are written
      with `track = walk_forward` and CPCV rows carry `path_id`, `fold` and
      `run_id`
- [ ] A test asserts the pinball of the calibrated endpoints is never computed
      and the optimizer input is `q_50`
- [ ] A test asserts every report begins with the §6h header and fails
      without the survivorship section
- [ ] A test asserts the purge removes, for a model of horizon `H`, exactly
      the training rows within `H` days **before and after** each test block
      (every removed row's label interval overlaps a test label interval and
      no kept row's does), and that the 63d model is purged by 63, not 252
- [ ] A test asserts the embargo removes the `h` rows after the back-side
      purge, with `h = max(21, ceil(0.01 * T))`: `T = 3,780` gives 38 and
      `T = 1,000` gives 21 (the minimum)
- [ ] A test asserts no training row is ever placed inside a test block and
      that only point-in-time eligible names enter a training set (C3)
- [ ] A test asserts metrics are computed per path, not on joined data
- [ ] A test asserts the DSR is only reported when `N` is provided
- [ ] A test asserts the lock rule runs inside each fold, as a pure
      function of the simulated order history, with `D = t+1` for a decision
      at the close of `t`
- [ ] A test asserts the lock examples: a simulated buy placed on Monday
      blocks sells Monday to Friday and permits Monday of the next week; a
      second buy on Wednesday moves the first sell to the following
      Wednesday; a Thanksgiving-week buy counts trading days, not weekdays
- [ ] A test asserts a simulated order that does not fill is not counted and
      creates no block
- [ ] A test asserts no simulated sell is produced inside a block and no
      simulated buy inside a sell block, with the lock-aware optimizer on
- [ ] A test asserts the restricted ticker (`ICE`) never appears in a fold's
      universe, targets or simulated orders
- [ ] A test asserts the block is dated from the simulated **last fill
      day** (first permitted opposite order `D + 5` trading days), that a
      simulated order that never fills is not counted, and that a partially
      filled order counts from its last fill day
- [ ] A test asserts R1 and R2 give the same block end date for the same
      fill day and differ only in prices/slippage and open-order blocking
- [ ] A test asserts the headline uses `p = 0` for the `post` arm and that
      the sensitivity runs are labelled
- [ ] A test asserts costs are charged only on days with fills and that
      tiers are never pooled in the cost model
- [ ] A test asserts the band multiplier is never selected on test paths
- [ ] A test asserts a fill uses `measured_1m` only when
      `quote_spread_1m` has a valid row for that name and date, falls back to
      `proxy` otherwise, and that dates before the recording start date are
      never `measured_1m`
- [ ] A test asserts every fill has a `spread_source` in
      `{measured_1m, proxy}` and that reported metrics are split by it, with
      the share of fills and notional per source
- [ ] A test asserts simulated fractional quantities are rounded down to 2
      decimals (0.7549 gives 0.75), never exceed `weight × NAV / price`, and
      are never limit orders or filled outside regular hours
- [ ] A test asserts that in the Bootstrap tier no fractional-ineligible
      (or `Unknown`) ticker is in the universe, targets or simulated orders,
      that in the Transition tier an ineligible name gets whole shares only
      with the remainder in cash, and that `ICE` is still excluded in every
      tier
- [ ] A test asserts the report states the eligibility assumption (current
      list applied to all history) and the recording start date

## 10. Related Skills

- `00-construction` §3 — no lookahead rule
- `02-panel-construction` — provides the `split` column
- `03-gspqr-model` — the model being evaluated; `PREDICTION-BACKFILL.md`
  defines the walk-forward track
- `04-tcp-calibration` — evaluated on the walk-forward track
- `05-evt-tails` — evaluated on the walk-forward track
- `06-regime-detection` — the walk-forward regime rows used by every path
- `08-portfolio-optimization` — optimizer inside each fold
- `09-lock-state-machine` — the lock rule inside each fold
- `10-execution-schwab/EXECUTION-WINDOW.md` — the cost model and execution design that §6a-6g simulate
- `13-parquet-lake/SCHEMAS.md` #27 — `quote_spread_1m.parquet`, the measured spread source (forward-only)
- `config/fractional_eligible.toml` — the fractional-eligible list (current list applied to all history, §6g)