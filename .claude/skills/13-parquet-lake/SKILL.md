---
name: parquet-lake
description: The data lake. Every Parquet dataset in the system, its schema,
  partitioning, I/O contract and query patterns. Load whenever touching any
  .parquet file, DuckDB query, schema definition, or migration.
---

# Parquet Lake

The lake is 35 datasets. Their schemas are frozen once written (constitution
§5). This skill is the authoritative definition of each dataset's layout; the
column lists are in `SCHEMAS.md`; the read/write semantics are in
`IO-CONTRACT.md`.

"Dataset" and "file" are used interchangeably in older text: a reference to
`prices.parquet` anywhere in the skills means the dataset `prices`, whose
physical form (single file or hive directory) is given in §3.

## 1. Storage Principles

- **Format:** Apache Parquet, ZSTD compression, level 3.
- **Layout:** a dataset is either a single file `lake/<name>.parquet` or a
  hive-partitioned directory `lake/<name>/<key>=<value>/.../data.parquet`.
  Every leaf partition holds exactly one file named `data.parquet`
  (`IO-CONTRACT.md` §2).
- **Naming:** `snake_case`. No version suffixes in the name.
- **Location:** all live data on the cache pool, never on the array.
  Paths are built from `QUANT_ROOT` (`IO-CONTRACT.md` §1), never hard-coded.
- **Ownership:** written by exactly one module. Read by many. The writer is
  in the table below and in `SCHEMAS.md`.
- **Pre-first-write status.** No dataset has been written yet (this is a
  specification). Until the first write of a dataset, its entry in
  `SCHEMAS.md` may still be corrected in place; those corrections are
  recorded in `migrations/2026-10-10-initial.md`, not as separate
  migrations. From the first write, §2 applies without exception.

## 2. Schema Immutability

Once a schema is committed and used, it is frozen. The rules:

- **Additive only.** New columns append to the right. No insertion.
- **No renames.** A rename is a new column plus a deprecation of the old.
- **No retypes.** Changing `float32` to `float64` requires a migration.
- **No reorders.** Column order is part of the schema hash.

Every dataset has a SHA-256 schema hash in `schemas/registry.json` (the
repository copy; §6). A CI job and the daily `verify-schemas.sh` check that
each dataset's schema matches its registered hash. Mismatch without a
migration file is a build failure.

See `MIGRATIONS.md` for the additive change procedure.

## 3. The 35 Datasets (layout, writer, cadence)

This is the single table of layouts. `SCHEMAS.md`, `UNRAID-STORAGE.md` and
`scripts/generate-registry.jl` must agree with it; `verify-schemas.sh` fails
if the registry and this table disagree on a name.

Legend. Layout `S` = single file `lake/<name>.parquet`. `P(keys)` = hive
directory `lake/<name>/<keys>/data.parquet`, partition keys are directory
names only, never stored columns.

| # | Dataset | Layout | Written by | Update |
|---|---|---|---|---|
| 1 | `universe` (derived daily membership and eligibility) | P(`year`) | Julia (universe builder) | Daily, after prices |
| 2 | `prices` (raw as-traded OHLCV) | P(`year`,`month`) | Rust (ingest) | Daily |
| 3 | `fundamentals` | S | Rust (ingest) | Event (filing) |
| 4 | `macro` (long form, ALFRED vintages) | S | Rust (ingest) | Daily |
| 5 | `panel` | P(`year`,`month`) | Julia (feature engine) | Daily |
| 6 | `regime_states` | S | Julia (regime) | Daily |
| 7 | `gspqr_predictions` | P(`year`,`month`) | Julia (GSPQR) | Daily |
| 8 | `tcp_calibration` | P(`year`,`month`) | Julia (TCP) | Daily |
| 9 | `evt_residuals` | P(`year`,`month`) | Julia (EVT) | Daily |
| 10 | `evt_tail_bounds` | P(year) | Julia (EVT) | Daily |
| 11 | `conviction_scores` | S | Julia (conviction) | Event + daily |
| 12 | `veto_flags` | S | Julia (conviction) | Event + daily |
| 13 | `portfolio_targets` | S | Julia (portfolio) | Event-driven |
| 14 | `position_locks` (derived view, **not authoritative**) | S | Rust (lock monitor) | Daily / on history refresh |
| 15 | `pending_orders` | S | Rust (order manager) | Event-driven |
| 16 | `orders` | S | Rust (execution) | Event-driven |
| 17 | `positions` | S | Rust (execution) | Daily |
| 18 | `fills` | S | Rust (execution) | Event-driven |
| 19 | `evaluation_metrics` | S | Julia (backtest) | Daily |
| 20 | `data_quality` | S | Rust (ingest) | Per run |
| 21 | `audit_log` (dataset; journal `audit.db` is the system of record) | P(`date`) | Rust (event bus) | Nightly export of closed days |
| 22 | `portfolio_ledger` | P(`year`) | Julia (ledger) | Daily |
| 23 | `tax_lots` | S | Rust (execution) | Event-driven |
| 24 | `trade_summary` | P(`year`) | Julia (ledger) | Event-driven (on close) |
| 25 | `order_history` (evidence snapshot of the audited order history) | S | Rust (compliance audit) | Daily, append-only |
| 26 | `execution_costs` (per-name execution cost estimates) | P(`year`) | Rust (execution) | Daily, append-only |
| 27 | `quote_spread_1m` (1-minute quoted-spread history, built forward) | P(`session`) | Rust (quote recorder) | Daily, written after the close |
| 28 | `splits` (split events) | S | Rust (ingest) | Daily |
| 29 | `dividends` (cash dividend events) | S | Rust (ingest) | Daily |
| 30 | `etf_holdings` (IWF/IWP membership snapshots) | P(`year`) | Rust (ingest) | Monthly at minimum; backfilled |
| 31 | `security_master` (identifier master) | S | Rust (ingest) | Weekly + on new tickers |
| 32 | `delistings` (EDGAR-derived delisting events) | S | Rust (ingest) | Daily |
| 33 | `filing_events` (8-K items, Form 25/15) | S | Rust (ingest) | Daily |
| 34 | `price_coverage` (survivorship coverage, per ticker) | S | Julia (universe builder) | Daily, after the universe |
| 35 | `listings` (listed securities per date, D12) | P(`year`) | Rust (ingest) | Daily |

Numbers #1-#27 are unchanged from the earlier inventory (other skills cite
them). #28-#35 are appended. Never renumber.

**Single-writer rule.** No dataset has more than one writer. If two modules
need to write the same data, they write two datasets. This eliminates write
contention and makes the data lineage unambiguous. Corollary used here:
`etf_holdings` is Rust (it is a fetched source); `universe` is Julia (it is
an analytic derived from holdings, prices and the identifier master); there
is no second universe writer.

**Not in the lake.** The ingestion manifest (`manifest.db`, SQLite) lives in
`QUANT_STATE`; raw landing files in `QUANT_ROOT/raw/`; staging in
`QUANT_ROOT/stage/`; reports in `QUANT_ROOT/reports/`. None of these is
registered or schema-hashed; the raw files are the replay source
(`01-data-ingestion/BACKFILL.md`).

## 4. Partitioning

Partitioning is by the layout column in §3. Rationale and sizing:

| Dataset | Keys | Rationale (row counts are estimates for 15+ years) |
|---|---|---|
| `prices` | `year`,`month` | ~8-10M rows total (about 2,500 tickers, each from its first listing or the backfill start), ~40-60K rows per month |
| `panel` | `year`,`month` | ~2.3M rows (about 600 eligible names x ~3,800 days) |
| `gspqr_predictions`, `tcp_calibration`, `evt_residuals` | `year`,`month` | ~7M rows each (panel rows x 3 horizons) |
| `universe` | `year` | ~3M rows (all snapshot members per day, eligible or not) |
| `etf_holdings` | `year` | ~0.1M rows at monthly snapshots; up to ~3M if daily |
| `audit_log` | `date` | One file per closed UTC day, exported from the SQLite journal; never rewritten |
| `quote_spread_1m` | `session` | ~59M rows/year (estimate), one directory per ET session date; the partition key is `session`, not `date`, because the stored `date` column would collide with a `date=` directory under hive reading |
| `portfolio_ledger`, `trade_summary`, `execution_costs` | `year` | Per `SCHEMAS.md` #22, #24, #26 |
| Everything else | none | < 1M rows; rewrite in full per update |

Partitioning is Hive-style. DuckDB and Polars read it natively. Do not
partition on `ticker`; the universe changes over time and the resulting
directory churn is not worth it. A partition key name must never equal a
stored column name.

## 5. Reading and Writing

All writers and readers follow `IO-CONTRACT.md`. In short:

- One function pair per language: `read_dataset(name, ...)` and
  `write_partition(name, partition, df)`. Scripts must use them; no script
  opens a `.parquet` path directly for writing.
- Every write is temp file in the same directory, `fsync`, atomic rename.
- "Append" to a partition means rewrite that partition (read, merge on the
  primary key, write).

**Julia:** read with `Parquet2` (or DuckDB.jl); write through the shared
writer, which uses `Parquet2`/`Arrow` (API names: verify). Never CSV as an
intermediate format. Arrow IPC or Parquet only.

**Rust:** write with `parquet::arrow::ArrowWriter`, `Compression::ZSTD(3)`;
read with `ParquetRecordBatchReaderBuilder`. Use `arrow` for the file I/O
boundary. Never hold a file open across an async boundary.

**Bun/DuckDB:** read-only. All queries via `@nshiab/simple-data-analysis-core`.
Never write to the lake from Bun.

## 6. Schema Registry

`schemas/registry.json` in the **repository** (top-level `schemas/`, as in
`scaffold.md`) is authoritative. It is committed, reviewed and bundled into
the container image; it is not stored in the lake. Each dataset entry holds
the readable column list and its hash:

```json
{
  "format_version": 2,
  "canonical_form": "DuckDB DESCRIBE: 'column_name:column_type' lines joined by '|', SHA-256 of the UTF-8 string",
  "datasets": {
    "prices": {
      "number": 2,
      "layout": "partitioned",
      "partition_keys": ["year", "month"],
      "partition_column": "date",
      "writer": "rust-ingest",
      "schemas_md": "#2",
      "fields": [
        { "name": "ticker", "type": "utf8",   "nullable": false },
        { "name": "date",   "type": "date32", "nullable": false },
        { "name": "...",    "type": "...",    "nullable": true }
      ],
      "primary_key": ["ticker", "date"],
      "sort_key": ["ticker", "date"],
      "merge": "overwrite",
      "columns": ["ticker:VARCHAR", "date:DATE", "..."],
      "hash": "a3f9...",
      "last_migration": "2026-10-10-initial"
    }
  }
}
```

The key is the dataset name without `.parquet` (so `audit_log`, not
`audit_log.parquet`; the audit log is a dataset, not a file). Datasets are
listed in `number` order. Every dataset in §3 is registered from the start,
before its first real write (the hash comes from a probe leaf, below).

**Fields of an entry (format_version 2).**

| Field | Meaning | Source |
|---|---|---|
| `number` | The `#` in §3; never reused | §3 |
| `layout` | `single` or `partitioned` | §3 |
| `partition_keys` | Directory keys in order (`year`, `month`, `date`, `session`); `[]` for `single` | §3 |
| `partition_column` | The stored column whose value places a row in a leaf (`year=`/`month=` from a `date32`; `date=` from the UTC date of a `timestamp`; `session=` from a `date32`); `null` for `single` | Generator table, checked against `SCHEMAS.md` |
| `writer` | Writer tag (one module, §3) | Generator table |
| `schemas_md` | `#<number>` section in `SCHEMAS.md` | `SCHEMAS.md` |
| `fields` | Every stored column in order: `name`, Arrow `type` (one of `utf8`, `bool`, `int16`, `int32`, `int64`, `float64`, `date32`, `timestamp[ns, UTC]`), `nullable` | `SCHEMAS.md` column table |
| `primary_key` | Primary-key columns in order; `[]` when the dataset has none (`audit_log`) | `SCHEMAS.md` "Primary key:" line |
| `sort_key` | Row order written to every leaf: the primary key, or for a dataset without one the generator table's key (`audit_log`: `timestamp`, `event_id`) | Derived |
| `merge` | `overwrite` (new row wins on a key collision) or `append_only` (a differing row is an error, an identical row is dropped), `IO-CONTRACT.md` §3 | Generator table |
| `columns`, `hash` | DuckDB `DESCRIBE` of a leaf written by the real Rust writer, and its SHA-256 (canonical form above) | Probe leaf |
| `last_migration` | Last migration that changed the entry | Generator |

`fields` is what writers validate against (`IO-CONTRACT.md` §3 step 1);
`columns`/`hash` are what the verifier compares lake files against. The
generator derives `fields`, `primary_key` and `schemas_md` by parsing
`SCHEMAS.md` (the four-column `Column | Type | Nullable | Notes` tables and the
`Primary key:` lines), so the registry cannot drift from this skill. It then
writes one empty leaf per dataset through the Rust writer
(`lake-schema-probe`) into a temporary lake and takes `columns`/`hash` from
DuckDB `DESCRIBE` of those leaves. A type in `SCHEMAS.md` outside the list
above, a section without a primary-key line (other than a documented
exception), or a `DESCRIBE` type that disagrees with the expected mapping in
`SCHEMAS.md` fails the generator.

**One canonical form.** The generator (`scripts/generate-registry.jl`) and
the verifier (`scripts/verify-schemas.sh`) both compute the hash from the
same string: DuckDB's `DESCRIBE SELECT * FROM read_parquet('<one leaf file>',
hive_partitioning = false)`, each row rendered `column_name:column_type`, the
rows joined by `|` in column order, SHA-256 of that UTF-8 string. Partition
keys are never in the string because they are not stored columns.

Update procedure when changing a schema is in `MIGRATIONS.md` (one commit:
writer, migration file, `SCHEMAS.md`, registry).

## 7. Query Patterns

See `QUERY-PATTERNS.md`. The most common:

- **Latest prediction for a ticker:** filter `date == max(date)` on
  `gspqr_predictions`, then join to `tcp_calibration` and `evt_tail_bounds` on
  `(ticker, date, horizon)`.
- **Lock status for a ticker (display only):** read `position_locks`, filter
  by ticker. This view is derived and NOT authoritative: the real answer is
  recomputed from order history at submission time (`09-lock-state-machine`).
  Never gate an order on this query.
- **Eligible universe on a date:** read `universe` for that `date` with
  `eligible = true`, then exclude any ticker with `veto_active = true` in
  `veto_flags` (the universe is a daily table; there are no
  effective/end-date intervals to range-filter).
- **As-of macro value:** `macro` is long form with vintages; pick the row
  with `known_from <= t` and the greatest `realtime_start`
  (`QUERY-PATTERNS.md`).

Do not write ad-hoc queries against the lake from production code. Those
queries live in Julia or Rust modules and are tested. DuckDB is for
debugging and dashboard.

## 8. What Not to Do

- Do not write to the array (`/mnt/user/`). See `UNRAID-STORAGE.md`.
- Do not create a new dataset without updating `SCHEMAS.md`, §3 above and
  `schemas/registry.json` in the same commit (no migration file needed for a
  new dataset; `MIGRATIONS.md`).
- Do not use float32 for prices or returns. float64 only.
- Do not store datetimes as strings. Use Arrow `timestamp[ns, UTC]`.
- Do not partition by ticker. Partition by time.
- Do not write a partition in place. Temp file in the same directory, then
  atomic rename (`IO-CONTRACT.md` §3).
- Do not read a Parquet file into memory when a predicate pushdown would
  suffice. Filter at the reader.
- Do not use pandas in any Julia or Rust code path. It is not a bridge.
- Do not hard-code `/mnt/cache/...` or `/data/...` in code or scripts. Read
  `QUANT_ROOT`, `QUANT_STATE`, `QUANT_CONFIG`.

## 9. Verification

Before committing any change to a Parquet writer:

- [ ] Run `scripts/verify-schemas.sh` and it passes
- [ ] If schema changed, `schemas/registry.json` is regenerated
- [ ] If schema changed, a migration file exists in `migrations/`
- [ ] The writer calls `write_partition`/`write_dataset` of the shared
      writer (no direct file write)
- [ ] ZSTD level is 3
- [ ] Layout matches the table in §3
- [ ] No column was renamed, retyped, or reordered
- [ ] A test reads back the dataset and asserts the schema
- [ ] A test re-runs the write and asserts the content is unchanged
      (`IO-CONTRACT.md` §4)

## 10. Related Skills

- `00-construction` §5 — schema immutability rule
- `01-data-ingestion` — writers for prices, splits, dividends, fundamentals,
  macro, etf_holdings, security_master, delistings, filing_events,
  data_quality
- `02-panel-construction` — writer for panel
- `03-gspqr-model` — writer for gspqr_predictions
- `04-tcp-calibration` — writer for tcp_calibration
- `05-evt-tails` — writers for evt_residuals, evt_tail_bounds
- `10-execution-schwab` — writers for orders, fills, positions
- `11-rust-event-bus` — writer for audit_log
- `14-backtest-protocol` — writer for evaluation_metrics
