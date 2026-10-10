# Last run 2026-10-10 with Julia 1.12: ALL ORACLE CHECKS PASSED, exit 0.
#
# Test runner: checks the Julia oracles against the language-neutral golden fixtures.
#   julia --project scripts/run_fixtures.jl            (from 09-lock-state-machine/)
# Exit code 0 = every oracle self-test passes and every fixture case matches; 1 otherwise.
# The Rust tests (`execution/tests/lock_fixtures.rs`) read the SAME two JSON files and must
# reproduce the SAME expected values; the fixtures are the contract, neither implementation is.
# Dependencies (declare in the Project.toml of whichever Julia environment runs this): JSON3 (TOML is a stdlib).

using Dates
using JSON3
using TOML   # stdlib

include(joinpath(@__DIR__, "lock_rule_oracle.jl"))
using .LockRuleOracle
const Cal = LockRuleOracle.NYSECalendarOracle

const FIXTURES = joinpath(@__DIR__, "..", "fixtures")

function load_orders(v)::Vector{Order}
    out = Order[]
    for o in v
        status = get(o, :status, nothing)
        lf = get(o, :last_fill, nothing)
        push!(out, Order(
            String(o.side),
            Date(String(o.placed)),
            status === nothing ? nothing : String(status),
            Float64(o.filled_qty),
            lf === nothing ? nothing : Date(String(lf)),
        ))
    end
    return out
end

function main()
    failures = String[]
    append!(failures, LockRuleOracle.selftest())

    lock = JSON3.read(read(joinpath(FIXTURES, "lock_cases.json"), String))
    n = 0
    for (kind, cases) in (("named", lock.named), ("random", lock.random))
        for (i, c) in enumerate(cases)
            got = blocked(load_orders(c.orders), String(c.side), Date(String(c.today)))
            n += 1
            got == Bool(c.expected_blocked) ||
                push!(failures, "lock $kind[$i] $(get(c, :name, "")): expected $(c.expected_blocked), got $got")
        end
    end

    cal = JSON3.read(read(joinpath(FIXTURES, "nyse_calendar_cases.json"), String))
    for (ystr, want) in pairs(cal.holidays)
        y = parse(Int, String(ystr))
        got = sort(collect(Cal.holidays(y)))
        exp = sort([Date(String(s)) for s in want])
        got == exp || push!(failures, "holidays $y: expected $exp, got $got")
    end
    m = 0
    for (i, c) in enumerate(cal.add_trading_days)
        got = Cal.add_trading_days(Date(String(c.start)), Int(c.n))
        m += 1
        got == Date(String(c.expected)) ||
            push!(failures, "add_trading_days[$i] $(c.start)+$(c.n): expected $(c.expected), got $got")
    end

    # Historical ad-hoc closures for the 15+ year backfill (outside the 2018-2040 fixture range).
    for d in (Date(2007, 1, 2), Date(2012, 10, 29), Date(2012, 10, 30))
        Cal.is_trading_day(d) && push!(failures, "historical closure $d is a trading day")
    end
    Cal.add_trading_days(Date(2012, 10, 26), 1) == Date(2012, 10, 31) ||
        push!(failures, "add_trading_days(2012-10-26, 1) != 2012-10-31 (Hurricane Sandy)")

    # The oracle's OVERRIDES_SEED must equal the checked-in seed file.
    seed_path = joinpath(@__DIR__, "..", "..", "..", "..", "config", "nyse_calendar_overrides.toml")
    if isfile(seed_path)
        seed = Set(Date(String(c["date"])) for c in TOML.parsefile(seed_path)["closure"])
        seed == Cal.OVERRIDES_SEED ||
            push!(failures, "config/nyse_calendar_overrides.toml != OVERRIDES_SEED: $(symdiff(seed, Cal.OVERRIDES_SEED))")
    else
        push!(failures, "seed file not found at $seed_path")
    end

    println("lock cases: $n, calendar add_trading_days cases: $m, holiday years: $(length(cal.holidays))")
    if isempty(failures)
        println("ALL ORACLE CHECKS PASSED")
        return 0
    end
    foreach(f -> println(stderr, "FAIL: ", f), first(failures, 50))
    println(stderr, length(failures), " failure(s)")
    return 1
end

exit(main())
