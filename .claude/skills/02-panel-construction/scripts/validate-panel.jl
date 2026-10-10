#!/usr/bin/env julia
# Validates the `panel` dataset for schema, feature integrity, label flags and
# split rules.  Run after every panel build.  A failure is a stop-the-line
# event.  SKETCH (not normative): the contract is 02-panel-construction
# (SKILL.md, TARGETS.md, CROSS-SECTIONAL.md); the Parquet2/DataFrames calls
# are unverified (no Julia packages in the authoring environment).
#
# Run inside the committed environment (no run-time Pkg.add):
#   QUANT_ROOT=/data julia --project=features --startup-file=no \
#       validate-panel.jl [path/to/panel_dataset_dir]
# features/Project.toml must pin: Parquet2, DataFrames.
# Input: the partitioned dataset directory lake/panel (leaves data.parquet), or
# the directory given as the first argument / VALIDATE_PANEL_INPUT.
#
# "Absent" below means null or NaN (a label that is NaN and a null are the
# same thing for this check).

using Parquet2
using DataFrames
using Dates
using Statistics

function input_dir()::String
    !isempty(ARGS) && return ARGS[1]
    haskey(ENV, "VALIDATE_PANEL_INPUT") && return ENV["VALIDATE_PANEL_INPUT"]
    root = get(ENV, "QUANT_ROOT", "")
    isempty(root) && error("QUANT_ROOT is not set and no path given (13-parquet-lake/IO-CONTRACT.md section 1)")
    return joinpath(root, "lake", "panel")
end

const EXPECTED_COLUMNS = [
    :ticker, :date, :split,
    :y_fwd_63d, :y_fwd_126d, :y_fwd_252d,
    :mom_20d, :mom_60d, :mom_12m_1m, :vol_60d_res, :sp_ratio, :turn_20d,
    :r_mom_20d, :r_mom_60d, :r_mom_12m_1m, :r_vol_60d, :r_sp, :r_turn,
    :label_flag_63d, :label_flag_126d, :label_flag_252d,
]

const HORIZONS = (63, 126, 252)
const FEATURE_RANK = [                      # (raw feature, rank column)
    (:mom_20d, :r_mom_20d), (:mom_60d, :r_mom_60d), (:mom_12m_1m, :r_mom_12m_1m),
    (:vol_60d_res, :r_vol_60d), (:sp_ratio, :r_sp), (:turn_20d, :r_turn),
]
const FLAGS_FINITE = Set([missing, "bankrupt", "acquired"])      # a finite label
const FLAGS_NAN    = Set(["immature", "survivorship_gap", "data_gap"])  # an absent label
const MIN_N = 50

isabsent(x) = ismissing(x) || (x isa AbstractFloat && isnan(x))

function read_panel(dir::String)::DataFrame
    leaves = String[]
    for (root, _, files) in walkdir(dir)
        "data.parquet" in files && push!(leaves, joinpath(root, "data.parquet"))
    end
    isempty(leaves) && error("no data.parquet leaves under $dir")
    return reduce(vcat, (DataFrame(Parquet2.Dataset(f)) for f in sort(leaves)); init = DataFrame())
end

function validate(dir::String)::Int
    if !isdir(dir)
        @error "Panel dataset directory not found: $dir"
        return 1
    end
    df = read_panel(dir)
    failures = 0

    # Rule 1: exact column set and order
    actual = Symbol.(names(df))
    if actual != EXPECTED_COLUMNS
        @error "Column mismatch"
        @error "  Expected: $EXPECTED_COLUMNS"
        @error "  Actual:   $actual"
        return 1                      # later rules need the columns
    end

    # Rule 2: split column has only valid values
    valid_splits = Set(["train", "purge", "cal", "test"])
    bad_splits = filter(r -> !(r.split in valid_splits), df)
    if nrow(bad_splits) > 0
        @error "$(nrow(bad_splits)) rows have invalid split value"
        failures += 1
    end

    dates = sort(unique(df.date))
    nd = length(dates)
    date_index = Dict(d => i for (i, d) in enumerate(dates))

    # Rule 3: split is a global calendar-date cut (60/20/20) with a 252-day
    # purge band before the train/cal boundary (TARGETS.md section 4).
    d_train = dates[max(1, floor(Int, 0.6 * nd))]
    d_cal   = dates[max(1, floor(Int, 0.8 * nd))]
    i_train = date_index[d_train]
    # Last `train` date: 252 trading days before D_train.  A panel with fewer
    # than 252 dates before D_train has no train rows at all.
    purge_start = i_train > 252 ? dates[i_train - 252] : typemin(Date)
    expected_split(d) = d <= purge_start ? "train" :
                        d <= d_train     ? "purge" :
                        d <= d_cal       ? "cal"   : "test"
    mismatch = filter(r -> r.split != expected_split(r.date), df)
    if nrow(mismatch) > 0
        @error "$(nrow(mismatch)) rows deviate from the global date split"
        failures += 1
    end

    # Rule 4: labels and flags (TARGETS.md section 3), for every horizon.
    #  4a. flags are from the allowed set;
    #  4b. a label is finite  <=>  its flag is null, bankrupt or acquired;
    #  4c. `immature` occurs only on the last H panel dates (the horizon passes
    #      the data end) and, on those dates, a null-flag label cannot exist;
    #  all of this is by GLOBAL date index, not "the last H rows of a ticker"
    #  (a delisted ticker has no last-H-rows meaning, and a bankrupt one has
    #  finite labels near its end).
    for H in HORIZONS
        ycol    = Symbol("y_fwd_$(H)d")
        flagcol = Symbol("label_flag_$(H)d")
        y = df[!, ycol]
        f = df[!, flagcol]
        idx = [date_index[d] for d in df.date]

        bad_flag = count(x -> !ismissing(x) && !(x in FLAGS_NAN) && !(x in FLAGS_FINITE), f)
        if bad_flag > 0
            @error "$flagcol: $bad_flag rows have an unknown flag"
            failures += 1
        end

        finite = .!isabsent.(y)
        flag_finite_ok = [ismissing(x) || x in ("bankrupt", "acquired") for x in f]
        n_inconsistent = count(finite .!= flag_finite_ok)
        if n_inconsistent > 0
            @error "$ycol: $n_inconsistent rows where finite-ness of the label and its flag disagree"
            failures += 1
        end

        tail_start = nd - H                      # date indices > tail_start cannot have T <= data end
        bad_immature_early = count(i -> idx[i] <= tail_start && !ismissing(f[i]) && f[i] == "immature", eachindex(f))
        if bad_immature_early > 0
            @error "$flagcol: $bad_immature_early rows are flagged immature but their horizon ends inside the data"
            failures += 1
        end
        bad_tail = count(i -> idx[i] > tail_start && finite[i] && ismissing(f[i]), eachindex(f))
        if bad_tail > 0
            @error "$ycol: $bad_tail rows in the last $H panel dates carry an ordinary label (future prices cannot exist)"
            failures += 1
        end
    end

    # Rule 5: rank features are in [0, 1] when present
    for (_, rcol) in FEATURE_RANK
        vals = collect(skipmissing(df[!, rcol]))
        vals = filter(!isnan, vals)
        if !isempty(vals) && (minimum(vals) < 0.0 || maximum(vals) > 1.0)
            @error "$rcol has values outside [0, 1]"
            failures += 1
        end
    end

    # Rule 6: no duplicate (ticker, date)
    dups = combine(groupby(df, [:ticker, :date]), nrow => :n)
    bad_dups = filter(r -> r.n > 1, dups)
    if nrow(bad_dups) > 0
        @error "$(nrow(bad_dups)) duplicate (ticker, date) keys"
        failures += 1
    end

    # Rule 7: per-feature floor.  On every date, the number of present values
    # of each rank column is either 0 or at least MIN_N (CROSS-SECTIONAL.md
    # section 4: the floor is per feature, not per date), and a rank is
    # present only where the raw feature is present.
    for (fcol, rcol) in FEATURE_RANK
        present = .!isabsent.(df[!, rcol])
        raw_absent = isabsent.(df[!, fcol])
        n_orphan = count(present .& raw_absent)
        if n_orphan > 0
            @error "$rcol is present on $n_orphan rows where $fcol is absent"
            failures += 1
        end
        counts = combine(groupby(DataFrame(date = df.date, p = present), :date), :p => sum => :n)
        small = filter(r -> 0 < r.n < MIN_N, counts)
        if nrow(small) > 0
            @error "$rcol has ranks on $(nrow(small)) dates with fewer than $MIN_N names (first: $(first(small.date)))"
            failures += 1
        end
    end

    # Rule 8: no ticker appears with fewer than 3 rows (warning)
    for (key, grp) in pairs(groupby(df, :ticker))
        nrow(grp) < 3 && @warn "$(key.ticker) has only $(nrow(grp)) rows"
    end

    if failures == 0
        println("OK: $(nrow(df)) rows, $(length(unique(df.ticker))) tickers, " *
                "date range $(minimum(df.date)) to $(maximum(df.date))")
        return 0
    else
        @error "Validation failed with $failures rule violations"
        return 1
    end
end

exit(validate(input_dir()))
