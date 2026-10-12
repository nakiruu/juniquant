# `universe` (#1): one row per (date, figi) with a price, every metric from data through
# `date` only (UNIVERSE.md §5, D20, D21).

const MIN_DOLLAR_VOL = 5_000_000.0
const MIN_HISTORY = 756
const MIN_MCAP_PCT = 0.30
const WINDOW = 60
const MIN_WINDOW = 40
const GICS = Set(["Communication Services", "Consumer Discretionary", "Consumer Staples",
    "Energy", "Financials", "Health Care", "Industrials", "Information Technology",
    "Materials", "Real Estate", "Utilities"])

"""
    load_sic_ranges(path) -> Vector{Tuple{Int,Int,String}}

The `[[range]]` entries of `config/sic_to_gics.toml` as `(from, to, sector)`.

# Example
```julia
ranges = load_sic_ranges("config/sic_to_gics.toml")
```
"""
load_sic_ranges(path::AbstractString) =
    [(Int(r["from"]), Int(r["to"]), String(r["sector"])) for r in TOML.parsefile(path)["range"]]

"""
    sic_sector(ranges, sic) -> Union{String,Missing}

The GICS sector of the narrowest range containing the 4-digit `sic` (`UNIVERSE.md` §6);
`missing` for no code or no range. Two narrowest ranges of equal width are a config error.

# Example
```julia
sic_sector([(100, 999, "Consumer Staples"), (800, 899, "Materials")], "0811") == "Materials"
```
"""
function sic_sector(ranges, sic)
    code = sic === missing ? nothing : tryparse(Int, sic)
    code === nothing && return missing
    hits = filter(r -> r[1] <= code <= r[2], ranges)
    isempty(hits) && return missing
    w = minimum(r[2] - r[1] for r in hits)
    best = filter(r -> r[2] - r[1] == w, hits)
    length(best) == 1 || error("sic_to_gics.toml: SIC $code is in $(length(best)) ranges of width $w")
    return best[1][3]
end

valid_bar(c, v) = c !== missing && v !== missing && isfinite(c) && c > 0 && v >= 0

"""
    universe_rows(prices, master, fund, splits, tdays) -> NamedTuple

The per-row metrics of `universe`, for the `prices` rows whose FIGI is in `master`, in
(figi, date) order: `src` (row index into `prices`), `history_days`,
`median_dollar_vol_60d`, `market_cap`, `mcap_pct`. Inputs are Tables.jl column tables:
`prices` (`figi, date, close, volume`), `master` (`figi, cik`), `fund` (`figi, filing_date,
shares_outstanding, shares_asof_date`), `splits` (`figi, ex_date, ratio`); `tdays` is the
sorted trading calendar covering the price dates. Point in time: row `t` reads bars, filings
and splits dated `<= t` only.

A valid bar has a positive close and a volume and lies outside the ranges G2 `rejected`
([`g2_rejected`]). shortcut: the de-SPAC start (§5a) waits for SIC 6770.

# Example
```julia
u = universe_rows(prices, master, fund, splits, tdays; rejected = g2_rejected(dq))
```
"""
function universe_rows(prices, master, fund, splits, tdays::AbstractVector{Date};
                       rejected = Rejected())
    has_cik = Dict(String(f) => c !== missing for (f, c) in zip(master.figi, master.cik))
    id = Dict{String,Int}()
    pf, pd = prices.figi, prices.date
    keys_ = Int64[]
    src = Int[]
    for i in eachindex(pf)
        f = pf[i]
        haskey(has_cik, f) || continue
        k = get!(id, f, length(id) + 1)
        push!(keys_, (Int64(k) << 22) | Dates.value(pd[i]))  # one sort over (figi, date)
        push!(src, i)
    end
    src = src[sortperm(keys_)]
    fund_by = group_rows(fund.figi)
    split_by = group_rows(splits.figi)
    n = length(src)
    hist = Vector{Int32}(undef, n)
    mdv = Vector{Union{Missing,Float64}}(missing, n)
    mcap = Vector{Union{Missing,Float64}}(missing, n)
    buf = Float64[]
    close_, vol = prices.close, prices.volume
    lo = 1
    while lo <= n
        f = pf[src[lo]]
        hi = lo
        while hi < n && pf[src[hi+1]] == f
            hi += 1
        end
        fr = sort(get(fund_by, f, Int[]); by = j -> fund.filing_date[j])
        sp = sort(get(split_by, f, Int[]); by = j -> splits.ex_date[j])
        spans = get(rejected, f, NO_SPANS)
        good = [valid_bar(close_[src[r]], vol[src[r]]) && !in_spans(spans, pd[src[r]]) for r in lo:hi]
        h, j0, k, shares, cover = Int32(0), lo, 1, missing, missing
        for r in lo:hi
            i = src[r]
            d = pd[i]
            c = close_[i]
            ok = good[r-lo+1]
            h += ok
            hist[r] = h
            ti = searchsortedfirst(tdays, d)
            while tdays_index(tdays, pd[src[j0]]) <= ti - WINDOW
                j0 += 1
            end
            empty!(buf)
            for q in j0:r
                iq = src[q]
                good[q-lo+1] && push!(buf, close_[iq] * vol[iq])
            end
            length(buf) >= MIN_WINDOW && (mdv[r] = median!(buf))
            while k <= length(fr) && fund.filing_date[fr[k]] <= d
                s = fund.shares_outstanding[fr[k]]
                if s !== missing
                    shares = s
                    a = fund.shares_asof_date[fr[k]]
                    cover = a === missing ? fund.filing_date[fr[k]] : a
                end
                k += 1
            end
            if ok && shares !== missing && has_cik[f]
                rho = prod((splits.ratio[j] for j in sp if cover < splits.ex_date[j] <= d); init = 1.0)
                mcap[r] = c * shares * rho
            end
        end
        lo = hi + 1
    end
    dates = [pd[i] for i in src]
    return (src = src, date = dates, history_days = hist, median_dollar_vol_60d = mdv,
        market_cap = mcap, mcap_pct = percentile_by_date(dates, mcap))
end

tdays_index(tdays, d) = searchsortedfirst(tdays, d)

"""Per FIGI, the `(from, to, reason)` date ranges gate G2 rejected."""
const Rejected = Dict{String,Vector{Tuple{Date,Date,String}}}
const NO_SPANS = Tuple{Date,Date,String}[]

in_spans(spans, d) = any(s -> s[1] <= d <= s[2], spans)

"""
    g2_rejected(dq) -> Rejected

The prices gate G2 rejected in its latest run, from `data_quality` columns `source,
check_id, run_id, entity, chunk`: per FIGI, each `spans {from} {to} {reason}` offender and
each failed row (`rows {date} {why}`, reason `rows`). Empty before the first G2 run.

# Example
```julia
rejected = g2_rejected(read_dataset("data_quality"; columns = ["source", "check_id",
    "run_id", "entity", "chunk"]))
```
"""
function g2_rejected(dq)
    g2(i) = coalesce(dq.source[i], "") == "Gate" && coalesce(dq.check_id[i], "") == "G2"
    star(i) = coalesce(dq.entity[i], "") == "*"
    runs = [String(dq.run_id[i]) for i in eachindex(dq.run_id) if g2(i) && star(i)]
    out = Rejected()
    isempty(runs) && return out
    run = maximum(runs)
    for i in eachindex(dq.run_id)
        (g2(i) && !star(i) && dq.run_id[i] == run) || continue
        p = split(dq.chunk[i], ' ')
        span = p[1] == "spans" ? (Date(p[2]), Date(p[3]), String(p[4])) :
               p[1] == "rows" ? (Date(p[2]), Date(p[2]), "rows") : nothing
        span === nothing || push!(get!(out, String(dq.entity[i]), Tuple{Date,Date,String}[]), span)
    end
    return out
end

function group_rows(figis)
    g = Dict{String,Vector{Int}}()
    for (i, f) in enumerate(figis)
        f === missing || push!(get!(g, f, Int[]), i)
    end
    return g
end

function median!(x::Vector{Float64})
    sort!(x)
    m = length(x)
    return isodd(m) ? x[(m+1)÷2] : (x[m÷2] + x[m÷2+1]) / 2
end

"""Percentile rank in [0, 1] of `x` among the non-missing values of the same date; ties take
the average rank; a single value is 1."""
function percentile_by_date(dates, x)
    out = Vector{Union{Missing,Float64}}(missing, length(x))
    idx = sort!(findall(!ismissing, x); by = i -> (dates[i], x[i]))
    a = 1
    while a <= length(idx)
        b = a
        while b < length(idx) && dates[idx[b+1]] == dates[idx[a]]
            b += 1
        end
        m = b - a + 1
        t = a
        while t <= b
            u = t
            while u < b && x[idx[u+1]] == x[idx[t]]
                u += 1
            end
            r = (t + u) / 2 - a + 1  # average 1-based rank within the date
            for q in t:u
                out[idx[q]] = m == 1 ? 1.0 : (r - 1) / (m - 1)
            end
            t = u + 1
        end
        a = b + 1
    end
    return out
end

"""
    universe_columns(u, rows, prices, master, holdings, ranges; rejected) -> NamedTuple

The `universe` columns (registry order) for `rows` of `u` (from [`universe_rows`]).
`master` has `figi, cik, sic`; `holdings` has `etf, figi, snapshot_date, available_date,
weight, sector`. Membership on `t` is the latest snapshot per ETF with `available_date <= t`.
A row inside a range G2 `rejected` is excluded first, with reason `data_quality`.

# Example
```julia
cols = universe_columns(u, eachindex(u.src), prices, master, holdings, ranges)
```
"""
function universe_columns(u, rows, prices, master, holdings, ranges; rejected = Rejected())
    cik = Dict(String(f) => c for (f, c) in zip(master.figi, master.cik))
    sic = Dict(String(f) => sic_sector(ranges, s) for (f, s) in zip(master.figi, master.sic))
    snaps = Dict(etf => snapshots(holdings, etf) for etf in ("IWF", "IWP"))
    n = length(rows)
    figi = Vector{String}(undef, n)
    ticker = Vector{String}(undef, n)
    c_cik = Vector{Union{Missing,String}}(missing, n)
    sector = Vector{Union{Missing,String}}(missing, n)
    source = Vector{Union{Missing,String}}(missing, n)
    in_iwf, in_iwp = falses(n), falses(n)
    sdate = Vector{Union{Missing,Date}}(missing, n)
    age = Vector{Union{Missing,Int32}}(missing, n)
    w_iwf = Vector{Union{Missing,Float64}}(missing, n)
    w_iwp = Vector{Union{Missing,Float64}}(missing, n)
    eligible = falses(n)
    reason = Vector{Union{Missing,String}}(missing, n)
    for (o, r) in enumerate(rows)
        i = u.src[r]
        f = String(prices.figi[i])
        d = u.date[r]
        figi[o], ticker[o], c_cik[o] = f, String(prices.ticker[i]), cik[f]
        gics = missing
        for (etf, flag, w) in (("IWF", in_iwf, w_iwf), ("IWP", in_iwp, w_iwp))
            s = snaps[etf]
            k = searchsortedlast(s.available, d)
            k == 0 && continue
            sdate[o] = sdate[o] === missing ? s.date[k] : max(sdate[o], s.date[k])
            m = get(s.members[k], f, nothing)
            m === nothing && continue
            flag[o], w[o] = true, m[1]
            gics === missing && m[2] in GICS && (gics = m[2])
        end
        sdate[o] === missing || (age[o] = Int32(Dates.value(d - sdate[o])))
        if gics !== missing
            sector[o], source[o] = gics, "gics_snapshot"
        elseif sic[f] !== missing
            sector[o], source[o] = sic[f], "sic_fallback"
        end
        dv, p = u.median_dollar_vol_60d[r], u.mcap_pct[r]
        reason[o] = in_spans(get(rejected, f, NO_SPANS), d) ? "data_quality" :
                    u.history_days[r] < MIN_HISTORY ? "history" :
                    (dv === missing || dv < MIN_DOLLAR_VOL) ? "liquidity" :
                    (p !== missing && p < MIN_MCAP_PCT) ? "size" : missing
        eligible[o] = reason[o] === missing
    end
    return (date = u.date[rows], figi = figi, ticker = ticker, cik = c_cik, sector = sector,
        sector_source = source, in_iwf = in_iwf, in_iwp = in_iwp, snapshot_date = sdate,
        snapshot_age_days = age, weight_iwf = w_iwf, weight_iwp = w_iwp,
        median_dollar_vol_60d = u.median_dollar_vol_60d[rows],
        history_days = u.history_days[rows], market_cap = u.market_cap[rows],
        mcap_pct = u.mcap_pct[rows], eligible = eligible, exclusion_reason = reason)
end

"""One ETF's snapshots in `available_date` order, each with its members `figi => (weight,
sector)`; a holding without a FIGI is not a `universe` member (it has no price row)."""
function snapshots(h, etf)
    by = Dict{Date,Tuple{Date,Dict{String,Tuple{Union{Missing,Float64},String}}}}()
    for i in eachindex(h.etf)
        (h.etf[i] == etf && h.figi[i] !== missing) || continue
        a = h.available_date[i]
        e = get!(by, a) do
            (h.snapshot_date[i], Dict{String,Tuple{Union{Missing,Float64},String}}())
        end
        e[2][String(h.figi[i])] = (h.weight[i], h.sector[i] === missing ? "" : String(h.sector[i]))
    end
    av = sort!(collect(keys(by)))
    return (available = av, date = [by[a][1] for a in av], members = [by[a][2] for a in av])
end
