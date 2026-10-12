# Data-Quality Gates

Preconditions to building the panel. The panel build (`02-panel-construction`)
begins with `preflight()`, which refuses to run unless every HARD gate below
has a passing record for the current data end. Gates are evaluated after a
backfill and after each daily run.

Thresholds live in `config/quality_gates.toml` (committed, read-only mount);
the values here are the defaults. Changing a threshold is a commit and is
copied into the survivorship caveat (`SURVIVORSHIP.md`). The SQL in this file
is a sketch (not normative; DuckDB syntax, untested in the authoring
environment).

## 1. Who Evaluates and Where Results Go

| Gates | Evaluated by | Result |
|---|---|---|
| G0-G3, G6-G9 | Rust `quant-ingest gates` (SQL over the lake and the manifest) | Rows in `data_quality` (`source = Gate`, `check_id`, `measured`, `threshold`, `status`) |
| G4, G5 | Julia `preflight()` (they need `universe` and `price_coverage`, which Julia writes) | `reports/gates/<run_id>.json`; `preflight()` also re-reads the Rust rows |
| G10 | Rust, only when a second price source exists | `data_quality` |

`data_quality` rows are keyed `(date, source, dataset, entity, chunk,
check_id, run_id)`; a gate row uses `entity = '*'` for the aggregate and one
row per offending entity when it lists failures.

HARD gates block the panel. WARN gates are reported and copied into the
caveat but do not block.

## 2. The Gates

| Gate | Check | Default threshold | Severity |
|---|---|---|---|
| **G0** Calendar | The trading calendar module (`09-lock-state-machine` §4 rules plus overrides) equals the set of dates of the `SPY` bars from `BACKFILL_START` to the data end. The calendar is never rebuilt from prices; a difference is a finding about the module | 0 differing dates | HARD |
| **G1** Raw and manifest | No chunk in `failed` or `fetching`; every chunk `compacted` or `gone`; every `raw_sha256` in `prices`, `splits`, `dividends`, `etf_holdings` exists in the manifest; 5% sample of raw files re-hashed (full re-hash monthly) | 0 failed; 0 missing; 0 hash mismatches | HARD |
| **G2** Price sanity | Per row: `high >= low`, `low <= close <= high`, prices > 0, `volume >= 0`. Split-factor consistency: for each `splits` row, `close[ex] / close[ex-1] * ratio` lies in [0.4, 2.5]. Unexplained jumps: one-day as-traded ratio `>= 1.8` or `<= 0.55` with no split within one day. 10x jumps: ratio `>= 10` or `<= 0.1` without a split, classified by the rules of §2a; the ones no rule explains count when they fall within 365 days of an eligible `universe` row (all of them without a universe). Reconciliation: our adjusted 20-day ratio vs the vendor's `adj_close_vendor` ratio within 0.5% | Row failures <= 0.1% of rows; 0 unexplained ten-fold jumps near eligible rows; unexplained jumps <= 0.05% of ticker-days outside the rejected spans; reconciliation >= 99.5% | HARD for row failures and 10x jumps near eligible rows; WARN for the rest |
| **G3** Price completeness | For tickers with vendor data: priced days / expected trading days (from the calendar) between each ticker's first and last price date, by year. Also gaps longer than 5 trading days inside a series, listed | >= 98% per year in aggregate; per ticker >= 95% (list below); gaps > 5 days listed | HARD for the 98% aggregate |
| **G4** Survivorship coverage | `price_coverage` has a row for every ticker in `etf_holdings`; `gap` rows all have a `gap_reason`; the yearly missing-share table and the caveat file are produced. Missing share of member-days above `survivorship_warn_pct` (2%) in any year is a WARN printed in the caveat; above `survivorship_ack_pct` (10%) a model generation may not be promoted to production without an owner acknowledgement recorded in the report (decision D3) | Report always produced; thresholds in `config/quality_gates.toml` | HARD for completeness, WARN at 2%, owner-ack gate at 10% |
| **G5** Universe vs holdings | Unresolved identifiers <= 1% of snapshot ticker-rows (0.5% by weight); unparseable symbols <= 0.1%; every snapshot at most 45 days old at each date it is used; holdings count per snapshot within 25% of the trailing median (truncated-file detection); null sector <= 1% of member rows; earliest snapshot date and the resulting first eligible date reported | as stated | HARD |
| **G6** FRED completeness | Each series: no run of more than 5 missing business days (daily) or any missing month (monthly) since 2004; latest observation not older than its lag plus 5 days; `DGS3MO` has a value known (`known_from <= t`) for **every** trading day `t` in the panel range; the share of `lag_rule` rows per series is reported; deferred series (ICE BofA if history is short) are named | `DGS3MO` coverage 100%; others as stated | HARD for `DGS3MO`; WARN for others |
| **G7** Filings | the `validate-filings` rules (§4) pass; staleness: for eligible `(ticker, t)` from 2011-07-01, the latest `fundamentals.filing_date <= t` is at most 135 days old; the quarterly EDGAR `form.idx` lists no 10-K/10-Q of a universe CIK that has XBRL facts in `companyfacts` but is absent from `fundamentals` (`index_misses`); those with no facts at the source are listed apart (`index_no_xbrl`, WARN; D23); revenue-tag switches with a level jump above 30% listed; TTM null share reported | staleness >= 97% of eligible ticker-days; index misses <= 1%; validate-filings: 0 FAIL | HARD for validate-filings and index misses; WARN for the rest |
| **G8** Events | Every CIK of `security_master` has a submissions chunk; `items` is non-empty for >= 99% of 8-K rows; every ticker whose series ends more than 10 trading days before the data end has a `delistings` row or is listed as a gap | as stated | WARN |
| **G9** Replay determinism | Re-parse a random 2% of chunks from the landing zone: stage hashes equal the recorded ones; each lake partition's recorded `inputs_hash` equals the recomputed one | 0 differences | HARD |
| **G10** Cross-source | Only with a second price source: as-traded close relative difference above 0.5% on more than 0.1% of common days WARN, above 1% FAIL. With one source: reported "not applicable", never "passed" | n/a | WARN/HARD when active |

**Built in `quant-ingest gates` (2026-10-12): G0, G2, G7, G8.** First G0/G2 run over
24.7M bars: G0 0 differing dates; G2 rows 0.0099% (2,442), ten-fold jumps 8,076 (HARD FAIL:
91% in about 150 securities whose vendor series mixes two price levels on zero-volume days,
the `vendor_wrong_holder` pattern of `SURVIVORSHIP.md` §4), unexplained jumps 0.144%,
595 inconsistent splits, reconciliation 99.26%. G2 offenders are `data_quality` rows
(`entity` = FIGI, `chunk` = `{check} {date} {why}`).

### 2a. G2 jump classification (owner decision, 2026-10-12)

A ten-fold jump is a data error until a rule explains it; the explanation is a **span**, a
range of the security's bars G2 rejects. Spans are `data_quality` rows (`chunk` = `spans
{from} {to} {reason}`) of the G2 run; J9 reads the latest G2 run and excludes the rows inside
them from `universe` (`exclusion_reason = data_quality`, with the per-row failures) and from
`price_coverage`. Raw `prices` are never changed. Rules, in order (`gates::classify`):

| Reason | Rule | Evidence |
|---|---|---|
| `reviewed` | The owner excluded the range in `config/price_reviews.toml` | owner |
| `vendor_adjusted` | The size of the jump equals the product of the security's later split ratios within 8% (in log): the vendor scaled a block by it, up or down. The block runs from the jump to the move back by the same factor (within 16%, so not always ten-fold); without one, it is the block before the jump when the split shows in the closes (the bars up to the split are as traded), else from the jump to the split | `splits` |
| `foreign_quotes` | A jump undone by an opposite one within 20 bars: the bars between | prices |
| `vendor_wrong_holder` | Two or more such flips (a mixed series): the segments between jumps priced more than 3.16x off the segment with the most `filing_events` of the security's filer (then the longest) | prices, `filing_events` |
| `symbol_reused` | A jump within 20 bars of either end drops that stub; one after a gap of more than 30 days drops the side on which the filer has no filing (else the shorter side) | prices, `filing_events` |
| `relisted` | A jump after a gap of more than 30 days with filings on both sides (new equity after a reorganisation): only the jump bar, so no return crosses it | prices, `filing_events` |
| accepted | The owner accepted the jump as a real move in `config/price_reviews.toml`: not a jump | owner |

A jump no rule explains is an offender of `ten_fold` (HARD) when it lies within 365 days of
an eligible row of the last `universe`, else of `ten_fold_unscoped` (WARN); either note
names split evidence when there is some (an 8-K Item 5.03 from 45 days before to 10 after,
or shares outstanding moving against the price). The owner clears a HARD offender with an
`accept` or an `exclude` entry. The `jumps` check and the reconciliation skip rejected bars.
Not a rule: bars after a delisting are kept (D0 in `SURVIVORSHIP.md` §3 removes the false
delistings, but the remaining rows still include real companies that trade on).

First run (2026-10-12, after D0): 8,076 jumps leave 71 HARD and 343 WARN; 5,848 spans over
376K bars (`vendor_wrong_holder` 134K bars and 7,051 jumps, `vendor_adjusted` 104K and 326,
`symbol_reused` 63K and 158, `foreign_quotes` 157 and 116, `relisted` 11 and 11); about
37.5K eligible rows of 8.48M fall in spans, most of them other holders' prices (Terra
Industries' ticker after 2010, Corinthian Colleges under Vita Coco). The `vendor_adjusted`
blocks (22.9K eligible rows) are right in level ratios but wrong as raw prices; undoing the
adjustment in the EODHD parser would recover them. J10 must treat a return whose window holds
a rejected bar as missing.

Owner review 1 (2026-10-12): AGII (renamed ARGO in May 2018), AIP (listed October 2021),
ALR (Five Star's years scaled for the 2019 split, mixed with Alere's closes), AOI (bars after
the Pyxus rename) and ASRT (Depomed's history, scaled by the vendor) are `exclude` entries.
ASRT showed that the first `vendor_adjusted` rule assumed the vendor always scales up: it
rejected Assertio's as-traded 2018-2023 and kept the scaled block. The rule now finds the
side from the move back or the split; `vendor_adjusted` fell from 104K to 41K rejected bars
(TRQ: the 2009 block only, not 2009-2020). After the review: 65 HARD, 340 WARN.

## 3. Sketch SQL

Completeness by year (G3). `calendar` is a table the gate runner loads from
the calendar module, not derived from prices:

```sql
WITH span AS (
  SELECT ticker, MIN(date) AS d0, MAX(date) AS d1, COUNT(*) AS n_priced
  FROM prices GROUP BY ticker
),
expected AS (
  SELECT s.ticker, year(c.date) AS yr, COUNT(*) AS n_expected
  FROM span s JOIN calendar c ON c.date BETWEEN s.d0 AND s.d1
  GROUP BY s.ticker, year(c.date)
),
priced AS (
  SELECT ticker, year(date) AS yr, COUNT(*) AS n_priced FROM prices GROUP BY ticker, year(date)
)
SELECT e.yr,
       SUM(COALESCE(p.n_priced, 0))::DOUBLE / SUM(e.n_expected) AS completeness
FROM expected e LEFT JOIN priced p USING (ticker, yr)
GROUP BY e.yr ORDER BY e.yr;
```

Unexplained and ten-fold jumps (G2):

```sql
WITH p AS (
  SELECT ticker, date, close,
         LAG(close) OVER (PARTITION BY ticker ORDER BY date) AS prev_close
  FROM prices
),
j AS (
  SELECT ticker, date, close / prev_close AS r
  FROM p
  WHERE prev_close > 0 AND (close / prev_close >= 1.8 OR close / prev_close <= 0.55)
)
SELECT j.ticker, j.date, j.r, (j.r >= 10 OR j.r <= 0.1) AS ten_fold
FROM j
LEFT JOIN splits s
  ON s.ticker = j.ticker AND s.ex_date BETWEEN j.date - INTERVAL 1 DAY AND j.date + INTERVAL 1 DAY
WHERE s.ticker IS NULL
ORDER BY ten_fold DESC, j.ticker, j.date;
```

Filing staleness (G7):

```sql
SELECT COUNT(*) FILTER (WHERE date_diff('day', f.filing_date, u.date) <= 135)::DOUBLE
       / COUNT(*) AS fresh_share
FROM universe u
ASOF LEFT JOIN fundamentals f ON u.figi = f.figi AND u.date >= f.filing_date
WHERE u.eligible AND u.date >= DATE '2011-07-01';
```

Risk-free coverage (G6): every trading day `t` must have a `DGS3MO` value
known by `t` that is not older than 7 calendar days:

```sql
SELECT c.date, MAX(m.obs_date) AS latest_obs
FROM calendar c
LEFT JOIN macro m
  ON m.series_id = 'DGS3MO' AND m.known_from <= c.date AND m.value IS NOT NULL
WHERE c.date BETWEEN DATE '2008-01-02' AND (SELECT MAX(date) FROM calendar)
GROUP BY c.date
HAVING latest_obs IS NULL OR date_diff('day', latest_obs, c.date) > 7;
```

## 4. `validate-filings`: Rule Order and Semantics

The G7 validator is `quant-ingest gates g7` (`ingest/src/gates.rs`, one
`data_quality` row per rule, `chunk = validate_filings.rN`); `scripts/validate-filings.jl`
is the earlier sketch of the same rules. Rule **order matters**: null checks run first so no comparison
meets a `missing`. Exit status: 1 if any FAIL rule fires, 0 otherwise
(WARN rules only print).

| # | Rule | Severity |
|---|---|---|
| 1 | `filing_date` and `period_end` are non-null | FAIL |
| 2 | `filing_date >= period_end` | FAIL |
| 3 | No `filing_date` after the validation date (default today) | FAIL |
| 4 | `(figi, accession)` unique | FAIL |
| 5 | `amends_accession` is null or an existing `accession` | FAIL |
| 6 | `fiscal_period` in `Q1`, `Q2`, `Q3`, `FY` (or null) | FAIL |
| 7 | `acceptance_datetime`, when present, falls on `filing_date` or the business day before | WARN |
| 8 | Gaps of more than 120 days between consecutive periodic filings of one ticker | WARN |

## 5. What Not to Do

- Do not let a gate pass because it could not run. A gate that cannot
  evaluate reports FAIL (or "not applicable" where that is defined, G10).
- Do not lower a threshold to make a build pass without recording it in the
  caveat.
- Do not derive the expected trading days from the prices being checked.
- Do not treat a missing price as 0 or carry the previous one forward.

## 6. Verification

- [ ] A fixture lake with one planted defect per gate makes that gate fail
      and no other
- [ ] `preflight()` aborts when a HARD gate has no record for the data end
- [ ] G10 is "not applicable" with one source
- [ ] The thresholds in `config/quality_gates.toml` are echoed in the caveat
