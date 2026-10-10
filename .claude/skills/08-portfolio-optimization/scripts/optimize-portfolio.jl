#!/usr/bin/env julia
# SKETCH, not compiled, not normative (Julia has not been run on it). The real home is portfolio/src/
# (pmpt.jl, covariance.jl, constraints.jl, conviction_apply.jl; scaffold.md).
# Runs the PMPT portfolio optimization and writes portfolio_targets.parquet.
#
# Sketch. The real optimizer maximizes the net-of-cost Sortino ratio via the
# Charnes-Cooper homogeneous SOCP with lock bounds and trade costs
# (08 PMPT-FORMULATION.md §3a, §4a; SKILL.md §5a-5d):
#   variables y >= 0, kappa > 0, t, d, beta >= 0 (buys), sigma >= 0 (sells);
#   minimize t  s.t.
#   y = kappa*w_cur + beta - sigma;  beta_i = 0 (counted SELL: cannot buy);
#   sigma_i = 0 (counted BUY: cannot sell);
#   (mu - Rf)'y - cb'beta - cs'sigma >= 1;  ||d||_2 <= sqrt(T) t;
#   d_s >= Rf*sum(y) - rt_s'y + cb'beta + cs'sigma, d >= 0;
#   sum(y) <= B kappa;  y_i <= u_i kappa;  sector sums <= C_g kappa
# (u_i = 0.05, but max(0.05, w_cur_i) for a counted-buy name; C_g = max(0.25 -
# restricted_g, locked_g): the lock wins over the caps). Then a second stage
# minimizes kappa subject to t <= t*(1+1e-7) (deploy to the frontier where kappa
# is free; a no-op where costs pin it). Zero costs and an empty book reduce this
# to the original program (§3).
# Units: scenarios, mu and R_target share ONE horizon unit; q_50 is already an
# excess return over the risk-free rate (mu = q_50 + Rf); scenarios are
# re-centred, rt_{i,s} = r_{i,s} - mean_i + mu_i. Budget: sum(w) <= 0.98 only,
# no sum(w) = 1.0. If no feasible portfolio has a positive net excess return,
# target = lower bounds (sell what the lock allows, hold the rest); if the
# locked lower bounds alone exceed the budget, HOLD (w_target = w_current), set
# opt_status = infeasible_locks and write a CRITICAL audit row.
# q_05 and the covariance are NOT inputs to the problem. q_05 is only carried
# to the output column `downside_risk` (diagnostic); the covariance is for
# risk reporting.
# Ordering (SKILL.md §4): veto filter -> restricted filter -> fractional-eligibility
# filter (Bootstrap tier only) -> entry spread filter
# -> solve -> conviction -> locked renormalize + caps -> regime gate (new capital
# only, applied last among the weight transforms) -> lock clamp (assertion) ->
# no-trade band -> portfolio_targets.
# Julia never places orders and never answers the authoritative lock check: the
# lock snapshot below only shapes the bounds; Rust re-checks at placement.
#
# The equal-weight block below is a placeholder, not the optimizer. This file
# has not been run (no Julia in the authoring environment); the numerical checks
# in PMPT-FORMULATION.md §3a cover the maths, not this code.
#
# Held vetoed names (SKILL.md §5b-2): not optimizer variables, but KEPT in the output with
# veto_active = true, w_*=0 and w_target = 0 unless lock-bound (then w_current). The order
# generator derives the sell intent from veto_active && w_current > 0. The band never skips them.
# Bootstrap defaults (SKILL.md §5f): absent positions / costs / lock snapshot => empty book,
# zero costs, default-empty lock; the defaults switch off once positions has a holding.

using Pkg
Pkg.activate(temp=true)
Pkg.add(["Parquet2", "DataFrames", "Dates", "Statistics",
         "PortfolioOptimisers", "Clarabel"])

using Parquet2
using DataFrames
using Dates
using Statistics
using TOML
using PortfolioOptimisers
using Clarabel

# ---- Configuration ----
# Convention (13-parquet-lake/IO-CONTRACT.md §1): QUANT_ROOT = data volume root (lake at $QUANT_ROOT/lake),
# QUANT_STATE = state volume, QUANT_CONFIG = config dir (read-only). No production default: an unset
# variable is an error. The per-file env overrides below stay for tests (script-specific names).
envpath(name) = get(() -> error("$name is not set (13-parquet-lake/IO-CONTRACT.md §1)"), ENV, name)
const QUANT_ROOT   = envpath("QUANT_ROOT")
const QUANT_STATE  = envpath("QUANT_STATE")
const QUANT_CONFIG = envpath("QUANT_CONFIG")
const LAKE = joinpath(QUANT_ROOT, "lake")
const PREDICTIONS_PATH = get(ENV, "OPTIMIZE_PORTFOLIO_PREDICTIONS", joinpath(LAKE, "gspqr_predictions"))   # dir, year=/month=
const TCP_PATH = get(ENV, "OPTIMIZE_PORTFOLIO_TCP", joinpath(LAKE, "tcp_calibration"))                     # dir, year=/month=
const VETO_PATH = get(ENV, "OPTIMIZE_PORTFOLIO_VETO", joinpath(LAKE, "veto_flags.parquet"))
const CONVICTION_PATH = get(ENV, "OPTIMIZE_PORTFOLIO_CONVICTION", joinpath(LAKE, "conviction_scores.parquet"))
const REGIME_PATH = get(ENV, "OPTIMIZE_PORTFOLIO_REGIME", joinpath(LAKE, "regime_states.parquet"))
const POSITIONS_PATH = get(ENV, "OPTIMIZE_PORTFOLIO_POSITIONS", joinpath(LAKE, "positions.parquet"))
# `prices` is a directory of RAW as-traded OHLCV (year=/month=); scenario returns come from the total-return
# index `tri` (02-panel-construction/FEATURES.md §0: prices + splits + dividends), never from raw closes.
const PRICES_PATH = get(ENV, "OPTIMIZE_PORTFOLIO_PRICES", joinpath(LAKE, "prices"))
const SPLITS_PATH = get(ENV, "OPTIMIZE_PORTFOLIO_SPLITS", joinpath(LAKE, "splits.parquet"))
const DIVIDENDS_PATH = get(ENV, "OPTIMIZE_PORTFOLIO_DIVIDENDS", joinpath(LAKE, "dividends.parquet"))
# Restricted list: ONE file shared with Rust (constitution §6). No copy, no hard-coded list.
const RESTRICTED_PATH = get(ENV, "OPTIMIZE_PORTFOLIO_RESTRICTED", joinpath(QUANT_CONFIG, "restricted.toml"))
const LOCK_SNAPSHOT_PATH = get(ENV, "OPTIMIZE_PORTFOLIO_LOCK_SNAPSHOT", joinpath(QUANT_STATE, "lock_snapshot.json"))
# Per-name cost model written by Rust after each session (10-execution-schwab/EXECUTION-WINDOW.md §5).
const COSTS_PATH = get(ENV, "OPTIMIZE_PORTFOLIO_COSTS", joinpath(LAKE, "execution_costs"))   # dir, year=
# Fractional-eligible list: ONE file shared with Rust, seeded by the owner. `Unknown` or absent =
# NOT eligible. Used only in the Bootstrap tier (SKILL.md §5b-1); the order generator uses it in Transition.
const FRACTIONAL_PATH = get(ENV, "OPTIMIZE_PORTFOLIO_FRACTIONAL", joinpath(QUANT_CONFIG, "fractional_eligible.toml"))
# Ledger: the tier input is the trailing 5-day mean NAV (00-construction §6a), read from here, the same
# quantity the Rust order generator reads. Never a second NAV computation, never cached.
const LEDGER_PATH = get(ENV, "OPTIMIZE_PORTFOLIO_LEDGER", joinpath(LAKE, "portfolio_ledger"))   # dir, year=
const OPTIMIZE_PORTFOLIO_OUTPUT = get(ENV, "OPTIMIZE_PORTFOLIO_OUTPUT", joinpath(LAKE, "portfolio_targets.parquet"))

const MAX_WEIGHT = 0.05
const MAX_SECTOR = 0.25
const CASH_BUFFER = 0.02

# ---- Restricted list (shared with Rust) ----
# Normalization: trim + upper case, then the canonical symbol map (09 SKILL.md section 3.8; here only the
# trivial part, the full map is Rust's config/symbol_map.toml).
normalize_ticker(t) = uppercase(strip(String(t)))

function load_restricted(path::AbstractString)::Set{String}
    # FAIL CLOSED: a missing/unparseable file, an absent or misspelled `restricted` key, an empty list,
    # or a list without ICE (rule never_trade) is an ERROR. There is no empty-set fallback.
    cfg = TOML.parsefile(path)
    haskey(cfg, "restricted") || error("restricted.toml: key `restricted` is absent (misspelled?)")
    entries = cfg["restricted"]
    (entries isa AbstractVector && !isempty(entries)) || error("restricted.toml: the restricted list is empty")
    for e in entries
        haskey(e, "ticker") || error("restricted.toml: an entry has no `ticker`")
    end
    never = Set(normalize_ticker(e["ticker"]) for e in entries if get(e, "rule", "") == "never_trade")
    "ICE" in never || error("restricted.toml: ICE must be present with rule = \"never_trade\" (constitution section 6)")
    return Set(normalize_ticker(e["ticker"]) for e in entries)
end

# ---- Fractional eligibility (SKILL.md §5b-1) ----
# The TOML schema is fixed by the owner's seed file; assumed here: [[fractional]] entries with
# `ticker` and `eligibility` ("Eligible" | "Unknown" | ...). Anything but "Eligible" is NOT eligible.
function load_fractional_eligible(path::AbstractString)::Set{String}
    # Missing/unparseable file halts a Bootstrap run: the filter cannot be skipped.
    cfg = TOML.parsefile(path)
    return Set(normalize_ticker(e["ticker"]) for e in get(cfg, "fractional", [])
               if get(e, "eligibility", "Unknown") == "Eligible")
end

# Tier from the trailing 5-day mean NAV; inclusive lower bounds ($2,500 = Transition, $10,000 = Standard).
tier_from_nav(nav5::Real) = nav5 < 2_500 ? :bootstrap : nav5 < 10_000 ? :transition : :standard

# Quantity rounding is the Rust order generator's job (floor_2dp(x) = floor(100x)/100, rounded DOWN,
# so the cap and the cash buffer never break); shown here only to document the rule. Julia never
# converts weights to quantities in production.
floor_2dp(x) = floor(100x) / 100

# ---- Lock bounds (SKILL.md §5a) ----
# status: :none | :no_sell (counted buy) | :no_buy (counted sell). Unknown/stale => :both (frozen).
# Returns (lb, ub) per name. The lock wins over the position cap.
function lock_bounds(status::Vector{Symbol}, w_cur::Vector{Float64})
    n = length(w_cur)
    lb = zeros(n); ub = fill(MAX_WEIGHT, n)
    for i in 1:n
        if status[i] == :no_sell                  # cannot sell: w >= w_cur, and no buy past the cap
            lb[i] = w_cur[i]; ub[i] = max(MAX_WEIGHT, w_cur[i])
        elseif status[i] == :no_buy               # cannot buy: w <= w_cur
            ub[i] = min(MAX_WEIGHT, w_cur[i])
        elseif status[i] == :both                 # fail closed: frozen
            lb[i] = w_cur[i]; ub[i] = w_cur[i]
        end
    end
    return lb, ub
end

# Sector cap relaxed by the locked excess: C_g = max(0.25 - restricted_g, sum of lb in g).
sector_cap(lb_in_g, restricted_g) = max(MAX_SECTOR - restricted_g, sum(lb_in_g))

# ---- Locked renormalize + caps (replaces the lock-blind apply_caps) ----
# One scale factor lambda, clipped to [lb, ub], found by bisection so sum == budget;
# a sector over its cap has its excess over lb scaled down to C_g and is frozen; repeat.
# With lb = 0, ub = 0.05 and no sector binding it equals the previous iterative redistribution.
# Throws if sum(lb) > budget: the caller holds (opt_status = infeasible_locks, CRITICAL).
function apply_caps_locked(w::Vector{Float64}; lb, ub, sectors, budget)
    sum(lb) <= budget + 1e-12 || error("sum of locked lower bounds exceeds budget")
    # ... bisection on lambda over the free names, sector freeze loop (see SKILL.md §5)
    error("sketch only")
end

# ---- Post-optimization no-trade band (SKILL.md §5d) ----
# tau_i = max(N_min/NAV, c * RT_i); forced exits (veto, cash buffer, cap breach) are exempt.
apply_band(w_target, w_cur, tau, forced) =
    ifelse.((abs.(w_target .- w_cur) .> tau) .| forced, w_target, w_cur)

# ---- Main ----
function main()
    rebalance_date = length(ARGS) >= 2 && ARGS[1] == "--date" ?
                     Date(ARGS[2]) : today()

    @info "Optimizing portfolio for $rebalance_date"

    # Load inputs
    preds = DataFrame(Parquet2.Dataset(PREDICTIONS_PATH))   # partitioned read (verify)
    tcp = DataFrame(Parquet2.Dataset(TCP_PATH))
    veto = DataFrame(Parquet2.read(VETO_PATH))
    conviction = DataFrame(Parquet2.read(CONVICTION_PATH))
    regime = DataFrame(Parquet2.read(REGIME_PATH))
    prices = DataFrame(Parquet2.Dataset(PRICES_PATH))       # RAW as traded: build `tri` (02 FEATURES §0) before any return
    positions = DataFrame(Parquet2.read(POSITIONS_PATH))

    # Filter to latest date
    preds = filter(r -> r.date == rebalance_date, preds)
    tcp = filter(r -> r.forecast_date == rebalance_date, tcp)
    # Vetoed names are NOT optimizer variables, but held vetoed names are kept for the output (SKILL.md 5b-2).
    veto_all = filter(r -> r.as_of_date == rebalance_date, veto)
    vetoed = Set(veto_all.ticker[veto_all.veto_active])
    veto = filter(r -> !r.veto_active, veto_all)
    conviction = filter(r -> r.as_of_date == rebalance_date, conviction)
    regime = filter(r -> r.date == rebalance_date, regime)

    # Regime factor. Applied last, to new capital only (08 SKILL §5). A
    # crisis regime does not return early: the optimizer's sells still flow.
    regime_factor = isempty(regime) ? 1.0 : regime.regime_factor[1]

    # Eligible universe (not vetoed)
    eligible = innerjoin(preds, veto, on = :ticker, makeunique = true)

    # Restricted filter (before the optimizer, like the veto): weight 0, never in
    # portfolio_targets. Same file as Rust. A restricted ticker that is held is
    # a fixed amount (reduces the budget/sector headroom), never a variable.
    restricted = load_restricted(RESTRICTED_PATH)
    eligible = filter(r -> !(r.ticker in restricted), eligible)

    # Fractional-eligibility filter (Bootstrap tier only; SKILL.md §5b-1). Tier = trailing 5-day mean
    # NAV from the ledger. Transition: no filtering (the order generator trades ineligible names in
    # whole shares only). Standard: not applicable. A held ineligible name in Bootstrap is a fixed
    # amount, not a variable (like a restricted holding).
    ledger = DataFrame(Parquet2.Dataset(LEDGER_PATH))
    tier = tier_from_nav(trailing_mean_nav(ledger, rebalance_date))   # hypothetical helper: mean NAV of the last 5 ledger days
    if tier == :bootstrap
        frac_ok = load_fractional_eligible(FRACTIONAL_PATH)
        eligible = filter(r -> r.ticker in frac_ok, eligible)
    end

    # Entry spread filter (EXECUTION-WINDOW.md §8): removes names from BUYING only. A held
    # name that fails it stays in the problem so the optimizer prices its sale with c_sell.

    # Expected returns (GSPQR q_50, 63d horizon)
    eligible = filter(r -> r.horizon == "63d", eligible)

    # Merge conviction
    eligible = leftjoin(eligible, conviction, on = :ticker)

    # Build the scenario matrix (in practice: pivot prices to wide format):
    # trailing 252 daily total returns, then re-centre each column on the
    # daily-unit forward mu_i = (1 + q_50_i + Rf_H)^(1/H) - 1 (same unit as
    # Rf_daily and the scenarios). The covariance (Ledoit-Wolf) is for
    # diagnostics only and is not used here.
    # ...

    # Per-name inputs for the general program (PMPT-FORMULATION.md §3a):
    #   w_cur       from positions.parquet at the decision close
    #   lock status from the lock snapshot for the placement session (:none/:no_sell/:no_buy/:both)
    #   cb, cs      buy/sell cost per name in the horizon unit (bps/1e4) from execution_costs.parquet
    #   (lb, ub)    = lock_bounds(status, w_cur);  sector caps C_g = sector_cap(...)
    #   budget      = 0.98 - restricted holdings
    # Pre-checks BEFORE the solve: sum(lb) + restricted > 0.98 => HOLD (w_target = w_cur,
    # opt_status = infeasible_locks, CRITICAL). Solver status must be OPTIMAL, else no
    # targets (no heuristic fallback). Everything lock-related is also re-checked in Rust.

    # Weights (simplified: equal weight for now; the real implementation solves
    # the homogeneous SOCP above with JuMP + Clarabel, status must be OPTIMAL,
    # and the weights satisfy sum(w) <= 0.98, w_i <= 0.05, sectors <= 0.25).
    n = nrow(eligible)
    if n == 0
        @warn "No eligible stocks"
        return
    end

    w_optimized = fill((1.0 - CASH_BUFFER) / n, n)

    # Apply conviction
    conviction_vals = coalesce.(eligible.conviction, 1.0)
    w_conviction = w_optimized .* conviction_vals
    w_final = w_conviction ./ sum(w_conviction) .* (1.0 - CASH_BUFFER)
    # (real implementation: w_final = apply_caps_locked(w_conviction; lb, ub, sectors,
    #  budget) here, before the regime gate. The simple renormalization above is
    #  lock-blind and is a placeholder: it can push a buy-locked name below w_current.)

    # Regime gate: scale only the increase over the current weight.
    # Never renormalize after this step; the residual stays in cash.
    cur = Dict(positions.ticker .=> positions.weight)
    w_current = [get(cur, t, 0.0) for t in eligible.ticker]
    w_gate = min.(w_current, w_final) .+
             regime_factor .* max.(w_final .- w_current, 0.0)
    # Lock clamp (assertion, not a repair): buy-locked >= w_current, sell-locked <= w_current.
    # No-trade band last (forced exits exempt), then portfolio_targets.
    w_target = w_gate   # real: apply_band(w_gate, w_current, tau, forced)

    # Build output
    out = DataFrame(
        rebalance_date = fill(rebalance_date, n),
        ticker = eligible.ticker,
        expected_return = eligible.q_50,
        downside_risk = eligible.q_05,    # GSPQR q_05 over H: diagnostic only, not a semi-deviation, not an optimizer input
        w_optimized = w_optimized,
        conviction = conviction_vals,
        w_conviction = w_conviction,
        w_final = w_final,
        veto_active = fill(false, n),   # vetoed names never reach this frame
        cap_applied = fill(false, n),
        regime_factor = fill(regime_factor, n),   # appended: column order is part of the schema hash
        w_target = w_target,
    )

    # Append the HELD vetoed names (SKILL.md 5b-2): w_optimized = w_conviction = w_final = 0,
    # w_target = 0 unless the lock binds (no_sell / BOTH / unusable snapshot => w_current),
    # veto_active = true. The no-trade band is never applied to them. The sell intent is derived by
    # the Rust order generator from veto_active && w_current > 0.
    #   held_vetoed = [t for t in vetoed if get(cur, t, 0.0) > 0]   # (Julia: comprehension over the set)
    #   target = lock_blocks_sell(t, snapshot) ? cur[t] : 0.0
    Parquet2.write(OPTIMIZE_PORTFOLIO_OUTPUT, out)   # TODO: through write_dataset (IO-CONTRACT §3)
    @info "Wrote $(nrow(out)) targets to $OPTIMIZE_PORTFOLIO_OUTPUT"
end

main()