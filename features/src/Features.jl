"""
    Features

Julia universe builder (J9, `01-data-ingestion/UNIVERSE.md` §8): `universe` (#1) and
`price_coverage` (#34) from `prices`, `security_master`, `etf_holdings`, `fundamentals`,
`splits`, `delistings` and the G2 rejections in `data_quality`; and the panel build's stage 0,
`preflight()` (the gates' verdict, gate G4, the survivorship caveat). Panel construction joins
this package later.

Run: `julia --project=features features/bin/build-universe.jl` (or `bin/preflight.jl`) with
`QUANT_ROOT`, `QUANT_STATE`, `QUANT_CONFIG` and `QUANT_REGISTRY_PATH` set and `lake-io` built.
"""
module Features

using Arrow
using Dates
using JSON3
using TOML
using LakeIO
using NYSECalendar

export universe_rows, universe_columns, coverage_table, member_days, sic_sector, load_sic_ranges,
    g2_rejected, build_universe, gate_verdict, g4, preflight

include("universe.jl")
include("coverage.jl")
include("build.jl")
include("preflight.jl")

end # module
