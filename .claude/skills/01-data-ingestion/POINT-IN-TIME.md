# Point-in-Time Discipline

The single most important rule in ingestion. Violating it introduces
lookahead bias that no downstream test can catch.

## The Rule

A data point is usable at decision time `t` only if it was publicly
available on or before `t`. Availability is determined by publication
date, not observation date. (Constitution §2: the constitution wins over
this file.)

## Per-Source Semantics

### SEC Filings

| Field | Meaning | Use for |
|---|---|---|
| `filing_date` | Date the filing became public on EDGAR | Point-in-time stamp |
| `period_end` | Date the fiscal period ended | Metadata only (ingestion arithmetic) |
| `acceptance_datetime` | Exact timestamp SEC accepted it | Finer stamping; audit |

**The 10-Q example:** filed 2024-02-15 for period ending 2023-12-31.
- Usable starting 2024-02-15.
- Not usable on 2024-02-14, even though the quarter ended 2023-12-31.
- Not usable on 2023-12-31, even though the numbers reflect that period.

**Same-day filings.** The constitutional rule is `filing_date <= t`. A filing
accepted after the daily decision time on `t` (the decision pipeline runs
from 16:10 ET; EDGAR dates filings accepted after 17:30 ET to the next
business day, verify) is counted as known on `t`. That window is at most
about 80 minutes of a decision made the same evening, and orders go out in
the next session; it is accepted and documented, not hidden. A stricter
`usable_date` from `acceptance_datetime` would be a constitutional
clarification (proposed in the report accompanying this change).

**First-reported values and restatements.** A `fundamentals` row is what one
accession reported for its own period. Rows are never overwritten:

- The earliest `filed` per fact is the as-first-reported value.
- A restated figure is a separate row (an amendment `10-K/A` / `10-Q/A`,
  with its own `filing_date` and `amends_accession`), usable from its own
  filing date. The original stays usable from its own date.
- A restated comparative inside a later filing creates no row and enters
  only later TTM and prior-period computations, as known at that later
  filing's date.

So a backtest at `t` sees the figures a trader would have seen at `t`,
including the original error before the correction.

### FRED / ALFRED Series

FRED's `date` is the period the value describes, not the date it was
released. ALFRED supplies, per observation, the vintage intervals
(`realtime_start`, `realtime_end`). The lake stores them in long form with a
computed `known_from` (`SCHEMAS.md` #4):

    known_from =
      first vintage of the observation, captured contemporaneously:
            max(realtime_start, obs_date + min_lag_days)      known_from_basis = alfred
      first vintage of the observation, captured long after publication:
            last_day_of_observation_period + buffer            known_from_basis = lag_rule
      a later (revised) vintage:
            realtime_start                                     known_from_basis = alfred

"Captured long after publication" means `realtime_start - obs_date` above
`max_capture_delay` (default 120 days, `config/macro_series.toml`); it marks
history that ALFRED reconstructed rather than archived. For such an
observation the revision history before capture is unavailable and the
stored value may already be a revised one: a small residual lookahead, flagged
by the basis column and counted by gate G6.

**Which value to use at `t` (the as-of-`t` rule).** Among rows with
`known_from <= t`, take the latest observation (`obs_date`), and for it the
vintage with the greatest `realtime_start`: the value as it stood at `t`,
revisions included only once published. The alternative, "as first
published" (the smallest `realtime_start`), is stable but ignores revisions
the live system would have seen; it is used only where the file says so.
Neither ever uses `obs_date` as the availability date.

**Floors and buffers** (`min_lag_days` = floor between observation date and
`known_from`; `buffer` = days after the **end** of the observation period
used when the vintage cannot be trusted). All values are my best knowledge
of the release practice and **verify** against the series' release pages:

| Series | Release practice (verify) | `min_lag_days` | Fallback buffer |
|---|---|---|---|
| `DGS3MO`, `DGS2`, `DGS10` | H.15 selected interest rates, daily; FRED posts the prior business day's values in the afternoon. The earlier text said "same day, 16:15 ET"; that is not established. Assume value for day `d` is known on `d + 1` | 1 | 1 day |
| `BAMLH0A0HYM2`, `BAMLC0A0CM` | ICE BofA, daily, posted with a one-business-day lag | 1 | 1 day |
| `VIXCLS` | CBOE close, posted by FRED with a lag of about a day | 1 | 1 day |
| `CPIAUCSL` | BLS CPI, about the 10th-15th of the next month; seasonal factors revised annually | 1 (the real lag is weeks; the vintage date carries it) | 45 days after month-end |
| `FEDFUNDS` | Monthly average, posted at the start of the following month | 1 | 30 days after month-end |
| `UNRATE` | Employment Situation, usually the first Friday of the next month; occasionally delayed | 1 | 14 days after month-end |

`min_lag_days` is a floor only: when ALFRED's vintage date is later, the
vintage date wins. The conservative buffer is used only for the
late-captured case. If a series' real publication is later than its buffer,
the buffer is wrong and is updated in `config/macro_series.toml`.

**The risk-free rate in the targets** is `DGS3MO` as-of-`t`: for `t` it is
the observation dated at most `t - 1` (the one published by `t`). It is never
the observation dated `t` itself, and never a value filled from the future.
Because every target uses it, this is the macro input that matters most
(`02-panel-construction/TARGETS.md` §1).

### Prices (EODHD, D15)

- **Close on date `t`:** usable at 16:00 ET on `t`. Open, high, low and
  volume on `t` are usable at the same time (the bar is complete).
- **Stored prices are as traded.** They do not change when a later split
  happens (`SOURCES.md`). The only permitted change to a stored bar is a
  vendor correction found by the overlap or monthly reconciliation fetch,
  logged with before and after in `data_quality` (`BACKFILL.md` §5.1).
- **Split and dividend events** are usable from their `ex_date` for the
  purpose of aligning shares and computing adjusted series at `t >= ex_date`.
  An announcement before the ex-date is not stored; using an ex-date `> t`
  in a feature computed at `t` would be lookahead.
- **Adjusted series.** A level of a split- or dividend-adjusted price uses
  events after `t` and is not point-in-time. A ratio of adjusted prices
  between two dates `<= t` uses only events in between and is
  (`02-panel-construction/FEATURES.md` §0). So momentum is fine and any use
  of an adjusted level as a feature, or any comparison of adjusted levels
  across tickers, is not.
- **Vendor snapshots** (`Ticker.info`, calendar) are not ingested.

### iShares Holdings

- The CSV header contains an "as of" date. That is `snapshot_date`.
- Usable from `available_date` = the next trading day after `snapshot_date`
  (conservative; the publication time of the file is **verify**).
- Between snapshots the membership is carried forward from the latest usable
  snapshot; its age is recorded in `universe.snapshot_age_days`.

### EDGAR Events

A `filing_events` row is usable from `filing_date`. A delisting known only
from a Form 25 filed after `t` is not known at `t`, so it never enters a
feature or an eligibility flag at `t`. It may enter **labels** (terminal
values, `SURVIVORSHIP.md`) because labels use the future by definition.

## The `period_end` Exception (ingestion only)

The constitution bans `period_end` as a decision variable and the pre-commit
hook bans the identifier in decision code. Ingestion legitimately needs it
to pair fiscal periods (YTD to quarter, TTM, "same period a year earlier")
and to define the fiscal period of a row. The exception path:

1. All use is inside the Rust crate `ingest/` (module `fundamentals`),
   covered by its tests. The hook's scan of decision directories
   (`features/`, `models/`, `backtest/`, `portfolio/`, `conviction/`) does
   not scan `ingest/`, so no change is needed while the hook is scoped that
   way.
2. If the hook is ever widened to "anywhere except ingestion", the allowed
   set is exactly: `ingest/**`, `schemas/**` (registry tooling),
   `migrations/**`, and `.claude/skills/01-data-ingestion/scripts/validate-filings.jl`.
   Nothing under `features/`, `models/`, `backtest/`, `portfolio/`,
   `conviction/`.
3. The lake column `period_end` stays in `fundamentals` as metadata. Decision
   code reads fundamentals by explicit column list that excludes it; the
   derived columns it needs (`*_ttm`, `prior_*`, `fiscal_period`) are in the
   table so decision code never recomputes period arithmetic.
4. SQL in this repository that mentions `period_end` is limited to the
   validation queries above (`QUALITY-GATES.md`).

The lock worker owns the hook; the required exception list is reported with
this change and is empty for the current scope.

## Implementation

### In joins

Every join between price data and any other data uses an as-of join on the
availability date:

```sql
-- Correct: as-of join on filing_date
SELECT p.ticker, p.date, f.revenue_ttm
FROM prices p
ASOF LEFT JOIN fundamentals f
  ON p.figi = f.figi AND p.date >= f.filing_date;

-- Equivalent, portable form
SELECT p.ticker, p.date, f.revenue_ttm
FROM prices p
LEFT JOIN fundamentals f
  ON p.figi = f.figi AND f.filing_date <= p.date
QUALIFY ROW_NUMBER() OVER (
  PARTITION BY p.ticker, p.date
  ORDER BY f.filing_date DESC, f.accession DESC
) = 1;

-- Wrong: equality join on period_end
SELECT p.ticker, p.date, f.revenue_ttm
FROM prices p
LEFT JOIN fundamentals f
  ON p.figi = f.figi AND f.period_end = p.date;
```

The third query is a lookahead violation. It uses data that was not public on
`p.date`.

### In feature construction

A feature computed "for date `t`" uses only data with availability date
`<= t`. The linter catches `period_end` in decision code, but ingestion code
must set the standard.

### In backtests

The backtest engine reads the ingestion output. If ingestion is correct, the
backtest is correct (with respect to point-in-time). If ingestion has
lookahead, the backtest inherits it and no amount of downstream testing will
catch it.

## Verification

`scripts/validate-filings.jl` scans `fundamentals` and asserts (rule order,
severities and exit codes are in the script header and `QUALITY-GATES.md`):

- Every row has a non-null `filing_date` and `period_end` (checked first)
- Every row has `filing_date >= period_end`
- No `filing_date` is after the validation date
- `(figi, accession)` is unique; `amends_accession` points at an existing
  accession
- `acceptance_datetime` falls on `filing_date` or the business day before
  (warning)
- Gaps between consecutive filings of one ticker above 120 days (warning)

`QUALITY-GATES.md` G6 and G7 cover macro and filing staleness. Run them
after every ingestion job. A FAIL is a stop-the-line event.

## What to Do When a New Source Is Added

Before writing any ingestion code for a new source, answer:

1. What is the publication date? Is it provided by the source, or must it be
   inferred?
2. Is the source restated (like SEC amendments and FRED vintages) or
   immutable (like completed trading data)?
3. What is the update cadence? Does it match the fetch schedule?
4. What is the failure mode? Does the source silently return stale data, or
   does it error?
5. Does it cover delisted entities, and what happens to its identifiers over
   time (survivorship, symbol reuse)?

Document the answers in this file before writing code. The code should
implement exactly what this file says.
