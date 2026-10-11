# Parser and consistency tests of schemas/generate-registry.jl on small documents (the full
# run against SCHEMAS.md, the writer and DuckDB is `generate-registry.jl --check`).
using Test

include(joinpath(@__DIR__, "..", "generate-registry.jl"))

const SKILL = """
| # | Dataset | Layout | Written by | Update |
|---|---|---|---|---|
| 1 | `prices` (raw) | P(`year`,`month`) | Rust | Daily |
| 2 | `splits` | S | Rust | Daily |
"""

section(n, name, rows, pk) = """
## $n. `$name`

| Column | Type | Nullable | Notes |
|---|---|---|---|
$rows
$pk
"""

@testset "generate-registry" begin
    l = parse_layouts(SKILL)
    @test l == [(number = 1, name = "prices", partition_keys = ["year", "month"]),
                (number = 2, name = "splits", partition_keys = String[])]

    s = parse_schemas(section(2, "splits.parquet", "| `ticker` | `utf8` | no | |\n| `r` | `float64` | yes | x |",
                              "Primary key: `(ticker, r)`. Prose."))
    @test s["splits"].fields == [(name = "ticker", type = "utf8", nullable = false),
                                 (name = "r", type = "float64", nullable = true)]
    @test s["splits"].primary_key == ["ticker", "r"]
    @test parse_schemas(section(3, "x", "| `a` | `utf8` | no | |", "Primary key: `a`."))["x"].primary_key == ["a"]
    @test parse_schemas(section(3, "x", "| `a` | `utf8` | no | |", ""))["x"].primary_key === nothing

    # The committed documents parse into all 35 entries without error.
    layouts = parse_layouts(read(joinpath(SKILL_DIR, "SKILL.md"), String))
    full = parse_schemas(read(joinpath(SKILL_DIR, "SCHEMAS.md"), String))
    entries = stage_entries(layouts, full)
    @test length(entries) == 35 == length(DATASETS)
    @test [e["number"] for (_, e) in entries] == 1:35
    @test Dict(entries)["audit_log"]["sort_key"] == ["timestamp", "event_id"]

    # A type outside SKILL.md §6 fails.
    bad = parse_schemas(section(2, "splits", "| `t` | `float32` | no | |", "Primary key: `t`."))
    err = try stage_entries(layouts, merge(full, Dict("splits" => (bad["splits"]..., number = 28)))); nothing catch e; e end
    @test occursin("float32` not allowed", sprint(showerror, err))
    @test schema_hash(["a:VARCHAR", "b:DATE"]) == bytes2hex(sha256("a:VARCHAR|b:DATE"))
end
