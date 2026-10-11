# ingest

Data ingestion and the 15+ year backfill, in Rust over direct HTTP. The binary is
`quant-ingest`. Specification: [01-data-ingestion](../.claude/skills/01-data-ingestion/SKILL.md)
(`BACKFILL.md`, `SOURCES.md`, `RETRY-POLICY.md`). Phase I2 status:
[`docs/status.md`](../docs/status.md).

## Pipeline

```text
fetch (rate bucket, retry) -> land raw body + sidecar -> parse to stage file -> compact into the lake
            manifest.db records every step, so a run resumes where it stopped
```

A parser is a pure function from raw bytes to rows, so a parser fix is replayed from the
landing zone and never re-fetches. Compaction rebuilds lake partitions from stage files and
skips the rebuild when the inputs are unchanged.

## Modules

| Module | Purpose |
|---|---|
| `infra::fetch` | One chunk: skip if done, pace through the bucket, retry by policy, land, record |
| `infra::bucket` | Per-source token bucket and concurrency cap; the rate is halved for 10 minutes after a 429 |
| `infra::landing` | Immutable raw zone `raw/{source}/{entity}/{chunk}/{fetched_at}__{sha8}.{ext}.zst` plus JSON sidecar with the SHA-256 |
| `infra::manifest` | SQLite state machine `pending, fetching, fetched, parsed, compacted`, plus `failed` and `gone` |
| `infra::stage` | Stage files and compaction into the lake |
| `infra::quality` | The `data_quality` rows of a run |
| `infra::SourceSpec` | Rates, retries, timeouts and the daily cap for EODHD, SEC, FRED and iShares |
| `eodhd` | W5: symbol lists, prices, splits and dividends (DECISIONS D15). **Work in progress** |
| `probe` | Phase 0 probes P1-P6 and the Yahoo probes; results in [`docs/probes/`](../docs/probes/README.md) |

## Commands

```bash
quant-ingest probe <p1..p6|y1..y4>                      # one-off live checks (BACKFILL.md §2)
quant-ingest eodhd backfill [--codes A,B] [--limit N]   # W5; resumable
```

| Variable | Meaning |
|---|---|
| `QUANT_ROOT`, `QUANT_STATE` | Data and state roots; required, no default |
| `QUANT_REGISTRY_PATH` | Registry file, default `schemas/registry.json` |
| `EODHD_API_KEY` | Never logged; redacted from landed URLs |
| `BACKFILL_START` | First date, default `2005-01-01` |

Start with `--codes SPY,AAPL` to check a small run. A fatal error (daily cap, refusal) stops
the run but keeps what was staged; re-running resumes.

## Tests

`cargo test -p ingest`: unit tests for the parsers, and `tests/pipeline.rs`, which drives the
whole pipeline against a scripted local HTTP server (retries, landing, manifest states,
idempotent re-runs, the daily cap, secret redaction, stage to compact). No test touches a live
source; probes are run by hand.
