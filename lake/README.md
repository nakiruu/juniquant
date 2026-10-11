# lake

The one implementation of the Parquet lake I/O contract, shared by `ingest` and (through the
`lake-io` binary) Julia. Specification:
[13-parquet-lake](../.claude/skills/13-parquet-lake/SKILL.md) and
[`IO-CONTRACT.md`](../.claude/skills/13-parquet-lake/IO-CONTRACT.md).

Every dataset's columns, keys, layout and merge rule come from
[`schemas/registry.json`](../schemas/registry.json) (35 datasets); nothing is hard-coded per
dataset. The library takes paths as arguments; only `Lake::from_env` and the binaries read
`QUANT_ROOT`, `QUANT_STATE` and `QUANT_REGISTRY_PATH` (no silent default to a production path).

## API

| Item | Purpose |
|---|---|
| `Lake::new`, `Lake::from_env` | A lake at `$QUANT_ROOT/lake` with its registry and single-writer locks in `$QUANT_STATE/locks` |
| `write_partition`, `write_dataset` | Write one leaf, or split an input by partition. Temporary dot-file, fsync, rename; partition leaves are written in parallel |
| `read_dataset` | Read with optional columns, key filters and a `from`/`to` date filter |
| `Mode::Append` / `Mode::Replace` | Merge on the primary key by the dataset's rule (including `append_only`, DECISIONS D14), or rebuild the partition from the input alone |
| `write_file`, `read_file` | The same atomic Parquet settings (ZSTD-3) for files outside the lake, such as ingest stage files |

## Binaries

| Binary | Purpose |
|---|---|
| `lake-io` | The Julia bridge: `write` and `read` over Arrow IPC stream files, so the contract is implemented once ([12-julia-rust-bridge](../.claude/skills/12-julia-rust-bridge/SKILL.md)). Build with `cargo build --release -p lake` |
| `lake-schema-probe` | Writes one empty leaf per dataset through the real writer; the registry generator hashes its `DESCRIBE` output |

## Tests

`cargo test -p lake` runs `tests/io_contract.rs`: schema checks, merge rules, idempotent
re-runs, interrupted writes, stale temp cleanup and date-filtered reads.
`make schemas-check` verifies that the committed registry equals the one regenerated from
`SCHEMAS.md` and the writer.
