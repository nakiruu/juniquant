# Query Patterns

Common DuckDB queries against the lake. Use these for debugging and for the
dashboard. Production queries live in Julia or Rust and are tested.

Paths below are written `$LAKE/...`, where `$LAKE` is `$QUANT_ROOT/lake` (`IO-CONTRACT.md` §1, the single definition; in the `quant-core` and `duckdb` containers that is `/data/lake`). DuckDB does not expand shell variables inside a string literal, so the caller substitutes `$LAKE` (or uses `getenv('QUANT_ROOT') || '/lake/...'` where the DuckDB build supports `getenv`; verify). Partitioned datasets are read with
`**/data.parquet` and `hive_partitioning = true`; single datasets with
`<name>.parquet`.

## Latest Prediction for a Ticker

```sql
WITH latest AS (
  SELECT ticker, MAX(date) AS date
  FROM read_parquet('$LAKE/gspqr_predictions/**/data.parquet', hive_partitioning = true)
  WHERE ticker = 'AAPL'
  GROUP BY ticker
)
SELECT
  p.ticker, p.date, p.horizon,
  p.q_05, p.q_50, p.q_95,
  c.q_hat_lower, c.q_hat_upper,
  e.q_extreme
FROM read_parquet('$LAKE/gspqr_predictions/**/data.parquet', hive_partitioning = true) p
JOIN latest l ON p.ticker = l.ticker AND p.date = l.date
LEFT JOIN read_parquet('$LAKE/tcp_calibration/**/data.parquet', hive_partitioning = true) c
  ON p.ticker = c.ticker AND p.date = c.forecast_date
  AND p.horizon = c.horizon
LEFT JOIN read_parquet('$LAKE/evt_tail_bounds/*/data.parquet', hive_partitioning = true) e
  ON p.ticker = e.ticker AND p.date = e.fit_date
  AND p.horizon = e.horizon
WHERE p.horizon = '63d';
```

## Lock Status for a Ticker (DISPLAY ONLY)

`position_locks` is a derived, non-authoritative view (SCHEMAS.md #14).
Never use this query to gate an order: the lock is recomputed from order
history at submission time (`09-lock-state-machine`). `lock_expiry_date` here
is the first permitted day, so the day itself is permitted (`<=` below).

**A null `lock_expiry_date` (or a null `first_permitted` in the lock snapshot)
on a ticker with any counted order is an OPEN order: blocked, with no end
date. It is never displayed as "unlocked".** Only a ticker with no counted
order at all (both `last_buy_date` and `last_sell_date` null) shows "no
counted order"; even that label is display only.

```sql
SELECT
  ticker, state, lock_expiry_date, shares_held, pending_side,
  CASE
    WHEN last_buy_date IS NULL AND last_sell_date IS NULL
         AND lock_expiry_date IS NULL      THEN 'no counted order'
    WHEN lock_expiry_date IS NULL          THEN 'blocked (open order, no end date)'
    WHEN lock_expiry_date <= current_date  THEN 'expired'
    ELSE 'blocked until ' || strftime(lock_expiry_date, '%Y-%m-%d')
  END AS lock_status
FROM read_parquet('$LAKE/position_locks.parquet')
WHERE ticker = 'AAPL';
```

## Eligible Universe on a Date

`universe` is a daily table (SCHEMAS.md #1): one row per member per day, so
there are no effective/end dates to range-filter.

```sql
WITH u AS (
  SELECT ticker, sector
  FROM read_parquet('$LAKE/universe/**/data.parquet', hive_partitioning = true)
  WHERE date = DATE '2024-06-14' AND eligible
),
v AS (
  SELECT ticker
  FROM read_parquet('$LAKE/veto_flags.parquet')
  WHERE as_of_date = DATE '2024-06-14'
    AND veto_active = false
)
SELECT u.ticker, u.sector
FROM u JOIN v USING (ticker)
ORDER BY u.ticker;
```

## Membership Spans of a Ticker (derived effective/end dates)

```sql
WITH u AS (
  SELECT ticker, date, eligible
  FROM read_parquet('$LAKE/universe/**/data.parquet', hive_partitioning = true)
),
days AS (
  SELECT date, DENSE_RANK() OVER (ORDER BY date) AS di FROM (SELECT DISTINCT date FROM u)
),
e AS (
  SELECT u.ticker, u.date,
         d.di - ROW_NUMBER() OVER (PARTITION BY u.ticker ORDER BY u.date) AS grp
  FROM u JOIN days d USING (date)
  WHERE u.ticker = 'AAPL' AND u.eligible
)
SELECT ticker, MIN(date) AS effective_date, MAX(date) AS last_date
FROM e GROUP BY ticker, grp ORDER BY effective_date;
```

Gaps-and-islands over the table's own trading days (`universe` has a row for
every member on every trading day). A span ends on the last eligible day.

## Survivorship: Missing Share by Year (the SURVIVORSHIP CAVEAT table)

Denominator is membership (price-independent), numerator is member-days
without a price, so names that vanished from the vendor still count
(`01-data-ingestion/SURVIVORSHIP.md` §4).

```sql
SELECT
  year(date) AS year,
  COUNT(*)                                             AS member_days,
  COUNT(*) FILTER (WHERE exclusion_reason = 'no_price_data') AS missing_days,
  ROUND(100.0 * COUNT(*) FILTER (WHERE exclusion_reason = 'no_price_data')
        / COUNT(*), 2)                                 AS missing_pct_by_count,
  ROUND(100.0 * SUM(COALESCE(weight_iwf, weight_iwp, 0))
        FILTER (WHERE exclusion_reason = 'no_price_data')
        / NULLIF(SUM(COALESCE(weight_iwf, weight_iwp, 0)), 0), 2) AS missing_pct_by_weight
FROM read_parquet('$LAKE/universe/**/data.parquet', hive_partitioning = true)
GROUP BY 1 ORDER BY 1;
```

Weight uses `weight_iwf` where present, else `weight_iwp` (a ticker in both
ETFs is counted once, by its IWF weight).

## Macro as of a Date (wide view of the long `macro` table)

The value of each series known at `:t`, as of `:t` (the vintage current on
`:t`, only rows with `known_from <= :t`). This replaces the old wide table.

```sql
WITH k AS (
  SELECT series_id, obs_date, value, realtime_start,
         ROW_NUMBER() OVER (PARTITION BY series_id
                            ORDER BY obs_date DESC, realtime_start DESC) AS rn
  FROM read_parquet('$LAKE/macro.parquet')
  WHERE known_from <= DATE '2024-06-14'
    AND realtime_start <= DATE '2024-06-14'
    AND value IS NOT NULL
)
SELECT series_id, obs_date, value
FROM k WHERE rn = 1 ORDER BY series_id;
```

For the latest observation, `ORDER BY obs_date DESC, realtime_start DESC`
picks the newest observation and, within it, the newest vintage known on the
date. `yield_curve_slope` is `DGS10 - DGS2` computed on read.

## Fundamentals as of a Date (point-in-time join)

DuckDB `ASOF JOIN`: for each price row, the latest filing with
`filing_date <= date`. Never an equality join on `period_end`
(constitution §2).

```sql
SELECT p.ticker, p.date, f.revenue_ttm, f.shares_outstanding
FROM read_parquet('$LAKE/prices/**/data.parquet', hive_partitioning = true) p
ASOF LEFT JOIN read_parquet('$LAKE/fundamentals.parquet') f
  ON p.figi = f.figi AND p.date >= f.filing_date
WHERE p.date BETWEEN DATE '2024-06-01' AND DATE '2024-06-14';
```

Several rows with equal `filing_date` (an original and an amendment filed
the same day) resolve arbitrarily in an ASOF join; production code breaks the
tie by `accession` order (verify the join's tie behaviour before relying on
it).

## Split-Adjusted Close (debugging only)

```sql
WITH s AS (
  SELECT ticker, ex_date, ratio FROM read_parquet('$LAKE/splits.parquet')
)
SELECT p.date, p.close,
       p.close / COALESCE(
         (SELECT EXP(SUM(LN(s.ratio))) FROM s
           WHERE s.ticker = p.ticker AND s.ex_date > p.date), 1.0) AS split_adj_close
FROM read_parquet('$LAKE/prices/**/data.parquet', hive_partitioning = true) p
WHERE p.ticker = 'AAPL' AND p.date BETWEEN DATE '2020-08-24' AND DATE '2020-09-04';
```

Production adjusted series are built in Julia (`02-panel-construction/FEATURES.md`
§0); this query is for eyeballing. The factor uses splits after the date, so
the level is not point-in-time; ratios of adjusted prices are
(`FEATURES.md` §0).

## Coverage Over Trailing 60 Days

```sql
WITH joined AS (
  SELECT
    r.ticker, r.date, r.horizon,
    r.realized_return,
    c.q_hat_lower, c.q_hat_upper
  FROM read_parquet('$LAKE/evt_residuals/**/data.parquet', hive_partitioning = true) r
  JOIN read_parquet('$LAKE/tcp_calibration/**/data.parquet', hive_partitioning = true) c
    ON r.ticker = c.ticker
    AND r.date = c.forecast_date
    AND r.horizon = c.horizon
  WHERE r.date >= current_date - INTERVAL 60 DAY
)
SELECT
  horizon,
  AVG(CASE WHEN realized_return BETWEEN q_hat_lower AND q_hat_upper
      THEN 1.0 ELSE 0.0 END) AS coverage_95
FROM joined
GROUP BY horizon;
```

## Regime Distribution Over Time

```sql
SELECT
  viterbi_state,
  COUNT(*) AS days,
  ROUND(COUNT(*) * 100.0 / SUM(COUNT(*)) OVER (), 2) AS pct
FROM read_parquet('$LAKE/regime_states.parquet')
WHERE date >= current_date - INTERVAL 252 DAY
GROUP BY viterbi_state
ORDER BY days DESC;
```

## Pending Orders Near Lock Expiry

```sql
SELECT
  pending_id, ticker, side, target_quantity,
  earliest_execution,
  earliest_execution - current_date AS days_until
FROM read_parquet('$LAKE/pending_orders.parquet')
WHERE status = 'Pending'
  AND earliest_execution BETWEEN current_date AND current_date + INTERVAL 5 DAY
ORDER BY earliest_execution;
```

## Veto History for a Ticker

```sql
SELECT
  as_of_date, altman_z, going_concern, veto_active
FROM read_parquet('$LAKE/veto_flags.parquet')
WHERE ticker = 'XYZ'
  AND as_of_date >= current_date - INTERVAL 365 DAY
ORDER BY as_of_date DESC;
```

## Audit Trail for an Order

```sql
SELECT timestamp, event_type, actor, details, severity
FROM read_parquet('$LAKE/audit_log/**/data.parquet', hive_partitioning = true)
WHERE details LIKE '%o001%'
ORDER BY timestamp;
```

This sees exported (closed) days only. Rows from the current day are in the
SQLite journal until the nightly export; read them with a read-only
connection to `audit.db` (`mode=ro`), never from the lake.

## Anti-Patterns

    Do not SELECT * on partitioned datasets. Always specify columns. The
    predicate pushdown is real, but column pruning is also real.

    Do not query without a date filter on partitioned datasets. Reading 15
    years of daily data to answer a "latest" question is wasteful.

    Do not use read_parquet on a specific leaf file in production code. Use
    the glob pattern and let DuckDB prune.

    Do not write DDL or DML from DuckDB. The lake is read-only from
    DuckDB's perspective. Writes go through Julia or Rust
    (IO-CONTRACT.md).
