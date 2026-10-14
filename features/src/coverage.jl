# `price_coverage` (#34): every IWF/IWP member against its prices and delisting record
# (SURVIVORSHIP.md §4, D22), and every other security with prices, for the labels' terminal
# values (TARGETS.md §3, D20: training is the broad universe).

const TOLERANCE = 0.02
const END_SLACK = 10
const BANKRUPT_DAYS = 30

"""
    coverage_table(holdings, master, price_dates, delistings, tdays, data_end, built_at;
                   rejected, spans)

The `price_coverage` columns, one row per member: per FIGI, or per holdings ticker for a
member without one; then one row per other FIGI in `spans` (first and last price date of
every security), with no member days. `holdings` as in [`universe_columns`] plus `ticker`;
`master` has `figi, ticker, cik` (and `exchange` for `listing_exchange`); `price_dates` maps
a member FIGI to its sorted price dates; `delistings` has `figi, delist_class,
est_last_trading_date, bankruptcy_filing_date` (and `form25_filing_date`). Member days are
the trading days from a snapshot's `available_date` to the next one of the same ETF, through
`data_end`.

A member whose days overlap a `vendor_wrong_holder` or `symbol_reused` range G2 `rejected` is
a `gap` with that reason; pass `price_dates` and `spans` without the rejected bars.

# Example
```julia
cov = coverage_table(holdings, master, Dict("BBG000B9XRY4" => dates), delistings, tdays,
                     Date(2026, 10, 9), now(UTC))
```
"""
function coverage_table(holdings, master, price_dates, delistings, tdays, data_end, built_at;
                        rejected = Rejected(), spans = Dict{String,Tuple{Date,Date}}())
    mticker = Dict(String(f) => String(t) for (f, t) in zip(master.figi, master.ticker))
    mcik = Dict(String(f) => c for (f, c) in zip(master.figi, master.cik))
    mexch = hasproperty(master, :exchange) ?
            Dict(String(f) => e for (f, e) in zip(master.figi, master.exchange)) : Dict()
    delist = Dict(String(delistings.figi[i]) => i for i in eachindex(delistings.figi)
                  if delistings.figi[i] !== missing)
    days = member_days(holdings, mticker, tdays, data_end)
    ctx = (mcik, mexch, delistings, delist, tdays, data_end, built_at, rejected)
    out = [coverage_row(key, sort!(collect(keys(d))), key[1] === missing ? Date[] :
                        get(price_dates, key[1], Date[]), ctx) for (key, d) in days]
    members = Set(k[1] for k in keys(days) if k[1] !== missing)
    for (f, (a, z)) in spans
        f in members || push!(out, coverage_row((f, get(mticker, f, f)), Date[], [a, z], ctx))
    end
    sort!(out; by = r -> (coalesce(r.figi, ""), r.ticker))
    return NamedTuple{keys(COVERAGE)}(Tuple(T[r[c] for r in out] for (c, T) in pairs(COVERAGE)))
end

"""
    member_days(holdings, mticker, tdays, data_end) -> Dict

The member days of each holding, keyed `(figi, ticker)` (the `security_master` ticker, else
the holdings one; `figi` is `missing` for a holding without one), each with its weight: the
trading days from a snapshot's `available_date` to the day before its ETF's next one,
through `data_end`. A name both ETFs hold weighs its IWF weight (QUERY-PATTERNS.md).

# Example
```julia
d = member_days(holdings, Dict("BBG000B9XRY4" => "AAPL"), tdays, Date(2026, 10, 30))
```
"""
function member_days(holdings, mticker, tdays, data_end)
    days = Dict{Tuple{Union{Missing,String},String},Dict{Date,Float64}}()
    for etf in sort!(unique(holdings.etf))  # IWF before IWP: the first weight stays
        rows = [i for i in eachindex(holdings.etf) if holdings.etf[i] == etf]
        av = sort!(unique(holdings.available_date[rows]))
        for i in rows
            a = holdings.available_date[i]
            k = searchsortedfirst(av, a)
            stop = k < length(av) ? av[k+1] - Day(1) : data_end
            span = tdays[searchsortedfirst(tdays, a):searchsortedlast(tdays, min(stop, data_end))]
            isempty(span) && continue
            f = holdings.figi[i]
            key = f === missing ? (missing, String(holdings.ticker[i])) :
                  (String(f), get(mticker, String(f), String(holdings.ticker[i])))
            w = hasproperty(holdings, :weight) ? coalesce(holdings.weight[i], 0.0) : 0.0
            d = get!(days, key, Dict{Date,Float64}())
            foreach(t -> get!(d, t, w), span)
        end
    end
    return days
end

const COVERAGE = (figi = Union{Missing,String}, ticker = String, cik = Union{Missing,String},
    first_member_date = Union{Missing,Date}, last_member_date = Union{Missing,Date},
    first_price_date = Union{Missing,Date}, last_price_date = Union{Missing,Date},
    n_member_days = Int32, n_member_days_priced = Int32, delist_class = Union{Missing,String},
    est_last_trading_date = Union{Missing,Date}, coverage_status = String,
    gap_reason = Union{Missing,String}, terminal_class = String, data_end = Date,
    built_at = DateTime, listing_exchange = Union{Missing,String},
    later_price = Union{Missing,Bool})

"""The `listing_exchange` of an EODHD `Exchange` (TARGETS.md §3 delisting returns)."""
listing_exchange(e) = e === missing || isempty(e) ? missing :
                      e in ("NYSE", "AMEX", "NYSE MKT", "NYSE ARCA") ? "nyse_amex" :
                      e == "NASDAQ" ? "nasdaq" : "other"

"""
    terminal(class, est, bk, f25, last_px, data_end, tdays) -> (terminal_class, later_price)

How a series ends (TARGETS.md §3): `none` while it trades at `data_end`; `acquired` when it
ends within [`END_SLACK`] trading days of an `acquired` row's estimate; `bankrupt` when it
reaches within [`BANKRUPT_DAYS`] days of the Item 1.03; `performance_delist` when it reaches
within [`END_SLACK`] trading days of a `delisted_performance` row's Form 25; else `unknown`.
`later_price`: a bankrupt or performance-delisted series trades more than [`END_SLACK`]
trading days past its exchange delisting (the Form 25, else the estimate), so its last
price holds the delisting loss.
"""
function terminal(class, est, bk, f25, last_px, data_end, tdays)
    last_px === missing && return "unknown", missing
    last_px >= data_end && return "none", missing
    class = coalesce(class, "")
    tidx(d) = searchsortedfirst(tdays, d)
    off = coalesce(f25, est)
    after = off === missing ? false : tidx(last_px) - tidx(off) > END_SLACK
    if class == "acquired" && est !== missing && abs(tidx(last_px) - tidx(est)) <= END_SLACK
        return "acquired", missing
    elseif class == "bankruptcy" && bk !== missing && last_px >= bk - Day(BANKRUPT_DAYS)
        return "bankrupt", after
    elseif class == "delisted_performance" && est !== missing &&
           tidx(last_px) >= tidx(est) - END_SLACK
        return "performance_delist", after
    end
    return "unknown", missing
end

function coverage_row((figi, ticker), member, px, (mcik, mexch, dl, delist, tdays, data_end,
                      built_at, rejected))
    pset = Set(px)
    j = figi === missing ? nothing : get(delist, figi, nothing)
    col(c) = j === nothing || !hasproperty(dl, c) ? missing : getproperty(dl, c)[j]
    class, est, bk = col(:delist_class), col(:est_last_trading_date), col(:bankruptcy_filing_date)
    first_px, last_px = isempty(px) ? (missing, missing) : (first(px), last(px))
    expected_end = est === missing ? data_end : min(est, data_end)
    expected = filter(<=(expected_end), member)
    missing_share = isempty(expected) ? 0.0 : count(!in(pset), expected) / length(expected)
    tidx(d) = searchsortedfirst(tdays, d)
    tclass, later = terminal(class, est, bk, col(:form25_filing_date), last_px, data_end, tdays)
    status, reason = if figi === missing
        "gap", "identity_unresolved"
    elseif isempty(px)
        "gap", "vendor_no_data"
    elseif !isempty(member) && (h = held_by_other(get(rejected, figi, NO_SPANS), member)) !== nothing
        "gap", h
    elseif !isempty(member) && first_px > first(member)
        "gap", "late_start"
    elseif tclass in ("acquired", "bankrupt", "performance_delist")
        "terminal_known", missing
    elseif tidx(expected_end) - tidx(last_px) > END_SLACK
        "gap", "vendor_truncated"
    elseif missing_share > TOLERANCE
        "gap", "internal_gaps"
    else
        "complete", missing
    end
    return (figi = figi, ticker = ticker, cik = figi === missing ? missing : get(mcik, figi, missing),
        first_member_date = isempty(member) ? missing : first(member),
        last_member_date = isempty(member) ? missing : last(member),
        first_price_date = first_px, last_price_date = last_px,
        n_member_days = Int32(length(member)), n_member_days_priced = Int32(count(in(pset), member)),
        delist_class = class, est_last_trading_date = est, coverage_status = status,
        gap_reason = reason, terminal_class = tclass, data_end = data_end, built_at = built_at,
        listing_exchange = figi === missing ? missing : listing_exchange(get(mexch, figi, missing)),
        later_price = later)
end

"""The reason of the first `vendor_wrong_holder` or `symbol_reused` range overlapping the
member days, else `nothing`."""
function held_by_other(spans, member)
    i = findfirst(s -> s[3] in ("vendor_wrong_holder", "symbol_reused") &&
                       s[1] <= last(member) && s[2] >= first(member), spans)
    return i === nothing ? nothing : spans[i][3]
end
