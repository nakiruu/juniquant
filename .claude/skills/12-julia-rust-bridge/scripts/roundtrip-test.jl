#!/usr/bin/env julia
# Round-trip test, Julia side: writes features and Julia's rearranged predictions
# for 100 rows (plus crossed rows) for the Rust loader to reproduce
# (03-gspqr-model/EXPORT.md §7).
#
# SKELETON. Never run (no Julia in the authoring environment). The Rust side
# (the `bosk` / `lightgbm-rust` loader) is unverified and not written, and there is no
# `compare-roundtrip.jl` yet. LightGBM.jl load/predict calls are unverified
# (verify). The Rust loader exists for this test and for optional on-demand
# prediction only; Julia is the sole writer of gspqr_predictions.

using Pkg
Pkg.activate(temp=true)
Pkg.add(["Parquet2", "DataFrames", "Arrow", "LightGBM"])

using Parquet2
using DataFrames
using Arrow
using LightGBM

# Paths per 13-parquet-lake/IO-CONTRACT.md §1: no production default; an unset variable is an error.
envpath(name) = get(() -> error("$name is not set (13-parquet-lake/IO-CONTRACT.md §1)"), ENV, name)
const QUANT_ROOT = envpath("QUANT_ROOT")
const LAKE = joinpath(QUANT_ROOT, "lake")
const PANEL_PATH  = get(ENV, "ROUNDTRIP_PANEL",      joinpath(LAKE, "panel"))      # dir of year=/month=/data.parquet
const MODELS_DIR  = get(ENV, "ROUNDTRIP_MODELS_DIR", joinpath(QUANT_ROOT, "models", "generations"))
const GENERATION  = get(ENV, "ROUNDTRIP_GENERATION", "2026-09-30")   # cutoff-date id (EXPORT.md §2)
const HORIZON     = get(ENV, "ROUNDTRIP_HORIZON",    "63d")
const QUANTILES   = [1, 5, 25, 50, 75, 95, 99]              # file/registry form: two digits, q01 ... q99

const FEATURES = [
    :mom_20d, :mom_60d, :mom_12m_1m,
    :vol_60d_res, :sp_ratio, :turn_20d,
    :r_mom_20d, :r_mom_60d, :r_mom_12m_1m,
    :r_vol_60d, :r_sp, :r_turn,
]

model_path(q) = joinpath(MODELS_DIR, GENERATION,
                         "gspqr_$(GENERATION)_$(HORIZON)_q$(lpad(q, 2, '0')).txt")

# Monotone rearrangement: sort ascending, flag if the raw vector was not non-decreasing.
# Non-finite input is an error; sorting must not hide a NaN.
function rearrange(raw::Vector{Float64})
    all(isfinite, raw) || error("non-finite prediction")
    return sort(raw), !issorted(raw)
end

function main()
    panel = DataFrame(Parquet2.Dataset(PANEL_PATH))   # partitioned read (verify)
    # 100 rows from the validation set of the generation (not just the first 100 panel
    # rows) plus at least 10 rows whose raw output is crossed: TODO select them.
    sample = first(panel, 100)
    X = Matrix{Float64}(coalesce.(sample[!, FEATURES], NaN))   # NaN features stay NaN

    raw = hcat((LightGBM.predict(LightGBM.load(model_path(q)), X)[:] for q in QUANTILES)...)   # verify
    out = Matrix{Float64}(undef, size(raw)...)
    crossed = Vector{Bool}(undef, size(raw, 1))
    for i in axes(raw, 1)
        out[i, :], crossed[i] = rearrange(vec(raw[i, :]))
    end

    Arrow.write("roundtrip_features.arrow", sample[!, FEATURES])
    Arrow.write("roundtrip_julia_preds.arrow",
                DataFrame(out, [Symbol("q_", lpad(q, 2, '0')) for q in QUANTILES]) |>
                df -> insertcols(df, :crossed => crossed))

    @info "Wrote features and rearranged Julia predictions for $(size(raw, 1)) rows"
    @info "Next: the Rust loader writes roundtrip_rust_preds.arrow; compare max |julia - rust| < 1e-6 and equal crossed flags"
end

main()
