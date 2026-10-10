---
name: portfolio-ledger
description: Daily portfolio state and lot-level tax tracking. Load when
  touching ledger/, tax_lots, NAV series, realized P&L, or any code that
  computes cost basis or holding periods.
---

# Portfolio Ledger

The ledger is the system's book of record. It answers the question "what
was the portfolio worth, and how did it get there?" in a single daily
row. It is the source of truth for performance, taxes, and reconciliation.

## 1. What the Ledger Contains

Two files:

**`portfolio_ledger.parquet`** — one row per trading day, written after
market close, after reconciliation.

**`tax_lots.parquet`** — one row per lot, updated on every fill.

Neither file is overwritten. Both are append-only. The ledger grows by
one row per day. Tax lots grow by one row per buy and are updated in
place on partial sells (a new row for the closed portion, the original
row updated for the remaining open portion).

**Fractional quantities.** In the Bootstrap and Transition tiers (see
`00-construction` §6a), positions and fills are fractional. The ledger
stores all quantities and prices as `float64`. The `cash` column reflects
the full cost of market-order spreads. The `daily_return` column
therefore includes spread costs in tiers where market orders are used.

**Amendment to the constitution:** The "append-only" rule of §5 applies
to `audit_log.parquet` specifically. The ledger is append-only for new
days; tax lots use a documented update pattern for partial closes. See
`TAX-LOTS.md` §4.

## 2. Why the Ledger Exists

Five things the system cannot do without it:

1. **Tax reporting.** Realized gains must be tracked by lot with holding
   period. Without lots, no Schedule D.
2. **Performance attribution.** Drawdown and Sortino require a daily NAV
   series. `positions.parquet` only has today.
3. **Broker reconciliation.** Schwab provides a daily statement with
   cash and cost basis. Without a local ledger, reconciliation only
   checks share counts.
4. **Lock forensics.** The 5-day lock is recomputed from order history, not
   from the ledger (`09-lock-state-machine`). The ledger and `tax_lots` show
   the fills behind a given position, which helps explain why an order was
   blocked; they are never an input to `lock::authorize`.
5. **Model evaluation in live.** Coverage and Winkler are model metrics.
   A daily NAV series is the portfolio-level equivalent.

## 3. The Daily Write Protocol

The ledger is written once per trading day, at 16:25 ET, after
reconciliation. See `LEDGER-PROTOCOL.md` for the full sequence.

The write is **idempotent**. If the job runs twice for the same date,
the second run overwrites the row with identical content. The row's
`date` is the primary key.

## 4. The Tax Lot Protocol

Tax lots are updated on every fill. The accounting method is **FIFO by
default**. See `TAX-LOTS.md`.

On a buy: a new lot is created with `close_date = null`.
On a sell: the oldest open lots for that ticker are closed in order
until the sold quantity is consumed. Each closed portion produces a
`realized_pnl` entry. Partial closes leave the original lot with reduced
`open_quantity`.

## 5. The Relationship to Other Files

| File | Role | Relationship to ledger |
|---|---|---|
| `positions.parquet` | Current snapshot | Ledger is the historical series |
| `orders.parquet` | Order lifecycle | Ledger aggregates orders into daily state |
| `fills.parquet` | Executions | Ledger's tax lots come from fills |
| `audit_log.parquet` | Event log | Ledger is the state; audit is the events |
| `evaluation_metrics.parquet` | Model metrics | Ledger holds portfolio metrics |
| `shadow_fills.parquet` | Simulated fills | Ledger reflects real fills, not simulated |

**Cross-check:** Every ledger row's position count and cash balance must
match the same-day `positions.parquet` and the Schwab statement. If they
disagree, trading halts. The reconciliation job (Rust, `execution/src/risk/reconciliation.rs`,
specified in `10-execution-schwab` §6c; the earlier Julia script is removed) does this check.
The ledger is written only after a clean reconciliation; a reconciliation that is only
`INCOMPLETE` (an outage, not a mismatch) leaves the day's row unwritten until a clean re-run.

**Spread cost note.** In the Bootstrap and Transition tiers, market
orders pay the full spread. The `daily_return` column reflects this
cost. A bootstrap-tier account's returns are systematically lower than a
standard-tier account's returns by the average spread per trade. This is
expected. The backtest protocol (`14-backtest-protocol` §6a) models the
same cost structure, so live and simulated results are comparable.

## 6. Query Patterns

See §7 for the common queries. The two most frequent:

**Current drawdown:**

```sql
WITH peak AS (
  SELECT MAX(total_value) AS peak_value
  FROM portfolio_ledger
  WHERE date >= current_date - INTERVAL 252 DAY
)
SELECT
  l.date,
  l.total_value,
  (l.total_value - p.peak_value) / p.peak_value AS drawdown
FROM portfolio_ledger l, peak p
WHERE l.date = (SELECT MAX(date) FROM portfolio_ledger);
```

**Year-to-date realized P&L:**

```sql
SELECT
  SUM(realized_pnl) AS ytd_realized
FROM tax_lots
WHERE close_date >= DATE_TRUNC('year', current_date)
  AND close_date IS NOT NULL;
```

## 7. What Not to Do

- Do not overwrite a previous ledger row. Each day is immutable.
- Do not write the ledger before reconciliation. Reconciliation
  confirms the state; the ledger records it.
- Do not use average cost basis for tax lots unless the account owner
  has elected it. FIFO is the default.
- Do not compute realized P&L from `orders.parquet`. It requires lot
  matching. Use `tax_lots.parquet`.
- Do not conflate `daily_return` (portfolio level) with the average of
  position returns. It is `total_value_t / total_value_{t-1} − 1`.
- Do not write the ledger on non-trading days. There is no row for
  weekends or holidays.
- Do not store fractional quantities as integers. The schema uses
  `float64` for a reason.

## 8. Verification

- [ ] A test asserts the ledger is written after reconciliation
- [ ] A test asserts one row per trading day, no duplicates
- [ ] A test asserts FIFO lot matching on a partial close
- [ ] A test asserts realized P&L matches sum of closed lots
- [ ] A test asserts `daily_return` uses prior day's total_value
- [ ] A test asserts ledger total_value matches positions + cash
- [ ] A test asserts fractional quantities are stored as `float64`
- [ ] A test asserts the `cash` column includes market-order spread cost
      in the Bootstrap and Transition tiers

## 9. Related Skills

- `10-execution-schwab` — produces the fills that update tax lots
- `00-construction` §8 — the audit log contract (extended to the ledger)
- `13-parquet-lake` — schemas for both files
- `14-backtest-protocol` — the simulated version of this ledger