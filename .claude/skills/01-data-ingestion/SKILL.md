---
name: data-ingestion
description: Fetching raw data from EODHD (prices), SEC EDGAR, FRED/ALFRED
  and iShares, in Rust over direct HTTP, for both the 15+ year backfill and the
  daily update. Load when touching ingest/src/ (backfill, parse/, holdings,
  identifiers, filing_events, delistings, ...), the shared symbols/ crate, the
  raw landing zone, or any dataset it writes (prices, splits, dividends, fundamentals, macro,
  etf_holdings, security_master, delistings, filing_events, data_quality).
---

# Data Ingestion

Four sources feed this system: EODHD for prices (paid, DECISIONS D15), and SEC
EDGAR, FRED (with its archival twin ALFRED) and iShares, which are free. EODHD
is the only paid source, and there is **no Python**:
ingestion is Rust over direct HTTP (`ingest/`), analytics are Julia, checks
are Julia, Rust, SQL (DuckDB) and bash. Each source has different rate
limits, reliability and point-in-time semantics. This skill documents how to
fetch from each without lookahead, without hammering the source, and without
silently producing incomplete data.

## 0. Current Phase: Collect Everything, 15+ Years

The critical path of the project is data collection, then training
GSPQR + TCP + EVT. So this skill has two modes:

| Mode | Purpose | Spec |
|---|---|---|
| **backfill** | One-time bulk collection of 15+ years of every dataset, resumable, with every raw response kept | `BACKFILL.md` |
| **daily** | Incremental update after the close | this file, `SOURCES.md` |

The panel is not built until the quality gates (`QUALITY-GATES.md`) pass on
the backfilled data. Survivorship bias is measured, not assumed away: EODHD
keeps delisted tickers, and its coverage of past delistings is quantified
(`SURVIVORSHIP.md`).

## 1. The Sources

| Source | Provides | Cadence | Auth |
|---|---|---|---|
| EODHD (REST API, D15) | Raw daily OHLCV, split and dividend events, delisted tickers included | Daily after close | `EODHD_API_KEY` (paid plan) |
| SEC EDGAR | Filings, XBRL facts, 8-K items, Form 25/15, identifiers | Event (filing) | `User-Agent` with a real email |
| FRED / ALFRED | Macro series with vintages | Daily / on release | Free API key |
| iShares | ETF holdings (IWF, IWP) by as-of date | Monthly at minimum | None |

`SOURCES.md` has the endpoints, parameters, parsing and quirks. Items marked
"verify" in that file were written from memory of the services and must be
confirmed by the first probe run (`BACKFILL.md` §2).

## 2. Non-Negotiables

Inherited from the constitution (§2, §3):

- **Every fundamental datum carries its `filing_date`.** The `period_end` is
  metadata, never a decision variable. Ingestion is the only code that may
  read `period_end` (it needs it to pair fiscal periods and compute TTM); the
  exception path is in `POINT-IN-TIME.md`.
- **No future data.** A fetch on 2024-06-14 cannot return data with a
  timestamp after 2024-06-14. If the source returns such data, drop it.
- **Failures are logged, never silent.** Every failed fetch writes a row to
  `data_quality` with `status = WARN` or `FAIL`, keyed by entity and chunk.
- **Every raw response is kept** (landing zone, with fetch timestamp and
  SHA-256), so parsing is replayable and a parser bug never loses data.
- **Idempotent.** Re-running ingestion for the same chunk produces the same
  output. No partial writes: temp file in the same directory, atomic rename
  (`13-parquet-lake/IO-CONTRACT.md`).
- **Never overwrite history.** A restated or corrected value is a new row
  with its own date; the first-reported value stays.

## 3. Write Targets (one writer per dataset)

Each job writes only the datasets listed, through the lake writer
(`13-parquet-lake/IO-CONTRACT.md`). The lake numbers are from `13-parquet-lake/SKILL.md` §3.

| Job (Rust, `ingest/`) | Writes | Frequency |
|---|---|---|
| `ingest_prices` | `prices` #2, `splits` #28, `dividends` #29 | Daily |
| `ingest_fundamentals` | `fundamentals` #3 | On new filing (daily check) |
| `ingest_filing_events` | `filing_events` #33, `delistings` #32 | Daily |
| `ingest_macro` | `macro` #4 | Daily |
| `ingest_holdings` | `etf_holdings` #30 | Monthly at minimum (daily check) |
| `ingest_identifiers` | `security_master` #31 | Weekly and on new tickers |
| `ingest_listings` | `listings` #35 | Daily |
| `ingest_gates` | `data_quality` #20 | Every run |

`universe` #1 and `price_coverage` #34 are **not** ingestion outputs: they are
derived by the Julia universe builder (`UNIVERSE.md`), one writer each.
Earlier text said `ingest_universe` (Rust) while `SCHEMAS.md` said Julia; the
split above is the single decision.

If a job produces no new data (market closed, no new filings), it writes
nothing. Do not append empty rows.

## 4. Fetch Discipline

Every external call follows the same pattern:

1. **Check the manifest.** The ingestion manifest (`BACKFILL.md` §4) says
   what is already fetched; skip a chunk that is `compacted` unless a refetch
   rule applies (`BACKFILL.md` §5).
2. **Take a token** from the source's bucket (§5).
3. **Fetch with a bounded timeout.** 30 s for JSON APIs, 120 s for bulk
   downloads (resumable ranges). On timeout, retry per `RETRY-POLICY.md`.
4. **Land the raw body.** Write the response bytes to the landing zone
   (temp file, `fsync`, atomic rename), with a sidecar holding the URL (API
   key redacted), status, selected headers and `fetched_at`; record the
   SHA-256 in the manifest. Nothing is parsed before this succeeds.
5. **Validate the response.** Non-empty, expected shape, no future dates. If
   validation fails, keep the raw file, mark the chunk `failed`, log.
6. **Parse** (a pure function of the raw bytes plus the manifest row) into a
   stage file, then **compact** into lake partitions via the lake writer.
7. **Log to `data_quality`.** Success or failure, always, keyed
   `(date, source, dataset, entity, chunk, check_id, run_id)`.

If any step fails, the job exits non-zero and the chunk stays resumable.
There is no partial-state window: a chunk is either `compacted` or it is not.

## 5. Rate Limiting (the one table)

Every source has a token bucket. The numbers below are the **only** rate
table; `SOURCES.md` and `RETRY-POLICY.md` refer here. They are starting
points, deliberately below known or suspected limits. On any 429, log it and
halve the refill rate for 10 minutes.

| Source | Burst | Sustained | Concurrency | Basis |
|---|---|---|---|---|
| EODHD | 20 | 8 requests / s, and at most 90,000 calls per day | 4 | Plan limit 1,000 requests/min and 100,000 calls/day (pricing page, 2026-10-11); about half the rate, 90% of the day. A bulk request counts 100 calls |
| Yahoo (secondary, only if registered) | 5 | 1 request / 2 s | 2 | Undocumented; conservative |
| SEC EDGAR | 10 | 5 requests / s | 4 | SEC fair-access maximum is 10 requests/s (verify); half of it for margin |
| FRED / ALFRED | 20 | 1 request / s | 2 | Documented limit 120 requests/min (verify); half of it |
| iShares | 2 | 1 request / 5 s | 1 | Polite scraping of a public web endpoint; no published limit |

Bulk downloads (`submissions.zip`, `companyfacts.zip`) count as one request
each and are streamed with resumable `Range` requests.

Implement with a token bucket plus `tokio::sync::Semaphore` for the
concurrency cap. Do not sleep manually.

SEC EDGAR requires a `User-Agent` header with a real contact email; requests
without it are rejected. Set `SEC_EDGAR_EMAIL` in the environment. FRED needs
`FRED_API_KEY`, EODHD `EODHD_API_KEY` (sent as `api_token=`). None is ever
logged or written to a sidecar.

## 6. Point-in-Time Rules

See `POINT-IN-TIME.md`. In one paragraph:

SEC data is usable from `filing_date` (the date the filing became public),
never from `period_end`. FRED data is usable from `known_from`, derived from
the ALFRED vintage (`realtime_start`) with a conservative lag floor, never
from the observation date. A close on date `t` is usable after 16:00 ET
on `t`; stored prices are as traded, adjusted series are computed in Julia
from the event tables. An iShares snapshot is usable from the trading day
after its "as of" date.

## 7. Handling Failures

| Failure mode | Action |
|---|---|
| HTTP timeout | Retry up to 3 times with exponential backoff |
| HTTP 429 (rate limit) | Back off 60 s, halve the refill rate for 10 min, retry |
| EODHD `200` with `[]` (unknown or renamed code) | Not a retryable failure: mark the chunk `gone` with reason `vendor_no_data` (this is the survivorship signal, `SURVIVORSHIP.md`) |
| HTTP 401 / 403 from EODHD (key or plan) | Stop the run; never retry. Log FAIL |
| HTTP 4xx (other) | Do not retry. Log FAIL |
| HTTP 5xx | Retry up to 5 times with backoff |
| Schema mismatch | Do not retry. Keep the raw file. Log FAIL. Alert |
| Future dates in response | Drop those rows. Log WARN |
| Empty response for a known-active source | Log WARN. Retry next scheduled run |
| EDGAR filing not yet parseable | Log WARN. Retry in 1 hour, up to 3 times |

All retries are logged with attempt number. A job that exhausts retries exits
non-zero. The scheduler is responsible for alerting.

## 8. What Not to Do

- Do not use `period_end` in any join, filter, or feature computation
  outside ingestion's own TTM and prior-period logic.
- Do not fetch without checking the manifest first.
- Do not write in place. Every write is temp + rename.
- Do not skip the `data_quality` log. Silent failures are the worst kind.
- Do not store vendor-adjusted prices as if they were raw. Do not store
  vendor-adjusted prices at all (`adjusted_close` is kept only as the
  diagnostic `adj_close_vendor`, `SOURCES.md` EODHD).
- Do not parse HTML to extract SEC data. Use the XBRL facts (bulk
  `companyfacts` and the `submissions` JSON). Text of filings (going-concern
  language, covenant breaches) is not parsed: those veto inputs are deferred
  (`SOURCES.md` "Deferred inputs").
- Do not trust vendor snapshots (Yahoo `quoteSummary`, EODHD `filter=extended` fields) as
  time series. They are not ingested; if one is ever added it is an
  append-only dataset stamped with its fetch time and used only forward.
- Do not add a Python dependency, script or reference (no `yfinance`,
  `edgartools`, `fredapi`, no `python3` in a script).
- Do not retry indefinitely. Bounded retries, then escalate.
- Do not fetch on a schedule tighter than the source's natural update
  cadence.
- Do not fabricate a missing price, a delisting reason or a release date. A
  gap is logged with a reason and flows into the coverage report.

## 9. Verification

Before committing any ingestion change:

- [ ] The fetch checks the manifest before calling
- [ ] The fetch uses the token bucket for its source (§5)
- [ ] The raw body is landed with `fetched_at` and SHA-256 before parsing
- [ ] Writes are temp + atomic rename, through the lake writer
- [ ] The `data_quality` row is written on both success and failure, with
      entity and chunk
- [ ] SEC data uses `filing_date`, never `period_end`, for stamping; macro
      data uses `known_from`
- [ ] Future-dated rows are dropped
- [ ] Re-running the job (and re-parsing the raw files) produces identical
      output
- [ ] A test mocks the source and asserts idempotency
- [ ] A test asserts failures are logged
- [ ] No Python anywhere in the change

## 10. Related Skills and Files

- `BACKFILL.md` — the 15+ year bulk plan, manifest, landing zone, ordering
- `SURVIVORSHIP.md` — price source interface, delistings, coverage report
- `UNIVERSE.md` — holdings snapshots, identifier master, symbol rules
- `QUALITY-GATES.md` — preconditions to building the panel
- `SOURCES.md`, `POINT-IN-TIME.md`, `RETRY-POLICY.md`
- `00-construction` §2, §3 — point-in-time and no-lookahead rules
- `13-parquet-lake` — target schemas and the I/O contract
- `02-panel-construction` — consumes ingestion output
- `14-backtest-protocol` — depends on correct point-in-time discipline
