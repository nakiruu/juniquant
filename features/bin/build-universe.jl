# J9: rebuild `universe` and `price_coverage` (UNIVERSE.md §8). Env: QUANT_ROOT, QUANT_STATE,
# QUANT_CONFIG, QUANT_REGISTRY_PATH, LAKE_IO_BIN (or lake-io on PATH).
using Features
build_universe()
