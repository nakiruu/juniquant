#!/usr/bin/env julia
# Regenerates schemas/registry.json (the repository copy) from the Parquet
# datasets currently in the lake.  SKETCH (not normative): the contract is
# 13-parquet-lake/SKILL.md section 6 and IO-CONTRACT.md; the DuckDB.jl API
# names below are unverified (no Julia in the authoring environment).
#
# Canonical form (identical to scripts/verify-schemas.sh): for ONE leaf file,
#   DESCRIBE SELECT * FROM read_parquet('<leaf>', hive_partitioning = false)
# each row rendered "column_name:column_type", rows joined by "|" in column
# order, SHA-256 of the UTF-8 string.  Partition keys are directory names and
# never appear in the string.
#
# Run inside the pinned environment (no run-time Pkg.add):
#   QUANT_ROOT=/data julia --project=schemas --startup-file=no \
#       generate-registry.jl [--migration <migration-id>]
# schemas/Project.toml must pin: DuckDB, Tables, JSON3, OrderedCollections
# (SHA is in the standard library).

using DuckDB
using Tables
using JSON3
using OrderedCollections
using SHA

const QUANT_ROOT = get(ENV, "QUANT_ROOT", "")
isempty(QUANT_ROOT) && error("QUANT_ROOT is not set (IO-CONTRACT.md section 1)")
const LAKE     = get(ENV, "GENERATE_REGISTRY_LAKE", joinpath(QUANT_ROOT, "lake"))
const REGISTRY = get(ENV, "QUANT_REGISTRY_PATH", joinpath("schemas", "registry.json"))

# --migration <id>: recorded as last_migration for datasets whose hash changed.
const MIGRATION = let i = findfirst(==("--migration"), ARGS)
    i === nothing ? nothing : ARGS[i + 1]
end

# (name, layout, partition keys, writer tag, SCHEMAS.md section).
# MUST match the table in SKILL.md section 3; verify-schemas.sh checks the names.
const DATASETS = [
    ("universe",          :partitioned, ["year"],            "julia-universe",   "#1"),
    ("prices",            :partitioned, ["year", "month"],   "rust-ingest",      "#2"),
    ("fundamentals",      :single,      String[],            "rust-ingest",      "#3"),
    ("macro",             :single,      String[],            "rust-ingest",      "#4"),
    ("panel",             :partitioned, ["year", "month"],   "julia-panel",      "#5"),
    ("regime_states",     :single,      String[],            "julia-regime",     "#6"),
    ("gspqr_predictions", :partitioned, ["year", "month"],   "julia-gspqr",      "#7"),
    ("tcp_calibration",   :partitioned, ["year", "month"],   "julia-tcp",        "#8"),
    ("evt_residuals",     :partitioned, ["year", "month"],   "julia-evt",        "#9"),
    ("evt_tail_bounds",   :partitioned, ["year"],            "julia-evt",        "#10"),
    ("conviction_scores", :single,      String[],            "julia-conviction", "#11"),
    ("veto_flags",        :single,      String[],            "julia-conviction", "#12"),
    ("portfolio_targets", :single,      String[],            "julia-portfolio",  "#13"),
    ("position_locks",    :single,      String[],            "rust-lock",        "#14"),
    ("pending_orders",    :single,      String[],            "rust-order",       "#15"),
    ("orders",            :single,      String[],            "rust-exec",        "#16"),
    ("positions",         :single,      String[],            "rust-exec",        "#17"),
    ("fills",             :single,      String[],            "rust-exec",        "#18"),
    ("evaluation_metrics",:single,      String[],            "julia-backtest",   "#19"),
    ("data_quality",      :single,      String[],            "rust-ingest",      "#20"),
    ("audit_log",         :partitioned, ["date"],            "rust-eventbus",    "#21"),
    ("portfolio_ledger",  :partitioned, ["year"],            "julia-ledger",     "#22"),
    ("tax_lots",          :single,      String[],            "rust-exec",        "#23"),
    ("trade_summary",     :partitioned, ["year"],            "julia-ledger",     "#24"),
    ("order_history",     :single,      String[],            "rust-audit",       "#25"),
    ("execution_costs",   :partitioned, ["year"],            "rust-exec",        "#26"),
    ("quote_spread_1m",   :partitioned, ["session"],         "rust-quotes",      "#27"),
    ("splits",            :single,      String[],            "rust-ingest",      "#28"),
    ("dividends",         :single,      String[],            "rust-ingest",      "#29"),
    ("etf_holdings",      :partitioned, ["year"],            "rust-ingest",      "#30"),
    ("security_master",   :single,      String[],            "rust-ingest",      "#31"),
    ("delistings",        :single,      String[],            "rust-ingest",      "#32"),
    ("filing_events",     :single,      String[],            "rust-ingest",      "#33"),
    ("price_coverage",    :single,      String[],            "julia-universe",   "#34"),
]

# Sample leaf: the single file, or the FIRST leaf in sorted path order of a
# partitioned dataset (verify-schemas.sh also checks the LAST leaf).
function sample_leaf(name::String, layout::Symbol)::Union{String,Nothing}
    if layout === :single
        p = joinpath(LAKE, name * ".parquet")
        return isfile(p) ? p : nothing
    end
    dir = joinpath(LAKE, name)
    isdir(dir) || return nothing
    leaves = String[]
    for (root, _, files) in walkdir(dir)
        "data.parquet" in files && push!(leaves, joinpath(root, "data.parquet"))
    end
    isempty(leaves) ? nothing : first(sort(leaves))
end

const CON = DBInterface.connect(DuckDB.DB, ":memory:")

# Canonical column strings via DuckDB (same query as verify-schemas.sh).
function canonical_columns(leaf::String)::Vector{String}
    occursin('\'', leaf) && error("single quote in path: $leaf")
    sql = "SELECT column_name || ':' || column_type AS c " *
          "FROM (DESCRIBE SELECT * FROM read_parquet('$(leaf)', hive_partitioning = false))"
    res = DBInterface.execute(CON, sql)
    String.(Tables.columntable(res).c)
end

schema_hash(cols::Vector{String})::String = bytes2hex(sha256(join(cols, "|")))

old = isfile(REGISTRY) ? JSON3.read(read(REGISTRY, String)) : nothing
old_ds(name) = old === nothing ? nothing : get(get(old, :datasets, Dict()), Symbol(name), nothing)

datasets = OrderedDict{String,Any}()
for (name, layout, keys, writer, section) in DATASETS
    leaf = sample_leaf(name, layout)
    if leaf === nothing
        @warn "Dataset not written yet, not registered: $name"
        continue
    end
    cols = canonical_columns(leaf)
    h = schema_hash(cols)
    prev = old_ds(name)
    last_mig = if prev !== nothing && prev.hash == h
        prev.last_migration
    elseif MIGRATION !== nothing
        MIGRATION
    elseif prev === nothing
        "2026-10-10-initial"            # new dataset: no migration file (MIGRATIONS.md)
    else
        error("schema of $name changed ($(prev.hash[1:12]) -> $(h[1:12])): pass --migration <id>")
    end
    datasets[name] = OrderedDict(
        "layout"         => String(layout),
        "partition_keys" => keys,
        "writer"         => writer,
        "schemas_md"     => section,
        "columns"        => cols,
        "hash"           => h,
        "last_migration" => last_mig,
    )
end

doc = OrderedDict(
    "format_version" => 1,
    "canonical_form" => "DuckDB DESCRIBE: 'column_name:column_type' lines joined by '|', SHA-256 of the UTF-8 string",
    "datasets"       => datasets,
)
mkpath(dirname(REGISTRY))
tmp = REGISTRY * ".tmp." * string(time_ns())
open(tmp, "w") do io
    JSON3.pretty(io, doc)
end
mv(tmp, REGISTRY; force = true)        # temp + rename, same directory
println("Wrote registry with $(length(datasets)) datasets to $REGISTRY")
