#!/usr/bin/env julia
# Walk-forward regime states: a monthly-refit 3-state Gaussian HMM, with
# FILTERED probabilities and an ONLINE Viterbi state per date, no lookahead
# (06-regime-detection/HMM-PROCEDURE.md).
#
# SKELETON. Never run (no Julia in the authoring environment). Written: own
# 1-D Gaussian HMM (scaled forward-backward EM, relabel by variance, filter,
# online Viterbi), the confirmation rule, the universe return, the month loop.
# NOT written: reading the real universe/prices schemas (column names below
# are assumptions: verify against SCHEMAS.md #1/#2 and the adjusted-price
# helper of 02-panel-construction), Parquet2 calls (verify), the atomic
# append write, the parameters JSON, the WARN rows, every test.
#
# Usage: julia fit-regime.jl   (no arguments parsed yet: it walks all dates)

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
# `prices` is RAW as-traded OHLCV (P(year,month)); returns come only from the total-return index `tri`
# (02-panel-construction/FEATURES.md §0), computed from prices + splits + dividends by features/src/raw.jl.
const PRICES_PATH      = get(ENV, "FIT_REGIME_PRICES",    joinpath(LAKE, "prices"))          # dir of year=/month=/data.parquet
const SPLITS_PATH      = get(ENV, "FIT_REGIME_SPLITS",    joinpath(LAKE, "splits.parquet"))
const DIVIDENDS_PATH   = get(ENV, "FIT_REGIME_DIVIDENDS", joinpath(LAKE, "dividends.parquet"))
const UNIVERSE_PATH    = get(ENV, "FIT_REGIME_UNIVERSE",  joinpath(LAKE, "universe"))        # dir of year=/data.parquet
const FIT_REGIME_OUTPUT = get(ENV, "FIT_REGIME_OUTPUT",   joinpath(LAKE, "regime_states.parquet"))  # single dataset (13 SKILL §3 #6)

const N_STATES = 3
const TRAIN_WINDOW = 504
const STATE_LABELS = ["low_vol", "high_vol", "crisis"]
const FACTOR = Dict("low_vol" => 1.0, "high_vol" => 0.5, "crisis" => 0.0)

# ---- Model ----
struct GHMM
    pi::Vector{Float64}
    A::Matrix{Float64}      # rows sum to 1
    mu::Vector{Float64}
    var::Vector{Float64}
end

logb(h::GHMM, x::Float64) = [-0.5 * (log(2pi * h.var[k]) + (x - h.mu[k])^2 / h.var[k]) for k in 1:N_STATES]

# Scaled forward pass. Returns filtered probabilities (T x K), per-step scales, and the log-likelihood.
function forward(h::GHMM, r::Vector{Float64})
    T = length(r); K = N_STATES
    alpha = zeros(T, K); c = zeros(T); ll = 0.0
    for t in 1:T
        lb = logb(h, r[t]); m = maximum(lb); b = exp.(lb .- m)
        v = t == 1 ? h.pi .* b : (h.A' * alpha[t-1, :]) .* b
        c[t] = sum(v); alpha[t, :] = v ./ c[t]
        ll += log(c[t]) + m
    end
    return alpha, c, ll
end

# Online Viterbi: state at t = argmax_j delta_t(j) using data through t only.
function online_viterbi(h::GHMM, r::Vector{Float64})
    T = length(r); K = N_STATES
    lA = log.(h.A); delta = log.(h.pi) .+ logb(h, r[1]); out = Vector{Int}(undef, T)
    out[1] = argmax(delta)
    for t in 2:T
        lb = logb(h, r[t])
        delta = [maximum(delta .+ lA[:, j]) + lb[j] for j in 1:K]
        out[t] = argmax(delta)
    end
    return out
end

# EM (Baum-Welch), scaled. Smoothed gammas are used for fitting only, never stored.
function em_fit(r::Vector{Float64}, h0::GHMM; max_iter = 100, tol = 1e-6)
    h = h0; T = length(r); K = N_STATES; prev = -Inf; converged = false
    for it in 1:max_iter
        alpha, c, ll = forward(h, r)
        ll < prev - 1e-9 && error("log-likelihood decreased; fit invalid")
        beta = ones(T, K); B = zeros(T, K)
        for t in 1:T
            lb = logb(h, r[t]); B[t, :] = exp.(lb .- maximum(lb))
        end
        for t in (T-1):-1:1
            beta[t, :] = h.A * (B[t+1, :] .* beta[t+1, :]) ./ c[t+1]
        end
        gamma = alpha .* beta; gamma ./= sum(gamma, dims = 2)
        xi = zeros(K, K)
        for t in 1:(T-1)
            xi .+= (alpha[t, :] * (B[t+1, :] .* beta[t+1, :])') .* h.A ./ c[t+1]
        end
        A = xi ./ sum(xi, dims = 2)
        w = vec(sum(gamma, dims = 1))
        mu = [sum(gamma[:, k] .* r) / w[k] for k in 1:K]
        v  = [max(sum(gamma[:, k] .* (r .- mu[k]) .^ 2) / w[k], 1e-8) for k in 1:K]
        h = GHMM(gamma[1, :], A, mu, v)
        if ll - prev < tol; converged = true; break; end
        prev = ll
    end
    return h, converged
end

# Initialization by terciles of |r - mean| (a volatility split, not a return-level split).
function init_hmm(r::Vector{Float64})
    d = abs.(r .- mean(r)); q1 = quantile(d, 1/3); q2 = quantile(d, 2/3)
    groups = [r[d .<= q1], r[(d .> q1) .& (d .<= q2)], r[d .> q2]]
    A = fill(0.05 / (N_STATES - 1), N_STATES, N_STATES)
    for i in 1:N_STATES; A[i, i] = 0.95; end
    return GHMM(fill(1 / N_STATES, N_STATES), A, mean.(groups), max.(var.(groups), 1e-8))
end

# Relabel by emission variance ascending and RETURN the permuted model.
function relabel_by_variance(h::GHMM)
    o = sortperm(h.var)
    return GHMM(h.pi[o], h.A[o, o], h.mu[o], h.var[o])
end

# ---- Universe return: mean over the point-in-time ELIGIBLE set of within-ticker log returns ----
# `adj` holds (ticker, date, tri); tri from 02 FEATURES §0, never the raw close.
function universe_returns(adj::DataFrame, uni::DataFrame, cal::Vector{Date})
    cidx = Dict(d => i for (i, d) in enumerate(cal))
    elig = Set((r.ticker, r.date) for r in eachrow(uni) if r.eligible)
    sums = zeros(length(cal)); cnts = zeros(Int, length(cal))
    for g in groupby(adj, :ticker)
        g = sort(DataFrame(g), :date)
        for k in 2:nrow(g)
            i = cidx[g.date[k]]; j = cidx[g.date[k-1]]
            i - j == 1 || continue                         # no return across a gap on the global calendar
            (g.ticker[k], g.date[k]) in elig || continue   # eligible on the date itself
            sums[i] += log(g.tri[k] / g.tri[k-1]); cnts[i] += 1
        end
    end
    return [cnts[i] > 0 ? sums[i] / cnts[i] : NaN for i in eachindex(cal)]
end

# ---- Confirmation: 2 consecutive equal raw days switch the confirmed state ----
confirm(raw_t::Int, raw_prev::Int, confirmed_prev::Int) = raw_t == raw_prev ? raw_t : confirmed_prev

# ---- Main: walk-forward over month-ends ----
function main()
    # TODO: tri via the 02 helper (features/src/raw.jl) over PRICES_PATH, SPLITS_PATH, DIVIDENDS_PATH;
    # columns (ticker, date, tri). Raw closes are never used as returns.
    adj = DataFrame(Parquet2.Dataset(PRICES_PATH))         # placeholder: replace by the tri helper (verify)
    uni = DataFrame(Parquet2.Dataset(UNIVERSE_PATH))
    cal = sort(unique(uni.date))                           # global calendar (to be asserted = NYSE trading days)
    r = universe_returns(adj, uni, cal)

    month_end(i) = i == length(cal) || month(cal[i+1]) != month(cal[i])
    fit_idx = [i for i in (TRAIN_WINDOW + 1):length(cal) if month_end(i) && all(isfinite, r[(i-TRAIN_WINDOW+1):i])]
    rows = DataFrame(date = Date[], regime_low_vol_prob = Float64[], regime_high_vol_prob = Float64[],
                     regime_crisis_prob = Float64[], viterbi_state = String[], regime_factor = Float64[],
                     fit_date = Date[], confirmed_state = String[])
    prev_raw = 0; prev_conf = 0
    prev_h = nothing

    for (n, m) in enumerate(fit_idx)
        w0 = m - TRAIN_WINDOW + 1
        win = r[w0:m]
        # best of: terciles init, previous parameters as init (HMM-PROCEDURE.md §7)
        cands = Any[em_fit(win, init_hmm(win))]
        isnothing(prev_h) || push!(cands, em_fit(win, prev_h))
        best = argmax([forward(c[1], win)[3] for c in cands])
        h = relabel_by_variance(cands[best][1])
        prev_h = h                                         # TODO: occupancy-shift WARN; persist hmm_<fit_date>.json

        m_next = n < length(fit_idx) ? fit_idx[n+1] : length(cal)
        for t in (m+1):m_next
            isfinite(r[t]) || continue
            obs = r[w0:t]                                  # fit-window start through t: causal
            alpha, _, _ = forward(h, obs)
            raw = online_viterbi(h, obs)[end]              # last element only
            conf = prev_raw == 0 ? raw : confirm(raw, prev_raw, prev_conf)
            push!(rows, (cal[t], alpha[end, 1], alpha[end, 2], alpha[end, 3],
                         STATE_LABELS[raw], FACTOR[STATE_LABELS[conf]], cal[m], STATE_LABELS[conf]))
            prev_raw = raw; prev_conf = conf
        end
    end

    # TODO: write FIT_REGIME_OUTPUT (single dataset) through write_dataset (IO-CONTRACT §3), never
    # drop earlier rows (the earlier script wrote only the latest window).
    @info "Built $(nrow(rows)) regime rows (not written: skeleton)"
end

main()
