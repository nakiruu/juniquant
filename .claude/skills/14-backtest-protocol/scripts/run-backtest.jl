#!/usr/bin/env julia
# CPCV evaluation of the RAW GSPQR quantile model and the portfolio, and the
# structure of the walk-forward metrics (14-backtest-protocol/SKILL.md §2a, §6).
#
# SKELETON. Never run (no Julia in the authoring environment). Written: the CPCV
# splitter with purge and embargo, path assembly, and the metric helpers. NOT
# written: the 21-fit-per-split training loop, test-block prediction, the
# portfolio/lock/cost simulator, the walk-forward metric computation, the
# evaluation_metrics write, and every test. The earlier version of this file
# returned one fold, read columns the panel does not have (q_hat_lower,
# q_hat_upper), and used the TCP midpoint as the median; all three were wrong
# and are removed.
#
# What it must do when finished:
#   CPCV track (one model per split; TCP/EVT are NOT run here):
#     - 120 splits x 21 fits, fixed n_estimators, no early stopping, fixed seed
#       (03-gspqr-model/HYPERPARAMETERS.md §2); predict the split's test blocks
#       (raw q_01..q_99, rearranged); store the portfolio-horizon predictions under
#       $QUANT_ROOT/reports/backtest/<run_id>/cpcv/split=<k>/ (run artifact, not a lake dataset; never under lake/).
#     - raw-model metrics per path: pinball at each tau, coverage_90 of [q_05, q_95],
#       rank_ic of q_50, approximate CRPS from the seven quantiles.
#     - portfolio per path, INSIDE each fold (SKILL.md §6, §6d-6f): the optimizer's
#       expected return is the raw q_50 (constitution §9, never a TCP midpoint);
#       lock rule as a pure function of the simulated order history (D = t+1 for a
#       decision at close t; blocks D..D+4; first opposite order D+5 trading days; only
#       filled orders count; computed NYSE calendar, not prices dates); lock-, restricted-
#       and cost-aware optimizer (08 §5a-5d); tier-aware spread-based costs
#       (10-execution-schwab/EXECUTION-WINDOW.md §5), reference mid proxy; fractional
#       rules and 2-decimal floor (§6g); spread_source tagging per fill; each contiguous
#       test block starts flat with an empty lock history; "cost of the lock" runs
#       R0/R1/R2 (§6f; LockMode.Off exists ONLY here); regime gate from the
#       walk-forward regime rows (forward-only), applied last.
#     - metrics per path: Sortino, Sharpe, MDD, DSR (only with N logged), turnover,
#       implementation shortfall, lock-binding frequency, spread-cost attribution,
#       each also split by spread_source (METRICS.md §8).
#   Walk-forward track (computed once from the stored as-of pipeline, NOT from CPCV):
#     coverage_95, avg_width, Winkler (alpha = 0.05) of the calibrated 95% interval
#     [q_hat_lower, q_hat_upper]; qhat_zero_frac / qhat_applied_nonpos_frac; EVT
#     exceedance rates; by calendar year and regime with HAC s.e. (lag >= H).
#   Every report starts with the survivorship / source header (SKILL.md §6h).

using Pkg
Pkg.activate(temp=true)
Pkg.add(["Parquet2", "DataFrames", "Dates", "Statistics"])

using Parquet2
using DataFrames
using Dates
using Statistics

# ---- Configuration ----
# Paths per 13-parquet-lake/IO-CONTRACT.md §1: no production default; an unset variable is an error.
envpath(name) = get(() -> error("$name is not set (13-parquet-lake/IO-CONTRACT.md §1)"), ENV, name)
const LAKE = joinpath(envpath("QUANT_ROOT"), "lake")
const PANEL_PATH = get(ENV, "RUN_BACKTEST_PANEL", joinpath(LAKE, "panel"))   # dir of year=/month=/data.parquet
const RUN_BACKTEST_OUTPUT = get(ENV, "RUN_BACKTEST_OUTPUT", joinpath(LAKE, "evaluation_metrics.parquet"))

const N_GROUPS = 10
const K_TEST = 3            # C(10,3) = 120 splits, C(9,2) = 36 paths
const HORIZONS = (63, 126, 252)   # label horizons H; the purge of a model is ITS H, both sides
const EMBARGO_MIN = 21            # SKILL.md §3 (owner decision C1): h = max(21, ceil(0.01 * T))
const EMBARGO_FRAC = 0.01

# Embargo after each test block (beyond the back-side purge). T = panel length in trading
# days (distinct panel dates), not the evaluable count: T = 3780 -> 38; T = 1000 -> 21.
embargo_days(T::Int) = max(EMBARGO_MIN, ceil(Int, EMBARGO_FRAC * T))

# All offsets below count on the GLOBAL trading-calendar index (positions in the sorted
# distinct panel dates), never on one ticker's rows.

# ---- Combinations of k groups out of n, lexicographic ----
function combos(n::Int, k::Int)
    out = Vector{Vector{Int}}()
    function rec(start, cur)
        if length(cur) == k
            push!(out, copy(cur)); return
        end
        for i in start:n
            push!(cur, i); rec(i + 1, cur); pop!(cur)
        end
    end
    rec(1, Int[])
    return out
end

# ---- CPCV splits on a vector of evaluable date positions 1:D ----
# Returns, per split, the test group ids, the test positions and the training positions
# for a model of label horizon `purge` (= H). Purge, both sides: a training row t has the
# label [t, t+H] and a test row s has [s, s+H]; they overlap iff |t - s| <= H, so rows within
# H before the block start and within H after the block end are removed. Embargo: a further
# `embargo` rows after the back-side purge (residual serial correlation). Feature lookback
# (mom_12m_1m) is not leakage by itself and adds nothing here. Training rows only.
# The splits differ by horizon: call once per H (HORIZONS).
function cpcv_splits(D::Int; n::Int = N_GROUPS, k::Int = K_TEST,
                     purge::Int, embargo::Int)
    purge >= 1 || error("purge must be the model's label horizon H")
    embargo >= EMBARGO_MIN || error("embargo below the minimum of $EMBARGO_MIN days")
    gsize = div(D, n)
    gstart(g) = (g - 1) * gsize + 1
    gend(g) = g == n ? D : g * gsize
    splits = NamedTuple[]
    for test_groups in combos(n, k)
        is_test = falses(D); excluded = falses(D)
        for g in test_groups
            is_test[gstart(g):gend(g)] .= true
            excluded[max(1, gstart(g) - purge):(gstart(g) - 1)] .= true             # front purge
            excluded[(gend(g) + 1):min(D, gend(g) + purge + embargo)] .= true      # back purge + embargo
        end
        train = findall(.!(is_test .| excluded))
        push!(splits, (test_groups = test_groups, test = findall(is_test), train = train))
    end
    return splits
end

# ---- Path assembly: path p uses, for each group g, the p-th split that has g as a test group ----
# phi = C(N-1, k-1) paths; every (split, group) pair is used by exactly one path.
function path_assignment(splits; n::Int = N_GROUPS, k::Int = K_TEST)
    phi = length(combos(n - 1, k - 1))
    lists = [findall(s -> g in s.test_groups, splits) for g in 1:n]
    all(length(l) == phi for l in lists) || error("unexpected number of splits per group")
    return [[lists[g][p] for g in 1:n] for p in 1:phi]    # path p -> split id per group
end

# ---- Metrics (helpers; the call sites are not written) ----
coverage(y, lower, upper) = mean((y .>= lower) .& (y .<= upper))

function winkler(y, lower, upper, alpha)
    total = 0.0
    for i in eachindex(y)
        w = upper[i] - lower[i]
        total += y[i] < lower[i] ? w + (2 / alpha) * (lower[i] - y[i]) :
                 y[i] > upper[i] ? w + (2 / alpha) * (y[i] - upper[i]) : w
    end
    return total / length(y)
end

# Pinball loss of a RAW quantile q_tau against y. NOT for q_hat_lower/q_hat_upper
# (those are the endpoints of a 95%-coverage interval, not 5%/95% quantiles).
function pinball(y, yhat, tau)
    total = 0.0
    for i in eachindex(y)
        d = y[i] - yhat[i]
        total += d >= 0 ? tau * d : (1 - tau) * (-d)
    end
    return total / length(y)
end

# Sortino with the standard downside deviation: mean over ALL observations of min(excess, 0)^2.
# (The earlier code averaged only the negative observations, which overstates downside risk.)
function sortino(returns::Vector{Float64}, target::Float64)
    excess = returns .- target
    dd = sqrt(mean(min.(excess, 0.0) .^ 2))
    return dd == 0 ? Inf : mean(excess) / dd
end

# ---- Main: plan only ----
function main()
    panel = DataFrame(Parquet2.Dataset(PANEL_PATH))  # partitioned read (verify)
    dates = sort(unique(panel.date))
    # TODO: restrict to the evaluable dates (first date with a feature row and a regime row,
    # through the last date with a 63-day label); here all dates, for the shape of the plan.
    # TODO: training rows are further restricted to names eligible at their date (C3).
    h = embargo_days(length(dates))
    for H in HORIZONS
        splits = cpcv_splits(length(dates); purge = H, embargo = h)
        paths = path_assignment(splits)
        @info "CPCV plan H=$H (purge $H both sides, embargo $h): $(length(splits)) splits, " *
              "$(length(paths)) paths, mean train positions " *
              "$(round(mean(length(s.train) for s in splits), digits = 1)) of $(length(dates))"
    end
    @info "Skeleton ends here: nothing was trained or evaluated"
end

main()
