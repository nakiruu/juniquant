#!/usr/bin/env julia
# SUPERSEDED: the rules run in `quant-ingest gates g7` (ingest/src/gates.rs).
# Validates the `fundamentals` dataset for point-in-time integrity (gate G7,
# QUALITY-GATES.md section 4).  SKETCH (not normative): the contract is the
# rule table in QUALITY-GATES.md; the Parquet2/DataFrames calls are unverified
# (no Julia packages in the authoring environment).
#
# Run inside the committed environment (no run-time Pkg.add):
#   QUANT_ROOT=/data julia --project=schemas --startup-file=no \
#       validate-filings.jl [path/to/fundamentals.parquet]
# schemas/Project.toml must pin: Parquet2, DataFrames.
#
# Exit status: 1 if any FAIL rule fires, else 0.  WARN rules only print.
# A rule that cannot evaluate (for example a missing column) is a FAIL.
# Rule ORDER matters: null checks run first so that no later comparison meets
# a `missing` (the earlier version compared before checking nulls).
#
# This script is allowed to name `period_end` (POINT-IN-TIME.md, "period_end
# exception"); it validates ingestion output and makes no decision from it.

using Parquet2
using DataFrames
using Dates

const AS_OF = haskey(ENV, "VALIDATE_FILINGS_AS_OF") ?
              Date(ENV["VALIDATE_FILINGS_AS_OF"]) : Dates.today()

function input_path()::String
    if !isempty(ARGS)
        return ARGS[1]
    end
    root = get(ENV, "QUANT_ROOT", "")
    isempty(root) && error("QUANT_ROOT is not set and no path argument given (IO-CONTRACT.md section 1)")
    return joinpath(root, "lake", "fundamentals.parquet")
end

const REQUIRED = [:ticker, :accession, :filing_date, :period_end, :form_type,
                  :fiscal_period, :acceptance_datetime, :amends_accession]

mutable struct Tally
    fails::Int
    warns::Int
end

fail!(t::Tally, msg) = (@error msg; t.fails += 1)
warn!(t::Tally, msg) = (@warn msg; t.warns += 1)

function show_rows(df::AbstractDataFrame, cols; n = 5)
    for r in eachrow(first(df, n))
        @error "  " * join(("$(c)=$(r[c])" for c in cols), " ")
    end
end

function validate(path::String)::Int
    if !isfile(path)
        @error "File not found: $path"
        return 1
    end
    df = DataFrame(Parquet2.Dataset(path))
    t = Tally(0, 0)

    missing_cols = setdiff(REQUIRED, Symbol.(names(df)))
    if !isempty(missing_cols)
        fail!(t, "missing columns: $missing_cols")
        return 1
    end

    # Rule 1 (FAIL): filing_date and period_end are non-null.  First, so the
    # rules below can assume non-missing values.
    nulls = filter(r -> ismissing(r.filing_date) || ismissing(r.period_end), df)
    if nrow(nulls) > 0
        fail!(t, "$(nrow(nulls)) rows have a null filing_date or period_end")
        show_rows(nulls, [:ticker, :accession, :filing_date, :period_end])
    end
    ok = filter(r -> !ismissing(r.filing_date) && !ismissing(r.period_end), df)

    # Rule 2 (FAIL): filing_date >= period_end.  Equality is allowed.
    bad2 = filter(r -> r.filing_date < r.period_end, ok)
    if nrow(bad2) > 0
        fail!(t, "$(nrow(bad2)) rows have filing_date < period_end")
        show_rows(bad2, [:ticker, :accession, :filing_date, :period_end])
    end

    # Rule 3 (FAIL): no filing_date after the validation date.
    bad3 = filter(r -> r.filing_date > AS_OF, ok)
    if nrow(bad3) > 0
        fail!(t, "$(nrow(bad3)) rows have filing_date after $AS_OF")
        show_rows(bad3, [:ticker, :accession, :filing_date])
    end

    # Rule 4 (FAIL): (ticker, accession) unique.
    dups = combine(groupby(df, [:ticker, :accession]), nrow => :n)
    bad4 = filter(r -> r.n > 1, dups)
    if nrow(bad4) > 0
        fail!(t, "$(nrow(bad4)) duplicate (ticker, accession) keys")
        show_rows(bad4, [:ticker, :accession, :n])
    end

    # Rule 5 (FAIL): amends_accession is null or an existing accession.
    known = Set(skipmissing(df.accession))
    bad5 = filter(r -> !ismissing(r.amends_accession) && !(r.amends_accession in known), df)
    if nrow(bad5) > 0
        fail!(t, "$(nrow(bad5)) rows amend an accession that is not in the dataset")
        show_rows(bad5, [:ticker, :accession, :amends_accession])
    end

    # Rule 6 (FAIL): fiscal_period is Q1/Q2/Q3/FY or null.
    valid_fp = Set(["Q1", "Q2", "Q3", "FY"])
    bad6 = filter(r -> !ismissing(r.fiscal_period) && !(r.fiscal_period in valid_fp), df)
    if nrow(bad6) > 0
        fail!(t, "$(nrow(bad6)) rows have an invalid fiscal_period")
        show_rows(bad6, [:ticker, :accession, :fiscal_period])
    end

    # Rule 7 (WARN): acceptance_datetime within one day of filing_date.
    bad7 = filter(r -> !ismissing(r.acceptance_datetime) && !ismissing(r.filing_date) &&
                       abs(Dates.value(Date(r.acceptance_datetime) - r.filing_date)) > 1, df)
    if nrow(bad7) > 0
        warn!(t, "$(nrow(bad7)) rows have acceptance_datetime more than one day from filing_date")
    end

    # Rule 8 (WARN): gaps over 120 days between consecutive filings of a ticker.
    for (key, grp) in pairs(groupby(ok, :ticker))
        g = sort(grp, :filing_date)
        nrow(g) < 2 && continue
        gaps = Dates.value.(diff(g.filing_date))
        n_long = count(>(120), gaps)
        n_long > 0 && warn!(t, "$(key.ticker) has $(n_long) gaps > 120 days")
    end

    if t.fails == 0
        println("OK: $(nrow(df)) rows validated across $(length(unique(df.ticker))) tickers ($(t.warns) warnings)")
        return 0
    else
        @error "Validation failed with $(t.fails) rule violations ($(t.warns) warnings)"
        return 1
    end
end

exit(validate(input_path()))
