"""
    NYSECalendar

NYSE trading calendar for Julia: holiday rules computed for any year, unioned with the
overrides seed (`\$QUANT_CONFIG/nyse_calendar_overrides.toml`) and the runtime overrides
(`\$QUANT_STATE/nyse_calendar_overrides.toml`).

Specification: `.claude/skills/09-lock-state-machine/SKILL.md` §4. Same semantics as the
Rust `calendar` crate; both reproduce
`.claude/skills/09-lock-state-machine/fixtures/nyse_calendar_cases.json` exactly.

This package is read-only: only Rust (`calendar-sync`, `quant-exec calendar add`) writes the
runtime overrides file. The lock itself is computed in Rust; Julia uses this calendar for
panel windows, horizons and schedules.

**Fail closed.** Overrides can only add closures; every malformed file is an error, never a
silent fallback to the rules alone.
"""
module NYSECalendar

using Dates
using TOML

export Calendar, Closure, CalendarError, rule_holidays, easter_sunday, load_calendar,
    calendar_from_env, rules_only, is_trading_day, is_closure, holidays, add_trading_days,
    next_trading_day, prev_trading_day, trading_days_between

"""File name of both overrides files (IO-CONTRACT §1)."""
const OVERRIDES_FILE_NAME = "nyse_calendar_overrides.toml"

"""First year in which NYSE closes for Juneteenth."""
const JUNETEENTH_FIRST_YEAR = 2022

"""Allowed `source` values of a runtime closure (09 SKILL.md §4)."""
const CLOSURE_SOURCES = ("schwab_marketdata", "same_day_signal", "price_bar_backfill", "manual")

const CLOSURE_KEYS = ("date", "name", "added_on", "first_seen", "source")

"""
    CalendarError(msg)

A calendar could not be loaded (missing environment variable, unreadable or malformed
overrides file). Never caught to fall back to a smaller calendar.
"""
struct CalendarError <: Exception
    msg::String
end
Base.showerror(io::IO, e::CalendarError) = print(io, "CalendarError: ", e.msg)

"""
    Closure(date, name, origin)

One ad-hoc full-day closure from an overrides file. `origin` is `:seed`, `:runtime` or
`:explicit`.
"""
struct Closure
    date::Date
    name::String
    origin::Symbol
end

"""
    Calendar

Rule holidays (any year) unioned with override closures. A trading day ("session") is a
weekday that is neither. Half days are sessions. Build with [`load_calendar`],
[`calendar_from_env`] or [`rules_only`].
"""
struct Calendar
    closures::Vector{Closure}
    closed::Set{Date}
end

"""
    rules_only()

The rules with no overrides. Not a production calendar (it lacks the seed); for tests.
"""
rules_only() = Calendar(Closure[], Set{Date}())

"""
    Calendar(closures)

The rules plus explicit closures. Throws [`CalendarError`] for a weekend date or an empty
name.
"""
function Calendar(closures::AbstractVector{Closure})
    for c in closures
        validate_closure(c.date, c.name, "explicit closure")
    end
    return Calendar(collect(closures), Set(c.date for c in closures))
end

is_weekend(d::Date) = dayofweek(d) >= 6

# The n-th (n >= 1) weekday `wd` (Monday = 1) of a month, or the last one for n = -1.
function nth_weekday(y::Integer, m::Integer, wd::Integer, n::Integer)
    if n > 0
        first = Date(y, m, 1)
        return first + Day(mod(wd - dayofweek(first), 7) + 7 * (n - 1))
    end
    last = lastdayofmonth(Date(y, m, 1))
    return last - Day(mod(dayofweek(last) - wd, 7))
end

"""
    easter_sunday(y)

Gregorian Easter Sunday (anonymous Gregorian algorithm, Meeus/Jones/Butcher).
"""
function easter_sunday(y::Integer)
    a = mod(y, 19); b = fld(y, 100); c = mod(y, 100)
    d = fld(b, 4); e = mod(b, 4)
    f = fld(b + 8, 25); g = fld(b - f + 1, 3)
    h = mod(19a + b - d - g + 15, 30)
    i = fld(c, 4); k = mod(c, 4)
    l = mod(32 + 2e + 2i - h - k, 7)
    m = fld(a + 11h + 22l, 451)
    return Date(y, fld(h + l - 7m + 114, 31), mod(h + l - 7m + 114, 31) + 1)
end

# Saturday -> Friday, Sunday -> Monday (Juneteenth, Independence Day, Christmas).
function observed(d::Date)
    dow = dayofweek(d)
    return dow == 6 ? d - Day(1) : dow == 7 ? d + Day(1) : d
end

"""
    rule_holidays(y) -> Vector{Date}

The rule-computed NYSE full-day holidays of year `y`, sorted. A Saturday 1 January closes
nothing; a Sunday one moves to Monday. Juneteenth from 2022. The rules are applied to every
year uniformly; for old years this can only add closures (the safe direction).
"""
function rule_holidays(y::Integer)
    h = Date[]
    ny = Date(y, 1, 1)
    dayofweek(ny) == 6 || push!(h, dayofweek(ny) == 7 ? ny + Day(1) : ny)
    push!(h, nth_weekday(y, 1, 1, 3))            # Martin Luther King Jr. Day
    push!(h, nth_weekday(y, 2, 1, 3))            # Washington's Birthday
    push!(h, easter_sunday(y) - Day(2))          # Good Friday
    push!(h, nth_weekday(y, 5, 1, -1))           # Memorial Day
    y >= JUNETEENTH_FIRST_YEAR && push!(h, observed(Date(y, 6, 19)))
    push!(h, observed(Date(y, 7, 4)))            # Independence Day
    push!(h, nth_weekday(y, 9, 1, 1))            # Labor Day
    push!(h, nth_weekday(y, 11, 4, 4))           # Thanksgiving
    push!(h, observed(Date(y, 12, 25)))          # Christmas
    return sort!(h)
end

is_rule_holiday(d::Date) = d in rule_holidays(year(d))

function validate_closure(date::Date, name::AbstractString, where_::AbstractString)
    is_weekend(date) && throw(CalendarError("$where_: $date is a weekend, not a closure"))
    isempty(strip(name)) && throw(CalendarError("$where_: closure $date has an empty name"))
    return nothing
end

function parse_date(v, where_::AbstractString)
    v isa Date && return v
    if v isa AbstractString
        m = match(r"^\d{4}-\d{2}-\d{2}$", v)
        m === nothing && throw(CalendarError("$where_: date $(repr(v)) is not YYYY-MM-DD"))
        d = tryparse(Date, v, dateformat"yyyy-mm-dd")
        d === nothing && throw(CalendarError("$where_: invalid date $(repr(v))"))
        return d
    end
    throw(CalendarError("$where_: date must be a YYYY-MM-DD string or a TOML date"))
end

"""
    parse_overrides(text, path, origin) -> Vector{Closure}

Parses one overrides file. All-or-nothing: any bad entry, unknown key or unknown top-level
key throws [`CalendarError`].
"""
function parse_overrides(text::AbstractString, path::AbstractString, origin::Symbol)
    doc = try
        TOML.parse(text)
    catch e
        throw(CalendarError("$path: invalid TOML: $(sprint(showerror, e))"))
    end
    for k in keys(doc)
        k == "closure" || throw(CalendarError("$path: unknown top-level key $(repr(k))"))
    end
    entries = get(doc, "closure", Any[])
    entries isa AbstractVector || throw(CalendarError("$path: `closure` must be an array of tables"))
    out = Closure[]
    for (i, e) in enumerate(entries)
        where_ = "$path closure #$i"
        e isa AbstractDict || throw(CalendarError("$where_: not a table"))
        for k in keys(e)
            k in CLOSURE_KEYS || throw(CalendarError("$where_: unknown key $(repr(k))"))
        end
        haskey(e, "date") || throw(CalendarError("$where_: missing date"))
        haskey(e, "name") || throw(CalendarError("$where_: missing name"))
        name = e["name"]
        name isa AbstractString || throw(CalendarError("$where_: name must be a string"))
        if haskey(e, "source")
            e["source"] in CLOSURE_SOURCES ||
                throw(CalendarError("$where_: unknown source $(repr(e["source"]))"))
        end
        date = parse_date(e["date"], where_)
        validate_closure(date, name, where_)
        push!(out, Closure(date, String(name), origin))
    end
    return out
end

"""
    load_calendar(seed, runtime) -> Calendar

The production calendar from explicit paths: the rules, the seed file (must exist) and the
runtime file (absent means no runtime closures).
"""
function load_calendar(seed::AbstractString, runtime::AbstractString)
    isfile(seed) || throw(CalendarError("seed overrides file not found: $seed"))
    closures = parse_overrides(read(seed, String), seed, :seed)
    isfile(runtime) && append!(closures, parse_overrides(read(runtime, String), runtime, :runtime))
    return Calendar(closures, Set(c.date for c in closures))
end

"""
    calendar_from_env() -> Calendar

[`load_calendar`] from `\$QUANT_CONFIG` and `\$QUANT_STATE`. An unset or empty variable
throws [`CalendarError`] naming it (no default path).
"""
function calendar_from_env()
    dir(var) = (v = get(ENV, var, ""); isempty(v) ? throw(CalendarError("$var is not set")) : v)
    config = dir("QUANT_CONFIG")
    state = dir("QUANT_STATE")
    return load_calendar(joinpath(config, OVERRIDES_FILE_NAME), joinpath(state, OVERRIDES_FILE_NAME))
end

"""
    is_trading_day(cal, d) -> Bool

A session: a weekday that is neither a rule holiday nor an override closure.
"""
is_trading_day(cal::Calendar, d::Date) = !is_weekend(d) && !(d in cal.closed) && !is_rule_holiday(d)

"""
    is_closure(cal, d) -> Bool

A weekday on which the exchange is closed (rule holiday or override).
"""
is_closure(cal::Calendar, d::Date) = !is_weekend(d) && !is_trading_day(cal, d)

"""
    holidays(cal, y) -> Vector{Date}

Rule holidays plus override closures of year `y`, sorted, without duplicates (the list the
golden fixture records per year).
"""
holidays(cal::Calendar, y::Integer) =
    sort!(collect(union(Set(rule_holidays(y)), Set(d for d in cal.closed if year(d) == y))))

"""
    next_trading_day(cal, d) -> Date

The first session strictly after `d`.
"""
function next_trading_day(cal::Calendar, d::Date)
    d += Day(1)
    while !is_trading_day(cal, d)
        d += Day(1)
    end
    return d
end

"""
    prev_trading_day(cal, d) -> Date

The last session strictly before `d`.
"""
function prev_trading_day(cal::Calendar, d::Date)
    d -= Day(1)
    while !is_trading_day(cal, d)
        d -= Day(1)
    end
    return d
end

"""
    add_trading_days(cal, d, n) -> Date

Roll `d` forward to the first session on or after it, then advance `n >= 0` sessions. For the
lock, `add_trading_days(cal, D, 5)` is the first permitted day of a block placed on session `D`.
"""
function add_trading_days(cal::Calendar, d::Date, n::Integer)
    n >= 0 || throw(ArgumentError("n must be >= 0, got $n"))
    while !is_trading_day(cal, d)
        d += Day(1)
    end
    for _ in 1:n
        d = next_trading_day(cal, d)
    end
    return d
end

"""
    trading_days_between(cal, from, to) -> Int

Sessions in `(from, to]` (`from` excluded, `to` included); negative when `to < from`. For a
session `d`, `trading_days_between(cal, d, add_trading_days(cal, d, n)) == n`.
"""
function trading_days_between(cal::Calendar, from::Date, to::Date)
    lo, hi, sign = from <= to ? (from, to, 1) : (to, from, -1)
    return sign * count(d -> is_trading_day(cal, d), (lo + Day(1)):Day(1):hi)
end

end # module
