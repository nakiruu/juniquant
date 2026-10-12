# The universe builder on synthetic inputs: windows, split-aligned market cap, percentile ties,
# eligibility precedence, the snapshot join, point-in-time truncation and coverage statuses.
using Features
using Features: percentile_by_date, MIN_HISTORY
using Dates
using Test

const S = Union{Missing,String}

function days(n)
    out, d = Date[], Date(2020, 1, 2)
    while length(out) < n
        dayofweek(d) <= 5 && push!(out, d)
        d += Day(1)
    end
    return out
end

const T = days(800)

function inputs(n = length(T))
    a = (figi = fill("A", n), ticker = fill("AAA", n), date = T[1:n], close = fill(10.0, n),
         volume = Union{Missing,Int}[1_000_000 for _ in 1:n])
    b = (figi = fill("B", n), ticker = fill("BBB", n), date = T[1:n], close = fill(1.0, n),
         volume = Union{Missing,Int}[100 for _ in 1:n])
    prices = map(vcat, a, b)
    master = (figi = ["A", "B"], ticker = ["AAA", "BBB"], cik = S["0000000001", missing],
              sic = S["7372", missing])
    fund = (figi = ["A", "B"], filing_date = [T[50], T[50]],
            shares_outstanding = Union{Missing,Float64}[1000.0, 5.0],
            shares_asof_date = Union{Missing,Date}[T[45], missing])
    splits = (figi = ["A"], ex_date = [T[60]], ratio = [2.0])
    return prices, master, fund, splits
end

row(u, f, d, prices) = only(r for r in eachindex(u.src) if prices.figi[u.src[r]] == f && u.date[r] == d)

@testset "Features" begin
    @testset "sic_sector" begin
        r = [(100, 999, "Consumer Staples"), (800, 899, "Materials")]
        @test sic_sector(r, "0811") == "Materials"
        @test sic_sector(r, "0200") == "Consumer Staples"
        @test sic_sector(r, "9999") === missing
        @test sic_sector(r, missing) === missing
        @test_throws ErrorException sic_sector([(1, 2, "X"), (1, 2, "Y")], "1")
        @test !isempty(load_sic_ranges(joinpath(@__DIR__, "..", "..", "config", "sic_to_gics.toml")))
    end

    @testset "percentile ties" begin
        d = fill(Date(2024), 4)
        @test percentile_by_date(d, Union{Missing,Float64}[1, 2, 2, 3]) == [0, 0.5, 0.5, 1]
        @test percentile_by_date([Date(2024)], Union{Missing,Float64}[5.0]) == [1.0]
    end

    prices, master, fund, splits = inputs()
    u = universe_rows(prices, master, fund, splits, T)
    @testset "trailing metrics" begin
        a = r -> row(u, "A", T[r], prices)
        @test u.history_days[a(1)] == 1 && u.history_days[a(800)] == 800
        @test u.median_dollar_vol_60d[a(39)] === missing
        @test u.median_dollar_vol_60d[a(40)] == 1.0e7
        @test u.market_cap[a(49)] === missing
        @test u.market_cap[a(55)] == 10.0 * 1000
        @test u.market_cap[a(61)] == 10.0 * 2000  # shares carried through the split
        @test all(ismissing, u.market_cap[[r for r in eachindex(u.src) if prices.figi[u.src[r]] == "B"]])
        @test u.mcap_pct[a(61)] == 1.0  # the only known market cap that day
    end

    @testset "point in time" begin
        cut = 500
        keep = findall(<=(T[cut]), prices.date)
        trunc = map(c -> c[keep], prices)
        v = universe_rows(trunc, master, fund, splits, T)
        for r in eachindex(v.src)
            f, d = trunc.figi[v.src[r]], v.date[r]
            s = row(u, f, d, prices)
            @test isequal((v.history_days[r], v.median_dollar_vol_60d[r], v.market_cap[r], v.mcap_pct[r]),
                          (u.history_days[s], u.median_dollar_vol_60d[s], u.market_cap[s], u.mcap_pct[s]))
        end
    end

    holdings = (etf = ["IWF", "IWP", "IWP", "IWP", "IWP"], figi = S["A", "A", "C", "E", missing],
                ticker = ["AAA", "AAA", "CCC", "EEE", "DDD"],
                snapshot_date = fill(T[789], 5), available_date = fill(T[790], 5),
                weight = Union{Missing,Float64}[1.5, 0.5, 0.1, 0.1, 0.1],
                sector = S["Information Technology", "Cash and/or Derivatives", "-", "-", "-"])
    ranges = [(7370, 7379, "Information Technology")]
    @testset "columns and eligibility" begin
        c = universe_columns(u, eachindex(u.src), prices, master, holdings, ranges)
        at(f, d) = only(i for i in eachindex(c.date) if c.figi[i] == f && c.date[i] == d)
        i = at("A", T[789])
        @test !c.in_iwf[i] && !c.in_iwp[i] && c.snapshot_date[i] === missing
        @test c.sector[i] == "Information Technology" && c.sector_source[i] == "sic_fallback"
        @test c.eligible[i] && c.exclusion_reason[i] === missing
        i = at("A", T[790])
        @test c.in_iwf[i] && c.in_iwp[i] && c.weight_iwf[i] == 1.5 && c.weight_iwp[i] == 0.5
        @test c.snapshot_date[i] == T[789] && c.snapshot_age_days[i] == 1
        @test c.sector_source[i] == "gics_snapshot"
        @test c.eligible[i]  # $10M a day, 790 days, the only market cap: not a member condition
        @test c.exclusion_reason[at("A", T[MIN_HISTORY-1])] == "history"
        j = at("B", T[790])
        @test c.exclusion_reason[j] == "liquidity" && !c.in_iwf[j] && c.snapshot_date[j] == T[789]
        @test c.sector[j] === missing && c.sector_source[j] === missing
    end

    @testset "size floor" begin
        p, m, f, s = inputs()
        m = (m..., cik = S["0000000001", "0000000002"])
        p = (p..., volume = Union{Missing,Int}[fill(1_000_000, 800); fill(10_000_000, 800)])
        f = (f..., shares_outstanding = Union{Missing,Float64}[1000.0, 1.0e9])
        v = universe_rows(p, m, f, s, T)
        c = universe_columns(v, eachindex(v.src), p, m, holdings, ranges)
        i = only(i for i in eachindex(c.date) if c.figi[i] == "A" && c.date[i] == T[790])
        @test c.mcap_pct[i] == 0.0 && c.exclusion_reason[i] == "size" && !c.eligible[i]
    end

    @testset "coverage" begin
        pd = Dict("A" => T, "E" => T[1:795])
        dl = (figi = S["E"], delist_class = S["acquired"], est_last_trading_date = [T[797]],
              bankruptcy_filing_date = Union{Missing,Date}[missing])
        m = (figi = ["A", "C", "E"], ticker = ["AAA", "CCC", "EEE"], cik = S["1", missing, "3"])
        cov = coverage_table(holdings, m, pd, dl, T, T[800], DateTime(2026, 10, 12))
        by = Dict(coalesce(f, t) => i for (i, (f, t)) in enumerate(zip(cov.figi, cov.ticker)))
        @test length(cov.ticker) == 4  # A once although in both ETFs
        a = by["A"]
        @test cov.coverage_status[a] == "complete" && cov.terminal_class[a] == "none"
        @test cov.n_member_days[a] == 11 && cov.n_member_days_priced[a] == 11
        @test cov.gap_reason[by["C"]] == "vendor_no_data"
        @test cov.gap_reason[by["DDD"]] == "identity_unresolved" && cov.figi[by["DDD"]] === missing
        e = by["E"]
        @test cov.coverage_status[e] == "terminal_known" && cov.terminal_class[e] == "acquired"
        empty = coverage_table(map(c -> c[1:0], holdings), m, pd, dl, T, T[800], DateTime(2026))
        @test eltype(empty.figi) == S && isempty(empty.ticker)
    end
end
