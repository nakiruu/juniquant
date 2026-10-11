# Lake I/O Contract

Defined once. Every writer and reader of the lake, in every language and in
every script, follows this file. Where another skill or a script disagrees,
this file wins (the constitution wins over this file).

## 1. Paths and Environment

Three environment variables locate everything. Code never hard-codes
`/mnt/cache/...` or `/data/...`; a script that finds one unset exits with a
message naming it (no silent default to a production path).

| Variable | Meaning | Container value | Host value (Unraid) | Mount mode |
|---|---|---|---|---|
| `QUANT_ROOT` | Data root: contains `lake/`, `raw/`, `stage/`, `reports/`, `models/` | `/data` | `/mnt/cache/quant` | rw (`quant-core`), ro (`quant-api`, `duckdb`) |
| `QUANT_STATE` | Mutable service state: `audit.db`, `audit-spool/`, `ingest/manifest.db`, `locks/`, `logs/`, and the lock and execution state files listed below | `/state` | `/mnt/cache/appdata/quant` | rw (`quant-core`) |
| `QUANT_CONFIG` | The repository's `config/` directory | `/config` | the checked-out repo's `config/` | **ro** |

Derived locations (computed in one helper per language, never re-spelled):

    LAKE      = $QUANT_ROOT/lake
    RAW       = $QUANT_ROOT/raw          # immutable landing zone (01 BACKFILL.md)
    STAGE     = $QUANT_ROOT/stage        # parsed per-chunk intermediates, rebuildable
    REPORTS   = $QUANT_ROOT/reports      # coverage / caveat reports
    MODELS    = $QUANT_ROOT/models

State files sit **directly** under `QUANT_STATE`; there is no `state/`
subdirectory (an earlier host spelling `/mnt/cache/appdata/quant/state/...`
would be `/state/state/...` in the container and is wrong):

    $QUANT_STATE/audit.db                       # + -wal / -shm (11-rust-event-bus)
    $QUANT_STATE/audit-spool/
    $QUANT_STATE/ingest/manifest.db             # 01-data-ingestion/BACKFILL.md
    $QUANT_STATE/locks/<name>.lock              # §3 single-writer locks
    $QUANT_STATE/lock_external.csv              # 09-lock-state-machine
    $QUANT_STATE/lock_snapshot.json             # 09 §8a
    $QUANT_STATE/TRADING_HALTED                 # 09 §8b
    $QUANT_STATE/acknowledged_violations.json   # 09 §8b
    $QUANT_STATE/nyse_calendar_overrides.toml   # runtime overrides, 09 §4
    $QUANT_STATE/compliance/                    # dated compliance reports
    $QUANT_STATE/schwab_refresh_token           # 10-execution-schwab/OAUTH.md
    $QUANT_STATE/logs/

The `config/` files (`restricted.toml`, `symbol_map.toml`,
`fractional_eligible.toml`, the seed `nyse_calendar_overrides.toml`,
`quality_gates.toml`, `macro_series.toml`, `identifier_overrides.csv`,
optional `price_sources.toml`) are read at `$QUANT_CONFIG/<file>`.

Note for other skills: `QUANT_ROOT` is the mount root, so the lake is
`$QUANT_ROOT/lake`. A script that wrote `QUANT_ROOT/prices` meaning the lake
must be changed to `QUANT_ROOT/lake/prices`.

The registry is the repository file `schemas/registry.json`. Scripts take its
path from the script-specific variable `QUANT_REGISTRY_PATH`, defaulting to
the repository's `schemas/registry.json`. Inside the image the deployment sets
it to the bundled copy (`/app/schemas/registry.json`, deployment choice,
verify at image build time).

**Script-specific overrides.** A script that needs an input or output path
other than the derived ones takes a command-line argument or a variable named
after the script (`VALIDATE_PANEL_INPUT`, `VERIFY_SCHEMAS_LAKE`,
`GENERATE_REGISTRY_LAKE`, `VALIDATE_FILINGS_AS_OF`; `validate-filings.jl`
takes its input path as an argument). Generic names such as `OUTPUT_PATH`, `INPUT_PATH`,
`LAKE_ROOT` are not used: they collide across scripts.

Julia scripts run inside a committed environment: `julia --project=<dir>
--startup-file=no script.jl` where `<dir>` holds a `Project.toml` that is a
member of the root Julia workspace, pinned by the single root
`Manifest.toml`. No script calls `Pkg.add` or `Pkg.activate(temp=true)` at
run time (reproducibility). Validation and registry tooling run in
`schemas/`; the panel validator runs in `features/` (both directories are
named in `scaffold.md`; `schemas/Project.toml` is an addition, with no
`Manifest.toml` of its own).

## 2. Layout

- **Single dataset:** `$LAKE/<name>.parquet`.
- **Partitioned dataset:** `$LAKE/<name>/<k1>=<v1>/<k2>=<v2>/data.parquet`.
  Every leaf partition is a directory containing exactly one file named
  `data.parquet`. This replaces the earlier mixture of `data.parquet`,
  `part.parquet` and `part-YYYY-MM-DD.parquet`.
- Partition keys and value formats: `year=YYYY`, `month=MM` (zero-padded),
  `date=YYYY-MM-DD` (audit_log only), `session=YYYY-MM-DD` (quote_spread_1m
  only). The key list per dataset is the table in `SKILL.md` §3.
- Partition keys are directory names only and never appear as stored columns;
  a key name never equals a stored column name.
- Temporary files are dot-files in the same directory: `.data.parquet.tmp.<pid>.<unix_ns>`
  (or `.<name>.parquet.tmp.<pid>.<unix_ns>` for a single dataset). Readers glob
  the exact name `data.parquet` (or `<name>.parquet`), so they never see a
  temporary file.
- Registry key: the dataset name without `.parquet`.

## 3. Write Semantics

`write_partition(name, partition, df)` writes one leaf partition. In Julia,
Rust and any future language it does exactly this:

1. **Validate.** `df` has exactly the registered columns, in order, with the
   registered types; the primary key (from `SCHEMAS.md`) is unique in `df`;
   every row belongs to `partition` (for example every `date` is in the given
   `year`/`month`). Any violation raises; nothing is written.
2. **Merge (append semantics).** There is no append to a Parquet file. If the
   mode is `append`, the writer reads the existing leaf (if any), unions it
   with `df`, and resolves primary-key collisions by the dataset's rule:
   - *overwrite* datasets (derived or re-fetched data such as `prices`,
     `panel`, `universe`): the new row wins;
   - *append-only* datasets (`audit_log`, `order_history`, `execution_costs`,
     `quote_spread_1m`, `regime_states`, `gspqr_predictions`): a colliding
     row with different content is an error, an identical row is dropped.
     `portfolio_ledger` is *overwrite* on its key, so a same-day re-run
     replaces that day's row (constitution §8).
   `mode = replace` skips the read and writes `df` alone (used for rebuilds).
   Key comparison treats a null as an ordinary value equal to another null
   (so `evaluation_metrics` legacy rows, whose run columns are null, stay
   unique on `(eval_date, horizon)`). A dataset with no primary key
   (`audit_log`) is `append_only` over whole rows. The primary key, sort key
   and merge rule come from `schemas/registry.json` (`SKILL.md` §6).
3. **Sort** by the registry `sort_key` (the primary key; `timestamp, event_id` for `audit_log`), a deterministic row order.
4. **Write** to the temporary path in the same directory with ZSTD level 3 and
   a fixed row-group size; `fsync` the file.
5. **Rename** the temporary file onto `data.parquet` (`rename(2)` within one
   filesystem replaces atomically); `fsync` the directory.
6. On any failure, delete the temporary file, leave the existing partition
   untouched, return an error.

`write_dataset(name, df)` splits `df` by the partition key and calls
`write_partition` per group; a failure in one group does not undo groups
already renamed (each partition is individually atomic; the dataset as a
whole is not). A run that fails midway is re-run: step 2 and the primary key
make re-running safe.

A single dataset (`S`) is written the same way: one temporary file, one
rename. "Append" to a single dataset rewrites the whole file.

**Stale temporaries.** At the start of a run, the writer deletes dot-files
matching the temporary pattern that are older than one hour.

**Single writer.** A partition has one writer module (`SKILL.md` §3). The
writer holds an advisory lock file `$QUANT_STATE/locks/<name>.lock` for the
duration of a write run; a second concurrent run exits with an error.

## 4. Idempotency

Per partition, writing the same input twice produces the same dataset:

- the same rows in the same order (sorted by primary key);
- the same content hash: `SELECT hash(...)` over the ordered rows, or the
  SHA-256 of the file bytes where the writer is deterministic (verify for
  each Parquet library; byte identity is a goal, content identity is the
  tested requirement);
- no duplicate primary keys, ever.

A writer test runs the write twice and asserts equal content hashes and equal
row counts.

## 5. Read Semantics

`read_dataset(name, columns, filters)` reads
`$LAKE/<name>.parquet` or `$LAKE/<name>/**/data.parquet` with hive
partitioning enabled, selecting columns and pushing down filters on the
partition keys and on `date` (the partition column, else `date`: leaves outside the range
are not opened, rows inside a leaf are filtered while decoding).

- Never read a leaf by hand-built path in production code; use the glob and
  let the reader prune.
- Each leaf is replaced atomically, so a reader sees an old or a new version
  of a partition, never a torn one. Across partitions there is **no snapshot
  isolation**: a reader running during a write run may see partition A new
  and partition B old. Jobs that need a consistent view (the panel build,
  training) run only after the ingest run they depend on has finished (the
  ordering in `01-data-ingestion/BACKFILL.md` §8).
- Readers never write to the lake. `quant-api` and `duckdb` containers mount
  read-only.

## 6. Scripts Must Follow This Contract

Every script that touches the lake (registry generator, verifier,
validators, panel builder, backfill drivers):

- obtains paths from `QUANT_ROOT`, `QUANT_STATE`, `QUANT_CONFIG` (§1);
- writes only through `write_partition`/`write_dataset`;
- globs `data.parquet` (or the single-file name) and ignores dot-files;
- treats `audit_log` as a dataset (`audit_log/date=*/data.parquet`), never as
  a file `audit_log.parquet`.

## 7. Differences From the Earlier Text (for the writers of other skills)

| Earlier | Now |
|---|---|
| audit export leaf `audit_log/date=YYYY-MM-DD/part.parquet` | `.../data.parquet` (`11-rust-event-bus`, `10-execution-schwab` exporters and readers must change) |
| `quote_spread_1m/year=/month=/part-YYYY-MM-DD.parquet` | `quote_spread_1m/session=YYYY-MM-DD/data.parquet` (`10-execution-schwab/EXECUTION-WINDOW.md` §2.1 recorder and readers) |
| `LAKE_ROOT`, `REGISTRY`, `OUTPUT_PATH` environment variables | `QUANT_ROOT`, `QUANT_STATE`, `QUANT_CONFIG`; script-specific names |
| `/mnt/cache/quant/schemas/registry.json` | repository `schemas/registry.json` |
| `/mnt/cache/appdata/quant/state/<file>` (host) | `$QUANT_STATE/<file>` (no `state/` segment, §1) |
| script defaults `/mnt/cache/quant/lake/<name>.parquet` | `$QUANT_ROOT/lake/<name>` from the environment; unset variable = error |
