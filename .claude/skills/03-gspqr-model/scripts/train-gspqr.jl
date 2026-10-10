#!/usr/bin/env julia
# Trains one GSPQR generation (up to 21 models) and updates the registry.
#
# SKELETON: the data selection and parameter helpers are written; the loop
# body (fit, evaluate, crossing report, abort checks, registry write) is NOT
# written. This file has never been run (no Julia in the authoring
# environment) and the LightGBM.jl calls are unverified (verify).
#
# Usage:
#   julia train-gspqr.jl --cutoff 2026-09-30              # production/backfill generation
#   julia train-gspqr.jl --mode research                  # static split==train model, research only
# A generation is trained at a cutoff m on the rows matured at m
# (03-gspqr-model/PREDICTION-BACKFILL.md). The `split` column is read only in
# --mode research.

using Pkg
Pkg.activate(temp=true)
Pkg.add([
    "Parquet2", "DataFrames", "Dates", "JSON3",
    "LightGBM", "Statistics", "Printf",
])

using Parquet2
using DataFrames
using Dates
using JSON3
using LightGBM
using Statistics
using Printf

# ---- Configuration ----
# Paths per 13-parquet-lake/IO-CONTRACT.md §1: no production default; an unset variable is an error.
envpath(name) = get(() -> error("$name is not set (13-parquet-lake/IO-CONTRACT.md §1)"), ENV, name)
const QUANT_ROOT = envpath("QUANT_ROOT")
const LAKE = joinpath(QUANT_ROOT, "lake")
const PANEL_PATH    = get(ENV, "TRAIN_GSPQR_PANEL",    joinpath(LAKE, "panel"))        # dir of year=/month=/data.parquet
const MODELS_DIR    = get(ENV, "TRAIN_GSPQR_MODELS",   joinpath(QUANT_ROOT, "models"))
const REGISTRY_PATH = get(ENV, "TRAIN_GSPQR_REGISTRY", joinpath(MODELS_DIR, "registry.json"))

const NUM_THREADS = parse(Int, get(ENV, "GSPQR_NUM_THREADS", "8"))  # fixed, recorded in the registry; never -1
const SEED = 42
const VAL_FRACTION = 0.2
const L_MIN_DATES = 504            # PREDICTION-BACKFILL.md OPEN-1 (placeholder)

const FEATURES = [
    :mom_20d, :mom_60d, :mom_12m_1m,
    :vol_60d_res, :sp_ratio, :turn_20d,
    :r_mom_20d, :r_mom_60d, :r_mom_12m_1m,
    :r_vol_60d, :r_sp, :r_turn,
]

const HORIZONS = [("63d", :y_fwd_63d, 63), ("126d", :y_fwd_126d, 126), ("252d", :y_fwd_252d, 252)]
const QUANTILES = [0.01, 0.05, 0.25, 0.50, 0.75, 0.95, 0.99]

# Abort the retrain if more than this share of a horizon's validation rows
# have crossing raw quantiles (TRAINING.md §7). Inference sorts the seven
# quantiles before storing them (monotone rearrangement); training and this
# check use the raw per-model outputs.
const CROSSING_THRESHOLD = 0.01

# ---- Hyperparameters ----
function params_for(tau::Float64)
    base = Dict{String,Any}(
        "objective"        => "quantile",
        "alpha"            => tau,
        "metric"           => "quantile",
        "boosting"         => "gbdt",
        "num_leaves"       => 31,
        "learning_rate"    => 0.05,
        "min_data_in_leaf" => 100,
        "feature_fraction" => 0.8,
        "bagging_fraction" => 0.8,
        "bagging_freq"     => 5,
        "lambda_l1"        => 0.1,
        "lambda_l2"        => 0.1,
        "max_bin"          => 255,
        "use_missing"      => true,
        "zero_as_missing"  => false,
        "verbose"          => -1,
        "seed"             => SEED,
        "num_threads"      => NUM_THREADS,
        "deterministic"    => true,
    )
    n_estimators = 500
    if tau in (0.01, 0.99)
        base["min_data_in_leaf"] = 200
        n_estimators = 800
    elseif tau in (0.25, 0.75)
        n_estimators = 400
    end
    return base, n_estimators
end

# ---- Pinball Loss ----
function pinball(y::Vector{Float64}, yhat::Vector{Float64}, tau::Float64)
    n = length(y)
    s = 0.0
    for i in 1:n
        d = y[i] - yhat[i]
        s += d >= 0 ? tau * d : (1 - tau) * (-d)
    end
    return s / n
end

# ---- File and registry-key naming: ONE convention (EXPORT.md §2) ----
# registry key "63d_q05"; file gspqr_<generation>_63d_q05.txt (two-digit percentile).
qtag(tau::Float64) = lpad(string(round(Int, tau * 100)), 2, '0')
model_key(horizon::String, tau::Float64) = string(horizon, "_q", qtag(tau))
model_file(generation::String, horizon::String, tau::Float64) =
    string("gspqr_", generation, "_", model_key(horizon, tau), ".txt")

# ---- Candidate rows for one horizon (PREDICTION-BACKFILL.md §2) ----
# `didx` maps every global trading date to its index on the PANEL'S GLOBAL
# calendar (sorted distinct dates), never a ticker's own row number.
function candidate_rows(panel::DataFrame, didx::Dict{Date,Int}, mode::String,
                        cutoff::Union{Date,Nothing}, target_col::Symbol, H::Int)
    if mode == "research"
        # The ONLY use of the split column in the whole pipeline.
        return filter(r -> r.split == "train" && !ismissing(r[target_col]) &&
                           !isnan(r[target_col]), panel)
    end
    m = didx[cutoff]
    # Matured at the cutoff: date + H <= m on the global index. The stored
    # target of later rows exists in the full panel but is not known at m.
    return filter(r -> didx[r.date] + H <= m && !ismissing(r[target_col]) &&
                       !isnan(r[target_col]), panel)
end

# Chronological 80/20 validation split with the H-day purge (TRAINING.md §2).
function split_train_val(cand::DataFrame, didx::Dict{Date,Int}, H::Int)
    dates = sort(unique(cand.date))
    val_boundary = dates[floor(Int, (1 - VAL_FRACTION) * length(dates))]
    vb = didx[val_boundary]
    train = filter(r -> didx[r.date] <= vb - H, cand)
    val   = filter(r -> didx[r.date] >  vb,     cand)
    return train, val, val_boundary
end

# ---- Main ----
function parse_args()
    cutoff = nothing
    mode = "generation"
    i = 1
    while i <= length(ARGS)
        if ARGS[i] == "--cutoff"; cutoff = Date(ARGS[i+1]); i += 2
        elseif ARGS[i] == "--mode"; mode = ARGS[i+1]; i += 2
        else; error("unknown argument $(ARGS[i])"); end
    end
    mode in ("generation", "research") || error("--mode must be generation or research")
    mode == "generation" && isnothing(cutoff) && error("--cutoff is required in generation mode")
    return cutoff, mode
end

function main()
    cutoff, mode = parse_args()
    generation = mode == "research" ? "research" : string(cutoff)   # the id is the cutoff date

    @info "Training GSPQR generation $generation (mode $mode)"
    isdir(PANEL_PATH) || error("Panel not found: $PANEL_PATH")

    panel = DataFrame(Parquet2.read(PANEL_PATH))   # verify: Parquet2 read API / partitioned panel
    # As-of panel: nothing after the cutoff is visible to this generation.
    if mode == "generation"
        panel = filter(r -> r.date <= cutoff, panel)
    end
    all_dates = sort(unique(panel.date))
    didx = Dict(d => i for (i, d) in enumerate(all_dates))
    @info "Panel: $(nrow(panel)) rows, $(length(unique(panel.ticker))) tickers, $(length(all_dates)) dates"

    registry_entry = Dict{String,Any}(
        "generation" => generation,
        "role" => mode == "research" ? "research" : "backfill_or_live",
        "trained_at" => string(now(UTC)),
        "panel_version" => string(maximum(panel.date)),   # the cutoff; the manifest holds the slice hash
        "seed" => SEED,
        "num_threads" => NUM_THREADS,
        "models" => Dict{String,Any}(),
        # Crossing report (SKILL.md §6, TRAINING.md §7): predict each horizon's
        # validation rows with the seven models (RAW), then per horizon
        # val_rows, val_crossed = count(!issorted(row)), rate, max_violation.
        "crossing" => Dict{String,Any}(
            "threshold" => CROSSING_THRESHOLD,
            "by_horizon" => Dict{String,Any}(),
            "max_rate" => nothing,
        ),
    )

    outdir = joinpath(MODELS_DIR, "generations", generation)
    mkpath(outdir)

    for (horizon_label, target_col, H) in HORIZONS
        cand = candidate_rows(panel, didx, mode, cutoff, target_col, H)
        # L_min counts feature-bearing dates (mom_12m_1m non-NaN from day 253).
        ndates = length(unique(cand.date[.!isnan.(coalesce.(cand.mom_12m_1m, NaN))]))
        if mode == "generation" && ndates < L_MIN_DATES
            @info "Horizon $horizon_label: $ndates matured dates < L_min, no models this generation"
            continue
        end
        train, val, val_boundary = split_train_val(cand, didx, H)
        @info "Horizon $horizon_label: train $(nrow(train)) val $(nrow(val)) val_boundary $val_boundary"

        for tau in QUANTILES
            params, n_estimators = params_for(tau)
            # --- LOOP BODY NOT WRITTEN ---
            # 1. X = Matrix{Float64}(select(train, FEATURES)), y = train[!, target_col]; NaN features kept.
            # 2. fit with early stopping on val (LightGBM.jl API: verify), then
            #    save to joinpath(outdir, model_file(generation, horizon_label, tau)).
            # 3. record in registry_entry["models"][model_key(horizon_label, tau)]:
            #    path, sha256, feature_names, train_metric, val_metric (pinball),
            #    val_coverage (= mean(y_val .<= yhat_val)), best_iteration.
            # 4. abort checks (TRAINING.md §7): coverage > 10pp abort (5pp WARN),
            #    > 3 failed models, > 6 h, crossing > 1%; pinball regression > 10% is
            #    an abort only for the live retrain, a WARN in the backfill.
        end
        # Crossing report for this horizon goes here (see registry_entry above).
    end

    # TODO: write registry.json (live pointer for role=live), manifest sha256s,
    # audit spool event on abort. Not written.
    @info "Skeleton ends here: nothing was trained"
end

main()
