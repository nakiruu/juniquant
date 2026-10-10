#!/usr/bin/env julia
# TCP calibration, pooled across tickers by date (04-tcp-calibration/CALIBRATION.md §2).
#
# SKELETON. Never run (no Julia in the authoring environment). What is written:
# the pure helpers (score, pooled order statistic, Robbins-Monro step) and the
# per-origin loop structure on the global calendar. What is NOT written: the
# resume-from-persisted-state read, month-partition atomic writes, the
# data_quality WARN rows, the per_ticker option, the NYSE-calendar assertion,
# and every test. Parquet2 reading calls are unverified (verify).
#
# Usage: julia calibrate.jl   (the skeleton parses no arguments yet; it loops over every date)
# A backfill MUST run in date order within a horizon: the Robbins-Monro state
# (C, n) is sequential per (ticker, horizon) (CALIBRATION.md §5).

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
const QUANT_ROOT = envpath("QUANT_ROOT")
const LAKE = joinpath(QUANT_ROOT, "lake")
const GSPQR_PATH  = get(ENV, "CALIBRATE_GSPQR",  joinpath(LAKE, "gspqr_predictions"))  # partitioned (year/month)
const PANEL_PATH  = get(ENV, "CALIBRATE_PANEL",  joinpath(LAKE, "panel"))              # partitioned; y_fwd_Hd lives here
const CALIBRATE_OUTPUT = get(ENV, "CALIBRATE_OUTPUT", joinpath(LAKE, "tcp_calibration"))

const ALPHA = 0.05
const GAMMA_0 = 0.01
const OFFSET_CLIP = (-0.5, 0.5)
const WIDTH_FLOOR_OMEGA = 0.1       # placeholder
const N_MIN_SCORES = 5000           # placeholder
const MIN_TICKERS_PER_DATE = 50     # placeholder
const MIN_DATE_FRACTION = 0.8       # placeholder

const HORIZONS = ["63d", "126d", "252d"]
const HORIZON_DAYS = Dict("63d" => 63, "126d" => 126, "252d" => 252)
const Y_COL = Dict("63d" => :y_fwd_63d, "126d" => :y_fwd_126d, "252d" => :y_fwd_252d)
window_dates(H) = H                 # M_d = H (PARAMETERS.md §1, placeholder)

# ---- Zero-truncated, width-normalized conformity score ----
# Truncation makes every score >= 0, hence Q_hat >= 0: the calibrated interval
# is never narrower than the raw GSPQR interval (before the RM offset).
function norm_score(q05::Float64, q95::Float64, y::Float64, w_eff::Float64)::Float64
    return max(q05 - y, y - q95, 0.0) / w_eff
end

# Width floor: omega times the cross-sectional median width of the same date.
effective_width(w::Float64, median_w::Float64) = max(w, WIDTH_FLOOR_OMEGA * median_w)

# ---- Pooled order statistic ----
function pooled_quantile(scores::Vector{Float64}, alpha::Float64)::Float64
    n = length(scores)
    k = ceil(Int, (n + 1) * (1 - alpha))
    k = min(k, n)
    return partialsort(copy(scores), k)       # k-th smallest
end

# ---- Robbins-Monro step (state is (c, n) per (ticker, horizon)) ----
# Called at most once per origin per ticker, and only with a matured outcome.
function rm_update(c::Float64, n::Int, miss::Bool)
    e = (miss ? 1.0 : 0.0) - ALPHA
    gamma = GAMMA_0 / sqrt(n + 1)
    return clamp(c + gamma * e, OFFSET_CLIP[1], OFFSET_CLIP[2]), n + 1
end

# ---- Main ----
function main()
    gspqr = DataFrame(Parquet2.Dataset(GSPQR_PATH))     # verify: partitioned read
    panel = DataFrame(Parquet2.Dataset(PANEL_PATH))

    # GLOBAL trading-calendar index: sorted distinct panel dates (to be asserted
    # equal to the computed NYSE calendar; assertion not written). Never a
    # ticker's own row index and never prices.parquet per ticker.
    cal = sort(unique(panel.date))
    cidx = Dict(d => i for (i, d) in enumerate(cal))

    # Lookups built once (no O(N) filters inside the loops).
    preds_by = Dict{Tuple{String,Int},DataFrame}()     # (horizon, date index) -> that day's predictions
    for g in groupby(gspqr, [:horizon, :date])
        preds_by[(g.horizon[1], cidx[g.date[1]])] = DataFrame(g)
    end
    y_of = Dict{Tuple{String,String,Int},Float64}()    # (horizon, ticker, date index) -> y_fwd_Hd
    for h in HORIZONS
        col = Y_COL[h]
        for r in eachrow(panel)
            v = r[col]
            (ismissing(v) || isnan(v)) || (y_of[(h, r.ticker, cidx[r.date])] = Float64(v))
        end
    end

    rm_state = Dict{Tuple{String,String},Tuple{Float64,Int}}()   # (ticker, horizon) -> (C, n); TODO: load from the last stored partition
    issued = Dict{Tuple{String,String,Int},Tuple{Float64,Float64}}()  # (horizon, ticker, t) -> (lo, hi) stored intervals
    out = Dict{String,Any}[]

    for h in HORIZONS
        H = HORIZON_DAYS[h]; Md = window_dates(H)
        for t in 1:length(cal)                       # strictly increasing date order
            haskey(preds_by, (h, t)) || continue
            # Window on the global index: s in [t-H-Md+1, t-H]
            lo_s, hi_s = t - H - Md + 1, t - H
            lo_s >= 1 || continue
            scores = Float64[]; dates_ok = 0
            for s in lo_s:hi_s
                day = get(preds_by, (h, s), nothing)
                isnothing(day) && continue
                med_w = median(day.q_95 .- day.q_05)
                cnt = 0
                for r in eachrow(day)
                    y = get(y_of, (h, r.ticker, s), nothing)
                    isnothing(y) && continue         # unmatured or NaN target
                    push!(scores, norm_score(r.q_05, r.q_95, y, effective_width(r.q_95 - r.q_05, med_w)))
                    cnt += 1
                end
                cnt >= MIN_TICKERS_PER_DATE && (dates_ok += 1)
            end
            if length(scores) < N_MIN_SCORES || dates_ok < ceil(Int, MIN_DATE_FRACTION * Md)
                continue                              # TODO: data_quality WARN row
            end
            q_norm = pooled_quantile(scores, ALPHA)
            today = preds_by[(h, t)]
            med_w_t = median(today.q_95 .- today.q_05)
            for r in eachrow(today)
                # Robbins-Monro: one matured update from the interval issued at s = t-H.
                c, n = get(rm_state, (r.ticker, h), (0.0, 0))
                prev = get(issued, (h, r.ticker, t - H), nothing)
                y_prev = get(y_of, (h, r.ticker, t - H), nothing)
                if !isnothing(prev) && !isnothing(y_prev)
                    c, n = rm_update(c, n, !(prev[1] <= y_prev <= prev[2]))
                end
                w = r.q_95 - r.q_05
                qhat = q_norm * effective_width(w, med_w_t)
                applied = max(qhat + c, -w / 2)       # cannot invert the interval
                lower, upper = r.q_05 - applied, r.q_95 + applied
                rm_state[(r.ticker, h)] = (c, n)
                issued[(h, r.ticker, t)] = (lower, upper)
                push!(out, Dict(
                    "ticker" => r.ticker, "forecast_date" => cal[t], "horizon" => h,
                    "alpha" => ALPHA, "m_cal" => Md, "k_order_stat" => min(ceil(Int, (length(scores) + 1) * (1 - ALPHA)), length(scores)),
                    "q_hat_lower" => lower, "q_hat_upper" => upper,
                    "rm_offset" => c, "calib_width" => upper - lower,
                    # appended columns (SCHEMAS.md #8)
                    "rm_n" => n, "calib_method" => "pooled_norm", "n_cal_scores" => length(scores),
                ))
            end
        end
    end

    isempty(out) && (@warn "No calibration results produced"; return)
    df = DataFrame(out)
    # TODO: write month partitions to a temp path and rename atomically; the
    # qhat_zero_frac / qhat_applied_nonpos_frac diagnostics (SKILL.md §4a) are
    # not computed here.
    @info "Built $(nrow(df)) calibration rows (not written: skeleton)"
end

main()
