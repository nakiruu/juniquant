# Trade Definition

A trade is a closed round-trip, not a fill and not a lot. This document
defines the matching rule and the edge cases.

## 1. What Constitutes a Trade

A trade is the aggregation of:

- All lots opened at entry (possibly across multiple fills on multiple
  days).
- All lots closed at exit (possibly across multiple fills on multiple
  days).

A trade is written when **all lots in the round-trip are closed**.

If a position is partially closed, the closed portion is a trade. The
remaining open portion is not.

## 2. The Matching Rule

Trades are matched via FIFO, consistent with `16-portfolio-ledger/TAX-LOTS.md`.

The round-trip begins when a ticker transitions from flat to long. It ends
when the ticker transitions from long to flat.

Consider:

    Day 1: Buy 100 shares at $50.
    Day 10: Buy 100 shares at $60.
    Day 30: Sell 100 shares at $70.
    Day 40: Sell 100 shares at $65.

After Day 30, FIFO closes the Day 1 lot (100 shares at $50 basis). The
trade is:

    entry_date = Day 1
    exit_date = Day 30
    entry_price = $50
    exit_price = $70
    quantity = 100
    gross_return_bps = 4000

The Day 10 lot remains open. The Day 40 sell closes it:

    entry_date = Day 10
    exit_date = Day 40
    entry_price = $60
    exit_price = $65
    quantity = 100
    gross_return_bps = 833

Two trades, not one. FIFO matches each lot to its own exit.

## 3. Round-Trip Boundaries

A new round-trip begins after a ticker has been flat (zero shares) and a
new buy is submitted. The buy can be a fractional market order in the
Bootstrap tier or a whole-share limit in the Standard tier. Either way,
the moment the position goes from zero to positive, a new round-trip
begins.

Consecutive buys while long are part of the same round-trip. They do not
start a new trade.

## 4. Partial Exits

A partial exit closes some lots and leaves others open. The closed
portion produces a trade. The open portion does not.

    Day 1: Buy 100 at $50.   (1 lot, 100 shares, open)
    Day 10: Sell 60 at $70.  (partial exit)

After Day 10:

    Day 1 lot is reduced to 40 shares (open).
    A new closed lot of 60 shares is created.
    A trade is written: entry $50, exit $70, quantity 60.

The remaining 40 shares continue to be the same round-trip. When they
are sold, another trade is written.

## 5. Veto Exits

If a veto activates while holding a position, the portfolio layer keeps the
name in its output with target 0 (clamped to the current weight while the lock
binds, `08-portfolio-optimization` §5b-2), new buys are blocked, and the sale
goes through the 5-day lock like any other sell: if the lock blocks it today it
is queued for the first permitted day and released to the order generator then
(`09-lock-state-machine/PENDING-ORDERS.md`). Such an exit is identified for
reporting by the veto having been active on the exit date (from `veto_flags`). **No exception exists.** The
trade is written normally when it closes, and `lock_overridden` is `false`.
The column is reserved (constant `false`) for schema stability; it will be
removed by migration, not reused.

## 6. Fractional Quantities

In the Bootstrap and Transition tiers (see `00-construction` §6a),
quantities are fractional. The trade's `quantity` is `float64`. A trade
of 0.30 shares is valid. The bps metrics are unaffected by quantity;
they are price ratios.

## 7. No Round-Trip Without an Exit

A buy with no exit produces no trade. There is no `trade_summary` row
for open positions. The position appears in `positions.parquet` and in
`portfolio_ledger.parquet.market_value`, but not in `trade_summary`.

This means the trade summary's alpha distribution is a **closed-trade
distribution**, not an all-positions distribution. It answers "how did
the trades we closed do?" not "how are all positions doing?"

## 8. What Not to Do

- Do not write a trade row for each fill. A fill is not a trade.
- Do not write a trade row for each lot. A lot is part of a trade.
- Do not start a new round-trip on a same-direction add. Only a
  flat-to-long transition starts a new round-trip.
- Do not write a trade row before the exit has settled. The exit date is
  the last close date in the round-trip.
- Do not set `lock_overridden` to `true`. No lock override exists.