# Tests for NYSECalendar: the golden fixtures (09-lock-state-machine), the cross-language
# holiday fixture shared with the Rust `calendar` crate, the verified oracle, and override
# loading (add-only, all-or-nothing, environment).
using NYSECalendar
using Dates
using JSON3
using Test

const REPO = normpath(joinpath(@__DIR__, "..", "..", ".."))
const SEED = joinpath(REPO, "config", "nyse_calendar_overrides.toml")
const NO_RUNTIME = joinpath(REPO, "calendar", "tests", "fixtures", "does-not-exist.toml")

production() = load_calendar(SEED, NO_RUNTIME)

function with_files(f, files::Pair...)
    mktempdir() do dir
        for (name, text) in files
            write(joinpath(dir, name), text)
        end
        f(dir)
    end
end

@testset "NYSECalendar" begin
    cal = production()

    @testset "golden fixtures" begin
        fx = JSON3.read(read(joinpath(REPO, ".claude", "skills", "09-lock-state-machine",
                                      "fixtures", "nyse_calendar_cases.json"), String))
        @test length(fx.holidays) == 23
        for (y, want) in pairs(fx.holidays)
            @test holidays(cal, parse(Int, String(y))) == Date.(want)
        end
        @test length(fx.add_trading_days) == 1500
        bad = [c for c in fx.add_trading_days
               if add_trading_days(cal, Date(c.start), c.n) != Date(c.expected)]
        @test isempty(bad)
    end

    @testset "cross-language rule holidays 1990-2100" begin
        fx = JSON3.read(read(joinpath(REPO, "calendar", "tests", "fixtures",
                                      "rule_holidays_1990_2100.json"), String))
        @test length(fx.rule_holidays) == 111
        for (y, want) in pairs(fx.rule_holidays)
            @test rule_holidays(parse(Int, String(y))) == Date.(want)
        end
    end

    @testset "agrees with the verified oracle" begin
        include(joinpath(REPO, ".claude", "skills", "09-lock-state-machine", "scripts",
                         "nyse_calendar_oracle.jl"))
        O = NYSECalendarOracle
        @test all(y -> rule_holidays(y) == sort(collect(O.rule_holidays(y))), 1990:2100)
        @test Set(c.date for c in cal.closures) == O.OVERRIDES_SEED
    end

    @testset "seed closures and named cases" begin
        @test length(cal.closures) == 10
        for d in (Date(2001, 9, 11), Date(2012, 10, 29), Date(2012, 10, 30), Date(2025, 1, 9))
            @test !is_trading_day(cal, d)
            @test is_closure(cal, d)
        end
        @test add_trading_days(cal, Date(2012, 10, 26), 1) == Date(2012, 10, 31)
        @test add_trading_days(cal, Date(2026, 10, 12), 5) == Date(2026, 10, 19)
        @test add_trading_days(cal, Date(2026, 11, 23), 5) == Date(2026, 12, 1)
        @test add_trading_days(cal, Date(2026, 10, 17), 0) == Date(2026, 10, 19)  # Saturday rolls
        @test !is_closure(cal, Date(2026, 10, 17))                               # weekend
        @test !(Date(2021, 12, 31) in holidays(cal, 2021))                        # Saturday New Year
        @test Date(2023, 1, 2) in holidays(cal, 2023)                             # Sunday New Year
        @test Date(2027, 6, 18) in holidays(cal, 2027)                            # Juneteenth Sat -> Fri
        @test !any(d -> month(d) == 6 && day(d) in 18:20, holidays(cal, 2021))
        @test next_trading_day(cal, Date(2026, 11, 25)) == Date(2026, 11, 27)
        @test prev_trading_day(cal, Date(2026, 11, 27)) == Date(2026, 11, 25)
        @test_throws ArgumentError add_trading_days(cal, Date(2026, 1, 5), -1)
    end

    @testset "trading_days_between consistent with add_trading_days" begin
        d = Date(2018, 1, 2)
        for k in 0:400
            s = add_trading_days(cal, d + Day(k * 3), 0)
            n = k % 9
            e = add_trading_days(cal, s, n)
            @test trading_days_between(cal, s, e) == n
            @test trading_days_between(cal, e, s) == -n
            @test n == 0 || is_trading_day(cal, e)
        end
    end

    @testset "extra closures only lengthen blocks" begin
        extra = Calendar([Closure(Date(2026, 10, 14), "test closure", :explicit)])
        base = rules_only()
        for k in 0:200
            s = add_trading_days(base, Date(2026, 9, 1) + Day(k), 0)
            @test add_trading_days(extra, s, 5) >= add_trading_days(base, s, 5)
        end
    end

    @testset "override loading is add-only and all-or-nothing" begin
        ok = """
        [[closure]]
        date = "2030-03-06"
        name = "test"
        source = "manual"
        added_on = "2030-03-06T14:00:00Z"
        [[closure]]
        date = 2030-03-07
        name = "TOML local date"
        """
        with_files("seed.toml" => read(SEED, String), "rt.toml" => ok) do dir
            c = load_calendar(joinpath(dir, "seed.toml"), joinpath(dir, "rt.toml"))
            @test !is_trading_day(c, Date(2030, 3, 6))
            @test !is_trading_day(c, Date(2030, 3, 7))
            @test length(c.closures) == 12
            @test count(x -> x.origin == :runtime, c.closures) == 2
        end
        bad_cases = [
            "weekend" => "[[closure]]\ndate = \"2030-03-09\"\nname = \"sat\"\n",
            "empty name" => "[[closure]]\ndate = \"2030-03-06\"\nname = \"  \"\n",
            "unknown key" => "[[closure]]\ndate = \"2030-03-06\"\nname = \"x\"\nopen = true\n",
            "unknown top-level" => "reopen = [\"2030-03-06\"]\n",
            "bad source" => "[[closure]]\ndate = \"2030-03-06\"\nname = \"x\"\nsource = \"guess\"\n",
            "bad date" => "[[closure]]\ndate = \"2030-3-6\"\nname = \"x\"\n",
            "missing date" => "[[closure]]\nname = \"x\"\n",
            "invalid toml" => "[[closure]\n",
        ]
        for (label, text) in bad_cases
            with_files("seed.toml" => read(SEED, String), "rt.toml" => text) do dir
                @test_throws CalendarError load_calendar(joinpath(dir, "seed.toml"),
                                                         joinpath(dir, "rt.toml"))
            end
        end
        @test_throws CalendarError load_calendar(joinpath(REPO, "missing-seed.toml"), NO_RUNTIME)
        @test_throws CalendarError Calendar([Closure(Date(2030, 3, 9), "sat", :explicit)])
    end

    @testset "environment" begin
        withenv("QUANT_CONFIG" => nothing, "QUANT_STATE" => "/tmp") do
            err = try
                calendar_from_env(); nothing
            catch e
                e
            end
            @test err isa CalendarError && occursin("QUANT_CONFIG", err.msg)
        end
        mktempdir() do state
            withenv("QUANT_CONFIG" => joinpath(REPO, "config"), "QUANT_STATE" => state) do
                @test length(calendar_from_env().closures) == 10
            end
        end
    end
end
