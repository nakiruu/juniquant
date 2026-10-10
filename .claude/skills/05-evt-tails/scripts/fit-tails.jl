#!/usr/bin/env julia
# Fits pooled GPD tails to standardized calibrated residuals and writes
# evt_tail_bounds (05-evt-tails/EVT-PROCEDURE.md).
#
# SKELETON. Never run (no Julia in the authoring environment). Written: the
# native GPD MLE (profile likelihood, the procedure checked in a scratch
# script outside the repo), the stable extreme-quantile formula, the tail
# episode block count, the fallback cascade and the per-date fit loop.
# NOT written: argument parsing, month-partition atomic writes, the
# data_quality rows, resume logic, the market-tail-date diagnostic and every
# test. Parquet2 reading calls are unverified (verify). No Python anywhere.
#
# Per horizon and fit date t: pool z = residual / calibrated_halfwidth over all
# tickers and the available matured dates s in [max(t0, t-H-W_full+1), t-H]
# (global calendar index), fit one GPD per tail, map the pooled quantile back
# to each ticker with its current TCP mid and half-width.

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
const RESIDUALS_PATH = get(ENV, "FIT_TAILS_RESIDUALS", joinpath(LAKE, "evt_residuals"))   # partitioned year/month
const TCP_PATH       = get(ENV, "FIT_TAILS_TCP",       joinpath(LAKE, "tcp_calibration"))
const PANEL_PATH     = get(ENV, "FIT_TAILS_PANEL",     joinpath(LAKE, "panel"))           # only its distinct dates are used (global calendar)
const FIT_TAILS_OUTPUT = get(ENV, "FIT_TAILS_OUTPUT",  joinpath(LAKE, "evt_tail_bounds"))  # partitioned by year (13 SKILL §3 #10)

const THRESHOLD_Q = 0.95
const P_EXTREME = 0.99           # upper-tail probability, used for BOTH tails
const HORIZONS = ["63d", "126d", "252d"]
const HORIZON_DAYS = Dict("63d" => 63, "126d" => 126, "252d" => 252)
# Placeholders (PARAMETERS.md §1): checked on the walk-forward track, not tuned on CPCV.
const W_FULL = Dict("63d" => 504, "126d" => 756, "252d" => 1008)
const W_MIN  = Dict("63d" => 252, "126d" => 378, "252d" => 756)
const B_MIN = 3                  # tail episode blocks
const N_U_MIN = 50               # raw exceedances
const N_POOLED_GPD = 1000
const N_POOLED_EMP = 5000
const XI_MAX = 1.0               # reject xi >= 1
const XI_MIN = -0.5              # reject xi <= -0.5

# ---- Native GPD MLE: profile likelihood in theta = xi/sigma (EVT-PROCEDURE.md §5) ----
# l_p(theta) = -N [ ln sigma + xi + 1 ],  xi(theta) = mean(log1p(theta x)),  sigma = xi/theta.
# Grid + golden-section search. Returns nothing when no interior maximum exists.
function gpd_mle(x::Vector{Float64})
    n = length(x); xmax = maximum(x); xbar = mean(x)
    function lp(theta)
        if abs(theta) < 1e-10
            return (-n * (log(xbar) + 1), 0.0, xbar)
        end
        s = 0.0
        for v in x
            a = 1 + theta * v
            a <= 0 && return nothing
            s += log(a)
        end
        xi = s / n
        sigma = xi / theta
        (sigma <= 0 || xi == 0) && return nothing
        return (-n * (log(sigma) + xi + 1), xi, sigma)
    end
    lo = -(1 - 1e-6) / xmax
    hi = 20 / xbar
    grid = vcat(collect(range(lo, 0.0, length = 401))[1:400], [0.0],
                [hi * 10.0^(-4 + 4 * i / 400) for i in 1:400])
    best_i, best_l = 0, -Inf
    for (i, th) in enumerate(grid)
        r = lp(th)
        r === nothing && continue
        if r[1] > best_l
            best_l = r[1]; best_i = i
        end
    end
    best_i == 0 && return nothing
    a = grid[max(best_i - 1, 1)]; b = grid[min(best_i + 1, length(grid))]
    f(t) = (r = lp(t); r === nothing ? -Inf : r[1])
    g = (sqrt(5) - 1) / 2
    c = b - g * (b - a); d = a + g * (b - a)
    for _ in 1:80
        if f(c) > f(d); b = d; else; a = c; end
        c = b - g * (b - a); d = a + g * (b - a)
    end
    r = lp((a + b) / 2)
    r === nothing && return nothing
    return (shape = r[2], scale = r[3])
end

# ---- Extreme quantile of the sample (upper-tail procedure) ----
# Reference: x_p = u + (sigma/xi) * [ a^(-xi) - 1 ],  a = (n/N_u)(1-p)   (the -1 is INSIDE the bracket)
# Stable form: u + sigma * expm1(-xi * ln a) / xi ; limit below |xi| < 1e-8: u - sigma * ln a.
function extreme_quantile(u::Float64, sigma::Float64, xi::Float64, n::Int, n_u::Int, p::Float64)::Float64
    a = (n / n_u) * (1 - p)
    return abs(xi) < 1e-8 ? u - sigma * log(a) : u + sigma * expm1(-xi * log(a)) / xi
end

# ---- Tail episode blocks: non-overlapping H-day blocks, counted back from s_hi, holding an exceedance ----
function episode_blocks(date_idx::Vector{Int}, s_hi::Int, H::Int)::Int
    return length(Set((s_hi - d) ÷ H for d in date_idx))
end

# ---- Fit one tail of the pooled sample (cascade of EVT-PROCEDURE.md §8) ----
# z, date_idx: pooled standardized residuals and their global date indices.
# w_avail: window length actually available. Returns nothing => write no row.
function fit_pooled_tail(z::Vector{Float64}, date_idx::Vector{Int}, s_hi::Int, w_avail::Int,
                         H::Int, wmin::Int, tail::String)
    sample = tail == "upper" ? z : -z          # the only place the lower tail is negated going in
    n = length(sample)
    u = quantile(sample, THRESHOLD_Q)
    idx = findall(x -> x > u, sample)
    exceedances = sample[idx] .- u
    n_u = length(exceedances)
    n_blk = episode_blocks(date_idx[idx], s_hi, H)
    n_dates = length(unique(date_idx[idx]))    # informational: n_exceedances in the schema

    empirical() = begin
        if w_avail < H || n < N_POOLED_EMP
            return nothing                      # no row; data_quality evt_no_fit
        end
        q = tail == "upper" ? quantile(z, 0.99) : quantile(z, 0.01)
        (q_pooled = q, threshold_u = tail == "upper" ? u : -u, n_exceedances = n_dates, n_blocks = n_blk,
         n_pooled = n, n_exceed_raw = n_u, xi = NaN, sigma = NaN, method = "pooled_empirical",
         window_days = w_avail)
    end

    gates = w_avail >= wmin && n >= N_POOLED_GPD && n_u >= N_U_MIN && n_blk >= B_MIN
    gates || (@warn "Tail $tail: gates failed (w=$w_avail blocks=$n_blk n_u=$n_u n=$n), pooled empirical"; return empirical())

    gpd = gpd_mle(exceedances)
    isnothing(gpd) && (@warn "Tail $tail: MLE failed"; return empirical())
    xi = gpd.shape
    if xi >= XI_MAX || xi <= XI_MIN
        @warn "Tail $tail: xi = $xi rejected"; return empirical()
    end
    (xi > 0.5 || abs(xi) < 0.01) && @warn "Tail $tail: xi = $xi outside the comfortable range"

    x_p = extreme_quantile(u, gpd.scale, xi, n, n_u, P_EXTREME)
    if !isfinite(x_p) || x_p <= u
        return empirical()
    end
    q_pooled = tail == "upper" ? x_p : -x_p    # the single negation back to the z scale
    return (q_pooled = q_pooled, threshold_u = tail == "upper" ? u : -u, n_exceedances = n_dates, n_blocks = n_blk,
            n_pooled = n, n_exceed_raw = n_u, xi = xi, sigma = gpd.scale, method = "gpd",
            window_days = w_avail)
end

# ---- Main (loops over all fit dates; daily in the backfill) ----
function main()
    panel_dates = sort(unique(DataFrame(Parquet2.Dataset(PANEL_PATH)).date))   # verify: read only the date column
    cidx = Dict(d => i for (i, d) in enumerate(panel_dates))                   # GLOBAL calendar index

    resid = DataFrame(Parquet2.Dataset(RESIDUALS_PATH))       # verify: partitioned read
    tcp   = DataFrame(Parquet2.Dataset(TCP_PATH))
    resid.didx = [cidx[d] for d in resid.date]
    tcp.didx   = [cidx[d] for d in tcp.forecast_date]

    results = Dict{String,Any}[]
    for horizon in HORIZONS
        H = HORIZON_DAYS[horizon]
        hres = filter(r -> r.horizon == horizon && !ismissing(r.std_residual) && isfinite(r.std_residual), resid)
        htcp = filter(r -> r.horizon == horizon, tcp)
        isempty(hres) && continue
        # History comes from the pooled RESIDUAL history (its first date), not from prices.
        # (t0 is the first TCP origin; a residual exists from t0 and matures at t0 + H.)
        first_resid = minimum(hres.didx)
        by_date = Dict(k.didx => DataFrame(g) for (k, g) in pairs(groupby(hres, :didx)))
        tcp_by_t = Dict(k.didx => DataFrame(g) for (k, g) in pairs(groupby(htcp, :didx)))

        for t in sort(collect(keys(tcp_by_t)))              # fit dates with a TCP row
            s_hi = t - H
            s_lo = max(first_resid, s_hi - W_FULL[horizon] + 1)
            w_avail = s_hi - s_lo + 1
            w_avail < H && continue                          # nothing fits below one label block (cascade row 3)
            z = Float64[]; di = Int[]
            for s in s_lo:s_hi
                day = get(by_date, s, nothing)
                isnothing(day) && continue
                append!(z, Float64.(day.std_residual)); append!(di, fill(s, nrow(day)))
            end
            fits = Dict(tail => fit_pooled_tail(z, di, s_hi, w_avail, H, W_MIN[horizon], tail) for tail in ("lower", "upper"))
            for r in eachrow(tcp_by_t[t])
                mid = (r.q_hat_lower + r.q_hat_upper) / 2
                hw  = (r.q_hat_upper - r.q_hat_lower) / 2
                hw > 0 || continue
                for tail in ("lower", "upper")
                    f = fits[tail]
                    isnothing(f) && continue                 # TODO: data_quality evt_no_fit row
                    q_ext = mid + hw * f.q_pooled            # q_pooled(lower) < 0
                    q_ext = tail == "upper" ? max(q_ext, r.q_hat_upper) : min(q_ext, r.q_hat_lower)   # never narrow TCP
                    push!(results, Dict(
                        "ticker" => r.ticker, "fit_date" => panel_dates[t], "horizon" => horizon, "tail" => tail,
                        "threshold_u" => f.threshold_u, "n_exceedances" => f.n_exceedances,
                        "gpd_shape_xi" => f.xi, "gpd_scale_sigma" => f.sigma, "q_extreme" => q_ext,
                        # appended columns (SCHEMAS.md #10)
                        "n_pooled" => f.n_pooled, "n_exceed_raw" => f.n_exceed_raw, "q_pooled" => f.q_pooled,
                        "fit_method" => f.method, "n_episode_blocks" => f.n_blocks, "window_days" => f.window_days,
                    ))
                end
            end
        end
    end

    isempty(results) && (@warn "No EVT results produced"; return)
    out = DataFrame(results)
    select!(out, :ticker, :fit_date, :horizon, :tail, :threshold_u, :n_exceedances,
            :gpd_shape_xi, :gpd_scale_sigma, :q_extreme,
            :n_pooled, :n_exceed_raw, :q_pooled, :fit_method, :n_episode_blocks, :window_days)
    # TODO: write month partitions to a temp path + atomic rename (production);
    # FIT_TAILS_OUTPUT is unused in this skeleton.
    @info "Built $(nrow(out)) EVT rows (not written: skeleton)"
end

main()
