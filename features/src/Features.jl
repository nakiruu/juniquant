"""
    Features

Julia universe builder (J9, `01-data-ingestion/UNIVERSE.md` §8): `universe` (#1) and
`price_coverage` (#34) from `prices`, `security_master`, `etf_holdings`, `fundamentals`,
`splits` and `delistings`. Panel construction joins this package later.

Run: `julia --project=features features/bin/build-universe.jl` with `QUANT_ROOT`,
`QUANT_STATE`, `QUANT_CONFIG` and `QUANT_REGISTRY_PATH` set and `lake-io` built.
"""
module Features

using Arrow
using Dates
using TOML
using LakeIO
using NYSECalendar

export universe_rows, universe_columns, coverage_table, sic_sector, load_sic_ranges,
    build_universe

include("universe.jl")
include("coverage.jl")
include("build.jl")

end # module
