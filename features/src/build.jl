# J9 entry point: read the lake, write `universe` (one `replace` per year) and `price_coverage`.

"""
    build_universe(; config = ENV["QUANT_CONFIG"]) -> NamedTuple

Rebuild `universe` for every year with prices and `price_coverage` in full, through the lake
writer, without the prices the latest G2 run rejected (`data_quality`). Prints and returns the
row counts and the eligible count per year.

shortcut: a full rebuild every run (about 25M rows); rebuild only the years touched since the
last run once the daily job needs it faster.

# Returns
`(universe = rows, coverage = rows, eligible = Dict(year => rows))`.

# Example
```julia
build_universe()
```
"""
function build_universe(; config::AbstractString = ENV["QUANT_CONFIG"])
    cal = calendar_from_env()
    ranges = load_sic_ranges(joinpath(config, "sic_to_gics.toml"))
    prices = read_dataset("prices"; columns = ["figi", "ticker", "date", "close", "volume"])
    master = read_dataset("security_master"; columns = ["figi", "ticker", "cik", "sic"])
    holdings = read_dataset("etf_holdings"; columns = ["etf", "figi", "ticker", "snapshot_date",
        "available_date", "weight", "sector"])
    fund = read_dataset("fundamentals"; columns = ["figi", "filing_date", "shares_outstanding",
        "shares_asof_date"])
    splits = read_dataset("splits"; columns = ["figi", "ex_date", "ratio"])
    delistings = read_dataset("delistings"; columns = ["figi", "delist_class",
        "est_last_trading_date", "bankruptcy_filing_date"])
    rejected = g2_rejected(read_dataset("data_quality";
        columns = ["source", "check_id", "run_id", "entity", "chunk"]))
    isempty(prices.date) && error("prices is empty")
    first_day, data_end = extrema(prices.date)
    tdays = [d for d in first_day:Day(1):data_end if is_trading_day(cal, d)]
    u = universe_rows(prices, master, fund, splits, tdays; rejected = rejected)
    years = year.(u.date)
    eligible = Dict{Int,Int}()
    for y in sort!(unique(years))
        cols = universe_columns(u, findall(==(y), years), prices, master, holdings, ranges;
                                rejected = rejected)
        write_partition("universe", [string(y)], cols; mode = :replace)
        eligible[y] = count(cols.eligible)
        println("universe $y: $(length(cols.date)) rows, $(eligible[y]) eligible")
    end
    members = Set(String(f) for f in holdings.figi if f !== missing)
    price_dates = Dict{String,Vector{Date}}()
    for r in eachindex(u.src)
        f = String(prices.figi[u.src[r]])
        f in members && !in_spans(get(rejected, f, NO_SPANS), u.date[r]) &&
            push!(get!(price_dates, f, Date[]), u.date[r])
    end
    cov = coverage_table(holdings, master, price_dates, delistings, tdays, data_end,
                         now(UTC); rejected = rejected)
    write_dataset("price_coverage", cov; mode = :replace)
    println("price_coverage: $(length(cov.ticker)) members, data end $data_end")
    return (universe = length(u.src), coverage = length(cov.ticker), eligible = eligible)
end
