# Generates schemas/registry.json (format_version 2, 13-parquet-lake/SKILL.md §6).
#
#   julia --project=schemas --startup-file=no schemas/generate-registry.jl [--check] [--migration <id>]
#
# fields / primary_key / schemas_md come from parsing SCHEMAS.md, layout and partition keys
# from the SKILL.md §3 table; partition_column, writer and merge from DATASETS below. Then
# `lake-schema-probe` writes one empty leaf per dataset through the real Rust writer and
# DuckDB `DESCRIBE` of each leaf gives columns/hash. `--check` regenerates and fails if the
# committed registry differs (CI). Needs cargo and the DuckDB CLI on PATH.

using JSON3
using SHA

const REPO = dirname(@__DIR__)
const SKILL_DIR = joinpath(REPO, ".claude", "skills", "13-parquet-lake")
const REGISTRY = get(ENV, "QUANT_REGISTRY_PATH", joinpath(REPO, "schemas", "registry.json"))
const INITIAL_MIGRATION = "2026-10-10-initial"

# Facts SCHEMAS.md does not state in a parseable form: (partition_column, writer, merge).
# merge: IO-CONTRACT.md §3 step 2; regime_states (#6 "append only") and gspqr_predictions
# (constitution §3: never recomputed for past dates) are also append_only.
const DATASETS = Dict(
    "universe" => ("date", "julia-universe", "overwrite"),
    "prices" => ("date", "rust-ingest", "overwrite"),
    "fundamentals" => (nothing, "rust-ingest", "overwrite"),
    "macro" => (nothing, "rust-ingest", "overwrite"),
    "panel" => ("date", "julia-panel", "overwrite"),
    "regime_states" => (nothing, "julia-regime", "append_only"),
    "gspqr_predictions" => ("date", "julia-gspqr", "append_only"),
    "tcp_calibration" => ("forecast_date", "julia-tcp", "overwrite"),
    "evt_residuals" => ("date", "julia-evt", "overwrite"),
    "evt_tail_bounds" => ("fit_date", "julia-evt", "overwrite"),
    "conviction_scores" => (nothing, "julia-conviction", "overwrite"),
    "veto_flags" => (nothing, "julia-conviction", "overwrite"),
    "portfolio_targets" => (nothing, "julia-portfolio", "overwrite"),
    "position_locks" => (nothing, "rust-lock", "overwrite"),
    "pending_orders" => (nothing, "rust-order", "overwrite"),
    "orders" => (nothing, "rust-exec", "overwrite"),
    "positions" => (nothing, "rust-exec", "overwrite"),
    "fills" => (nothing, "rust-exec", "overwrite"),
    "evaluation_metrics" => (nothing, "julia-backtest", "overwrite"),
    "data_quality" => (nothing, "rust-ingest", "overwrite"),
    "audit_log" => ("timestamp", "rust-eventbus", "append_only"),
    "portfolio_ledger" => ("date", "julia-ledger", "overwrite"),  # same-day re-run overwrites (constitution §8)
    "tax_lots" => (nothing, "rust-exec", "overwrite"),
    "trade_summary" => ("exit_date", "julia-ledger", "overwrite"),
    "order_history" => (nothing, "rust-audit", "append_only"),
    "execution_costs" => ("date", "rust-exec", "append_only"),
    "quote_spread_1m" => ("date", "rust-quotes", "append_only"),
    "splits" => (nothing, "rust-ingest", "overwrite"),
    "dividends" => (nothing, "rust-ingest", "overwrite"),
    "etf_holdings" => ("snapshot_date", "rust-ingest", "overwrite"),
    "security_master" => (nothing, "rust-ingest", "overwrite"),
    "delistings" => (nothing, "rust-ingest", "overwrite"),
    "filing_events" => (nothing, "rust-ingest", "overwrite"),
    "price_coverage" => (nothing, "julia-universe", "overwrite"),
    "listings" => ("date", "rust-ingest", "overwrite"),
    "security_spells" => (nothing, "rust-ingest", "overwrite"),
)

# Keys SCHEMAS.md states in prose rather than in its "Primary key:" line.
const KEY_EXCEPTIONS = Dict(
    # #19: the merge key is the logical run key; nulls compare equal (IO-CONTRACT.md §3), so
    # legacy rows with null run columns stay unique on (eval_date, horizon).
    "evaluation_metrics" => ["run_id", "track", "path_id", "fold", "eval_date", "horizon", "spread_source"],
    "audit_log" => String[],  # #21: no primary key
)
const SORT_KEYS = Dict("audit_log" => ["timestamp", "event_id"])

# Arrow type (SCHEMAS.md) -> DuckDB DESCRIBE type of a leaf written by the Rust writer.
const DUCKDB_TYPE = Dict(
    "utf8" => "VARCHAR", "bool" => "BOOLEAN", "int16" => "SMALLINT", "int32" => "INTEGER",
    "int64" => "BIGINT", "float64" => "DOUBLE", "date32" => "DATE",
    "timestamp[ns, UTC]" => "TIMESTAMP WITH TIME ZONE",
)

"""
    parse_layouts(skill_md) -> Vector{NamedTuple}

Rows of the `SKILL.md` §3 table: `(number, name, partition_keys)`, in table order.
"""
function parse_layouts(text::AbstractString)
    rows = NamedTuple[]
    for m in eachmatch(r"^\| (\d+) \| `([a-z0-9_]+)`[^|]*\| (S|P\(([^)]*)\)) \|"m, text)
        keys = m[4] === nothing ? String[] : [strip(k, ['`', ' ']) for k in split(m[4], ',')]
        push!(rows, (number = parse(Int, m[1]), name = String(m[2]), partition_keys = String.(keys)))
    end
    rows
end

"""
    parse_schemas(schemas_md) -> Dict{String,NamedTuple}

Per `## N. \\`name\\`` section: `(number, fields, primary_key)`. `fields` are the
`Column | Type | Nullable | Notes` rows; `primary_key` the backticked list of the first
`Primary key:` line (`nothing` when the section has none).
"""
function parse_schemas(text::AbstractString)
    out = Dict{String,NamedTuple}()
    for sec in split(text, r"^## "m)[2:end]
        m = match(r"^(\d+)\. `([a-z0-9_]+)(?:\.parquet)?`", sec)
        m === nothing && continue
        fields = [(name = String(r[1]), type = String(r[2]), nullable = r[3] == "yes")
                  for r in eachmatch(r"^\| `([a-z0-9_]+)` \| `([^`]+)` \| (yes|no) \|"m, sec)]
        pk = match(r"^Primary key: `\(?([^`)]+)\)?`"m, sec)
        out[String(m[2])] = (number = parse(Int, m[1]), fields = fields,
                             primary_key = pk === nothing ? nothing : String.(strip.(split(pk[1], ','))))
    end
    out
end

"""
    stage_entries(layouts, schemas) -> Vector{Pair{String,Dict}}

Registry entries without `columns`/`hash`/`last_migration`, in `number` order. Errors on
any disagreement between the two documents and the tables above.
"""
function stage_entries(layouts, schemas)
    entries = Pair{String,Dict{String,Any}}[]
    names = [l.name for l in layouts]
    Set(names) == Set(keys(DATASETS)) ||
        error("SKILL.md §3 and DATASETS differ: $(symdiff(Set(names), Set(keys(DATASETS))))")
    for l in sort(layouts; by = l -> l.number)
        s = get(schemas, l.name, nothing)
        s === nothing && error("$(l.name): no section in SCHEMAS.md")
        s.number == l.number || error("$(l.name): SCHEMAS.md #$(s.number) vs SKILL.md #$(l.number)")
        isempty(s.fields) && error("$(l.name): no column table")
        for f in s.fields
            haskey(DUCKDB_TYPE, f.type) || error("$(l.name).$(f.name): type `$(f.type)` not allowed (SKILL.md §6)")
        end
        pk = get(KEY_EXCEPTIONS, l.name, s.primary_key)
        pk === nothing && error("$(l.name): no `Primary key:` line")
        col, writer, merge = DATASETS[l.name]
        (col === nothing) == isempty(l.partition_keys) ||
            error("$(l.name): partition_column and SKILL.md layout disagree")
        push!(entries, l.name => Dict{String,Any}(
            "number" => l.number,
            "layout" => isempty(l.partition_keys) ? "single" : "partitioned",
            "partition_keys" => l.partition_keys,
            "partition_column" => col,
            "writer" => writer,
            "schemas_md" => "#$(l.number)",
            "fields" => s.fields,  # NamedTuples: JSON3 keeps their key order
            "primary_key" => pk,
            "sort_key" => get(SORT_KEYS, l.name, pk),
            "merge" => merge,
        ))
    end
    entries
end

const ENTRY_ORDER = ["number", "layout", "partition_keys", "partition_column", "writer", "schemas_md",
                     "fields", "primary_key", "sort_key", "merge", "columns", "hash", "last_migration"]

"""Registry JSON text with a fixed key order (the file is diffed in review and by `--check`)."""
function render(entries)
    obj(d, order) = "{" * join(["$(JSON3.write(k)):$(JSON3.write(d[k]))" for k in order if haskey(d, k)], ",") * "}"
    ds = ["$(JSON3.write(name)):" * obj(e, ENTRY_ORDER) for (name, e) in entries]
    raw = "{\"format_version\":2," *
          "\"canonical_form\":\"DuckDB DESCRIBE: 'column_name:column_type' lines joined by '|', SHA-256 of the UTF-8 string\"," *
          "\"datasets\":{" * join(ds, ",") * "}}"
    io = IOBuffer()
    JSON3.pretty(io, raw; allow_inf = false)
    String(take!(io)) * "\n"
end

"""Canonical `column_name:column_type` lines of one leaf (same query as verify-schemas.sh)."""
function describe_leaf(leaf::AbstractString)
    occursin('\'', leaf) && error("single quote in path: $leaf")
    sql = "SELECT column_name || ':' || column_type FROM (DESCRIBE SELECT * FROM read_parquet('$leaf', hive_partitioning = false))"
    split(chomp(read(`duckdb -noheader -list -c $sql`, String)), '\n')
end

schema_hash(cols) = bytes2hex(sha256(join(cols, "|")))

function leaf_path(lake, name, e)
    e["layout"] == "single" && return joinpath(lake, name * ".parquet")
    vals = Dict("year" => "1970", "month" => "01", "date" => "1970-01-01", "session" => "1970-01-01")
    joinpath(lake, name, ["$k=$(vals[k])" for k in e["partition_keys"]]..., "data.parquet")
end

"""Probe every entry through the Rust writer and fill columns/hash/last_migration."""
function probe!(entries, old, migration)
    mktempdir() do tmp
        stage = joinpath(tmp, "stage.json")
        write(stage, render(entries))
        lake, locks = joinpath(tmp, "lake"), joinpath(tmp, "locks")
        run(Cmd(`cargo run --quiet --release -p lake --bin lake-schema-probe -- $stage $lake $locks`; dir = REPO))
        for (name, e) in entries
            cols = describe_leaf(leaf_path(lake, name, e))
            want = ["$(f.name):$(DUCKDB_TYPE[f.type])" for f in e["fields"]]
            cols == want || error("$name: DuckDB DESCRIBE disagrees with SCHEMAS.md types:\n  got  $cols\n  want $want")
            e["columns"] = cols
            e["hash"] = schema_hash(cols)
            prev = old === nothing ? nothing : get(old["datasets"], name, nothing)
            e["last_migration"] = if prev === nothing
                INITIAL_MIGRATION  # new dataset: no migration file (MIGRATIONS.md)
            elseif prev["hash"] == e["hash"]
                prev["last_migration"]
            elseif migration !== nothing
                migration
            else
                error("$name: schema changed ($(prev["hash"][1:12]) -> $(e["hash"][1:12])): pass --migration <id>")
            end
        end
    end
    entries
end

function main(args)
    check = "--check" in args
    i = findfirst(==("--migration"), args)
    migration = i === nothing ? nothing : args[i+1]
    layouts = parse_layouts(read(joinpath(SKILL_DIR, "SKILL.md"), String))
    schemas = parse_schemas(read(joinpath(SKILL_DIR, "SCHEMAS.md"), String))
    old = isfile(REGISTRY) ? JSON3.read(read(REGISTRY, String), Dict{String,Any}) : nothing
    text = render(probe!(stage_entries(layouts, schemas), old, migration))
    if check
        old_text = isfile(REGISTRY) ? read(REGISTRY, String) : ""
        old_text == text || (println(stderr, "FAIL: $REGISTRY is stale; run schemas/generate-registry.jl"); exit(1))
        println("OK: registry matches SCHEMAS.md and the writer ($(length(layouts)) datasets)")
    else
        tmp = REGISTRY * ".tmp." * string(time_ns())
        write(tmp, text)
        mv(tmp, REGISTRY; force = true)
        println("Wrote $(length(layouts)) datasets to $REGISTRY")
    end
end

abspath(PROGRAM_FILE) == (@__FILE__) && main(ARGS)
