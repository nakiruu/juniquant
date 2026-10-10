# Ledger Write Protocol

The daily sequence for writing the portfolio ledger.

## 1. Timing

The ledger is written at **16:25 ET** on trading days, after the
Schwab daily statement is available and after reconciliation completes.
The write must not happen before reconciliation, because the ledger
records the reconciled state, not a pre-reconciliation guess.

## 2. The Full Daily Sequence

```
16:00 ET  Market close
16:05     Schwab daily statement available via API
16:10     Ingest today's prices (from prices.parquet, already ingested)
16:15     Reconcile positions (Rust: 10-execution-schwab §6c, execution/src/risk/reconciliation.rs)
16:20     Update tax lots from today's fills (scripts/update-lots.rs)
16:22     Build trade summary rows for round-trips closed today (18-trade-summary)
16:25     Write portfolio_ledger row for today (scripts/write-ledger.jl)
16:30     Run model inference for tomorrow (03-gspqr-model)
16:40     Daily lock-compliance audit and dated report (09-lock-state-machine §8; retries to 17:40)
16:50     Write the lock snapshot for the next session (Rust: $QUANT_STATE/lock_snapshot.json, 09 §8a)
17:00     Run portfolio optimization for tomorrow (08-portfolio-optimization; reads the 16:50 snapshot)
17:30     Next-session order plan (intents only; nothing is sent to Schwab)
09:00 +1  Pre-open refresh (token, calendar-sync incl. today, lock history refresh, cost/quote-cache warm-up)
10:00-15:30 +1  Execution window: orders placed in session only (10-execution-schwab/EXECUTION-WINDOW.md)
18:00     Audit export, then nightly rsync to array (13-parquet-lake/UNRAID-STORAGE.md)

The 16:22, 16:40 and 16:50 slots are also in the `outline.md` daily schedule
(merged 2026-10-10).
```

## 3. Preconditions

Before writing the ledger:

- [ ] Reconciliation passed (no mismatches with Schwab)
- [ ] Tax lots are up to date (all of today's fills processed)
- [ ] Today's prices are ingested
- [ ] Today is a trading day (not a weekend or holiday)

If any precondition fails, the ledger write is skipped and a `CRITICAL`
alert is logged. The ledger for the day remains unwritten. Backfilling
happens manually after the root cause is fixed.

## 4. The Write

The write is a single row insert into `portfolio_ledger.parquet`. The
file is partitioned by year, so the write appends to the current year's
partition.

```julia
function write_ledger_row(date::Date)
    positions = read_positions(date)
    cash = read_cash_balance(date)
    prev = read_previous_ledger_row(date)

    market_value = sum(positions.market_value)
    total_value = cash + market_value
    cost_basis = sum(positions.cost_basis)
    unrealized = market_value - cost_basis

    realized_today = sum_realized_today(date)
    realized_ytd = sum_realized_ytd(date)

    daily_return = prev === nothing ? 0.0 :
                   (total_value / prev.total_value) - 1.0

    cumulative_return = prev === nothing ? 0.0 :
                        (total_value / prev.first_value) - 1.0

    max_dd = compute_trailing_drawdown(total_value)

    row = (
        date = date,
        cash = cash,
        cash_pct = cash / total_value,
        market_value = market_value,
        total_value = total_value,
        cost_basis = cost_basis,
        unrealized_pnl = unrealized,
        realized_pnl_today = realized_today,
        realized_pnl_ytd = realized_ytd,
        n_positions = nrow(positions),
        daily_return = daily_return,
        cumulative_return = cumulative_return,
        max_drawdown = max_dd,
        sortino_252d = compute_sortino_252d(),
        regime_factor = read_regime_factor(date),
        reconciled = true,
    )

    append_to_ledger(row)
end
```

## 5. Idempotency

If the job runs twice for the same `date`, the second run:

1. Reads the existing row for that date.
2. Compares it to the newly computed row.
3. If identical, no-op.
4. If different, overwrites and logs a `WARN` with both values.

The overwrite case is a bug indicator. It means the underlying state
changed between runs (e.g., a late fill arrived). The `WARN` triggers a
manual review.

## 6. Backfill

If the ledger for a date is missing:

1. Do not run the normal job. It writes today's row only.
2. Use the manual backfill script with the specific date.
3. The script reads historical positions from the audit log and
   reconstructs the state.
4. The backfilled row is marked with `backfilled = true` (a column
   that defaults to false).

Backfilled rows are legitimate but flagged, so that any analysis can
exclude them if needed.

## 7. What Not to Do

- Do not write the ledger before reconciliation. The state may be wrong.
- Do not write on non-trading days.
- Do not skip the idempotency check.
- Do not backfill without flagging.
- Do not compute drawdown from a partial series. Use the trailing 252
  days.

## 8. Verification

- [ ] A test asserts the row has all 17 columns
- [ ] A test asserts the daily return uses the prior row
- [ ] A test asserts the idempotency path produces no duplicate
- [ ] A test asserts backfill marks the row with `backfilled = true`