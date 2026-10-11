# Unraid Storage Layout

This system runs on Unraid. Storage paths and pool rules matter because
a misconfiguration silently destroys query performance.

## Storage Tiers

| Tier | Path | Purpose | Redundancy |
|---|---|---|---|
| Cache pool (NVMe, XFS) | `/mnt/cache/quant/` | Live Parquet lake, DuckDB workspace, model artifacts | Single drive or XFS on a pool |
| Appdata | `/mnt/cache/appdata/quant/` | Container state, logs, config, the audit journal `audit.db` (+ `-wal`/`-shm`) and the `audit-spool/` directory | Same pool as cache |
| Array (HDD, XFS, parity) | `/mnt/user/quant-backup/` | Daily rsync of cache pool | Unraid parity |
| Boot | `/boot/config/plugins/` | Unraid plugins, User Scripts | USB flash |

**Audit journal.** `audit.db` is the system of record for the audit log
(`11-rust-event-bus` §5). It must live on the cache pool path
(`/mnt/cache/appdata/quant/`, `/state/audit.db` in the container), never
on `/mnt/user/...` (FUSE) or the array: WAL needs real fsync and shared
memory semantics. It is written only by the Rust audit writer in
`quant-core`; every other mount of it is read-only. Never copy it with
plain `rsync`/`cp` while the writer runs (see the backup script). Julia
writes audit events to `audit-spool/` (temp file + atomic rename), same
filesystem as the journal.

**Rule:** All live reads and writes go to the cache pool. The array is
read-only backup. Never write a Parquet file to `/mnt/user/`.

## Filesystem Notes

Unraid uses **XFS** as the default filesystem for both the array and
cache pools. XFS is a mature, high-performance filesystem well-suited
to this workload. Two implications:

**No native snapshots.** XFS does not support snapshots in the mainline
kernel. The nightly rsync to the array is the backup mechanism. If
snapshot-style point-in-time recovery is desired, it must be done at the
application layer (e.g., copy Parquet files to a timestamped directory
before a schema migration).

**No checksumming.** XFS does not checksum data blocks the way ZFS does.
Silent bit rot is possible. Mitigation: the nightly rsync verifies file
size and modification time, and the schema registry hash (the repository's
`schemas/registry.json`, bundled in the image) detects schema-level
corruption. The raw landing zone is additionally verified by SHA-256 against
the ingestion manifest (`01-data-ingestion/BACKFILL.md` §3). For
data-level integrity, add a periodic `parquet-tools` or DuckDB
read-through validation job that reads every file and checks row counts
and null percentages against expected values.

**Cache pool configuration.** XFS on a single NVMe drive is the simplest
configuration. If redundancy is desired, Unraid supports a cache pool
with two drives. However, Unraid's cache pool redundancy for XFS uses
**btrfs RAID1**, not XFS mirroring. For an XFS-only setup, a single
cache drive is the norm, and the nightly array backup is the safety net.

## Directory Structure on Cache Pool

```
/mnt/cache/quant/                       # host; QUANT_ROOT=/data in the container
├── lake/                               # one entry per dataset, SKILL.md §3 (34)
│   ├── universe/                       # #1 derived daily membership, P(year)
│   │   └── year=YYYY/data.parquet
│   ├── prices/                         # #2 raw as-traded OHLCV
│   │   ├── year=2024/month=01/data.parquet
│   │   ├── year=2024/month=02/data.parquet
│   │   └── ...
│   ├── fundamentals.parquet            # #3
│   ├── macro.parquet                   # #4 long form, ALFRED vintages
│   ├── panel/                          # #5
│   │   └── year=YYYY/month=MM/data.parquet
│   ├── regime_states.parquet           # #6
│   ├── gspqr_predictions/              # #7
│   │   └── year=YYYY/month=MM/data.parquet
│   ├── tcp_calibration/                # #8
│   │   └── year=YYYY/month=MM/data.parquet
│   ├── evt_residuals/                  # #9
│   │   └── year=YYYY/month=MM/data.parquet
│   ├── evt_tail_bounds/                # #10
│   │   └── year=YYYY/data.parquet
│   ├── conviction_scores.parquet       # #11
│   ├── veto_flags.parquet              # #12
│   ├── portfolio_targets.parquet       # #13
│   ├── position_locks.parquet          # #14 derived view, not authoritative
│   ├── pending_orders.parquet          # #15
│   ├── orders.parquet                  # #16
│   ├── positions.parquet               # #17
│   ├── fills.parquet                   # #18
│   ├── evaluation_metrics.parquet      # #19
│   ├── data_quality.parquet            # #20
│   ├── audit_log/                      # #21 exported closed days only
│   │   └── date=YYYY-MM-DD/data.parquet
│   ├── portfolio_ledger/               # #22 P(year)
│   │   └── year=YYYY/data.parquet
│   ├── tax_lots.parquet                # #23
│   ├── trade_summary/                  # #24 P(year)
│   │   └── year=YYYY/data.parquet
│   ├── order_history.parquet           # #25 evidence snapshot, append-only
│   ├── execution_costs/                # #26 per-name execution cost estimates, P(year)
│   │   └── year=YYYY/data.parquet
│   ├── quote_spread_1m/                # #27 1-minute quoted-spread history (built forward)
│   │   └── session=YYYY-MM-DD/data.parquet
│   ├── splits.parquet                  # #28
│   ├── dividends.parquet               # #29
│   ├── etf_holdings/                   # #30 IWF/IWP membership snapshots, P(year)
│   │   └── year=YYYY/data.parquet
│   ├── security_master.parquet         # #31
│   ├── delistings.parquet              # #32
│   ├── filing_events.parquet           # #33
│   ├── price_coverage.parquet          # #34
│   └── listings/                       # #35 listed securities per date (D12), P(year)
│       └── year=YYYY/data.parquet
├── raw/                                # immutable landing zone (01 BACKFILL.md §3); not registered
│   └── <source>/<entity>/<chunk>/<fetched_at>__<sha8>.<ext>.zst (+ .meta.json)
├── stage/                              # parsed per-chunk intermediates, rebuildable; not registered
├── reports/                            # coverage and SURVIVORSHIP CAVEAT reports, gate reports, backtest/<run_id>/ run artifacts
├── models/
│   ├── generations/<generation>/       # 21 GSPQR model files per monthly generation
│   ├── backfill_manifest.json          # generation backfill resumability
│   ├── regime/hmm_<fit_date>.json      # HMM parameters per refit
│   └── registry.json                   # model registry (not the schema registry)
└── ...
/mnt/cache/appdata/quant/               # host; QUANT_STATE=/state in the container
├── audit.db (+ -wal/-shm), audit-spool/
├── ingest/manifest.db                  # ingestion manifest (SQLite; Rust ingest only)
├── locks/<dataset>.lock                # single-writer advisory locks
├── lock_external.csv, lock_snapshot.json, TRADING_HALTED, acknowledged_violations.json
├── nyse_calendar_overrides.toml        # runtime calendar overrides (append-only)
├── schwab_refresh_token                # 0600, Rust only
├── compliance/                         # dated lock-compliance reports
└── logs/
```

Every state file sits **directly** under `QUANT_STATE` (no `state/`
subdirectory): `$QUANT_STATE/lock_external.csv` is
`/mnt/cache/appdata/quant/lock_external.csv` on the host and
`/state/lock_external.csv` in the container. The variables are defined once
in `IO-CONTRACT.md` §1.

The schema registry and the migration notes are **repository** files
(`schemas/registry.json`, `migrations/`), not lake files; the image bundles
a copy (`SKILL.md` §6). `config/` is the repository's config directory,
mounted read-only at `/config` (`QUANT_CONFIG`).

## Unraid Share Configuration

The share backing `/mnt/cache/quant/` must be configured:

- **Primary storage:** Cache
- **Secondary storage:** Array (for backup only)
- **Allocation method:** High-water
- **Mover action:** Cache → Array (nightly, for backup copy)
- **Minimum free space:** 20 GB (prevents cache pool exhaustion)

**Critical:** If `Mover action` is set to "Array → Cache" or "Cache: yes"
without care, the mover will move live Parquet files from cache to array
overnight. Your next morning's DuckDB query will be 100× slower. Set the
mover action explicitly and verify it after any Unraid version update.

## Container Mounts

| Container | Mount | Mode | Environment | Notes |
|---|---|---|---|---|
| `quant-core` (Julia + Rust) | `/mnt/cache/quant:/data` | rw | `QUANT_ROOT=/data` | Only writer of the lake; also `raw/`, `stage/`, `reports/` |
| `quant-core` | `/mnt/cache/appdata/quant:/state` | rw | `QUANT_STATE=/state` | Logs, `audit.db` (Rust audit writer only), `audit-spool/` (Julia writes, Rust ingests), `ingest/manifest.db` (Rust ingest only), `locks/` |
| `quant-core` | `<checked-out repo>/config:/config` | **ro** | `QUANT_CONFIG=/config` | `restricted.toml`, `nyse_calendar_overrides.toml`, `quality_gates.toml`, `macro_series.toml`, `identifier_overrides.csv`; host path is a deployment choice |
| `quant-api` (Bun) | `/mnt/cache/quant:/data` | ro | `QUANT_ROOT=/data` | Read-only, no writes |
| `duckdb` | `/mnt/cache/quant:/data` | ro | `QUANT_ROOT=/data` | Read-only, debug queries |

The same three variables apply on a development machine with the host paths
(or any scratch directories); no code defaults to a production path
(`IO-CONTRACT.md` §1).

The read-only mounts enforce the single-writer rule from the constitution
at the filesystem level. Bun cannot accidentally overwrite a Parquet file
because the kernel refuses the write. Julia and Rust share the `quant-core`
container, so "only Rust writes `audit.db`" is not a mount-level guarantee;
it rests on the SQL triggers, the language-boundary CI check, and review.

## UID/GID Mapping

Unraid uses UID 99 (`nobody`) and GID 100 (`users`) for share access.
Containers must run with matching UID/GID or they will fail to write.

```yaml
# In the Dockerfile or run command
user: "99:100"
```

If a container needs a different UID, chown the target directory:

```bash
chown -R 1000:1000 /mnt/cache/quant/
```

But prefer running as 99:100. Mixing UIDs across containers leads to
permission confusion and silent write failures.

## Docker Image Storage

On Unraid 7+, Docker image storage options depend on the filesystem:

- **If the Docker image lives on the XFS cache pool:** use the `overlay2`
  storage driver. Do not use the legacy `native` driver; it has known
  stability issues on non-ZFS pools.
- **If using a separate Docker image file:** a single `docker.img` file
  on the cache pool. Monitor its size; when it fills, Docker stops
  working.

**Settings → Docker → Docker image storage:** set to the cache pool path.
**Default appdata storage:** `/mnt/cache/appdata/`.
**Enable Docker custom networks:** yes.
**Host access to custom networks:** enabled.
**Custom network type:** ipvlan (not macvlan).

## Backup Script

Nightly at 18:00, first run the audit export (closed days of `audit.db` to
`lake/audit_log/date=*/data.parquet`, verified, atomic rename), then back
up. The journal is copied with SQLite's online backup, not rsync, because
a live WAL database copied file by file can be torn. This is a backup,
not a migration. Do not delete the source after.

```bash
#!/usr/bin/env bash
set -euo pipefail

SRC=/mnt/cache/quant/
DST=/mnt/user/quant-backup/
JOURNAL=/mnt/cache/appdata/quant/audit.db

# 1. Export closed audit days to the lake (Rust subcommand; verify the name)
docker exec quant-core quant-exec audit-export

# 2. Consistent copy of the journal (needs the sqlite3 CLI on the host or run it inside the container; verify) (SQLite online backup; never rsync a live WAL db)
mkdir -p "$DST/audit"
sqlite3 "$JOURNAL" ".backup '$DST/audit/audit.db.tmp'"
mv "$DST/audit/audit.db.tmp" "$DST/audit/audit.db"

# 3. Lake backup

rsync -a --delete --exclude='.*.tmp.*' "$SRC" "$DST"

# Log the result
DATE=$(date +%F)
echo "$DATE backup completed: $(du -sh "$DST" | cut -f1)" \
  >> /mnt/cache/appdata/quant/logs/backup.log
```

The array is slower, so the backup takes 10–30 minutes for a few GB.
Do not run it during market hours. The temporary files of the I/O contract
are dot-files (`.data.parquet.tmp.<pid>.<ns>`), hence the exclude pattern.
`$SRC` includes `raw/` (about 3-6 GB after the backfill, an estimate); the
ingestion manifest in `/state/ingest/manifest.db` is rebuildable from the
raw files' names and sidecars and is not part of this script
(`01-data-ingestion/BACKFILL.md` §4).

**Note on XFS and rsync:** rsync on XFS is reliable but does not verify
data integrity beyond file size and mtime. For stronger verification,
add a `--checksum` flag to rsync, which forces a content hash comparison
at the cost of a slower backup. Given the Parquet files are a few GB
total (plus the raw zone), `--checksum` is affordable and recommended:

```bash
rsync -a --checksum --delete --exclude='.*.tmp.*' "$SRC" "$DST"
```

## Integrity Monitoring (XFS, No Snapshots)

Since XFS does not provide snapshots, the following monitoring is
required:

| Check | Frequency | Tool |
|---|---|---|
| Schema hash matches registry (repository `schemas/registry.json`) | Daily | `schemas/verify-schemas.sh` |
| Row count per Parquet file | Weekly | DuckDB `SELECT COUNT(*)` |
| Null percentage per critical column | Weekly | DuckDB `SELECT AVG(...)` |
| File size and mtime | Daily | `find` + comparison log |
| Full read-through of all Parquet | Monthly | DuckDB read-all script |
| `audit_export` rows vs `audit_log/date=*` files (count and hash) | Weekly | Rust `audit-verify` subcommand (verify) |
| `PRAGMA integrity_check` on the backed-up `audit.db` | Weekly | `sqlite3` |

A full read-through job reads every Parquet file and asserts the row
count is greater than zero and the schema matches the registry. This is
the substitute for ZFS's automatic checksumming.

## What Breaks on Unraid

| Symptom | Cause | Fix |
|---|---|---|
| Parquet queries 100× slower after a night | Mover moved files to array | Set share mover action to Cache → Array, verify |
| Write failures "permission denied" | Container running as wrong UID | Set `user: "99:100"` |
| Docker stops working | `docker.img` full | Expand it or move to cache pool path |
| Random kernel crashes | macvlan on bridge | Switch to ipvlan |
| GPU not detected | Unraid update broke driver | Regenerate CDI, pin Unraid version |

## What Does Not Break on Unraid

The constitution, the schemas, the pipeline logic, the language boundaries,
the 5-day lock, the point-in-time rules. None of these are affected by
the host OS or the filesystem. If you find yourself changing one of them
"for Unraid," you are solving the wrong problem.