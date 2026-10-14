# Panel stage 0 (02-panel-construction §3): the gates' verdict on the current lake, gate G4
# (survivorship coverage, the one gate Julia evaluates) and the survivorship caveat
# (01-data-ingestion/QUALITY-GATES.md §1-2, SURVIVORSHIP.md §4).

"""The gates `quant-ingest gates` evaluates that hold a HARD check (G8 is WARN only)."""
const RUST_GATES = ("G0", "G1", "G2", "G3", "G5", "G6", "G7", "G9")

"""The UTC start of a run from its id (`20261013T212836Z-gates`)."""
run_started(run) = DateTime(run[1:15], dateformat"yyyymmddTHHMMSS")

"""
    gate_verdict(dq, gate, written) -> NamedTuple

The latest run of `gate` in `data_quality` (columns `source, check_id, run_id, entity, chunk,
status, measured, threshold, notes`): its aggregate checks, whether it started after `written`
(the last lake write, UTC), and whether it passed: fresh, and no check `FAIL`.

# Example
```julia
gate_verdict(dq, "G2", DateTime(2026, 10, 13, 21))
```
"""
function gate_verdict(dq, gate, written)
    rows = [i for i in eachindex(dq.run_id) if coalesce(dq.source[i], "") == "Gate" &&
            coalesce(dq.check_id[i], "") == gate && coalesce(dq.entity[i], "") == "*"]
    isempty(rows) && return (gate = gate, run_id = missing, fresh = false, passed = false,
                             checks = NamedTuple[])
    run = maximum(String(dq.run_id[i]) for i in rows)
    checks = [(chunk = String(dq.chunk[i]), status = String(dq.status[i]),
               measured = dq.measured[i], threshold = dq.threshold[i],
               note = String(coalesce(dq.notes[i], ""))) for i in rows if dq.run_id[i] == run]
    fresh = run_started(run) >= written
    return (gate = gate, run_id = run, fresh = fresh,
            passed = fresh && all(c -> c.status != "FAIL", checks), checks = checks)
end

"""
    g4(days, cov, priced, member_px, thresholds) -> (checks, years)

Gate G4 (QUALITY-GATES.md §2; D20: membership exists from the first archived snapshot).
HARD: a `price_coverage` row (`cov`: `figi, ticker, coverage_status, gap_reason`) for every
holding of `days` ([`member_days`]) and every FIGI in `priced` (a price G2 did not reject),
and a `gap_reason` on every gap row. WARN: a year whose share of member days without a price
(none in `member_px`) is above `thresholds["warn_pct"]`; above `thresholds["ack_pct"]` no
model generation is promoted without an owner acknowledgement (D3). `years` is the
SURVIVORSHIP CAVEAT table: per year the member days, those without a price, both shares, and
the missing days by the member's `gap_reason` (else its coverage status).

# Example
```julia
checks, years = g4(days, cov, Set(["BBG000B9XRY4"]), px, Dict("warn_pct" => 2.0, "ack_pct" => 10.0))
```
"""
function g4(days, cov, priced, member_px, thresholds)
    by_figi = Dict(String(cov.figi[i]) => i for i in eachindex(cov.figi) if cov.figi[i] !== missing)
    by_ticker = Dict(String(cov.ticker[i]) => i for i in eachindex(cov.ticker)
                     if cov.figi[i] === missing)
    row(k) = k[1] === missing ? get(by_ticker, k[2], nothing) : get(by_figi, k[1], nothing)
    no_row = sort!(unique([coalesce(k[1], k[2]) for k in keys(days) if row(k) === nothing]))
    append!(no_row, sort!([f for f in priced if !haskey(by_figi, f)]))
    no_reason = sort!([String(coalesce(cov.figi[i], cov.ticker[i])) for i in eachindex(cov.ticker)
                       if cov.coverage_status[i] == "gap" && cov.gap_reason[i] === missing])
    tally = Dict{Int,Vector{Float64}}()  # member days, missing days, weight, missing weight
    reasons = Dict{Int,Dict{String,Int}}()
    for (k, d) in days
        px = k[1] === missing ? Set{Date}() : get(member_px, k[1], Set{Date}())
        i = row(k)
        why = i === nothing ? "no_coverage_row" :
              String(coalesce(cov.gap_reason[i], cov.coverage_status[i]))
        for (t, w) in d
            gone = !(t in px)
            get!(() -> zeros(4), tally, year(t)) .+= (1, gone, w, gone * w)
            if gone
                r = get!(reasons, year(t), Dict{String,Int}())
                r[why] = get(r, why, 0) + 1
            end
        end
    end
    pct(a, b) = b > 0 ? round(100 * a / b; digits = 2) : 0.0
    years = [(year = y, member_days = Int(v[1]), missing_days = Int(v[2]),
              missing_pct_by_count = pct(v[2], v[1]), missing_pct_by_weight = pct(v[4], v[3]),
              by_reason = get(reasons, y, Dict{String,Int}())) for (y, v) in sort!(collect(tally))]
    warn, ack = Float64(thresholds["warn_pct"]), Float64(thresholds["ack_pct"])
    worst = isempty(years) ? 0.0 : maximum(y.missing_pct_by_count for y in years)
    list(v) = isempty(v) ? "" : ": " * join(first(v, 20), " ") * (length(v) > 20 ? " ..." : "")
    share = isempty(years) ?
            "no member day on or before the data end (membership starts with the first archived snapshot, D20)" :
            "worst year $(worst)% of member days without a price" *
            (worst > ack ? "; above $(ack)%: no model generation is promoted without an owner acknowledgement (D3)" : "")
    checks = [
        (chunk = "coverage_rows", status = isempty(no_row) ? "OK" : "FAIL",
         measured = length(no_row), threshold = 0,
         note = "$(length(no_row)) holdings or priced securities without a price_coverage row" * list(no_row)),
        (chunk = "gap_reasons", status = isempty(no_reason) ? "OK" : "FAIL",
         measured = length(no_reason), threshold = 0,
         note = "$(length(no_reason)) gap rows without a gap_reason" * list(no_reason)),
        (chunk = "missing_share", status = worst > warn ? "WARN" : "OK", measured = worst,
         threshold = warn, note = share),
    ]
    return checks, years
end

"""The last write to the lake under `lake` (UTC), `data_quality` aside: what a gate run must
postdate."""
function last_write(lake)
    t = 0.0
    for (dir, _, files) in walkdir(lake), f in files
        startswith(relpath(joinpath(dir, f), lake), "data_quality") && continue
        t = max(t, mtime(joinpath(dir, f)))
    end
    return unix2datetime(t)
end

"""A UTC `DateTime` as ISO 8601 with its zone (`2026-10-14T17:15:03Z`)."""
utc(t) = Dates.format(t, "yyyy-mm-ddTHH:MM:SS") * "Z"

function write_json(path, x)
    tmp = path * ".tmp"
    open(io -> JSON3.pretty(io, JSON3.write(x)), tmp, "w")
    mv(tmp, path; force = true)
end

"""
    preflight(; config = ENV["QUANT_CONFIG"], root = ENV["QUANT_ROOT"]) -> NamedTuple

Stage 0 of the panel build (02-panel-construction §3). Every Rust gate with a HARD check
([`RUST_GATES`]) must have a latest run that started after the last lake write and has no
`FAIL` ([`gate_verdict`]); G4 is evaluated here ([`g4`], thresholds from `[survivorship]` in
`quality_gates.toml`). Writes `reports/gates/<run_id>.json` (every gate's checks) and
`reports/survivorship_caveat.json` (SURVIVORSHIP.md §4; the panel build adds the label
counts and the bias bound), then throws if a HARD gate failed or is stale.

# Returns
`(passed = true, report, caveat)`: the two paths.

# Example
```julia
preflight()
```
"""
function preflight(; config::AbstractString = ENV["QUANT_CONFIG"],
                   root::AbstractString = ENV["QUANT_ROOT"])
    started = now(UTC)
    run_id = Dates.format(started, "yyyymmddTHHMMSS") * "Z-preflight"
    thresholds = TOML.parsefile(joinpath(config, "quality_gates.toml"))["survivorship"]
    dq = read_dataset("data_quality"; columns = ["source", "check_id", "run_id", "entity",
                                                 "chunk", "status", "measured", "threshold", "notes"])
    written = last_write(joinpath(root, "lake"))
    verdicts = [gate_verdict(dq, g, written) for g in RUST_GATES]
    cal = calendar_from_env()
    holdings = read_dataset("etf_holdings"; columns = ["etf", "figi", "ticker", "snapshot_date",
                                                       "available_date", "weight"])
    master = read_dataset("security_master"; columns = ["figi", "ticker"])
    cov = read_dataset("price_coverage"; columns = ["figi", "ticker", "coverage_status",
                                                    "gap_reason"])
    prices = read_dataset("prices"; columns = ["figi", "date"])
    isempty(prices.date) && error("prices is empty")
    data_end = maximum(prices.date)
    rejected = g2_rejected(dq)
    first_day = isempty(holdings.available_date) ? data_end + Day(1) :
                minimum(holdings.available_date)
    tdays = [d for d in first_day:Day(1):data_end if is_trading_day(cal, d)]
    mticker = Dict(String(f) => String(t) for (f, t) in zip(master.figi, master.ticker))
    days = member_days(holdings, mticker, tdays, data_end)
    members = Set(k[1] for k in keys(days) if k[1] !== missing)
    priced, member_px = Set{String}(), Dict{String,Set{Date}}()
    for (f, d) in zip(prices.figi, prices.date)
        (f === missing || (f in priced && !(f in members))) && continue
        in_spans(get(rejected, f, NO_SPANS), d) && continue
        push!(priced, f)
        f in members && push!(get!(member_px, f, Set{Date}()), d)
    end
    checks, years = g4(days, cov, priced, member_px, thresholds)
    gates = [verdicts; (gate = "G4", run_id = run_id, fresh = true,
                        passed = all(c -> c.status != "FAIL", checks), checks = checks)]
    passed = all(g -> g.passed, gates)
    # The broad universe's price gap is the identity job's count (SURVIVORSHIP.md, D20).
    ids = [i for i in eachindex(dq.run_id) if endswith(String(dq.run_id[i]), "-identity")]
    id_run = isempty(ids) ? missing : maximum(String(dq.run_id[i]) for i in ids)
    unresolved = count(i -> dq.run_id[i] == id_run &&
                            startswith(coalesce(dq.notes[i], ""), "identity_unresolved"), ids)
    snap = isempty(holdings.snapshot_date) ? "none" :
           "$(minimum(holdings.snapshot_date)) (usable $first_day)"
    member = isempty(years) ? "no member day falls on or before the data end yet" :
             "$(sum(y.member_days for y in years)) member days, $(sum(y.missing_days for y in years)) without a price"
    text = "SURVIVORSHIP CAVEAT (data end $data_end): prices come from EODHD alone (G10 not " *
           "applicable). IWF/IWP membership exists from the first archived snapshot, $snap: " *
           "$member. Training uses the broad universe (D20), which has no membership history: " *
           "the identity run $(coalesce(id_run, "none")) left $unresolved EODHD codes without " *
           "a key (no FIGI, no training-only key; not all of them in-scope companies), whose " *
           "prices are not in the lake. The label censoring counts and the bias " *
           "bound come with the panel build."
    dir = joinpath(root, "reports")
    mkpath(joinpath(dir, "gates"))
    report = joinpath(dir, "gates", "$run_id.json")
    write_json(report, (run_id = run_id, generated_at = utc(started), data_end = data_end,
                        lake_written = utc(written), passed = passed, gates = gates))
    caveat = joinpath(dir, "survivorship_caveat.json")
    write_json(caveat, (caveat_version = 1, generated_at = utc(started), data_end = data_end,
                        price_source = "eodhd", other_price_sources = String[],
                        gates_passed = sort!([g.gate for g in gates if g.passed]),
                        thresholds = thresholds, years = years,
                        broad_universe = (identity_run = id_run,
                                          identity_unresolved_codes = unresolved),
                        labels = Dict{String,Any}(),
                        bias_bound = (r_missing = [-1.0, -0.5, 0.0], by_year = Dict{String,Any}()),
                        text = text))
    for g in gates
        bad = [c.chunk for c in g.checks if c.status != "OK"]
        println(rpad(g.gate, 4), g.passed ? "pass" : "FAIL", "  ", coalesce(g.run_id, "no run"),
                g.fresh ? "" : " (stale: before the last lake write $written)",
                isempty(bad) ? "" : "  not OK: " * join(bad, " "))
    end
    println(text)
    passed || error("preflight: HARD gates not passed: ",
                    join([g.gate for g in gates if !g.passed], " "), "; see $report")
    return (passed = passed, report = report, caveat = caveat)
end
