# Oracle, not production code. Verified 2026-10-10 with Julia 1.12: `run_fixtures.jl` checks it
# against the golden fixtures and prints ALL ORACLE CHECKS PASSED (re-run after any change).
#
# ORACLE for the 5-day lock rule (constitution section 1, amendment 1.4; 09 SKILL.md section 1).
# It re-implements the pure functions that the Rust module `execution/src/locks/` must implement
# (`counted`, `block_end`, `blocked`) and is checked against the language-neutral golden file
# `fixtures/lock_cases.json` (19 named cases + 3000 random cases). The Rust tests
# `execution/tests/lock_fixtures.rs` consume the SAME fixture file. Where the two disagree, the
# fixture wins and one implementation is wrong.
#
# Semantics are the verified Python oracle's, ported exactly (Python is retired from this
# repository; the fixtures survive it):
#   - 15 open statuses block with no end date; any unknown / missing / non-string status is treated
#     as OPEN (fail closed). UNKNOWN is one of Schwab's 21 values and is also open.
#   - FILLED counts from its last fill day.
#   - CANCELED, REJECTED, EXPIRED and REPLACED are NOT counted with zero fills (REPLACED is treated
#     exactly like CANCELED: owner decision) and are counted from the last fill day with any fill.
#   - first permitted day = add_trading_days(D, 5), D = last fill day (placement day if no later fill).
# The fixtures carry only the rule. Loader-level behaviour (an unknown enum VALUE degrading to a
# per-ticker block, symbol mapping, journal merge, Incomplete) is specified in SKILL.md section 3
# and tested in Rust, not here.

module LockRuleOracle

using Dates
include("nyse_calendar_oracle.jl")
using .NYSECalendarOracle

export Order, is_open, counted, block_end, blocked, selftest

# The 15 statuses that may still fill: they block the opposite side with NO end date.
const OPEN_STATUSES = Set([
    "WORKING", "QUEUED", "ACCEPTED", "NEW", "PENDING_ACTIVATION", "PENDING_ACKNOWLEDGEMENT",
    "PENDING_CANCEL", "PENDING_REPLACE", "PENDING_RECALL", "AWAITING_PARENT_ORDER",
    "AWAITING_CONDITION", "AWAITING_STOP_CONDITION", "AWAITING_MANUAL_REVIEW", "AWAITING_UR_OUT",
    "AWAITING_RELEASE_TIME",
])
# Terminal statuses that count only if something filled ("no fill, no count"). FILLED always filled.
const TERMINAL_NEEDS_FILL = Set(["CANCELED", "REJECTED", "EXPIRED", "REPLACED"])

struct Order
    side::String                       # "BUY" | "SELL"
    placed::Date                       # ET placement date; rolling off a non-session day happens inside add_trading_days
    status::Union{String,Nothing}      # nothing = missing
    filled_qty::Float64
    last_fill::Union{Date,Nothing}     # ET date of the last fill; nothing if never filled
end

function is_open(status)::Bool
    status isa AbstractString || return true           # missing / nothing / not a string: fail closed
    s = String(status)
    s in OPEN_STATUSES && return true
    (s == "FILLED" || s in TERMINAL_NEEDS_FILL) && return false
    return true                                        # any other string (incl. "UNKNOWN"): open
end

counted(o::Order)::Bool = is_open(o.status) || o.filled_qty > 0

# `nothing` = the order may still fill: it blocks the opposite side with no end date.
# Otherwise the FIRST PERMITTED opposite-side day: last fill day (placement day if none) + 5 trading days.
function block_end(o::Order)::Union{Date,Nothing}
    is_open(o.status) && return nothing
    d = o.last_fill === nothing ? o.placed : o.last_fill
    return add_trading_days(d, 5)
end

function blocked(orders::AbstractVector{Order}, side::AbstractString, today::Date)::Bool
    opp = side == "BUY" ? "SELL" : "BUY"
    for o in orders
        (o.side == opp && counted(o)) || continue
        e = block_end(o)
        (e === nothing || today < e) && return true
    end
    return false
end

mk(side, placed, status = "FILLED", q = 1.0; fill = nothing) =
    Order(side, placed, status, q, fill === nothing ? (q > 0 ? placed : nothing) : fill)

# Returns the list of failures (empty = pass). Ported from the retired Python oracle's assertions.
function selftest()::Vector{String}
    bad = String[]
    check(cond, msg) = cond || push!(bad, msg)
    mon = Date(2026, 10, 12)
    b = [mk("BUY", mon)]
    check(blocked(b, "SELL", Date(2026, 10, 16)) && !blocked(b, "SELL", Date(2026, 10, 19)), "buy Mon: Fri blocked, next Mon allowed")
    b2 = [mk("BUY", mon), mk("BUY", Date(2026, 10, 14))]
    check(blocked(b2, "SELL", Date(2026, 10, 19)) && blocked(b2, "SELL", Date(2026, 10, 20)) && !blocked(b2, "SELL", Date(2026, 10, 21)), "buys Mon+Wed")
    check(!blocked([mk("BUY", mon, "CANCELED", 0.0)], "SELL", Date(2026, 10, 13)), "canceled zero-fill not counted")
    check(!blocked([mk("BUY", mon, "REJECTED", 0.0)], "SELL", Date(2026, 10, 13)), "rejected zero-fill not counted")
    check(blocked([mk("BUY", mon, "WORKING", 0.0)], "SELL", Date(2026, 10, 13)), "working counted")
    check(blocked([mk("BUY", mon, "CANCELED", 0.4)], "SELL", Date(2026, 10, 13)), "partial then canceled counted")
    check(blocked([mk("SELL", mon)], "BUY", Date(2026, 10, 16)) && !blocked([mk("SELL", mon)], "BUY", Date(2026, 10, 19)), "sell blocks buy symmetric")
    check(!blocked(b, "BUY", Date(2026, 10, 13)), "same side never blocked")
    check(add_trading_days(Date(2026, 11, 23), 5) == Date(2026, 12, 1), "Thanksgiving week")
    check(add_trading_days(Date(2026, 10, 17), 5) == add_trading_days(Date(2026, 10, 19), 5), "Saturday == Monday")
    g = [mk("BUY", mon, "FILLED", 1.0; fill = Date(2026, 10, 15))]   # GTC placed Mon, filled Thu
    check(blocked(g, "SELL", Date(2026, 10, 19)) && blocked(g, "SELL", Date(2026, 10, 21)) && !blocked(g, "SELL", Date(2026, 10, 22)), "late fill counts from the fill")
    check(blocked([mk("BUY", mon, "WORKING", 0.0)], "SELL", Date(2026, 12, 1)), "open blocks with no end date")
    p = [mk("BUY", mon, "CANCELED", 0.5; fill = Date(2026, 10, 14))]
    check(blocked(p, "SELL", Date(2026, 10, 20)) && !blocked(p, "SELL", Date(2026, 10, 21)), "partial fills Mon+Wed then cancel")
    check(add_trading_days(Date(2027, 1, 13), 5) == Date(2027, 1, 21), "MLK 2027")
    check(add_trading_days(Date(2025, 11, 24), 5) == Date(2025, 12, 2), "Thanksgiving 2025")
    check(add_trading_days(Date(2025, 1, 8), 5) == Date(2025, 1, 16), "9 Jan 2025 override")
    for st in ("QUEUED", "ACCEPTED", "PENDING_CANCEL", "PENDING_REPLACE", "AWAITING_CONDITION", "UNKNOWN", "SOMETHING_NEW")
        check(blocked([mk("BUY", mon, st, 0.0)], "SELL", Date(2026, 12, 1)), "open/unknown blocks with no end: $st")
    end
    check(blocked([Order("BUY", mon, nothing, 0.0, nothing)], "SELL", Date(2026, 12, 1)), "missing status counted open")
    check(blocked([mk("BUY", mon, "PENDING_CANCEL", 0.5; fill = mon)], "SELL", Date(2026, 12, 1)), "partial fill, cancel not confirmed: still open")
    for st in ("CANCELED", "REJECTED", "EXPIRED", "REPLACED")      # REPLACED == CANCELED (owner decision)
        check(!blocked([mk("BUY", mon, st, 0.0)], "SELL", Date(2026, 10, 13)), "zero-fill terminal not counted: $st")
        check(blocked([mk("BUY", mon, st, 0.3; fill = mon)], "SELL", Date(2026, 10, 13)), "any fill counts: $st")
    end
    return vcat(bad, NYSECalendarOracle.selftest())
end

end # module
