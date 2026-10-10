# Oracle, not production code. Verified 2026-10-10 with Julia 1.12: `run_fixtures.jl` checks it
# against the golden fixtures and prints ALL ORACLE CHECKS PASSED (re-run after any change).
#
# ORACLE for the NYSE full-day-closure calendar (09-lock-state-machine/SKILL.md section 4).
# It re-implements, in Julia, the functions that the Rust `calendar` crate (`calendar/src/`)
# must implement (rules + overrides, computed for ANY year), and is checked against the language-
# neutral golden file `fixtures/nyse_calendar_cases.json` (23 years of holidays, 1500
# add_trading_days cases). The Rust `calendar` tests and the Julia `NYSECalendar` package tests consume the SAME
# fixture file. Where this file and the fixture disagree, the fixture wins and one of the two
# implementations is wrong.
#
# The semantics are the verified Python oracle's, ported exactly (the Python files are retired;
# the fixtures are what survives). VERIFY the holiday rules against NYSE's published lists.
#
# This module is a TEST ORACLE. Production trading days come from the Rust calendar module (rules
# + overrides), the single calendar used by everything; Julia production code never computes the
# lock calendar (it is handed the next session date by the scheduler, see 08 SKILL.md section 5a).

module NYSECalendarOracle

using Dates

export rule_holidays, holidays, is_trading_day, add_trading_days, OVERRIDES_SEED   # selftest is qualified, not exported

# Seed overrides (national days of mourning, disasters); mirrors config/nyse_calendar_overrides.toml
# exactly. Historical entries for the backfill (prices from 2005, macro from 2004, plus 2001):
# verify against NYSE notices.
const OVERRIDES_SEED = Set([
Date(2001, 9, 11),    # September 11 attacks (verify)
Date(2001, 9, 12),    # September 11 attacks (verify)
Date(2001, 9, 13),    # September 11 attacks (verify)
Date(2001, 9, 14),    # September 11 attacks (verify)
Date(2004, 6, 11),    # National Day of Mourning, President Reagan (verify)
Date(2007, 1, 2),     # National Day of Mourning, President Ford
    Date(2012, 10, 29),   # Hurricane Sandy
    Date(2012, 10, 30),   # Hurricane Sandy
    Date(2018, 12, 5),    # National Day of Mourning, President G. H. W. Bush
    Date(2025, 1, 9),     # National Day of Mourning, President Carter
])

# n > 0: the n-th such weekday of the month; n < 0: counted from the end (-1 = last).
# `wd` follows Julia: Monday = 1 ... Sunday = 7.
function nth_weekday(y::Int, m::Int, wd::Int, n::Int)::Date
    ds = [Date(y, m, d) for d in 1:daysinmonth(y, m) if dayofweek(Date(y, m, d)) == wd]
    return n > 0 ? ds[n] : ds[end + n + 1]
end

# Anonymous Gregorian algorithm (Meeus/Jones/Butcher). Good Friday = Easter - 2 days.
function easter(y::Int)::Date
    a = y % 19; b = y ÷ 100; c = y % 100
    d = b ÷ 4; e = b % 4
    f = (b + 8) ÷ 25; g = (b - f + 1) ÷ 3
    h = (19a + b - d - g + 15) % 30
    i = c ÷ 4; k = c % 4
    l = (32 + 2e + 2i - h - k) % 7
    m = (a + 11h + 22l) ÷ 451
    month = (h + l - 7m + 114) ÷ 31
    day = (h + l - 7m + 114) % 31 + 1
    return Date(y, month, day)
end

# Saturday -> preceding Friday (except New Year's Day: NYSE does not close on Dec 31);
# Sunday -> following Monday.
function observed(d::Date; new_years::Bool = false)::Union{Date,Nothing}
    dow = dayofweek(d)
    if dow == 6
        return new_years ? nothing : d - Day(1)
    elseif dow == 7
        return d + Day(1)
    end
    return d
end

function rule_holidays(y::Int)::Set{Date}
    h = Set{Date}()
    ny = observed(Date(y, 1, 1); new_years = true)
    ny === nothing || push!(h, ny)
    push!(h, nth_weekday(y, 1, 1, 3))              # Martin Luther King Jr. Day (3rd Monday of Jan)
    push!(h, nth_weekday(y, 2, 1, 3))              # Washington's Birthday (3rd Monday of Feb)
    push!(h, easter(y) - Day(2))                   # Good Friday
    push!(h, nth_weekday(y, 5, 1, -1))             # Memorial Day (last Monday of May)
    y >= 2022 && push!(h, observed(Date(y, 6, 19)))  # Juneteenth (NYSE from 2022)
    push!(h, observed(Date(y, 7, 4)))              # Independence Day
    push!(h, nth_weekday(y, 9, 1, 1))              # Labor Day (1st Monday of Sep)
    push!(h, nth_weekday(y, 11, 4, 4))             # Thanksgiving (4th Thursday of Nov)
    push!(h, observed(Date(y, 12, 25)))            # Christmas
    return h
end

function holidays(y::Int, overrides = OVERRIDES_SEED)::Set{Date}
    return union(rule_holidays(y), Set(d for d in overrides if year(d) == y))
end

is_trading_day(d::Date, overrides = OVERRIDES_SEED)::Bool =
    dayofweek(d) <= 5 && !(d in holidays(year(d), overrides))

# Roll `d` forward to a session, then advance n sessions.
function add_trading_days(d::Date, n::Int, overrides = OVERRIDES_SEED)::Date
    while !is_trading_day(d, overrides)
        d += Day(1)
    end
    while n > 0
        d += Day(1)
        is_trading_day(d, overrides) && (n -= 1)
    end
    return d
end

# Published full-day closures (from memory of NYSE's holiday pages: VERIFY against nyse.com).
const PUBLISHED = Dict(
    2022 => [Date(2022,1,17), Date(2022,2,21), Date(2022,4,15), Date(2022,5,30), Date(2022,6,20), Date(2022,7,4), Date(2022,9,5), Date(2022,11,24), Date(2022,12,26)],
    2023 => [Date(2023,1,2), Date(2023,1,16), Date(2023,2,20), Date(2023,4,7), Date(2023,5,29), Date(2023,6,19), Date(2023,7,4), Date(2023,9,4), Date(2023,11,23), Date(2023,12,25)],
    2024 => [Date(2024,1,1), Date(2024,1,15), Date(2024,2,19), Date(2024,3,29), Date(2024,5,27), Date(2024,6,19), Date(2024,7,4), Date(2024,9,2), Date(2024,11,28), Date(2024,12,25)],
    2025 => [Date(2025,1,1), Date(2025,1,9), Date(2025,1,20), Date(2025,2,17), Date(2025,4,18), Date(2025,5,26), Date(2025,6,19), Date(2025,7,4), Date(2025,9,1), Date(2025,11,27), Date(2025,12,25)],
    2026 => [Date(2026,1,1), Date(2026,1,19), Date(2026,2,16), Date(2026,4,3), Date(2026,5,25), Date(2026,6,19), Date(2026,7,3), Date(2026,9,7), Date(2026,11,26), Date(2026,12,25)],
    2027 => [Date(2027,1,1), Date(2027,1,18), Date(2027,2,15), Date(2027,3,26), Date(2027,5,31), Date(2027,6,18), Date(2027,7,5), Date(2027,9,6), Date(2027,11,25), Date(2027,12,24)],
)

# Property checks that do not need the fixture file. Returns the list of failures (empty = pass).
function selftest()::Vector{String}
    bad = String[]
    for (y, want) in PUBLISHED
        got = sort(collect(holidays(y)))
        got == sort(want) || push!(bad, "published list mismatch $y: $got vs $(sort(want))")
    end
    # Saturday 1 Jan 2022 and Saturday 1 Jan 2028 close nothing on the preceding 31 Dec.
    Date(2021, 12, 31) in holidays(2021) && push!(bad, "31 Dec 2021 must not be a closure")
    Date(2027, 12, 31) in holidays(2027) && push!(bad, "31 Dec 2027 must not be a closure")
    # Far-future years need no table.
    length(rule_holidays(2040)) == 10 || push!(bad, "2040 should have 10 rule holidays")
    # Safety property: treating extra dates as closed can only LENGTHEN a block (never shorten it).
    # Deterministic pseudo-random sweep (no RNG dependency): stride through start dates and extras.
    for k in 0:2999
        d0 = Date(2026, 1, 1) + Day((k * 7) % 600)
        while !is_trading_day(d0)
            d0 += Day(1)
        end
        extra = Set(Date(2026, 1, 1) + Day((k * 13 + j * 101) % 700) for j in 1:(1 + k % 6))
        add_trading_days(d0, 5, union(OVERRIDES_SEED, extra)) >= add_trading_days(d0, 5) ||
            push!(bad, "an extra closure shortened a block from $d0")
    end
    # Weekday count vs trading-day count: buy Mon 23 Nov 2026 -> first sell Tue 1 Dec (not Mon 30 Nov).
    add_trading_days(Date(2026, 11, 23), 5) == Date(2026, 12, 1) || push!(bad, "Thanksgiving week case")
    return bad
end

end # module
