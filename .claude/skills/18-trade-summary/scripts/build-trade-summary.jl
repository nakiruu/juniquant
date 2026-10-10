#!/usr/bin/env julia
# SKETCH, not compiled, not normative (Julia has not been run on it). Real home: ledger/src/ (scaffold.md).
# Builds trade summary rows for round-trips closed today.
# Runs after update-lots, before write-ledger.

using Pkg
Pkg.activate(temp=true)
Pkg.add(["Parquet2", "DataFrames", "Dates", "Statistics"])

using Parquet2
using DataFrames
using Dates
using Statistics
using UUIDs            # uuid4() comes from the UUIDs standard library (the earlier sketch used it undeclared)

# ---- Configuration ----
# Paths per 13-parquet-lake/IO-CONTRACT.md §1: no production default; an unset variable is an error.
envpath(name) = get(() -> error("$name is not set (13-parquet-lake/IO-CONTRACT.md §1)"), ENV, name)
const LAKE = joinpath(envpath("QUANT_ROOT"), "lake")
const LOTS_PATH = get(ENV, "TRADE_SUMMARY_LOTS", joinpath(LAKE, "tax_lots.parquet"))
const ORDERS_PATH = get(ENV, "TRADE_SUMMARY_ORDERS", joinpath(LAKE, "orders.parquet"))
# `prices` is a directory of RAW as-traded OHLCV (year=/month=); returns use the total-return index `tri`
# (02-panel-construction/FEATURES.md §0: prices + splits + dividends), never raw closes.
const PRICES_PATH = get(ENV, "TRADE_SUMMARY_PRICES", joinpath(LAKE, "prices"))
const SPLITS_PATH = get(ENV, "TRADE_SUMMARY_SPLITS", joinpath(LAKE, "splits.parquet"))
const DIVIDENDS_PATH = get(ENV, "TRADE_SUMMARY_DIVIDENDS", joinpath(LAKE, "dividends.parquet"))
const REGIME_PATH = get(ENV, "TRADE_SUMMARY_REGIME", joinpath(LAKE, "regime_states.parquet"))
const CONVICTION_PATH = get(ENV, "TRADE_SUMMARY_CONVICTION", joinpath(LAKE, "conviction_scores.parquet"))
# trade_summary is P(year): a directory of year=YYYY/data.parquet leaves (13 SKILL §3 #24).
const TRADE_SUMMARY_OUTPUT = get(ENV, "TRADE_SUMMARY_OUTPUT", joinpath(LAKE, "trade_summary"))

# ---- Universe index ----
# `prices` here must carry a `tri` column (02 FEATURES §0); the raw `close` would book splits as returns.
function build_universe_index(prices::DataFrame)::DataFrame
    prices = sort(prices, [:ticker, :date])
    # Per-ticker SIMPLE daily return (the index compounds 1 + r, so r must be simple, not log), computed
    # inside each ticker so no return straddles two tickers. The first row of each ticker is missing.
    transform!(groupby(prices, :ticker),
               :tri => (c -> [missing; c[2:end] ./ c[1:end-1] .- 1]) => :ret)
    daily = combine(groupby(dropmissing(prices, :ret), :date), :ret => mean => :univ_ret)
    daily = sort(daily, :date)
    daily.cum_index = cumprod(1 .+ daily.univ_ret)
    return daily
end

# ---- Benchmark return over holding period ----
function benchmark_return_bps(
    universe::DataFrame,
    entry_date::Date,
    exit_date::Date,
)::Float64
    entry_row = filter(r -> r.date == entry_date, universe)
    exit_row = filter(r -> r.date == exit_date, universe)
    if nrow(entry_row) == 0 || nrow(exit_row) == 0
        return 0.0
    end
    return (exit_row.cum_index[1] / entry_row.cum_index[1] - 1.0) * 10000.0
end

# ---- Slippage aggregation ----
function aggregate_slippage(
    orders::DataFrame,
    order_ids::Vector{String},
)::Float64
    fills = filter(r -> r.order_id in order_ids, orders)
    if nrow(fills) == 0
        return 0.0
    end
    weights = fills.fill_quantity
    slips = coalesce.(fills.slippage_bps, 0.0)
    if sum(weights) == 0
        return mean(slips)
    end
    return sum(weights .* slips) / sum(weights)
end

# ---- Build one trade row ----
function build_trade_row(
    lots::DataFrame,
    orders::DataFrame,
    universe::DataFrame,
    regime::DataFrame,
    conviction::DataFrame,
    closed_lots::DataFrame,
)::Dict{String,Any}
    ticker = closed_lots.ticker[1]
    entry_date = minimum(closed_lots.open_date)
    exit_date = maximum(skipmissing(closed_lots.close_date))

    total_qty = sum(closed_lots.close_quantity)
    entry_price = sum(closed_lots.open_price .* closed_lots.close_quantity) / total_qty
    exit_price = sum(closed_lots.close_price .* closed_lots.close_quantity) / total_qty

    gross_bps = (exit_price - entry_price) / entry_price * 10000.0

    # Collect order ids involved in this round-trip
    order_ids = vcat(closed_lots.order_id_open, skipmissing(closed_lots.order_id_close))
    slip_bps = aggregate_slippage(orders, unique(order_ids))

    # Tier of the earliest entry order (orders.tier, amendment 1.2). The
    # ledger does not store the tier.
    entry_orders = sort(filter(r -> r.order_id in closed_lots.order_id_open, orders), :submitted_at)
    tier_at_entry = nrow(entry_orders) == 0 ? "unknown" : entry_orders.tier[1]

    net_bps = gross_bps - slip_bps
    bench_bps = benchmark_return_bps(universe, entry_date, exit_date)
    alpha_bps = net_bps - bench_bps

    holding_days = length(
        filter(d -> entry_date <= d <= exit_date,
               unique(universe.date))
    )

    return Dict(
        "trade_id" => string(uuid4()),
        "ticker" => ticker,
        "entry_date" => entry_date,
        "exit_date" => exit_date,
        "entry_price" => entry_price,
        "exit_price" => exit_price,
        "quantity" => total_qty,
        "gross_return_bps" => gross_bps,
        "slippage_bps" => slip_bps,
        "net_return_bps" => net_bps,
        "benchmark_return_bps" => bench_bps,
        "alpha_bps" => alpha_bps,
        "holding_days" => holding_days,
        "return_per_day_bps" => holding_days > 0 ? net_bps / holding_days : 0.0,
        "downside_deviation_bps" => 0.0,  # computed separately
        "trade_sortino" => missing,
        "regime_at_entry" => lookup_regime(regime, entry_date),
        "regime_at_exit" => lookup_regime(regime, exit_date),
        "conviction_at_entry" => lookup_conviction(conviction, ticker, entry_date),
        "tier_at_entry" => tier_at_entry,
        "lock_overridden" => false,     # reserved; constant false (no lock override exists, amendment 1.4)
    )
end

# ---- Lookups ----
function lookup_regime(regime::DataFrame, date::Date)::String
    row = filter(r -> r.date == date, regime)
    return nrow(row) == 0 ? "unknown" : row.viterbi_state[1]
end

function lookup_conviction(conviction::DataFrame, ticker::String, date::Date)::Float64
    row = filter(r -> r.ticker == ticker && r.as_of_date <= date, conviction)
    return nrow(row) == 0 ? 1.0 : row.conviction[end]
end

# ---- Main ----
function main()
    date = length(ARGS) >= 2 && ARGS[1] == "--date" ?
           Date(ARGS[2]) : today()

    @info "Building trade summary for round-trips closed on $date"

    lots = DataFrame(Parquet2.read(LOTS_PATH))
    orders = DataFrame(Parquet2.read(ORDERS_PATH))
    prices = DataFrame(Parquet2.Dataset(PRICES_PATH))   # RAW: TODO add `tri` via the 02 helper (features/src/raw.jl)
    regime = DataFrame(Parquet2.read(REGIME_PATH))
    conviction = DataFrame(Parquet2.read(CONVICTION_PATH))

    # Find lots closed today
    closed_today = filter(r -> !ismissing(r.close_date) && r.close_date == date, lots)

    if nrow(closed_today) == 0
        @info "No closed round-trips today"
        return
    end

    universe = build_universe_index(prices)

    # Group by ticker for trades closed today
    rows = []
    for (ticker, grp) in pairs(groupby(closed_today, :ticker))
        row = build_trade_row(lots, orders, universe, regime, conviction, grp)
        push!(rows, row)
    end

    out = DataFrame(rows)

    # Append to the year leaf (in production: write_partition, IO-CONTRACT §3)
    leaf = joinpath(TRADE_SUMMARY_OUTPUT, "year=$(year(date))", "data.parquet")
    if isfile(leaf)
        existing = DataFrame(Parquet2.read(leaf))
        combined = vcat(existing, out)
        Parquet2.write(leaf, combined)
    else
        mkpath(dirname(leaf))
        Parquet2.write(leaf, out)
    end

    @info "Wrote $(nrow(out)) trade rows for $date"
    @info "Mean alpha_bps: $(mean(out.alpha_bps))"
end

main()