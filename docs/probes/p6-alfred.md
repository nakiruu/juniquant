# P6: ALFRED full-vintage fetch

Spec: `.claude/skills/01-data-ingestion/BACKFILL.md` §2 (P6), `SOURCES.md` "FRED / ALFRED".
Run: 2026-10-11 about 14:05 UTC, by hand with `curl` against `https://api.stlouisfed.org/fred/`
(the key is not recorded here). 9 requests at 1 request/s, all answered in 0.3 to 0.6 s. The
Rust probe `quant-ingest probe p6` repeats these calls and writes the evidence JSON.

## Result

| Question | Answer |
|---|---|
| Is a full-vintage fetch possible in one request? | **No for `DGS3MO`** (5,127 vintage dates). Yes for `CPIAUCSL` (669). |
| Vintage-count limit | **2,000 vintage dates per request**, for `file_type=json` and `xml`, and for `output_type=1` and `4` alike. Over it: HTTP 400, `"There are 5127 vintage dates in the specified real-time period ... exceeds the maximum number of vintage dates allowed for this file type (2000)."` |
| Does chunking work? | Yes. `realtime_start=2005-06-28 .. 2014-02-04` (exactly the first 2,000 vintage dates of `DGS3MO`) returned 2,635 rows in 250 KB, HTTP 200. The 100,000-row page limit is nowhere near. |
| Earliest `realtime_start` | `DGS3MO`: **2005-06-28** (first vintage date). `CPIAUCSL`: vintage dates start 1972-07-21; for observations from 2004-01-01 the first `realtime_start` is 2004-02-20. |
| Late-captured history (quirk) | **Confirmed for `DGS3MO`.** 388 observations dated 2004-01-01 to 2005-06-27 all carry `realtime_start = 2005-06-28`, the capture date. They get `known_from_basis = lag_rule`. `CPIAUCSL` shows real publication dates (the January 2004 CPI has `realtime_start` 2004-02-20). |
| ICE BofA history | `BAMLH0A0HYM2` and `BAMLC0A0CM` both start at **2023-10-10** (794 observations to 2026-10-08), although `observation_start=1990-01-01` was requested. One vintage only (`realtime_start` 2026-10-09). The three-year window in the spec is confirmed. |

## Consequences

1. **The fetch plan needs windows.** For each series: call `fred/series/vintagedates` first
   (one request, up to 10,000 dates), cut the list into runs of at most 2,000, and request
   `realtime_start = first date of the run`, `realtime_end = last date of the run`.
   `DGS3MO` takes 3 windows (2,000 + 2,000 + 1,127). `DGS2`, `DGS10`, `VIXCLS` and
   `FEDFUNDS` are daily or similar: expect 2 to 3 windows each (not probed). Monthly series
   fit in one.
2. **Stitching.** A window clips `realtime_end` to its own last date (the first `DGS3MO` row has
   `realtime_end = 2014-02-04`, not the real end of that value). When merging windows, join an
   interval that ends at a window boundary to the next window's interval with the same
   `(date, value)`. Otherwise the `macro` table gets fake revisions at the boundaries.
3. **`.` values.** `value = "."` appears as a real row (the first `DGS3MO` row, 2004-01-01, a
   holiday). They become null, as `SOURCES.md` says.
4. **Credit spreads are deferred.** Nothing before 2023-10-10 exists on FRED for either
   series. This is the decision the probe was meant to settle; the owner confirms it in
   `config/macro_series.toml`.
5. **Request budget.** About 9 series x 1 to 3 windows plus 9 vintage-date calls: under 40
   requests, about 40 seconds at the Fred bucket (1 request/s).

## Not checked

- `DGS2`, `DGS10`, `VIXCLS`, `FEDFUNDS`, `UNRATE` vintage counts (expected to follow `DGS3MO`
  or `CPIAUCSL`; the windowing code does not depend on them).
- Whether `min_lag_days` for the daily series is long enough. That needs the distribution of
  (`realtime_start` minus `date`) over the real-publication part of the history, which is a
  job for the P6 run once the fetch code exists.
- Behaviour on HTTP 429: none was met.
