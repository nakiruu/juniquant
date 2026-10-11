"""
    LakeIO

Julia side of the lake I/O contract (`13-parquet-lake/IO-CONTRACT.md`). The write is done by
the Rust writer: the table goes to the `lake-io` binary as an Arrow IPC stream file, so validation,
merge, sort and the atomic rename exist once (`12-julia-rust-bridge` §0). Paths come from
`QUANT_ROOT`, `QUANT_STATE` and `QUANT_REGISTRY_PATH`, read by the binary.

The binary is `ENV["LAKE_IO_BIN"]`, else `lake-io` on `PATH` (`cargo build --release -p lake`).
"""
module LakeIO

using Arrow

export write_dataset, write_partition, read_dataset

lake_io() = get(ENV, "LAKE_IO_BIN", "lake-io")

function run_lake_io(args)
    err = IOBuffer()
    ok = success(pipeline(`$(lake_io()) $args`; stderr = err))
    ok || error(strip(String(take!(err))))
    nothing
end

function write_ipc(f, table)
    mktempdir() do dir
        path = joinpath(dir, "in.arrow")
        Arrow.write(path, table; file = false)  # IPC stream, as lake-io reads and writes
        f(path)
    end
end

"""
    write_dataset(name, table; mode = :append)

Write a Tables.jl `table` to dataset `name`, one atomic leaf per partition it touches.
`mode` is `:append` (merge on the primary key by the dataset's rule) or `:replace`.
Columns must be the registered ones, in order; `Int` columns are narrowed to the registered
width (overflow is an error) and `DateTime` columns are taken as UTC.

# Arguments
- `name::AbstractString`: registry key, e.g. `"panel"`.
- `table`: any Tables.jl table (a `NamedTuple` of vectors works).
- `mode::Symbol`: `:append` or `:replace`.

# Returns
`nothing`; throws `ErrorException` with the writer's message on any failure (nothing is
written to a partition that fails).

# Example
```julia
write_dataset("regime_states", (date = [Date(2024, 1, 2)], ...))
```
"""
write_dataset(name::AbstractString, table; mode::Symbol = :append) =
    write_partition(name, String[], table; mode)

"""
    write_partition(name, partition, table; mode = :append)

As [`write_dataset`](@ref) for one leaf: `partition` holds the partition-key values in
order, e.g. `["2024", "01"]` for `year`/`month`. Every row must belong to it.

# Returns
`nothing`; throws on failure.

# Example
```julia
write_partition("prices", ["2024", "01"], prices_jan_2024)
```
"""
function write_partition(name::AbstractString, partition::AbstractVector{<:AbstractString}, table;
                         mode::Symbol = :append)
    mode in (:append, :replace) || throw(ArgumentError("mode must be :append or :replace"))
    write_ipc(table) do path
        run_lake_io(["write", name, path, String(mode), partition...])
    end
end

"""
    read_dataset(name; columns = nothing, partition = Pair{String,String}[], dates = nothing) -> Arrow.Table

Read dataset `name`: the leaves matching every `key => value` in `partition`, projected to
`columns` (all when `nothing`). `dates = (from, to)` (two `Date`s, inclusive) keeps rows whose
date column (the partition column, else `date`) is in range; other leaves are not opened. Partition keys are not columns. An unwritten dataset reads
as an empty table.

# Returns
An `Arrow.Table` (a Tables.jl table; `DataFrame(t)` converts it).

# Example
```julia
t = read_dataset("prices"; columns = ["ticker", "close"], partition = ["year" => "2024"])
```
"""
function read_dataset(name::AbstractString; columns = nothing,
                      partition::AbstractVector{<:Pair} = Pair{String,String}[], dates = nothing)
    mktempdir() do dir
        path = joinpath(dir, "out.arrow")
        args = ["read", name, path]
        columns === nothing || push!(args, "columns=" * join(columns, ","))
        dates === nothing || append!(args, ["from=$(dates[1])", "to=$(dates[2])"])
        append!(args, ["$k=$v" for (k, v) in partition])
        run_lake_io(args)
        Arrow.Table(read(path))  # bytes, not mmap: the file is deleted on return
    end
end

end # module
