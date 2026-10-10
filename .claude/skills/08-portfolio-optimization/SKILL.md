---
name: portfolio-optimization
description: Post-Modern Portfolio Theory (PMPT) portfolio construction.
  Load when touching portfolio/ (src/pmpt.jl, covariance.jl, constraints.jl,
  conviction_apply.jl), scripts/optimize-portfolio.jl, JuMP optimization,
  Sortino ratio, or weight constraints.
---

# Portfolio Optimization

The portfolio layer takes the prediction engine's outputs and produces
target weights. It uses Post-Modern Portfolio Theory (PMPT), which
minimizes downside risk rather than total variance.

## 1. Why PMPT Over MPT

Modern Portfolio Theory (MPT) defines risk as the variance of returns.
This penalizes upside volatility equally with downside losses. Research
in behavioral finance (Kahneman and Tversky 1979) demonstrates that
investors are loss-averse: the psychological disutility of a loss is
roughly twice the utility of an equivalent gain.

Post-Modern Portfolio Theory (PMPT) replaces variance with **downside
deviation** (Lower Partial Moment), and the objective is the Sortino
ratio built on it. It measures risk only from returns
below a target, aligning the optimization with the actual utility
function of a loss-averse investor.

Empirical results from a 2025 study on US equities (2008–2024):

| Metric | MPT (Min Variance) | PMPT (Min Semivariance) |
|---|---|---|
| Max Drawdown (large-cap) | −39.62% | −23.72% |
| Sortino Ratio (skewed assets) | 0.96 | 1.30 |
| CAGR (skewed assets) | — | 11.48% |

The PMPT portfolio reduced maximum drawdown by approximately 16
percentage points compared to MPT in the large-cap universe. A
separate simulation-based study found PMPT achieved a higher Sortino
ratio (1.60) and lower maximum drawdown (0.14) than MPT.

## 2. Inputs

| Input | Source | Purpose |
|---|---|---|
| Expected returns | GSPQR `q_50` (an excess return over the risk-free rate, horizon `H`) | Numerator of the Sortino ratio; re-centres the scenarios |
| Scenarios | Trailing daily total returns (252 days), re-centred on the forward `μ` | Shape of the downside; same horizon unit as `μ` and `R_target` |
| Risk-free return | FRED `DGS3MO`, horizon `H` | `R_target`; return on cash |
| GSPQR `q_05` | Carried to `portfolio_targets.downside_risk` | **Diagnostic only; not in the problem** |
| Covariance | Ledoit-Wolf shrinkage | **Diagnostics and risk reporting only; not in the problem** (`COVARIANCE.md`) |
| Regime factor | `regime_states.parquet` | Post-conviction gate on new capital |
| Current weights | `positions.parquet` | Regime gate, lock bounds, trade costs (`w_cur`) |
| Veto flags | `veto_flags.parquet` | Pre-optimization filter; held vetoed names are kept in the output (§5b-2) |
| Restricted list | `config/restricted.toml` (single source shared with Rust) | Pre-optimization filter (§5b) |
| Fractional eligibility | `config/fractional_eligible.toml` (single source shared with Rust; seeded by the owner) | Bootstrap-tier pre-optimization filter (§5b-1); `Unknown` means not eligible |
| Execution tier | Trailing 5-day mean NAV from `portfolio_ledger.parquet` (§5b-1) | Selects whether the eligibility filter applies |
| Lock snapshot | `$QUANT_STATE/lock_snapshot.json` (one JSON contract, `09-lock-state-machine` §8a): per ticker `blocked_side` (`BUY`/`SELL`/`BOTH`), `first_permitted`, plus `complete`, `for_session`, `generated_at`; written by Rust at about 16:50 ET for the next session | Weight bounds (§5a). Advisory; the placement-time check in Rust is authoritative. Neutral default before execution exists: §5f |
| Trade costs | `execution_costs.parquet` (per-name cost model, `10-execution-schwab/EXECUTION-WINDOW.md` §5) | Cost vectors (§5c), no-trade band (§5d), entry spread filter |
| Conviction | `conviction_scores.parquet` | Post-optimization scale |

## 3. The Optimization Problem

Maximize the portfolio Sortino ratio

    (E[R_p] − R_target) / σ_downside

subject to:

- **Long-only:** `w_i ≥ 0`
- **Position cap:** `w_i ≤ 0.05` (5% single-name)
- **Sector cap:** `Σ_{i ∈ sector} w_i ≤ 0.25` (25% per GICS sector)
- **Budget:** `Σ w_i ≤ 0.98` (2% minimum cash). This is the **only**
  budget rule. There is no `Σ w = 1`: unallocated weight is cash, and
  cash earns `R_target`, so holding it costs nothing in Sortino terms. The
  ratio is invariant to scale, so the solver returns a direction and the
  weights are scaled up to the constraint frontier afterwards
  (`PMPT-FORMULATION.md` §3).

A ratio is not directly an SOCP. It is solved by Charnes–Cooper
homogenization: variables `y ≥ 0` and `κ > 0` with `y = κ w`, minimize `t`
subject to `(μ − R_f)ᵀy = 1`, `‖d‖₂ ≤ √T t`, `d_s ≥ R_f 1ᵀy − r̃_sᵀy`,
`d ≥ 0`, `1ᵀy ≤ 0.98κ`, `y_i ≤ 0.05κ`, sector sums `≤ 0.25κ`, and
`w = y/κ` is recovered afterwards. That is the base case (empty book, no
lock, no cost). With lock bounds and trading costs the same program
carries `y = κ w_cur + β − σ`, buy and sell parts `β, σ ≥ 0`, and costs
`c_bᵀβ + c_sᵀσ` in the numerator and in the shortfall, which stays exact
(`PMPT-FORMULATION.md` §3a, §5c below). See `PMPT-FORMULATION.md` §3 for the
derivation, and for the case where `PortfolioOptimisers.jl` offers a native
ratio objective (verify before using).

**Units.** `μ_i` is a forecast over the horizon `H`; scenarios are
historical returns. Scenarios, `μ`, and `R_target` must be in the same
horizon unit, and each asset's scenarios are re-centred,
`r̃_{i,s} = r_{i,s} − mean_i + μ_i`. `q_50` is already net of the
risk-free return. See `PMPT-FORMULATION.md` §2.

**Not in the problem.** `q_05` and the covariance matrix are not inputs
to the optimization (§2 table, `PMPT-FORMULATION.md` §8).

## 4. Application Order

```
GSPQR q_50 → VETO FILTER (held vetoed names set aside, §5b-2) → RESTRICTED FILTER
  → FRACTIONAL-ELIGIBILITY FILTER (Bootstrap tier only) → entry spread filter
  → Optimizer (lock bounds, relaxed sector caps, costs)
  → CONVICTION SCALE → locked renormalize + caps → REGIME GATE
  → LOCK CLAMP (assert) → NO-TRADE BAND (veto exits exempt)
  → merge held vetoed names (target 0 + lock clamp) → portfolio_targets
  → Lock Gate (Rust, placement time, authoritative) → Execution
```

**Veto and restricted before the optimizer.** Vetoed stocks and stocks on
`config/restricted.toml` are removed from the eligible universe. The
optimizer never sees them. A restricted name never appears in
`portfolio_targets` with weight (§5b). A vetoed name that is **held** does
appear, with target 0 plus the lock clamp, so that its exit is generated and
queued (§5b-2); a vetoed name that is not held has no row.

**Fractional eligibility before the optimizer, Bootstrap tier only.** In
the Bootstrap tier a ticker that is not fractional-eligible (or whose
eligibility is `Unknown`) is removed from the eligible set exactly like a
vetoed or restricted name: at that NAV a whole share is unaffordable, so
the name cannot be bought at all (§5b-1). In the Transition tier the
filter does not apply: eligible names may use the fractional market
remainder, ineligible names trade whole shares only (§6a). In the Standard
tier it is not applicable.

**Conviction after optimizer.** The optimizer produces weights subject
to the 5% cap, the (relaxed) 25% sector cap and the lock bounds.
Conviction then scales each weight:

    w_conviction = w_optimized × conviction

Then renormalize to sum to 0.98 (cash buffer) and re-apply the caps and
the lock bounds. See §5.

**Regime gate last among the weight transforms.** The regime factor scales
only new capital, after renormalization and caps, and is never followed by
another renormalization (a renormalize-to-0.98 step would cancel it). See §5.

**Lock clamp and no-trade band.** After the gate, every weight is checked
against its lock bound (an assertion; the earlier steps preserve it, §5a).
The no-trade band (§5d) then zeroes trades that are too small to pay for
themselves. Both preserve the lock direction.

## 5. Conviction and Regime Application

Conviction scales weights **after** the optimizer has enforced
constraints. Because the caps and lock bounds are re-applied after the
renormalization, the final weights always satisfy the 5% and 25% limits
(with the lock exceptions of §5a) and the lock directions.

```julia
# After optimizer produces w_optimized (lock-aware, cost-aware)
w_conviction = w_optimized .* conviction

# Renormalize to sum to (1 - cash_buffer), respecting per-name bounds
# [lb_i, ub_i] from the lock (§5a). Guard: if sum(w_conviction) == 0
# (empty eligible universe), emit no targets. Lock-blind renormalization
# is a bug: it can push a buy-locked name below its current weight.
target_sum = 0.98 - restricted_weight
w_final = apply_caps_locked(w_conviction; lb, ub, sector_caps = C_g,
                            budget = target_sum)   # one scale factor, clipped to [lb, ub]

# Regime gate: scale only the increase over the current weight.
# Applied last among the weight transforms, never renormalized.
# Residual stays in cash.
w_gate = min.(w_current, w_final) .+
         regime_factor .* max.(w_final .- w_current, 0.0)

# Lock clamp (assertion, not a repair)
@assert all(w_gate[no_sell] .>= w_current[no_sell] .- 1e-12)
@assert all(w_gate[no_buy]  .<= w_current[no_buy]  .+ 1e-12)

# No-trade band (§5d)
w_target = ifelse.(abs.(w_gate .- w_current) .> tau, w_gate, w_current)
```

`apply_caps_locked` generalizes the previous iterative redistribution:
scale all free names by one factor `λ`, clip each to `[lb_i, ub_i]`, and
find `λ` by bisection so the clipped sum equals the budget (a water-fill;
with `lb = 0`, `ub = 0.05` and no sector cap it is identical to the old
"redistribute the excess proportionally to uncapped positions" loop,
checked on 200 random cases). If a sector exceeds its cap `C_g`, its
excess over the lower bounds is scaled down to `C_g`, its names are frozen
and the water-fill is repeated for the rest; at most one pass per sector.
If `Σ lb > budget`, the function does not return weights: the run holds
(§5a).

The previous description of the cap application, "if a weight hits the 5%
cap, the excess is redistributed proportionally to uncapped positions;
repeat until convergence (typically 2-3 passes)", is the special case of
this. Redistribution must respect the sector cap as well as the
single-name cap.

`w_current` is the position's weight in NAV from `positions.parquet` as
of the rebalance date. Without locks, `w_target ≤ w_final` element-wise
after the gate, so the cap and cash-buffer constraints continue to hold.
With `regime_factor = 0.0` no target exceeds the current weight: nothing is
bought, nothing is force-sold by the gate. The gate and the band keep
the lock direction: for a buy-locked name `w_final ≥ w_current`, so
`w_gate = w_current + regime_factor (w_final − w_current) ≥ w_current`; for
a sell-locked name `w_final ≤ w_current`, so `w_gate = w_final ≤ w_current`;
the band replaces a weight by `w_current`, which satisfies both bounds.

### 5a. Lock-Aware Bounds and Precedence

The lock (`00-construction` §1) is a contractual obligation. The optimizer
must not choose weights execution cannot reach, otherwise targets are
silently dropped at the order generator and the delivered portfolio differs
from the optimized one in an unmeasured way.

Per name, from the lock snapshot for the **placement session** `t+1`
(first permitted day `D + 5` trading days, where `D` is a counted
order's last fill date, equal to its placement date when it filled the same
day; an order still open blocks with no end date; the snapshot is evaluated for the date on which the orders
will be placed, not for the decision date):

| Status | Meaning | Bound |
|---|---|---|
| `none` | no counted order blocks either side | `0 ≤ w_i ≤ 0.05` |
| `no_sell` | a counted **buy** blocks sells until `first_permitted` | `w_cur_i ≤ w_i ≤ max(0.05, w_cur_i)` |
| `no_buy` | a counted **sell** blocks buys until `first_permitted` | `0 ≤ w_i ≤ min(0.05, w_cur_i)` |

In the homogeneous program these are `y_i ≥ κ w_cur_i` and
`y_i ≤ κ w_cur_i` (`PMPT-FORMULATION.md` §3a). The source of the snapshot
is the lock module (Rust, a pure function of order history over every
account). The snapshot is the JSON file of `09-lock-state-machine` §8a; the
mapping is `blocked_side = SELL` → `no_sell`, `BUY` → `no_buy`, `BOTH` → frozen
(`w_i = w_cur_i`, no trade, whatever `first_permitted` says). A ticker absent
from a `complete: true` snapshot for the planned `for_session` is `none`. If the
snapshot cannot be read, is `complete: false`, its `for_session` is not the
session being planned (given by the scheduler as `--for-session`), or
`generated_at` is older than 26 hours, **every** name is treated as blocked on
both sides (`w_i = w_cur_i`, no trade), as the lock itself fails closed. The snapshot only avoids wasted targets;
it never authorizes an order. The placement-time recomputation in Rust is
authoritative and a target that fails it is dropped.

**Precedence: the lock wins over the position and sector caps.**

- A `no_sell` name that has drifted above 5% is **not** a violation. The
  caps apply to orders at placement: an order may not take the resulting
  weight above the cap. The name stays at `w_cur` (no sell is possible and
  no buy is wanted).
- The sector cap is relaxed for sector `g` to
  `C_g = max(0.25 − F_g, L_g)` where `L_g` is the sum of `w_cur` over
  `no_sell` names in `g`, so the problem stays feasible.
- If even with the relaxation the locked lower bounds do not fit
  (`Σ_i lb_i + restricted weight > 0.98`), do **not trade**: emit
  `w_target = w_current` for every held name, set `opt_status =
  infeasible_locks`, and write a `CRITICAL` audit row. Never sell a name
  that the lock protects to make room.
- An unlocked name above its cap is still sold down by the optimizer (cap
  `0.05`), subject to the band (§5d).
- The lock is never relaxed for a veto, regime change, stop or conviction:
  those steps clamp to the bounds, not the other way round
  (`00-construction` §1 rule 8).

What the optimizer does **not** know: a lock that will be created by this
session's own orders (buying today blocks sells for five trading days).
That inter-temporal cost is not modelled; the band and the minimum
rebalance interval (`EXECUTION-WINDOW.md` §7) lower its frequency.

### 5b. Restricted Securities

Any ticker in `config/restricted.toml` (read at `$QUANT_CONFIG/restricted.toml`, `13-parquet-lake/IO-CONTRACT.md` §1)
(`[[restricted]]` entries with `ticker`, `rule`, `reason`; the initial entry
is `ICE`, `rule = "never_trade"`) is removed from the eligible set **before**
the optimizer, like the veto filter. It gets weight 0 and never appears in
`portfolio_targets`. Julia and Rust read the same file (single source; Julia
parses the TOML with the standard library `TOML`; no copy, no hard-coded
list). A missing, unreadable or unparseable file halts the run (no
targets, `CRITICAL`): the optimizer must not produce targets without
knowing the list. The loader must also **error** (not fall back to an empty set)
when the `restricted` key is absent or misspelled or the list is empty, and it
asserts that `ICE` is present with `rule = "never_trade"` (the same guarantee as
`tests/restricted.rs`). Tickers are normalized (trimmed, upper case, then the
canonical symbol map of `09-lock-state-machine` §3.8) before comparison, so a
spelling cannot slip through.

If the account somehow holds a restricted ticker, its current weight is a
fixed amount that reduces the budget and the sector headroom; it is not
sold by this system, and it is not a variable of the problem. Adding or
removing an entry is a commit approved by the account owner
(`00-construction` §6).

### 5b-2. Vetoed Names That Are Held

The constitution removes a vetoed name from the eligible universe before
optimization. That cannot mean "no row": a held vetoed name dropped from the
output would never get a sell target and would be held indefinitely. So:

- The vetoed name is **not a variable** of the optimizer and receives no buy,
  now or later. Its current weight is a fixed amount in the budget and sector
  headroom while it is held (like a restricted holding, but it is expected to
  go).
- After the solve, the output **keeps every held vetoed name** with
  `veto_active = true`, `w_optimized = w_conviction = w_final = 0` and
  `w_target = 0` **unless the lock binds**. If the snapshot says `no_sell` (a
  counted buy blocks the sale) or `BOTH`, or the snapshot is unusable, the lock
  clamp holds: `w_target = w_current`. The clamp is never relaxed for a veto
  (`00-construction` §1 rule 8).
- The **sell intent** does not depend on `w_target`: the order generator derives
  it from `veto_active = true` and `w_current > 0` (a sale of the whole
  position). It goes through the normal gate: allowed today → a normal sequence;
  `BlockedUntil(d)` → a pending sell with `earliest_execution = d`, released to
  the generator on that day (`09-lock-state-machine/PENDING-ORDERS.md` §2-§3). A
  veto never cancels a queued sell, and it also cancels the name's pending and
  working buys. The no-trade band does not apply to a veto exit (§5d).
- The test therefore reads: **target 0 unless lock-bound, in which case the sell
  is queued** (not "weight exactly 0.0").

### 5b-1. Fractional Eligibility (Bootstrap Tier)

Schwab fractional trading (Stock Slices) is reported to cover only some
stocks (S&P 500 stocks, no ETFs). A fractional order is a market order in
regular trading hours with at most 2 decimal places (owner-confirmed that
the Trader API accepts it; the 2-decimal limit is owner-reported and still
to be verified). The eligible list is `config/fractional_eligible.toml`,
seeded by the account owner and shared with Rust (single source; Julia
parses it with the standard library `TOML`, like the restricted list; no
copy, no hard-coded list). Any ticker not listed as eligible, including one
marked `Unknown`, is treated as **not eligible**.

| Tier | Effect on the optimizer |
|---|---|
| Bootstrap (< $2,500) | A non-eligible ticker is removed from the eligible set **before** the optimizer, like the veto and restricted filters. It gets weight 0 and never appears in `portfolio_targets` with weight. |
| Transition ($2,500 to $10,000) | No filtering. The order generator sends the fractional remainder as a market order for eligible names; for ineligible names it trades whole shares only, the sub-share remainder stays in cash and a weight below one share yields no trade (§6a). |
| Standard (≥ $10,000) | Not applicable. |

**Tier input.** The tier used here is the **trailing 5-day mean NAV**
decision input, the same quantity and the same boundaries
(`00-construction` §6a) the Rust order generator reads at submission. The
optimizer reads it from `portfolio_ledger.parquet`; it does not compute a
second NAV and does not cache it. A tier that changes between the decision
and the placement session is handled by the order generator (authoritative),
which re-reads the tier and the list; the optimizer's filter is a
pre-filter.

If a non-eligible name is already held when the tier is Bootstrap (a NAV
that fell back from Transition), it is not a variable of the problem and
keeps its current weight as a fixed amount (like a restricted holding); the
order generator, not the optimizer, decides any exit. A missing,
unreadable or unparseable file halts the run in the Bootstrap tier (no
targets, `CRITICAL`): the filter cannot be skipped. In the other tiers the
file is read by the order generator only.

### 5c. Cost Awareness Without Breaking the Maths

Transaction costs are proportional to `|trade|` and therefore **not**
homogeneous in `(y, κ)`. The treatment is the perspective form of
`PMPT-FORMULATION.md` §3a: split the trade into buy and sell parts,
`w = w_cur + b − s`, with costs `c^b_iᵀ b + c^s_iᵀ s`, and homogenize the
parts too (`β = κ b`, `σ = κ s`). The result is **exact**, a single SOCP
solve, with no bisection or line search over the budget scale, and it
keeps the lock bounds linear. The alternative considered, a fixed-scale
problem solved by bisection on the ratio (Dinkelbach) or at a few values of
a risk-aversion multiplier, was rejected because it is more solves for no
gain in exactness.

- `c^b_i` is the entry cost and `c^s_i` the exit cost of name `i`, as
  fractions of traded notional in the horizon unit, from the per-name
  cost model (`EXECUTION-WINDOW.md` §5: half spread plus calibrated
  slippage, per tier and arm). **This file does not restate the model.**
  Before any fills exist the cost is the half-spread prior
  (`cost_source = prior`).
- Held names the optimizer would reduce pay `c^s`; names not currently
  held pay `c^b` on the full weight. A name that is held and kept pays
  nothing. The optimizer therefore holds a position unless the improvement
  in the net Sortino ratio exceeds the cost, which is an optimizer-level
  no-trade region.
- Exact: the optimum of the net-of-cost Sortino ratio on the given
  scenarios. Approximate: proportional cost with no market impact; a
  one-off cost against an `H`-day return; estimated costs; and the steps
  after the solve (conviction, renormalization, caps, regime gate) which
  move the weights away from the cost-optimal point. See the table in
  `PMPT-FORMULATION.md` §3a. Zero cost vectors reduce this to the base
  program plus the lock bounds.
- The entry spread filter (`EXECUTION-WINDOW.md` §8) is an eligibility
  rule for *buying*: a held name that fails it stays in the problem so the
  optimizer can price its sale, instead of being assigned weight 0 and
  liquidated at the worst spread.

### 5d. Post-Optimization No-Trade Band

After the regime gate and the lock clamp a name is traded only if

    |w_target − w_current| > τ_i,    τ_i = max( N_min / NAV , c × RT_i )

with `N_min` the tier's minimum notional, `RT_i` the round-trip cost
fraction of the name and `c` a dimensionless multiplier chosen on CPCV
paths (`14-backtest-protocol` §6e). Otherwise `w_target := w_current`. The
band is a heuristic hysteresis (it is not derived from a utility). It is
exempt for forced exits (veto, cash buffer, cap breach created by an
order; a restricted holding is never an exit, it is never sold by this system);
a position that drifted slightly above 5%
without a lock is held by the band and is not a violation. Details and
rationale: `10-execution-schwab/EXECUTION-WINDOW.md` §7.1.

### 5e. Output

`portfolio_targets` keeps its columns (`13-parquet-lake/SCHEMAS.md` §13;
`w_target` is still after the regime gate, now also after the lock clamp
and the band; `w_optimized` is the solve's weight). Restricted names have
no row. **Proposed appended columns** (for the requester to add to
`SCHEMAS.md`; this skill does not edit it; all nullable, appended after
`w_target`):

| Column | Type | Notes |
|---|---|---|
| `w_current` | `float64` | The current weight used |
| `lock_status` | `utf8` | `none` / `no_sell` / `no_buy` |
| `lock_first_permitted` | `date32` | From the snapshot: `D + 5` trading days with `D` the blocking order's last fill date; null if `none` or if the blocking order is still open (no end date) |
| `lock_binding` | `bool` | True if the lock-free solve (diagnostic) wanted a trade the lock forbade |
| `band_skipped` | `bool` | True if the band replaced the weight by `w_current` |
| `cost_buy_bps`, `cost_sell_bps` | `float64` | The cost estimates used |
| `opt_status` | `utf8` | `optimal` / `infeasible_locks` / `no_positive_excess` / `solver_failed` |

### 5f. Bootstrap Defaults (the portfolio phase runs before execution exists)

The portfolio phase is built before execution, the ledger and the lock module.
The script must run with their outputs absent, using neutral defaults and saying
so in logs, never failing and never inventing data:

| Input | Absent before execution | Neutral default |
|---|---|---|
| `positions.parquet` | no book yet | empty book: `w_current = 0` for every name |
| `execution_costs.parquet` | no fills, no quotes | zero cost vectors (`cost_source = prior`); the base program (§5c) |
| `portfolio_ledger.parquet` (tier input) | no NAV series | a configured `bootstrap_nav` (the intended starting NAV, a config value) selects the tier; if the tier is Bootstrap and `fractional_eligible.toml` (owner-seeded) is absent the run halts as in §5b-1 (report, do not guess) |
| `lock_snapshot.json` | no lock module | with an empty book **and** no snapshot file, `none` for every name (nothing can be locked without orders), logged `lock_source = default_empty`; as soon as `positions.parquet` has any holding, a missing or stale snapshot freezes every name (§5a) |
| `regime_states`, `conviction_scores`, `veto_flags` | may be absent early | regime factor 1.0, conviction 1.0, no vetoes, each logged |

These defaults switch off by themselves once `positions.parquet` has a holding,
so a missing lock snapshot can never be mistaken for "unlocked" on a live book.

## 6. Constraint Implementation

The homogeneous SOCP of `PMPT-FORMULATION.md` §3–§4 can be written
directly in JuMP with Clarabel (sketch in §4 there). The snippet below
shows the intended `PortfolioOptimisers.jl` front end for the same
constraints. **The names used (`WeightBounds`, `LinearConstraintEstimator`,
`Solver`, `JuMPOptimiser`) are from memory and must be verified against the
pinned version.** In the homogeneous form the bounds are `y_i ≤ 0.05κ`,
sector sums `≤ 0.25κ`, `Σy ≤ 0.98κ`; if the package only offers a
non-homogeneous interface, its ratio objective (if any) must be checked
against the brute-force test in §8 before use.

```julia
using PortfolioOptimisers, Clarabel

# Weight bounds: 0 ≤ w_i ≤ 0.05
wb = WeightBounds(; lb = 0.0, ub = 0.05)

# Sector caps: 25% max per sector
sector_constraints = LinearConstraintEstimator(
    "tech <= 0.25",
    "financials <= 0.25",
    "energy <= 0.25",
    # ... all GICS sectors
)

# Cash buffer: sum of weights ≤ 0.98
cash_constraint = LinearConstraintEstimator("sum <= 0.98")

# Solver
slv = Solver(; name = :clarabel, solver = Clarabel.Optimizer)
```

The `PortfolioOptimisers.jl` package supports weight bounds, linear
group constraints, and sector bands as `JuMPOptimiser` keywords.

### 6a. Fractional-Share Constraint

The optimizer produces **weights**, not share quantities. The conversion
from weights to quantities happens in the Rust order generator, not here.
The optimizer is unaware of the execution tier.

The order generator handles the three tiers as follows:

| Tier | Conversion |
|---|---|
| Bootstrap | Eligible name only (ineligible names were removed before the optimizer, §5b-1): `quantity = floor_2dp(weight × NAV / price)`, fractional, rounded **down** to 2 decimal places, submitted as a market order in regular hours |
| Transition, eligible | `whole = floor(weight × NAV / price)`, submitted as a limit; `remainder = floor_2dp(weight × NAV / price - whole)`, submitted as a market order in regular hours if `remainder > 0` |
| Transition, ineligible (or `Unknown`) | `whole = floor(weight × NAV / price)` as a limit, **no** remainder order; the sub-share remainder stays in cash. `whole = 0` yields no trade |
| Standard | `quantity = floor(weight × NAV / price)`, rounded **down** to whole shares, submitted as a limit; residual to cash |

`floor_2dp(x) = floor(100 x) / 100`. Rounding down, never to nearest,
means a quantity can never exceed `weight × NAV / price`, so the 5% cap and
the 2% cash buffer still hold after rounding (the residual returns to
cash). A fractional quantity is a market order in regular hours only;
whole-share quantities may use limit orders in any tier. If `floor_2dp`
gives 0.00 there is no order.

The optimizer's weights are the same in all three tiers. The tier affects
only the order generator's rounding. Take a 3% weight in a $200 stock:

| Tier | Example NAV | `weight × NAV / price` | Result |
|---|---|---|---|
| Bootstrap | $2,000 | 0.30 | 0.30 shares (eligible name), one market order |
| Transition, eligible | $5,000 | 0.75 | 0 whole shares, so no limit order, and a 0.75-share fractional market order |
| Transition, ineligible | $5,000 | 0.75 | 0 whole shares and no remainder order: no trade, weight stays in cash |
| Standard | $20,000 | 3.00 | 3 shares, one limit order |

With a Standard NAV just above $10,000 the same weight is 1.50 shares and
rounds down to 1 share (a limit), with the 0.50-share residual returned to
cash. Whole-share tiers cannot hold a position smaller than one share,
which is why the Bootstrap tier exists. The 5% cap means a $2,000 account
can hold at most 0.50 shares of a $200 stock.

Note that in the Transition tier a small weight on an eligible name can
produce an order that is entirely a fractional market order (as for $5,000
above), because the integer part is zero. A quantity such as 0.7549 becomes
0.75, not 0.76.

### 6b. Interaction With the Cash Buffer

The 2% cash buffer is enforced at the weight level, before quantity
conversion. The optimizer produces weights summing to at most 0.98. The
order generator converts these to quantities and rounds. The rounding
residual (from whole-share rounding) returns to cash, so the actual cash
after execution is at least the buffer.

In the Bootstrap tier and for eligible names in the Transition tier,
fractional quantities mean the rounding residual is small (below 0.01 share
per order). The cash buffer is approximately the target. For ineligible
names in the Transition tier the sub-share remainder stays in cash, as in
the Standard tier.
In the Standard tier, whole-share rounding can leave several percent in
cash for small positions. This is acceptable; the buffer is a minimum,
not a target.

## 7. What Not to Do

- Do not use variance as the risk measure. Use downside deviation.
- Do not write `Σ w = 1` (fully invested) or renormalize to 1. The only
  budget rule is `Σ w ≤ 0.98`.
- Do not feed a horizon-H `μ` into daily scenarios (or the reverse), and
  do not subtract the risk-free rate from `q_50` again. Same unit for
  scenarios, `μ`, `R_target`.
- Do not describe `q_05` as a risk input or the covariance as a
  constraint. Neither enters the problem; both are diagnostics.
- Do not apply conviction before the optimizer. Constraints would be
  violated after renormalization.
- Do not skip the cash buffer. 2% minimum cash is a constitution
  requirement (§6).
- Do not use a raw sample covariance matrix for the diagnostics. Use
  Ledoit-Wolf shrinkage (see `COVARIANCE.md`).
- Do not allow the optimizer to produce weights above 5% for any name.
  The cap is a hard constraint.
- Do not ignore sector caps. 25% per GICS sector is a hard constraint.
- Do not choose a weight the lock refuses, and do not renormalize in a way
  that is blind to the lock bounds.
- Do not relax the lock for the caps, the regime, a veto or the optimizer;
  relax the *caps* for the lock (§5a).
- Do not put a ticker from `config/restricted.toml` in the problem, in
  `portfolio_targets`, or in a hard-coded list.
- Do not add `c·|w − w_cur|` to the homogeneous program without the
  perspective split; it is not homogeneous.
- Do not remove a *held* name from the optimizer because of a spread
  filter; the filter is for entries.
- Do not convert weights to share quantities in Julia. The conversion
  happens in the Rust order generator, which knows the execution tier.
- Do not assume whole-share quantities in the Bootstrap tier, or for
  eligible names in the Transition tier. Positions there are fractional (2
  decimals, rounded down).
- Do not put a non-eligible (or `Unknown`) ticker in the Bootstrap
  problem or targets, and do not hard-code the fractional list; read
  `config/fractional_eligible.toml`.
- Do not assume Schwab turns a fractional quantity into a limit order or
  rounds it up. Fractional means market order, regular hours only;
  whole-share limits are fine.

## 8. Verification

- [ ] A test asserts weights sum to ≤ 0.98 (cash buffer), and that no
      code path normalizes the optimizer output to 1.0
- [ ] A brute-force test on a tiny synthetic problem (2–3 assets, ~50
      scenarios, a grid over weights) asserts the homogeneous SOCP's
      optimal Sortino ratio `1/t` matches the grid maximum of the
      directly computed ratio, and that the recovered `w = y/κ` satisfies
      `Σw ≤ 0.98` and the caps
- [ ] A units test asserts scenarios, `μ`, and `R_target` share one
      horizon unit (daily scenarios with an H-day `μ` is rejected), and
      that `q_50` is not net of the risk-free rate twice
- [ ] A test asserts scenarios are re-centred: the mean of `r̃_i` equals
      `μ_i` for every asset
- [ ] A test asserts scale invariance is handled: the returned weights
      are on the constraint frontier (`κ` minimal), not an arbitrarily
      small multiple of the optimal ray
- [ ] A test asserts that when every `q_50 ≤ 0` the optimizer emits no
      targets (cash), not an error or a heuristic portfolio
- [ ] A test asserts `downside_risk` in `portfolio_targets` equals the
      GSPQR `q_05` and is not read by the optimizer
- [ ] A test asserts no weight exceeds 0.05
- [ ] A test asserts no sector exceeds 0.25
- [ ] A test asserts all weights are ≥ 0 (long-only)
- [ ] A test asserts a vetoed stock is not an optimizer variable and gets no buy,
      and that its target is 0 unless lock-bound; a held vetoed name whose sale
      the lock blocks keeps `w_target = w_current` and a queued sell (a sell
      intent for the whole position, queued for `first_permitted`); the veto
      never cancels the queued sell; a vetoed name that is not held has no row;
      the no-trade band does not suppress a veto exit
- [ ] A test asserts the loader errors when the restricted key is absent or the
      list is empty, and when `ICE` is missing or its rule is not `never_trade`
- [ ] A test asserts a restricted name found in the account is never given a
      sell target or exit (halt and CRITICAL are Rust's job, no auto-sell)
- [ ] A test asserts the lock snapshot is read as JSON from `QUANT_STATE`,
      `BOTH` freezes the name, and a `for_session` mismatch freezes every name
- [ ] A test asserts conviction is applied after optimization
- [ ] A test asserts the regime gate is applied after renormalization and
      caps: `w_target <= w_final`, sum <= 0.98, and `regime_factor = 0.5`
      halves additions rather than being undone by renormalization
- [ ] A test asserts `regime_factor = 0.0` yields `w_target <= w_current`
- [ ] A test asserts the SOCP solver converges (no fallback to heuristic)
- [ ] A test asserts the optimizer output is weights, not quantities
- [ ] A test asserts lock bounds: a `no_sell` name has `w_target ≥ w_cur`,
      a `no_buy` name has `w_target ≤ w_cur`, after the optimizer, after
      conviction/renormalize/caps, after the regime gate and after the band
- [ ] A brute-force test with lock bounds on a tiny problem (3 assets, a
      buy-lock and a sell-lock, ~50 scenarios, a grid over weights inside
      the bounds) asserts the homogeneous program's `1/t` matches the grid
      maximum and the recovered `w` respects the bounds; `κ` is the left end
      of its feasible interval
- [ ] A brute-force test with proportional costs and a held book asserts the
      perspective program matches a direct grid search of the net-of-cost
      ratio; with `w_cur = 0` the ratio is scale-free, with a held book and
      costs it is not
- [ ] A test asserts the second-stage `κ` minimization is a no-op where `κ`
      is pinned and equals the closed form where it is free
- [ ] A test asserts a `no_sell` name drifted above 5% is neither sold nor
      bought and is not reported as a violation; the weights of the other
      names still satisfy the caps
- [ ] A test asserts the sector cap is relaxed by the locked excess, and
      that the problem stays feasible
- [ ] A test asserts that when `Σ lb + restricted > 0.98` the run holds
      (`w_target = w_current`), sets `opt_status = infeasible_locks`, and
      writes a `CRITICAL` audit row; no sell is produced
- [ ] A test asserts `apply_caps_locked` equals the previous iterative
      redistribution when there are no locks and no sector binding
- [ ] A test asserts `ICE` (from `config/restricted.toml`) never appears in
      `portfolio_targets`, is not in the optimizer's variables, and that a
      missing or unparseable restricted file halts the run
- [ ] A test asserts the Julia and Rust readers parse the same file and
      agree on the set
- [ ] A test asserts that in the Bootstrap tier a ticker that is not
      fractional-eligible never appears in `portfolio_targets` and is not
      in the optimizer's variables, and that a ticker marked `Unknown` (or
      absent from `config/fractional_eligible.toml`) is treated as
      ineligible
- [ ] A test asserts `ICE` still never appears in the targets in every
      tier, including when it is fractional-eligible
- [ ] A test asserts the filter is not applied in the Transition and
      Standard tiers (the optimizer's eligible set is unchanged), and that
      in the Transition tier an ineligible name gets no remainder order and
      a weight below one share yields no trade
- [ ] A test asserts `floor_2dp` rounds down (0.7549 gives 0.75, 0.999
      gives 0.99), and that the rounded quantity times price never exceeds
      the capped weight times NAV, so the 5% cap and the 2% cash buffer
      still hold after rounding
- [ ] A test asserts the optimizer reads the tier from the ledger's
      trailing 5-day mean NAV (not a second NAV) with inclusive lower
      boundaries ($2,500 is Transition, $10,000 is Standard), and that a
      missing or unparseable `fractional_eligible.toml` halts a Bootstrap
      run
- [ ] A test asserts a held name failing the entry spread filter stays in
      the problem, and that zero cost vectors reproduce the base solution
- [ ] A test asserts the no-trade band exempts forced exits and never
      changes a weight's lock direction
- [ ] A test asserts that an unreadable or stale lock snapshot makes every
      name `w = w_current` (blocked both ways)

## 9. Related Skills

- `03-gspqr-model` — provides `q_50` (expected return) and `q_05` (diagnostic only)
- `06-regime-detection` — provides `regime_factor`
- `07-fundamental-overlay` — provides veto and conviction
- `09-lock-state-machine` — gates execution after optimization; supplies the lock snapshot for the bounds
- `10-execution-schwab/EXECUTION-WINDOW.md` — the per-name cost model and the no-trade band
- `config/restricted.toml` — the restricted list (shared with Rust)
- `config/fractional_eligible.toml` — the fractional-eligible list (owner-seeded, shared with Rust)
- `10-execution-schwab` — the order generator that converts weights to quantities
- `00-construction` §6, §6a — position, sector, cash constraints and NAV tiers
