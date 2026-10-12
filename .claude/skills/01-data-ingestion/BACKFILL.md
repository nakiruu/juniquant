# Bulk Backfill Plan (15+ Years)

The project is in the data-collection phase: every dataset is collected with
15+ years of history before GSPQR, TCP and EVT are trained. The daily job
alone (`SKILL.md`) cannot do that, so this file specifies the one-time bulk
plan per source, the raw landing zone, the resumable manifest, ordering,
estimates and the gates that must pass before the panel is built.

All request counts, durations and sizes here are **estimates** (labelled), to
be replaced by the manifest's measured values after the first run. Endpoint
facts marked "verify" are in `SOURCES.md` and are settled by the probes (§2).

## 1. Targets

| Item | Target |
|---|---|
| Price history start | `BACKFILL_START = 2005-01-01` (config). With the 3-year history rule this gives the first eligible universe date in early January 2008 and includes the 2008 crisis (`02-panel-construction/FEATURES.md` §6) |
| Macro observations | from 2004-01-01, all vintages |
| Filings | everything EDGAR has for the universe's CIKs; XBRL facts exist from 2009 (SOURCES.md) |
| Holdings snapshots | as far back as iShares serves, at least month-ends; OPEN §7 |
| End | the fetch date; the data end of the report is the last full trading day |

15+ years from 2026-10 is 2011-10 or earlier. The first fully featured panel
date (every feature non-null for most names) is about mid-2011 because of the
XBRL phase-in (`SOURCES.md`); rows from 2008 to 2011 exist with `sp_ratio`
NaN. That is a usable 15-year fully featured window, plus a partial window
before it.

## 2. Phase 0: Probes (settle the "verify" items)

Run once before the bulk fetch; each result is stored in `reports/probes/` and
the manifest table `probes`, and sets a config value. A failed probe stops the
backfill for that source until the owner decides.

| Probe | Question | Sets / decides |
|---|---|---|
| P1 | EODHD adjustment: are `open`-`close` raw and `volume` split-adjusted, and is dividend `unadjustedValue` the amount paid? Known splits: AAPL 4-for-1 on 2020-08-31 (as-traded close on 2020-08-28 about $499, split-adjusted about $125), NVDA 10-for-1 on 2024-06-10, GE 1-for-8 reverse on 2021-08-02; AAPL dividend 2020-08-07 ($0.82 as paid) | `SOURCES.md` EODHD normalization, the rounding rule |
| P2 | What does EODHD return for delisted, acquired and renamed codes? TWX, MON, CELG, RHT, LLTC, SHLD, LEH, TWTR (delisted, acquired), FB and META (rename: which code holds the pre-2022 bars). How large is `exchange-symbol-list/US?delisted=1`? | The survivorship expectation; recorded per code in `reports/probes/p2.json` |
| P3 | History depth (SPY, a 2005 listing), ticker reuse (GM: old GM to 2009, new GM from 2010-11-18; does `GM.US` mix them?) and the share-class code (`BRK-B.US`) | History depth; the reuse guard; `symbols` vendor form |
| P4 | Plan and usage: `/api/user` counters, the call cost of an eod, a splits and a bulk request (counter deltas), and when the previous trading day appears in `eod-bulk-last-day` | EODHD bucket and daily cap (`SKILL.md` §5), daily job time |
| P5 | EDGAR bulk files: URLs, `Last-Modified`, size, whether `filings.files[]` pages are in the submissions zip, a multi-class company's dei shares facts (GOOGL, BRK), the `fy`/`fp` quirk, the `sic` and `sicDescription` fields | SOURCES.md EDGAR items |
| P6 | ALFRED: full-vintage fetch of `DGS3MO` and `CPIAUCSL`; the earliest `realtime_start`; vintage-count limits; the earliest observation of the ICE BofA series | `config/macro_series.toml` lags, deferral of the credit spreads |
| P7 | iShares: is `asOfDate` valid, the earliest date served for IWF and IWP, whether non-month-end dates work, the CSV layout and sector labels, publication time of the latest file | Snapshot grid; §7 OPEN |
| P8 | EDGAR Form 25 timing: for 3 known delistings, the Form 25 filing date vs the last EODHD bar | The "+10 days" rule in `SURVIVORSHIP.md` |

Probe output contains the evidence (response status, headers, first and last
dates, a few values). Nothing in the probe output is a secret.

## 3. Raw Landing Zone

Every response is kept. Parsing is a replayable pure function of the raw
bytes, so a parser bug is fixed and replayed, never "re-fetched and hoped".

    $QUANT_ROOT/raw/{source}/{entity}/{chunk}/{fetched_at_utc}__{sha8}.{ext}.zst
    $QUANT_ROOT/raw/{source}/{entity}/{chunk}/{fetched_at_utc}__{sha8}.{ext}.meta.json

- `source`: `eodhd`, `yahoo_chart` (secondary, if registered), `sec_bulk`, `sec_api`, `fred_alfred`, `ishares`,
  `sec_tickers`.
- `fetched_at_utc`: compact UTC timestamp `20261011T143502Z`.
- `sha8`: first 8 hex characters of the SHA-256 of the **uncompressed** body.
  The full SHA-256 is in the sidecar and the manifest.
- Body compressed with ZSTD (the SEC zips are kept as downloaded, as `.zip`,
  not recompressed).
- Sidecar (`.meta.json`): request URL with the API key and email redacted,
  HTTP status, selected response headers (`date`, `etag`, `last-modified`,
  `content-type`, `content-length`), `fetched_at`, client version,
  attempt number, full SHA-256, byte length.
- A refetch that yields the same SHA-256 writes no new file (the manifest
  row records `last_checked_at`); a different SHA-256 writes a new file next
  to the old one. Raw files are never deleted or edited. The "active" raw
  file of a chunk is the newest one unless the manifest pins another (used
  to reproduce an earlier build).
- Written with temp file in the same directory, `fsync`, atomic rename; the
  sidecar is renamed first, the body second, so a body without sidecar is
  the only possible torn state and is treated as an orphan (§4).

## 4. The Manifest and Resume

The manifest is a SQLite database `$QUANT_STATE/ingest/manifest.db`, written
only by the Rust ingest crate (it is not a lake dataset; it is rebuildable
from the raw files' names and sidecars). Key: **(source, entity, chunk)**.

```sql
CREATE TABLE chunks (
  source TEXT NOT NULL, entity TEXT NOT NULL, chunk TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN
    ('pending','fetching','fetched','parsed','compacted','failed','gone')),
  attempts INTEGER NOT NULL DEFAULT 0,
  http_status INTEGER, fetched_at TEXT, last_checked_at TEXT,
  bytes INTEGER, sha256 TEXT, raw_path TEXT,
  parse_version TEXT, parsed_at TEXT, stage_path TEXT, stage_sha256 TEXT,
  compacted_at TEXT, reason TEXT, last_error TEXT,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (source, entity, chunk));
CREATE TABLE partitions (               -- lake partitions built from stage files
  dataset TEXT NOT NULL, partition TEXT NOT NULL,
  inputs_hash TEXT NOT NULL,            -- hash of the sorted stage_sha256 + parse_version of its inputs
  content_hash TEXT NOT NULL, written_at TEXT NOT NULL,
  PRIMARY KEY (dataset, partition));
CREATE TABLE probes (probe TEXT PRIMARY KEY, ran_at TEXT, result_json TEXT);
```

(`PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL`.)

States: `pending -> fetching -> fetched -> parsed -> compacted`. A chunk
that fails goes to `failed` with `attempts` and `last_error` (retryable on
the next run). A permanent "no data" answer (EODHD `[]`, Yahoo `Not Found`) goes to
`gone` with `reason` (`vendor_no_data`), which is a result, not a failure.

**Chunk keys by source**

| Source | entity | chunk |
|---|---|---|
| `eodhd` | vendor code (`BRK-B.US`) | `eod-full`, `splits-full`, `div-full` (history to the fetch date) or `bulk-YYYY-MM-DD[-splits|-div]` (daily, entity `US`) |
| `yahoo_chart` | vendor symbol (`BRK-B`) | `full` (history to the fetch date) or `incr-YYYY-MM-DD` |
| `sec_bulk` | `submissions_zip` / `companyfacts_zip`; `form_idx`; `daily_idx` | `YYYY-MM-DD` (fetch date of the zip); `YYYYqN`; `YYYY-MM-DD` (index day) |
| `sec_api` | `CIK##########` | `submissions-YYYY-MM-DD` / `facts-YYYY-MM-DD` |
| `fred_alfred` | series id | `vintagedates-YYYY-MM-DD`, `all_vintages-FIRST-LAST[-open][-oN]` (fetch), `all_vintages` (stage) |
| `ishares` | `IWF` / `IWP` | `asof=YYYYMMDD` |
| `sec_tickers` | `company_tickers` / `company_tickers_exchange` / `cik_lookup` | `YYYY-MM-DD` |

**Idempotent per-chunk steps** (each step is atomic and skippable):

1. *Fetch*: land the raw file (§3), then update the row to `fetched` in one
   SQLite transaction. A crash between the rename and the commit leaves a raw
   file without a row; recovery adopts it from its sidecar.
2. *Parse*: pure function of (raw bytes, `parse_version`) to a stage file
   `stage/{dataset}/{source}/{entity}/{chunk}.parquet` (temp + rename), hash
   in the row. If the stage file exists with the same inputs, skip.
3. *Compact*: for each lake partition touched, rebuild it from **all** stage
   files that contribute to it (mode `replace`, `13-parquet-lake/IO-CONTRACT.md`
   §3) and record `inputs_hash` in `partitions`. If the recorded
   `inputs_hash` equals the new one, skip. Rebuild-from-inputs is what makes
   a compaction idempotent without reading the partition.

**Resume algorithm** (what `quant-ingest backfill --resume` does on start):

1. Reset rows `fetching` older than 15 minutes to `pending`.
2. For each raw file on disk: ensure a row exists (adopt from the sidecar,
   or move to `raw/_orphans/` if there is none); with `--verify`, recompute
   the SHA-256 and compare (about 4 GB at disk speed: a minute or two,
   estimate).
3. Work the `pending` and `failed` rows in the order of §8 within a source
   (failed first if `attempts < 5`).
4. Never re-fetch a chunk in `fetched`, `parsed`, `compacted` or `gone`;
   resume at the next step instead.

Killing the process at any instruction and restarting converges to the same
final state; a test does exactly that (§12).

## 5. Per-Source Plans

### 5.1 EODHD: raw OHLCV, splits, dividends (D15)

- **Entities.** The vendor code of every ticker in the D12 scope: US common
  stocks from `exchange-symbol-list/US?type=common_stock`, active and
  `delisted=1` (both snapshots landed as raw files), restricted to NYSE, NYSE
  American and Nasdaq, plus every ticker in any holdings snapshot (from
  `etf_holdings` and `security_master`, so this job runs after J4 in §8), plus
  the reference codes `SPY`, `IWF`, `IWP` and `VIX.INDX` (calendar and gate
  checks). 6,059 active and 16,588 delisted codes (P2); a historical holder of a
  reused ticker is fetched under its `_old` code (`SOURCES.md` EODHD).
- **Chunks.** Three per code: `eod-full`, `splits-full`, `div-full`, each
  `from` = `BACKFILL_START` (earlier is harmless) to the fetch date, fetched
  together in that order (the volume normalization needs the splits up to the
  eod fetch, `SOURCES.md` EODHD). One request returns the whole range, so no
  date-range chunking. The daily update is three `bulk-YYYY-MM-DD` chunks for
  the whole exchange.
- **Rate.** `SKILL.md` §5: 8 requests/s, burst 20, concurrency 4, and at most
  90,000 calls per day. Estimated calls: 3 x (5,500 + delisted) + 10% retries
  + probes: about 68,000, so the backfill fits in one day's allowance.
- **Result per chunk.** Rows into `prices` (partitions `year`/`month`),
  `splits` and `dividends` (`source = eodhd`). An empty array -> `gone`
  (`vendor_no_data`); a series that starts after the first holdings date, or
  ends before the EDGAR delisting estimate, is logged and classified by the
  coverage job (`SURVIVORSHIP.md`).
- **Resume.** By chunk; each chunk is one request.
- **Raw volume (estimate).** About 0.3 MB uncompressed per symbol, about 0.75
  GB uncompressed, 0.15-0.2 GB ZSTD.
- **Daily hand-off.** After the backfill the daily job fetches
  `incr-YYYY-MM-DD` for current members only: the window starts 7 trading
  days before the last stored bar and ends at the fetch time (the overlap
  catches vendor corrections; differing overlap rows are logged as a
  correction event in `data_quality`, G2). A newly seen split in an
  incremental response schedules a `full` refetch of that symbol, and each
  month a rotating twelfth of the symbols gets a `full` refetch as a
  reconciliation. Vendor corrections of past bars are the only permitted
  change to a stored price and are always logged with before and after.
  Estimated daily load: about 700 requests, about 25 minutes.

### 5.2 SEC EDGAR: filings, facts, events, delistings

- **Bulk first.** Download `submissions.zip` and `companyfacts.zip`
  (`SOURCES.md`; sizes on the order of 1 GB each, estimate, verify via P5),
  resumable with `Range`, kept as downloaded. Entities `submissions_zip`,
  `companyfacts_zip`, one chunk each. Estimated time 10-60 minutes each,
  bandwidth-bound.
- **Parse by streaming** the zip entries; parse only the CIKs in
  `security_master` (about 2,500 of about 18,000 filers with facts), not the
  whole archive. Outputs (stage): fundamentals rows, `filing_events` rows,
  and delisting evidence.
- **Filing index cross-check.** The quarterly `form.idx` files (4 per year x
  17 years = about 70 requests, about 15 seconds at 5/s) are fetched into
  the landing zone and used by a gate to confirm that every 10-K/10-Q/8-K/25
  row of the universe's CIKs in the index is present in the submissions-based
  data (G7). Not a source of values.
- **Daily hand-off** (`quant-ingest edgar daily`, built 2026-10-12). One request per
  weekday: the EDGAR daily form index (`daily-index/YYYY/QTRn/form.YYYYMMDD.idx`, entity
  `daily_idx`, a 404 on a past weekday is an SEC holiday, `gone`). For each of our CIKs listed
  there with a 10-K/10-Q/10-K/A/10-Q/A: its `submissions` and `companyfacts` JSON (chunks
  `submissions-YYYY-MM-DD` / `facts-YYYY-MM-DD`, the index day), rows rebuilt with the older
  filings from the zip pages, staged under `sec_api`, compacted with the zip rows (zip first
  on a duplicate key). This replaces polling every member CIK (about 4,700 with D20, 16
  minutes at 5/s). J6 (`quant-ingest edgar events` from the zip, built 2026-10-12, then the same
  daily run): each of our CIKs listed with an event form (8-K, 8-K/A, 25, 25-NSE, 15-*, NT
  10-K/Q) gets its `submissions` JSON (the same `submissions-YYYY-MM-DD` chunk, no second
  request for a periodic filer), its event rows since the day minus 3 are staged on that
  chunk's key, and `delistings` is rebuilt in full from `filing_events` and the last price
  date of each FIGI. First run: 18 periodic filers and 241 event filers on 2 days, rows
  identical to the zip apart from 3 rows of an 8-K accepted after the zip was built. J6 runs
  natively on the Debian host (owner decision 2026-10-12), not as an Unraid User Script.
  The zips are re-downloaded monthly and compared with the accumulated API
  deltas (any difference is logged).
- **Request estimate for the backfill.** 2 bulk downloads + about 70 index
  files + a handful of ticker maps = under 100 requests. Raw size about 2-4
  GB (estimate).

### 5.3 FRED / ALFRED: macro with vintages

- **Entities.** The 9 series of `SOURCES.md`; one chunk per window of at most 2,000
  vintage dates (the full real-time range is refused, P6), `observation_start=2004-01-01`,
  paged at 100,000 rows, windows stitched as in `SOURCES.md`. Estimated requests: 9 series,
  1 to 3 windows each plus one vintage-date call; under 40; about 40 seconds at 1 request/s.
- **Result.** Rows into `macro` with `known_from` computed (`POINT-IN-TIME.md`).
  The ICE BofA series return only about three years (P6: from 2023-10-10): the
  gate records the earliest observation and the series is marked `deferred`
  because it does not reach 2004.
- **Daily hand-off** (`quant-ingest macro daily`, the same run as the backfill). The
  vintage-date list is refetched (chunk `vintagedates-YYYY-MM-DD`, 9 requests); a new vintage
  renames the open last window (`all_vintages-FIRST-LAST-open`), which alone is refetched;
  the series is then restitched and recompacted. No new vintage: 9 requests, nothing
  rewritten.

### 5.4 iShares: holdings by as-of date

- **Entities.** `IWF`, `IWP`. Chunks `asof=YYYYMMDD`.
- **Grid.** The last trading day of every month from the earliest date the
  probe shows (P7) to the latest, for both ETFs. Estimated if the history
  reaches 2008: about 225 months x 2 = about 450 requests, plus probes; at 1
  request / 5 s about 40 minutes. A weekly grid would be about 1,950
  requests (about 2.7 hours); it is an owner option (§7) taken only if the
  probe shows arbitrary dates work.
- **Result.** Rows into `etf_holdings` (partition `year`); the snapshot's
  `sector` is the GICS sector snapshot for that date. There is no separate
  GICS fetch: the sector as of date `t` is the sector in the latest
  snapshot usable at `t`. GICS reclassifications (the schemes changed in
  2018 and 2023; verify) therefore appear as the files published them.
- **Daily hand-off.** The monthly job fetches the newest month-end snapshot
  on the first business days of the next month; the daily run only checks
  whether a new as-of date exists.

### 5.5 Identifier master

- **J4a, identity (D17).** `quant-ingest identity [--codes A,B] [--quarters 2008q1-2008q4]`:
  the EODHD symbol lists, the 13(f) lists from the quarter before
  `BACKFILL_START` (about 88 files), OpenFIGI for every EODHD ISIN and every
  13(f) CUSIP no ISIN covers (about 600 requests with the key, a few minutes),
  the SEC `company_tickers_exchange.json` for current CIKs, and EDGAR's
  `cik-lookup-data.txt` (every filer's current and former names, landed once a month) for
  the rest: a security without a current-ticker CIK (every delisted one) takes the one CIK
  whose name or former name has its normalized name; a name shared by two CIKs stays null, and the CIK must have filed (its submissions JSON, one
  request per candidate CIK, landed once a month) within a year of the security's price span
  (`resolution_note` ends `; cik:name`). A SPAC name is out of scope before every route (D19). Writes
  `security_master` (#31) and `security_spells` (#36); unresolved codes go to
  `data_quality`. Without `OPENFIGI_API_KEY` a mapping of more than 2,000
  lookups is refused (it would take hours). Rules: `SOURCES.md` Identifiers.
- **J4, EDGAR side. Inputs.** `company_tickers.json` and `company_tickers_exchange.json`
  (current), `cik-lookup-data.txt` (names), the submissions' `name`,
  `tickers`, `exchanges`, `formerNames`, the snapshots' tickers and names,
  and `config/identifier_overrides.csv`. About 5 requests plus the bulk
  files already downloaded.
- **Algorithm and rules.** `UNIVERSE.md` §4. Output `security_master`. The
  share of snapshot tickers left `unresolved` is a gate (G5).

### 5.6 Trading calendar

Trading days for the backfill come from the same calendar module as the lock
(`09-lock-state-machine` §4: NYSE holiday rules for any year plus the
overrides set), never from the dates present in `prices`. The module must
encode the historical irregularities that fall inside the window (all
**verify**): New Year's Day falling on a Saturday does not close the
preceding Friday (2010-12-31 was a trading day); Juneteenth is a holiday from
2022; ad-hoc closures 2007-01-02 (national mourning), 2012-10-29 and
2012-10-30 (Hurricane Sandy), 2018-12-05 (national mourning), 2025-01-09
(national mourning). The seed file `config/nyse_calendar_overrides.toml`
must contain the ad-hoc ones. Gate G0 compares the calendar with the dates of
the `SPY` bars as a **check** (a mismatch is a finding, the calendar is not
rebuilt from prices).

## 6. Order of Work Within a Run

`backfill` runs the DAG of §8 with these pipeline stages per dataset:
fetch -> parse -> compact. Stages are separate commands too
(`quant-ingest fetch|parse|compact`), so a parser fix is replayed with
`quant-ingest parse --source eodhd --since-parse-version N` over the
landing zone without any network.

## 7. iShares History Depth and Cadence (decided: D1, D2)

**D1:** probe P7 is the mandatory FIRST step of the holdings backfill; the
options below are chosen from its result (D is never allowed).

**D2 (cadence):** month-end snapshots, plus one snapshot dated just after the
annual Russell reconstitution (effective after the close of the last Friday of
June; verify the date each year), plus an extra snapshot on any as-of date
where the month-end diff shows more than 5% of holdings by weight changed
(large turnover, M&A). Weekly snapshots are not needed by default.

Whether iShares serves 15+ years of holdings is unknown until P7. Options
(after the probe):

| Option | Effect |
|---|---|
| A. iShares history reaches 2008 or earlier | Nothing to decide; month-end grid; weekly grid optional |
| B. History starts later (for example 2018) | The universe, and so the panel, starts then; or use C |
| C. Fill the earlier years from the funds' SEC filings (N-PORT / N-Q holdings, CUSIP-keyed, quarterly, lagged; verify availability, format and the CUSIP -> ticker mapping from submissions) | Coarser membership (quarterly) and a mapping effort; an unmatched CUSIP is a gap logged in the coverage report |
| D. Use today's membership for earlier years | **Not allowed**: that is survivorship bias by construction (it selects the winners that grew into the index) |
| E. Cadence: month-end vs weekly | Weekly reduces membership staleness around reconstitution and M&A; costs about 4x the requests |

Rule: month-end grid plus the D2 extras; option A if P7 allows, else B (or C if the owner opts in after seeing the probe); never D.

## 8. Ordering and Dependencies

```
J0  calendar (module, static)                      no data dependency
J1  probes P1-P8                                   -> config flags
J2  sec_tickers + EDGAR bulk zips (submissions, companyfacts)
J3  ishares holdings backfill (IWF, IWP)           -> etf_holdings
J4a identity: security_master, security_spells      needs EODHD symbol lists, 13(f) lists, OpenFIGI (D17)
J4  security_master CIK, SIC (EDGAR side)          needs J2, J3, J4a
J5  eodhd prices/splits/dividends                  needs J4a (code -> FIGI)
J6  filing_events + delistings                     needs J2, J4
J7  fundamentals                                   needs J2 (facts), J4 (cik -> ticker)
J8  macro (ALFRED)                                 independent; run in parallel from the start
J9  universe + price_coverage (Julia)              needs J3, J4, J5, J6
GATES (QUALITY-GATES.md)                           need J5-J9 -> panel build allowed or not
J10 panel                                          needs gates passed (02-panel-construction)
```

J2, J3 and J8 run in parallel (different sources, separate buckets). J5 is
the long pole. `delistings` holds EDGAR-derived dates only; the last price
date is joined in `price_coverage`, which removes the J5 -> J6 cycle.

## 9. Estimates (labelled; replace with measured values)

| Item | Estimate | Basis |
|---|---|---|
| Distinct tickers ever in IWF/IWP, 2008-2026 | about 2,500 (range 2,000-3,000) | about 600 members now, roughly 15% annual replacement over 17 years |
| EODHD calls | about 68,000: 3 x (6,059 active + 16,588 delisted codes, P2) | eod + splits + dividends per code (1 call each, P4); under the 90,000-call daily cap, so one day |
| EDGAR requests | under 100 | bulk + index + maps |
| FRED requests | under 30 | 9 series, paged |
| iShares requests | about 500 (month-end) | 2 ETFs x about 225 months + probes |
| Wall clock, backfill fetching | about 2-3 hours of EODHD at 8/s | 68,000 calls / 8 per s is about 2.4 hours |
| Raw landing zone | about 3-6 GB | EODHD about 0.5 GB, SEC zips 2-4 GB, rest small |
| Stage | about 0.5 GB | rebuildable; may be deleted after compaction |
| Lake after backfill | about 0.6-0.8 GB | `prices` about 9M rows x about 40 B; `universe` about 3M rows; fundamentals about 150K rows; events about 300K rows |
| Daily steady state | 3 EODHD bulk requests (300 calls) + about 700 EDGAR + 9 FRED requests, a few minutes | the whole US exchange per day |

## 10. Quality Gates Are Preconditions to the Panel

The panel build (`02-panel-construction`) calls `preflight()` first. It
refuses to run unless the gates in `QUALITY-GATES.md` (G0-G9) have a passing
`data_quality` record for the current data end, and attaches the survivorship
report to the panel. No flag skips them; a threshold change is a commit to
`config/quality_gates.toml`, which is recorded in the caveat.

## 11. What Not to Do

- Do not start the panel build, or train anything, before the gates pass.
- Do not rebuild history from today's constituents (§7, D).
- Do not delete or edit raw files; fix the parser and replay.
- Do not widen the rate limits to finish sooner. A 429 spiral costs more
  than the hours it saves.
- Do not fetch an old window that does not end at the fetch time (the splits that undo a split-adjusted field must be known up to the fetch).
- Do not fabricate rows for symbols the vendor does not serve.

## 12. Verification

- [ ] Kill-and-resume test: killing the backfill at random points and
      restarting yields the same manifest end state and the same lake content
      hashes
- [ ] Idempotency test: a second full run performs no network request for
      `compacted` chunks and rewrites no partition (equal `inputs_hash`)
- [ ] Replay test: delete the stage directory and the lake datasets derived
      from it, run `parse` and `compact` offline, get the same content hashes
- [ ] Every row in `prices`, `splits`, `dividends`, `etf_holdings` has a
      `raw_sha256` that exists in the landing zone
- [ ] A torn write (body without sidecar, stage file without row) is
      adopted or quarantined, never read as data
- [ ] Probe results are recorded and the config flags match them
- [ ] The gates run on the final state and are all green, or the failures are
      listed in the caveat
