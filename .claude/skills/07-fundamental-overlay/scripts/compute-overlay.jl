#!/usr/bin/env julia
# SKETCH, not compiled, not normative (Julia has not been run on it). The real home is
# conviction/src/ (veto.jl, f_score.jl, altman_z.jl, components.jl, standardize.jl; scaffold.md).
# Computes veto flags and conviction scores for all eligible stocks.
# Writes veto_flags.parquet and conviction_scores.parquet using the column names of
# 13-parquet-lake/SCHEMAS.md #11 and #12.
#
# KNOWN PLACEHOLDERS (not implemented here): veto triggers other than Altman Z (going concern,
# negative book + OCF, delisting, restatement, auditor resignation, covenant breach); market value of
# equity (shares * 100 is a stand-in); WACC (fixed 0.08); gross-margin trend (level, not slope);
# revenue persistence and analyst revisions (constants); the 'no equity issuance' Piotroski test.
# Because a Piotroski test is missing, the F-Score component is MISSING for that stock (never an
# 8-point score treated as 9): see CONVICTION.md section 1.1.

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
const FUNDAMENTALS_PATH = get(ENV, "COMPUTE_OVERLAY_FUNDAMENTALS", joinpath(LAKE, "fundamentals.parquet"))
const UNIVERSE_PATH = get(ENV, "COMPUTE_OVERLAY_UNIVERSE", joinpath(LAKE, "universe"))   # dir of year=/data.parquet
const VETO_OUTPUT = get(ENV, "COMPUTE_OVERLAY_VETO_OUTPUT", joinpath(LAKE, "veto_flags.parquet"))
const CONVICTION_OUTPUT = get(ENV, "COMPUTE_OVERLAY_CONVICTION_OUTPUT", joinpath(LAKE, "conviction_scores.parquet"))

const COMPONENT_WEIGHTS = Dict(
    "f_score" => 0.25,
    "roic_spread" => 0.20,
    "accruals" => 0.20,
    "gm_trend" => 0.15,
    "rev_persist" => 0.10,
    "revision" => 0.10,
)
# Output column names per SCHEMAS.md #11: f_score_z, roic_spread_z, accruals_z, gm_trend_z,
# rev_persist_z, revision_z (so the component keys above are the schema prefixes).
const MAD_SCALE = 1.4826   # consistent with a standard deviation (CONVICTION.md section 2)

# ---- Altman Z-Score ----
function altman_z(wc, ta, re, ebit, mve, tl, sales)
    if any(ismissing, [wc, ta, re, ebit, mve, tl, sales]) || ta == 0 || tl == 0
        return missing
    end
    return 1.2*(wc/ta) + 1.4*(re/ta) + 3.3*(ebit/ta) + 0.6*(mve/tl) + 1.0*(sales/ta)
end

# ---- Piotroski F-Score ----
function piotroski_f_score(row)
    score = 0
    # Profitability
    roa = row.net_income / row.total_assets
    if roa > 0; score += 1; end
    # ΔROA (simplified: compare to prior quarter if available)
    if !ismissing(row.prior_roa) && roa > row.prior_roa; score += 1; end
    # CFO > 0
    if row.operating_cash_flow > 0; score += 1; end
    # Accruals: CFO/assets > ROA
    if row.operating_cash_flow / row.total_assets > roa; score += 1; end
    # ΔLever < 0 (simplified)
    if !ismissing(row.prior_debt_ratio) && row.long_term_debt / row.total_assets < row.prior_debt_ratio; score += 1; end
    # ΔLiquid > 0 (simplified)
    if !ismissing(row.prior_current_ratio) && row.assets_current / row.liabilities_current > row.prior_current_ratio; score += 1; end
    # No equity issuance: needs share-count history. Not implemented, so the 9-point score cannot
    # be completed: the caller treats f_score as MISSING (see below), never as an 8-point score.
    # ΔMargin > 0 (simplified)
    if !ismissing(row.prior_gross_margin) && row.gross_profit / row.revenues > row.prior_gross_margin; score += 1; end
    # ΔTurn > 0 (simplified)
    if !ismissing(row.prior_asset_turnover) && row.revenues / row.total_assets > row.prior_asset_turnover; score += 1; end
    return missing   # placeholder: only 8 of 9 tests are evaluated, so no valid F-Score exists yet
end

# ---- Cross-sectional z-score ----
function cross_sectional_z(values::Vector{Union{Float64,Missing}})::Vector{Union{Float64,Missing}}
    valid = collect(skipmissing(values))
    if length(valid) < 10
        return fill(missing, length(values))
    end
    med = median(valid)
    mad = MAD_SCALE * median(abs.(valid .- med))
    if mad == 0
        return fill(missing, length(values))
    end
    z = [(ismissing(v) ? missing : clamp((v - med) / mad, -3.0, 3.0)) for v in values]
    return z
end

# ---- Main ----
function main()
    @info "Computing fundamental overlay"

    fundamentals = DataFrame(Parquet2.read(FUNDAMENTALS_PATH))
    universe = DataFrame(Parquet2.Dataset(UNIVERSE_PATH))   # partitioned read (verify)

    # Get latest filing per ticker
    fundamentals = sort(fundamentals, [:ticker, :filing_date])
    latest = combine(groupby(fundamentals, :ticker), last)

    # Merge with universe
    eligible = filter(r -> r.eligible, universe)
    latest = innerjoin(latest, eligible, on = :ticker)

    # ---- Veto computation ----
    veto_rows = []
    for row in eachrow(latest)
        z = altman_z(
            row.assets_current - row.liabilities_current,
            row.total_assets, row.retained_earnings, row.ebit,
            row.shares_outstanding * 100,  # placeholder market cap
            row.total_liabilities, row.revenues
        )

        veto = false
        if !ismissing(z) && z < 1.8; veto = true; end
        # Add other triggers as columns become available

        push!(veto_rows, Dict(
            "ticker" => row.ticker,
            "as_of_date" => today(),
            "filing_date" => row.filing_date,
            "altman_z" => z,
            "going_concern" => false,
            "neg_book_neg_ocf" => false,
            "delisting_notice" => false,
            "restatement_12m" => false,
            "auditor_resigned" => false,
            "covenant_breach" => false,
            "veto_active" => veto,
        ))
    end

    veto_df = DataFrame(veto_rows)
    Parquet2.write(VETO_OUTPUT, veto_df)
    @info "Wrote $(nrow(veto_df)) veto rows, $(sum(veto_df.veto_active)) active"

    # ---- Conviction computation ----
    # Compute raw components
    latest.f_score = Vector{Union{Int,Missing}}([piotroski_f_score(row) for row in eachrow(latest)])
    latest.roic_spread = latest.ebit ./ latest.total_assets .- 0.08  # simplified WACC
    latest.accruals = (latest.net_income .- latest.operating_cash_flow) ./ latest.total_assets
    latest.gm_trend = latest.gross_profit ./ latest.revenues  # simplified
    latest.rev_persist = fill(0.5, nrow(latest))  # placeholder
    latest.revision = fill(0.0, nrow(latest))     # placeholder

    # Cross-sectional z-scores
    for comp in keys(COMPONENT_WEIGHTS)
        col = Symbol(comp)
        if comp == "accruals"
            # Sign-flip: lower accruals is better
            latest[!, col] = -1.0 .* latest[!, col]
        end
        z_col = Symbol("$(comp)_z")
        latest[!, z_col] = cross_sectional_z(latest[!, col])
    end

    # Composite
    composite = zeros(Union{Float64,Missing}, nrow(latest))
    for i in 1:nrow(latest)
        total = 0.0
        weight_sum = 0.0
        for (comp, w) in COMPONENT_WEIGHTS
            z_col = Symbol("$(comp)_z")
            val = latest[i, z_col]
            if !ismissing(val)
                total += w * val
                weight_sum += w
            end
        end
        composite[i] = weight_sum > 0 ? total / weight_sum : missing
    end
    latest.composite_z = composite

    # Conviction
    latest.conviction = [ismissing(z) ? 1.0 : clamp(1.0 + 0.12*z, 0.50, 1.25) for z in latest.composite_z]
    latest.components_used = [sum(!ismissing(latest[i, Symbol("$(c)_z")]) for c in keys(COMPONENT_WEIGHTS)) for i in 1:nrow(latest)]

    # Output
    # SCHEMAS.md #11 order: ticker, as_of_date, filing_date, f_score_z, roic_spread_z, accruals_z,
    # gm_trend_z, rev_persist_z, revision_z, composite_z, conviction, components_used.
    latest.as_of_date = fill(today(), nrow(latest))   # decision date; filing_date = the filing used
    conviction_df = select(latest,
        :ticker, :as_of_date, :filing_date,
        :f_score_z, :roic_spread_z, :accruals_z,
        :gm_trend_z, :rev_persist_z, :revision_z,
        :composite_z, :conviction, :components_used
    )
    # NOTE: with fewer than 4 of 6 components the spec sets conviction = 1.0 and flags review
    # (CONVICTION.md section 2); the placeholder above does not implement that yet.

    Parquet2.write(CONVICTION_OUTPUT, conviction_df)
    @info "Wrote $(nrow(conviction_df)) conviction rows"
end

main()