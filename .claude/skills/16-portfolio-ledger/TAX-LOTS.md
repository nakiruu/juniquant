# Tax Lots

Lot-level tracking for cost basis and realized P&L. FIFO by default.

## 1. Why Lot-Level

Average cost basis is a valid accounting method but loses information.
Lot-level tracking allows:

- FIFO, LIFO, or specific identification
- Short-term vs long-term holding period classification
- Wash sale detection (not enforced here, but the data is present)
- Audit trail for every share from purchase to sale

The pipeline uses **FIFO**. It is the default for tax purposes in the
US unless the taxpayer elects specific identification.

## 2. The Lot Lifecycle

**On a buy:**

A new row is written to `tax_lots.parquet` with:

- `open_date` = fill date
- `open_price` = fill price
- `open_quantity` = fill quantity
- `close_date` = null
- `close_price` = null
- `close_quantity` = 0
- `realized_pnl` = null
- `holding_days` = 0
- `order_id_open` = the buy order's ID
- `order_id_close` = null

**On a sell:**

The oldest open lots for that ticker (by `open_date`, then by
`lot_id` for same-day lots) are closed in order:

1. Sort open lots for the ticker by `open_date` ascending, then `lot_id`.
2. For each lot, close `min(lot.open_quantity, remaining_to_sell)` shares.
3. If the lot is fully closed, update `close_date`, `close_price`,
   `close_quantity`, `realized_pnl`, `holding_days`, `order_id_close`.
4. If the lot is partially closed, split it:
   - Update the original lot's `open_quantity` to the remaining shares.
   - Write a new row for the closed portion, with the same `open_date`
     and `open_price` but with `close_date`, `close_price`, etc.

## 3. Realized P&L

For a closed portion:

    realized_pnl = (close_price - open_price) × close_quantity

The `holding_days` is computed as the number of trading days between
`open_date` and `close_date`. If > 365 calendar days, the gain or loss
is long-term. If ≤ 365, it is short-term. The classification is
determined by calendar days, not trading days, per IRS rules.

## 4. Partial Closes and the Append-Only Rule

The ledger is strictly append-only. The tax lot file is **append-mostly**.
The documented exception:

- **Full close:** the lot row is updated in place. This is not a new
  row; it is an update of the existing row's null fields.
- **Partial close:** the original row's `open_quantity` is reduced, and
  a new row is written for the closed portion. This means one buy can
  produce two rows: one open (the remainder) and one closed.

This is the only place in the system where a row is updated rather than
appended. The amendment to constitution §5 is documented in `SKILL.md`
§1.

## 5. Example

Buy 100 shares on 2024-01-10 at $50.00.
Buy 100 shares on 2024-02-15 at $60.00.
Sell 150 shares on 2024-06-20 at $75.00.

After the sell, the tax lot table has:

| lot_id | open_date | open_price | open_qty | close_date | close_price | close_qty | realized_pnl | holding_days |
|---|---|---|---|---|---|---|---|---|
| L1 | 2024-01-10 | 50.00 | 100 | 2024-06-20 | 75.00 | 100 | 2500.00 | 162 |
| L2 | 2024-02-15 | 60.00 | 50 | 2024-06-20 | 75.00 | 50 | 750.00 | 126 |
| L2' | 2024-02-15 | 60.00 | 50 | — | — | 0 | — | 0 |

`L1` is fully closed (100 shares). `L2` is partially closed (50 of 100
shares), producing `L2'` with the remaining 50 shares still open.

Total realized P&L for the sell: $3,250.00.

## 6. Reconciliation

The sum of `realized_pnl` across all closed lots must equal the realized
gain computed from the Schwab statement. A mismatch is a `CRITICAL`
alert and halts trading.

The sum of `open_quantity` across all open lots for a ticker must equal
the current `positions.parquet.shares` for that ticker.

## 7. What Not to Do

- Do not use average cost basis unless elected.
- Do not close lots out of order (must be FIFO).
- Do not update a fully-closed lot more than once.
- Do not leave a lot with `open_quantity = 0` and `close_date = null`.
- Do not compute holding period from trading days. Use calendar days.

## 8. Verification

- [ ] A test asserts FIFO ordering on a multi-lot sell
- [ ] A test asserts partial close splits the lot correctly
- [ ] A test asserts realized P&L matches (close − open) × qty
- [ ] A test asserts holding period uses calendar days
- [ ] A test asserts sum of open quantities matches positions