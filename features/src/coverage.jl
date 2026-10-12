# `price_coverage` (#34): every IWF/IWP member against its prices and delisting record
# (SURVIVORSHIP.md §4, D22).

const TOLERANCE = 0.02
const END_SLACK = 10
const BANKRUPT_DAYS = 30

"""
    coverage_table(holdings, master, price_dates, delistings, tdays, data_end, built_at)

The `price_coverage` columns, one row per member: per FIGI, or per holdings ticker for a
member without one. `holdings` as in [`universe_columns`] plus `ticker`; `master` has
`figi, ticker, cik`; `price_dates` maps a FIGI to its sorted price dates; `delistings` has
`figi, delist_class, est_last_trading_date, bankruptcy_filing_date`. Member days are the
trading days from a snapshot's `available_date` to the next one of the same ETF, through
`data_end`.

A member whose days overlap a `vendor_wrong_holder` or `symbol_reused` range G2 `rejected` is
a `gap` with that reason; pass `price_dates` without the rejected bars.

# Example
```julia
cov = coverage_table(holdings, master, Dict("BBG000B9XRY4" => dates), delistings, tdays,
                     Date(2026, 10, 9), now(UTC))
```
"""
function coverage_table(holdings, master, price_dates, delistings, tdays, data_end, built_at;
                        rejected = Rejected())
    mticker = Dict(String(f) => String(t) for (f, t) in zip(master.figi, master.ticker))
    mcik = Dict(String(f) => c for (f, c) in zip(master.figi, master.cik))
    delist = Dict(String(delistings.figi[i]) => i for i in eachindex(delistings.figi)
                  if delistings.figi[i] !== missing)
    days = Dict{Tuple{Union{Missing,String},String},Set{Date}}()
    for etf in unique(holdings.etf)
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
            union!(get!(days, key, Set{Date}()), span)
        end
    end
    out = [coverage_row(key, sort!(collect(d)), price_dates, mcik, delistings, delist, tdays,
                        data_end, built_at, rejected) for (key, d) in days]
    sort!(out; by = r -> (coalesce(r.figi, ""), r.ticker))
    return NamedTuple{keys(COVERAGE)}(Tuple(T[r[c] for r in out] for (c, T) in pairs(COVERAGE)))
end

const COVERAGE = (figi = Union{Missing,String}, ticker = String, cik = Union{Missing,String},
    first_member_date = Date, last_member_date = Date, first_price_date = Union{Missing,Date},
    last_price_date = Union{Missing,Date}, n_member_days = Int32, n_member_days_priced = Int32,
    delist_class = Union{Missing,String}, est_last_trading_date = Union{Missing,Date},
    coverage_status = String, gap_reason = Union{Missing,String}, terminal_class = String,
    data_end = Date, built_at = DateTime)

function coverage_row((figi, ticker), member, price_dates, mcik, dl, delist, tdays, data_end,
                      built_at, rejected)
    px = figi === missing ? Date[] : get(price_dates, figi, Date[])
    pset = Set(px)
    j = figi === missing ? nothing : get(delist, figi, nothing)
    class = j === nothing ? missing : dl.delist_class[j]
    est = j === nothing ? missing : dl.est_last_trading_date[j]
    bk = j === nothing ? missing : dl.bankruptcy_filing_date[j]
    first_px, last_px = isempty(px) ? (missing, missing) : (first(px), last(px))
    expected_end = est === missing ? data_end : min(est, data_end)
    expected = filter(<=(expected_end), member)
    missing_share = isempty(expected) ? 0.0 : count(!in(pset), expected) / length(expected)
    tidx(d) = searchsortedfirst(tdays, d)
    ended = last_px !== missing && last_px < data_end
    terminal = ended && (
        (class == "acquired" && est !== missing && abs(tidx(last_px) - tidx(est)) <= END_SLACK) ||
        (class == "bankruptcy" && bk !== missing && last_px >= bk - Day(BANKRUPT_DAYS)))
    status, reason = if figi === missing
        "gap", "identity_unresolved"
    elseif isempty(px)
        "gap", "vendor_no_data"
    elseif (h = held_by_other(get(rejected, figi, NO_SPANS), member)) !== nothing
        "gap", h
    elseif first_px > first(member)
        "gap", "late_start"
    elseif terminal
        "terminal_known", missing
    elseif tidx(expected_end) - tidx(last_px) > END_SLACK
        "gap", "vendor_truncated"
    elseif missing_share > TOLERANCE
        "gap", "internal_gaps"
    else
        "complete", missing
    end
    tclass = terminal ? (class == "acquired" ? "acquired" : "bankrupt") :
             (last_px !== missing && !ended) ? "none" : "unknown"
    return (figi = figi, ticker = ticker, cik = figi === missing ? missing : get(mcik, figi, missing),
        first_member_date = first(member), last_member_date = last(member),
        first_price_date = first_px, last_price_date = last_px,
        n_member_days = Int32(length(member)), n_member_days_priced = Int32(count(in(pset), member)),
        delist_class = class, est_last_trading_date = est, coverage_status = status,
        gap_reason = reason, terminal_class = tclass, data_end = data_end, built_at = built_at)
end

"""The reason of the first `vendor_wrong_holder` or `symbol_reused` range overlapping the
member days, else `nothing`."""
function held_by_other(spans, member)
    i = findfirst(s -> s[3] in ("vendor_wrong_holder", "symbol_reused") &&
                       s[1] <= last(member) && s[2] >= first(member), spans)
    return i === nothing ? nothing : spans[i][3]
end
