# LakeIO against the real writer and registry: round trip with Julia's natural types
# (Int64, DateTime, missing), merge on re-write, errors surface, and the written leaf has the
# registry's DuckDB DESCRIBE hash (schemas/verify-schemas.sh).
using LakeIO
using Dates
using Test

const REPO = normpath(joinpath(@__DIR__, "..", "..", ".."))

function with_lake(f)
    mktempdir() do dir
        withenv("QUANT_ROOT" => dir, "QUANT_STATE" => joinpath(dir, "state"),
                "QUANT_REGISTRY_PATH" => joinpath(REPO, "schemas", "registry.json")) do
            f(dir)
        end
    end
end

prices(tickers, closes) = (
    figi = ["F" * t for t in tickers],
    ticker = tickers, date = fill(Date(2024, 1, 2), length(tickers)),
    open = closes, high = closes, low = closes, close = closes,
    volume = fill(100, length(tickers)),
    adj_close_vendor = Union{Missing,Float64}[missing for _ in tickers],
    source = fill("yahoo", length(tickers)),
    fetched_at = fill(DateTime(2024, 1, 2, 21), length(tickers)),
    raw_sha256 = fill("ab", length(tickers)),
)

@testset "LakeIO" begin
    run(Cmd(`cargo build --quiet --release -p lake --bin lake-io`; dir = REPO))
    ENV["LAKE_IO_BIN"] = joinpath(REPO, "target", "release", "lake-io")

    with_lake() do dir
        write_dataset("prices", prices(["MSFT", "AAPL"], [2.0, 1.0]))
        @test isfile(joinpath(dir, "lake", "prices", "year=2024", "month=01", "data.parquet"))
        t = read_dataset("prices")
        @test collect(t.ticker) == ["AAPL", "MSFT"]          # sorted by primary key
        @test all(ismissing, t.adj_close_vendor)
        @test eltype(t.volume) == Int64

        write_partition("prices", ["2024", "01"], prices(["AAPL"], [9.0]))
        t = read_dataset("prices"; columns = ["close", "ticker"], partition = ["month" => "01"])
        @test collect(t.close) == [9.0, 2.0]                 # new row wins, MSFT kept
        @test collect(propertynames(t)) == [:close, :ticker]
        @test length(read_dataset("prices"; partition = ["month" => "02"]).ticker) == 0
        @test length(read_dataset("prices"; dates = (Date(2024, 1, 3), Date(2024, 12, 31))).ticker) == 0
        @test length(read_dataset("prices"; dates = (Date(2024, 1, 2), Date(2024, 1, 2))).ticker) == 2

        @test_throws ErrorException write_dataset("prices", (ticker = ["X"],))
        @test_throws ArgumentError write_dataset("prices", prices(["A"], [1.0]); mode = :upsert)

        verify = joinpath(REPO, "schemas", "verify-schemas.sh")
        @test success(Cmd(`bash $verify`; env = ("VERIFY_SCHEMAS_LAKE" => joinpath(dir, "lake"),
                                                "PATH" => ENV["PATH"])))
    end
end
