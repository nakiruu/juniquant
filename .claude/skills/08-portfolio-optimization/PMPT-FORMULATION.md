# PMPT Formulation

The Post-Modern Portfolio Theory optimization problem, formulated as a
Second-Order Cone Program (SOCP) by Charnes–Cooper homogenization.

## 1. The Objective

Maximize the Sortino ratio:

    Sortino(w) = (E[R_p] − R_target) / σ_downside(w)

Where:
- `E[R_p]` = expected portfolio return over the horizon `H` (from GSPQR
  `q_50`, §6)
- `R_target` = minimum acceptable return (MAR), the risk-free return over
  the same horizon `H`
- `σ_downside` = downside deviation (lower semi-deviation) below `R_target`

A ratio is not itself a cone program. §3 shows how it becomes one. The
phrase "minimize downside deviation" elsewhere in this skill refers to the
`t` of the homogeneous problem in §3, not to a separate objective.

## 2. Units: One Horizon for Everything

`μ_i` is a forecast over a horizon `H`, while historical scenarios are
daily. The problem is only meaningful if `μ`, `R_target`, and the
scenarios are expressed over the **same** horizon. This is a correctness
requirement, not a convenience.

- **`H`** is the GSPQR horizon whose `q_50` is used (default 63 trading
  days, `y_fwd_63d`). It is not the rebalance cadence.
- **`q_50` is already an excess return.** The GSPQR target `y_fwd_Hd` is
  the H-day return minus the risk-free return over the same H
  (`02-panel-construction/TARGETS.md` §1). So the total-return forecast is

      μ_i = q_50_i + R_f,H          R_f,H = (1 + DGS3MO/100)^(H/252) − 1

  and `μ_i − R_f,H = q_50_i`. Never subtract the risk-free rate from
  `q_50` a second time.
- **`R_target = R_f,H`**, the same quantity as in `μ_i`.
- **Scenarios** `r_{i,s}`, `s = 1..T`, are total returns in the same unit.
  Default (daily units): the trailing 252 days of daily total returns, with
  `μ` and `R_f` converted by geometric de-compounding
  `x_daily = (1 + x_H)^(1/H) − 1`. Alternative (horizon units): overlapping
  H-day compounded returns, with `μ` and `R_f` left at `H`. Mixing the two
  (daily scenarios with an H-day `μ`) is a bug: it makes the numerator
  about `H` times too large relative to the risk.
- **Re-centre** each asset's scenarios on the forward expectation so the
  downside is measured around what the model expects, not around the
  sample's own historical mean:

      r̃_{i,s} = r_{i,s} − mean_s(r_{i,s}) + μ_i

  This keeps the historical shape (skew, tails) and replaces the location.
  (`q_50` is a median used as the mean; that approximation is accepted.)

All formulas below use `r̃`, `μ`, `R_f` in the single chosen unit.

## 3. Charnes–Cooper Homogenization

Portfolio return with the unallocated weight held in cash that earns the
target return:

    R_p,s = Σ_i w_i r̃_{i,s} + (1 − Σ_i w_i) × R_f

so `E[R_p] − R_f = Σ_i w_i (μ_i − R_f)` and the scenario shortfall below
target is `max(0, R_f − R_p,s) = max(0, R_f Σ_i w_i − Σ_i w_i r̃_{i,s})`.
Both numerator and denominator are positively homogeneous of degree 1 in
`w`, so the ratio is invariant to the scale of `w`.

Substitute `y = κ w` with `κ > 0` and normalize the numerator to 1:

    min  t
    s.t. (μ − R_f)ᵀ y = 1
         ‖d‖₂ ≤ √T · t                      (i.e. ‖d‖₂ / √T ≤ t)
         d_s ≥ R_f (1ᵀ y) − r̃_sᵀ y,  d_s ≥ 0   for s = 1..T
         y ≥ 0
         1ᵀ y ≤ 0.98 κ
         y_i ≤ 0.05 κ                       for all i
         Σ_{i ∈ sector g} y_i ≤ 0.25 κ      for all sectors g

Variables: `y ∈ R^N`, `κ`, `t`, `d ∈ R^T`. `κ > 0` follows from the
equality and `y_i ≤ 0.05 κ`. At the optimum `t = σ_downside(y)`, and the
maximal Sortino ratio is `1 / t`. The weights are recovered as `w = y / κ`.

**The shortfall constraint.** With cash at `R_f` the shortfall term is
`R_f (1ᵀ y) − r̃_sᵀ y`. An earlier draft of this note wrote `R_f κ − r̃_sᵀ y`;
that equals the term above only when `1ᵀ w = 1` (fully invested), and it
would penalise holding cash, contradicting the rule that cash costs
nothing. Use `R_f (1ᵀ y)`. In excess terms (`r̃^ex = r̃ − R_f`) it is
`−r̃^exₛᵀ y`, with no `R_f` left.

**κ is not pinned by the objective (base case).** This paragraph, and
the whole of §3 above, is the special case with an empty book, no lock and
no trading costs. With locks or costs see §3a, where κ is an interval or
is pinned. In the base case, because the ratio is scale invariant, any larger `κ` (a smaller, more cash-heavy `w`) has the same
`t`. After solving, set `κ` to the smallest feasible value so the
portfolio is deployed to the constraint frontier along the optimal ray:

    κ = max( 1ᵀy / 0.98,  max_i y_i / 0.05,  max_g Σ_{i∈g} y_i / 0.25 ),   w = y / κ

(Equivalently add `ε κ` to the objective; verify the solver's handling.)
The result satisfies `1ᵀw ≤ 0.98`, `0 ≤ w_i ≤ 0.05`, sectors `≤ 0.25`,
and has the maximal Sortino ratio. The post-conviction renormalization
in `SKILL.md` §5 is a separate, later step.

**Infeasible case.** If `μ_i ≤ R_f` for every eligible asset (that is,
`q_50_i ≤ 0` for all `i`), the equality `(μ − R_f)ᵀy = 1` has no solution
with `y ≥ 0`. Hold cash: emit no targets, log it. If some `q_50_i > 0`,
only those assets can carry weight. (With an existing book and locks the
policy is §3a "Infeasible and degenerate cases".)

**Native objective.** If `PortfolioOptimisers.jl` provides a native
maximum-ratio or Sortino objective, using it is acceptable provided the
constraints in §3 and the tests in `SKILL.md` §8 still hold (same
`Σw ≤ 0.98`, caps, units, and agreement with the brute-force check). Verify
in the pinned version whether such an objective exists and what its
risk-free-rate, return-unit and cash handling are before relying on it;
this note does not assume it does.

## 3a. Lock-Aware, Restricted-Aware and Cost-Aware Extension

§3 is the special case `w_cur = 0`, no lock, no cost. The optimizer must
not choose weights that execution cannot reach (the contractual lock,
`00-construction` §1) or that the system may not trade (restricted list,
§6), and should not ignore what trading costs. This section is the general
program; the numerical checks are listed at its end.

### Inputs per eligible name `i`

| Symbol | Meaning |
|---|---|
| `w_cur_i` | Current weight in NAV at the decision close (`positions.parquet`, same `NAV` as the regime gate) |
| `ℓ_i` | Lock status for the **placement session** `t+1` from the lock module: `none`; `no_sell` (a counted **buy** blocks sells until `first_permitted`); `no_buy` (a counted **sell** blocks buys). A snapshot, advisory: the placement-time check in Rust is authoritative (`10-execution-schwab/EXECUTION-WINDOW.md` §8.1) |
| `c^b_i, c^s_i ≥ 0` | Buy / sell cost as a fraction of traded notional, in the horizon unit: `bps / 10000` from `execution_costs.parquet` (`EXECUTION-WINDOW.md` §5; not restated here) |
| `u_i` | Position cap for name `i`: `0.05`, except `u_i = max(0.05, w_cur_i)` for `no_sell` names (a drifted, buy-locked name cannot be sold, so its weight is a fact; an order may not take a weight above the cap, so it is also not added to) |
| `C_g` | Sector cap, relaxed by the locked excess: `C_g = max(0.25 − F_g , L_g)`, with `L_g = Σ_{i∈g, no_sell} w_cur_i` and `F_g` the restricted holdings in `g` (zero in practice) |
| `B` | Budget `0.98 − Σ_{restricted} w_cur`; restricted names are fixed, not variables |

Restricted tickers (`config/restricted.toml`) are removed before the
problem is built (SKILL.md §5b); they never appear in the variables or in
`portfolio_targets`.

### Bounds on the weights, then in homogeneous form

    no_sell  (counted buy):   w_i ≥ w_cur_i          (cannot reduce)
    no_buy   (counted sell):  w_i ≤ w_cur_i          (cannot increase)

Write `w = w_cur + b − s`, `b, s ≥ 0` (buy and sell parts), with `b_i = 0`
for `no_buy` and `s_i = 0` for `no_sell`. Multiply by `κ`: `y = κ w`,
`β = κ b`, `σ = κ s`. The bounds are linear in `(y, κ)`:

    y_i ≥ κ w_cur_i   (no_sell),     y_i ≤ κ w_cur_i   (no_buy)

and are carried by the identity `y = κ w_cur + β − σ` with `σ_i = 0` or
`β_i = 0` as above.

### The program

    min  t
    s.t. y = κ w_cur + β − σ,   β, σ ≥ 0,   β_i = 0 (no_buy),  σ_i = 0 (no_sell)
         (μ − R_f)ᵀ y − c_bᵀβ − c_sᵀσ ≥ 1
         ‖d‖₂ ≤ √T · t,   d ≥ 0
         d_s ≥ R_f (1ᵀy) − r̃_sᵀ y + c_bᵀβ + c_sᵀσ        for s = 1..T
         y ≥ 0,  1ᵀy ≤ Bκ,  y_i ≤ u_i κ,  Σ_{i∈g} y_i ≤ C_g κ

Recover `w = y / κ`. This is a second-order cone program (all constraints
linear except the one cone), same solver and same size class as §3 plus
`2N` variables.

**Why the costs fit the homogeneous form (exact).** Portfolio return after
paying costs on the trade is
`R_p,s = Σ_i w_i r̃_{i,s} + (1 − Σ w_i) R_f − C(b, s)` with the convex
(here linear) cost `C(b, s) = c_bᵀb + c_sᵀs`. The numerator is
`N(w) = (μ − R_f)ᵀw − C(b, s)` and the shortfall is
`R_f − R_p,s = R_f 1ᵀw − r̃_sᵀw + C(b, s)`. Both are **not** homogeneous in
`w` alone, because `b` and `s` are measured from a fixed `w_cur`. But in
the variables `(y, κ, β, σ)` the *perspective* of each is positively
homogeneous of degree 1: `κ·C(b, s) = c_bᵀβ + c_sᵀσ`, and
`κ·N(w) = (μ − R_f)ᵀy − c_bᵀβ − c_sᵀσ`. The ratio `N/g` depends on
`(y, κ, β, σ)` only through `w = y/κ`, so Charnes–Cooper applies unchanged:
normalise the numerator to 1. A `≥ 1` rather than `= 1` is enough: if the
numerator exceeds 1, scaling all variables by `1/numerator` stays
feasible (every constraint is homogeneous) and lowers `t`, so the numerator
is 1 at the optimum. The cost-aware optimum is the **exact** maximiser of
the net-of-cost Sortino ratio under the lock bounds, caps and budget. No
fixed-scale line search, bisection or multiple-λ search is needed; those
were considered and rejected because the perspective trick is exact and a
single solve.

**Simultaneous buy and sell.** With `c^b_i + c^s_i > 0` the optimum has
`β_i σ_i = 0` (buying and selling the same name only adds cost). With zero
costs the split is not unique; take `y − κ w_cur` as the net trade.

### κ: interval, pinned, or tie-broken

- **Locks, zero costs.** The ratio is invariant to the scale of `w`, but
  the lock bounds make the feasible scales an **interval**, not a ray from
  zero: with `y` fixed, `κ` must satisfy
  `κ ≥ max( 1ᵀy / B, max_i y_i / u_i, max_g Σ_g y / C_g, max_{i: no_buy, w_cur_i>0} y_i / w_cur_i )`
  and `κ ≤ min_{i: no_sell, w_cur_i>0} y_i / w_cur_i`. The solver's `κ`
  lies in it, so it is non-empty. The deployment rule of §3 is kept: take
  the **smallest** feasible `κ` (the left end), the largest weights on the
  optimal ray, `w = y / κ_lo`.
- **Costs and a non-empty book.** The objective is no longer constant along
  `κ`: a portfolio scaled up from `w_cur` pays more cost. `(y, κ, β, σ)` is
  pinned up to the common scale fixed by the numerator, and the closed
  form above must **not** be applied (it would change the ratio).
- **General implementation.** After the solve, add the constraint
  `t ≤ t*(1 + 10⁻⁷)` and minimise `κ` (a second, lexicographic LP/SOCP
  stage). It is a no-op where `κ` is pinned and reproduces the closed form
  where `κ` is free, so one code path covers all cases. With an empty book
  and costs the problem is homogeneous again (`w_cur = 0`: costs scale with
  `w`), and the second stage applies.

### Infeasible and degenerate cases

| Case | Policy |
|---|---|
| `Σ_i lb_i + Σ_restricted w_cur > 0.98`, where `lb_i = w_cur_i` for `no_sell` | The locked holdings alone exceed the budget. **Do not trade. Hold.** Emit `w_target = w_current` for every held name, `opt_status = infeasible_locks`, audit `CRITICAL`. Orders that would breach the budget are never produced |
| A sector's locked holdings exceed `0.25` | Not infeasible: `C_g = L_g` (the relaxation). No buy in that sector's other names; the unlocked names there are sold toward 0 |
| No portfolio satisfying the bounds has a positive net excess return | Solver reports primal infeasible. Target is the lower bound: `w_target_i = w_cur_i` for `no_sell`, `0` otherwise (sell what the lock allows, hold the rest). This is the old "all `q_50 ≤ 0`: cash" rule restricted to what is reachable |
| Solver status other than `OPTIMAL` | No targets, no orders (hold), `ERROR` audit; no heuristic fallback |

### Precedence: lock over caps

The lock is a contractual obligation; the position cap and sector cap are
risk limits. Where they conflict the **lock wins**:

- A buy-locked (`no_sell`) name that has drifted above 5% is **not** a
  violation. The caps apply to orders at placement: an order may not take
  the resulting weight above the cap. So `u_i = max(0.05, w_cur_i)` (no
  buys, no forced sale).
- The sector cap is relaxed by the locked excess so the problem stays
  feasible (`C_g = max(0.25 − F_g, L_g)`).
- If even then the budget cannot hold the locked holdings (`> 0.98`), do
  not trade (table above).
- Sells that **reduce** a cap breach in names that are *not* buy-locked are
  still produced.
- A buy inside a sector whose cap is relaxed is only possible together with
  a sale in the same sector in the same session (the solution then does not
  increase the sector weight); the sale must fill first (the order
  generator sequences sells before buys within a sector, and the pre-trade
  check of `10-execution-schwab/SKILL.md` §1 evaluates against settled
  holdings; whether it nets same-session sells is an open point for that
  skill).

### What is exact and what is an approximation

| Statement | Status |
|---|---|
| Lock bounds as linear constraints in `(y, κ)` | Exact |
| Cost-aware optimum = exact maximiser of the net Sortino ratio on the scenarios | Exact for the stated model (proportional costs, scenarios fixed) |
| Costs are proportional to traded notional, no market impact | Approximation (impact is negligible at current size; `EXECUTION-WINDOW.md` §7.4) |
| One-off trade cost subtracted from an `H`-day return | Approximation: positions are not necessarily held for `H` days; the exit cost is charged when the sale is made (`c^s` applies to reductions), not at purchase |
| Costs shift every scenario by the same constant | Consistent with the cost being a certain amount, not a risk |
| Cost estimates `c^b, c^s` | Estimates from quotes and fills; uncertain |
| Lock snapshot | Advisory: may be stale (external-account orders after the snapshot); the placement-time check is authoritative |
| Restricted holdings (zero in practice) | Excluded from the return/risk of the ratio; they only reduce `B` and the sector headroom |
| Conviction scaling, renormalisation, caps, regime gate (SKILL.md §4-5) after the solve | Applied to a cost-aware solution, so the result is no longer exactly cost-optimal: hence the no-trade band (SKILL.md §5d) as a last step |
| No-trade band | Heuristic hysteresis, not derived |
| Inter-temporal lock cost (a buy today locks sells for 5 days) | **Not modelled** in the optimizer; the band and the minimum rebalance interval (`EXECUTION-WINDOW.md` §7) reduce its frequency |

### Numerical checks that back §3a (small synthetic problems)

Run once with an external SLSQP solver during authoring; the scripts were scratch
files and are not kept (the repository has no Python). The SKILL.md §8 tests
are the Julia equivalents and are what CI runs. Three assets,
30 scenarios (re-centred), solved with SLSQP against a direct grid search of
the ratio under the same bounds:

- no lock: grid ratio 3.0342, homogeneous solve 3.0343 (matches);
- buy-lock on asset 1 (`w ≥ 0.30`) and sell-lock on asset 3 (`w ≤ 0.20`):
  grid 3.0556 and solve 3.0556 with the recovered weights respecting both
  bounds; the `κ` interval is non-empty and `κ_lo` reproduces the optimal
  ratio;
- relaxed sector cap (locked weights 0.45 over a 0.40 cap): feasible with
  the relaxation, infeasible without it (solver and bound arithmetic);
- proportional costs on a held book: grid 5.4803 vs solve 5.5209 (the solve
  is exact, the grid is coarse, step 0.02) and the solve holds the book
  where the cost-blind optimum would trade, whose net ratio is 4.7724;
- costs plus locks: grid 3.5631 vs solve 3.5639;
- empty book with costs: the ratio is the same at `w` and `w/2`
  (homogeneous again); non-empty book: 5.2423 at `w` and 4.2300 at `w/2`
  (pinned);
- the numerator constraint is active (equals 1) at the optimum;
- the second `κ`-minimising stage matches the closed form
  (72.4639 vs 72.4895 in a case where it is free) and leaves a pinned
  solution's ratio unchanged;
- when every excess return is non-positive the program is infeasible, as
  stated.

The post-optimization `apply_caps_locked` of SKILL.md §5 was checked on
random cases: with no locks it equals the existing iterative redistribution
(200 cases); with random locks, drifted names and relaxed sector caps it
satisfies the bounds, the budget and the sector caps (2000 cases); the
regime gate and the no-trade band preserve the lock direction (2000 cases);
and a lock-blind renormalisation violates a buy-lock lower bound in 500 of
500 random cases (why the locked version is needed). These are checks on
synthetic problems with a generic solver, not on the Julia code, which
cannot be run in this environment.

## 4. JuMP Sketch

```julia
using JuMP, Clarabel, LinearAlgebra

# R̃ :: T×N re-centred scenarios, ex = μ .- Rf (= q_50), Rf scalar, same unit.
m = Model(Clarabel.Optimizer)
@variable(m, y[1:N] >= 0)
@variable(m, κ >= 0)
@variable(m, t >= 0)
@variable(m, d[1:T] >= 0)
@constraint(m, dot(ex, y) == 1)
@constraint(m, [s = 1:T], d[s] >= Rf * sum(y) - dot(R̃[s, :], y))
@constraint(m, [sqrt(T) * t; d] in SecondOrderCone())
@constraint(m, sum(y) <= 0.98 * κ)
@constraint(m, y .<= 0.05 * κ)
@constraint(m, [g in sectors], sum(y[idx[g]]) <= 0.25 * κ)
@objective(m, Min, t)
optimize!(m)
# then: κ* = max(...) from §3; w = value.(y) ./ κ*
```

The statuses must be `OPTIMAL`; any other status is a failure and the run
emits no targets (no heuristic fallback).

### 4a. JuMP Sketch of the General Program (§3a)

```julia
# Inputs: N eligible names (restricted, vetoed and, in the Bootstrap tier, fractional-ineligible already removed; held vetoed names are fixed amounts and re-added to the output with target 0 plus the lock clamp, SKILL.md §5b-2, §5b-1), R̃ (T×N), ex = μ .- Rf,
# w_cur, no_sell::BitVector, no_buy::BitVector, cb, cs (horizon-unit costs),
# u (per-name upper bounds), Cg (relaxed sector caps), B = 0.98 - restricted weight.
m = Model(Clarabel.Optimizer)
@variable(m, y[1:N] >= 0);  @variable(m, κ >= 1e-9);  @variable(m, t >= 0)
@variable(m, β[1:N] >= 0);  @variable(m, σ[1:N] >= 0);  @variable(m, d[1:T] >= 0)
@constraint(m, y .== κ .* w_cur .+ β .- σ)
@constraint(m, β[no_buy]  .== 0)                  # counted sell: cannot buy
@constraint(m, σ[no_sell] .== 0)                  # counted buy: cannot sell
@constraint(m, dot(ex, y) - dot(cb, β) - dot(cs, σ) >= 1)
@constraint(m, [s = 1:T], d[s] >= Rf * sum(y) - dot(R̃[s, :], y) + dot(cb, β) + dot(cs, σ))
@constraint(m, [sqrt(T) * t; d] in SecondOrderCone())
@constraint(m, sum(y) <= B * κ)
@constraint(m, y .<= u .* κ)
@constraint(m, [g in sectors], sum(y[idx[g]]) <= Cg[g] * κ)
@objective(m, Min, t)
optimize!(m)                                      # status must be OPTIMAL
tstar = value(t)
# stage 2 (lexicographic): deploy to the frontier where κ is free; a no-op where it is pinned
@constraint(m, t <= tstar * (1 + 1e-7))
@objective(m, Min, κ)
optimize!(m)
w_opt = value.(y) ./ value(κ)
```

The infeasible-lock and no-positive-excess cases (§3a table) are detected
before the solve (`sum(lb) + restricted > 0.98`) or from the solver status.

## 5. Why SOCP Over Heuristic

Earlier downside-risk literature used heuristic approximations because
the problem was computationally complex. Modern convex optimization solves
the Global Minimum Semivariance problem exactly, and the homogeneous form
above solves the ratio problem exactly: the feasible set is a convex cone
section, `t` is a norm, and the constraints are linear. Clarabel supports
second-order cones natively. `PortfolioOptimisers.jl` is the intended
front end for the constraints; the formulation above is what it must
reproduce (verify its API).

## 6. The Target Return

The minimum acceptable return (MAR) is the risk-free return over the
horizon `H` (from FRED `DGS3MO`):

    R_target = R_f,H = (1 + DGS3MO/100)^(H/252) − 1

with `H` the GSPQR horizon of the `q_50` in use (63 by default). In daily
units use `(1 + DGS3MO/100)^(1/252) − 1` and the de-compounded `μ`.

## 7. Expected Returns

The expected returns come from the GSPQR `q_50` (median) forecast for
horizon `H`. They are **not** historical mean returns. They are
forward-looking quantile estimates from the LightGBM model, already net of
the risk-free return (§2).

Using GSPQR `q_50` rather than historical mean is critical. Historical
means are notoriously noisy and unstable. The GSPQR median is a
regularized, feature-conditioned estimate. The historical scenarios
contribute only the **shape** of the downside (after re-centring).

## 8. Inputs That Do Not Enter the Problem

- **GSPQR `q_05`** is not an input to the optimization. It is carried to
  `portfolio_targets` as the column `downside_risk` for diagnostics only.
  Its meaning is the stock's GSPQR 5th-percentile forecast over `H`, not a
  semi-deviation, and it does not constrain or weight anything.
- **The Ledoit-Wolf covariance** (`COVARIANCE.md`) does not enter the
  problem either. It is used for diagnostics and risk reporting. Unless a
  volatility cap (for example `√(wᵀΣw) ≤ σ_max`) is added to the problem
  in a future change, there are no risk-contribution or covariance-based
  constraints. Sector caps are linear constraints on weights, not
  covariance-based.

## 9. What Not to Do

- Do not maximize the ratio directly as a nonconvex program, and do not
  describe the ratio as an SOCP without the homogenization.
- Do not use variance as the risk measure. The objective uses downside
  deviation.
- Do not use historical mean returns as `μ_i`. Use GSPQR `q_50`.
- Do not subtract the risk-free rate from `q_50` again; it is already an
  excess return.
- Do not mix horizons: scenarios, `μ`, and `R_target` share one unit.
- Do not skip re-centring the scenarios on `μ`.
- Do not add `Σ w = 1` or renormalize to 1. The only budget rule is
  `Σ w ≤ 0.98`.
- Do not use `R_f κ` for the cash term in the shortfall constraint unless
  the portfolio is fully invested; use `R_f (1ᵀ y)`.
- Do not use a heuristic approximation. The SOCP solves to global
  optimality.
- Do not solve a cost-aware problem by adding `c·|w − w_cur|` to the
  homogeneous program in `w` alone: it is not homogeneous. Use the
  perspective form of §3a (`β, σ` with `y = κ w_cur + β − σ`).
- Do not apply the closed-form smallest-`κ` rule when costs and a
  non-empty book pin `κ`; use the second stage.
- Do not let the optimizer choose a weight the lock refuses (`w_i < w_cur_i`
  on a buy-locked name, `w_i > w_cur_i` on a sell-locked name).
- Do not build the problem with a restricted ticker (`ICE`) in it.
- Do not set `R_target` to 0. Use the risk-free return.
- Do not skip the SOCP formulation and use a QP solver. The downside
  deviation term is a cone, not a quadratic.
- Do not describe `q_05` as a risk input or the covariance as a constraint
  input. Neither enters the problem.
