# Pending Orders

When the target portfolio wants a trade that the lock blocks, the order is
queued. On the first permitted day the queue is re-validated, re-authorized
and **released to the normal order generator**, which recomputes the quantity
from the fresh price and NAV, applies the spread gate and the ladder, and
obtains its own permit at placement. The queue is a convenience for the order
generator; it is **not** an authority and it does not carry a quantity or a
permit. `lock::authorize` is asked again at placement and may say no.

## 1. The Queue

`pending_orders.parquet` holds every order that is waiting on the lock.

| Column | Type | Notes |
|---|---|---|
| `pending_id` | `utf8` | UUID |
| `ticker` | `utf8` | |
| `side` | `utf8` | `Buy` / `Sell` |
| `target_quantity` | `float64` | Fractional permitted. **Informational only** (the quantity at queue time, for the dashboard and the audit): it is never what gets submitted. For a veto exit it is the whole position at queue time |
| `earliest_execution` | `date32` | **First permitted day** (`D + 5 trading days` of the latest blocking order), recomputed from order history |
| `signal_snapshot` | `utf8` | JSON of quantiles at queue time |
| `created_at` | `timestamp[ns, UTC]` | |
| `status` | `utf8` | `Pending` / `Fired` / `Cancelled` / `Expired` |
| `cancelled_reason` | `utf8` | `signal_reversed` / `signal_weakened` / `veto_triggered` / `regime_crisis` / `restricted` / `nothing_to_sell` / `lock_extended` / null. Identical to `13-parquet-lake/SCHEMAS.md` #15 (free-text enum, no schema change). `lock_extended` is reserved: an extended block re-dates the row (`earliest_execution`), it does not cancel it. A halt (`TRADING_HALTED`) never cancels a row; it stays `Pending` |

## 2. The Queueing Rule

When a target order is generated and `authorize` returns
`Denied::BlockedUntil(first_permitted)`:

1. Set `earliest_execution = first_permitted` (the date `authorize` returned,
   computed from the history; never from a stored expiry).
2. Capture the current signal as a JSON snapshot.
3. Write a row with `status = Pending`. Do not submit the order.

Only `BlockedUntil` queues. `Restricted`, `NothingToSell`, `HistoryIncomplete`
and `CalendarOutOfRange` do **not** queue: a restricted ticker is never
ordered, `NothingToSell` is a rejected request, and an incomplete history is
an incident (halt/alert), not something to wait out.

A pending **sell** means the target weight for that ticker is 0 (or lower)
and a new buy of the same ticker would extend the block and delay the sale.
The order generator therefore does not generate a buy for a ticker that has
a pending sell.

**Veto exits.** For a held name with an active veto, `portfolio_targets` carries
a row with `veto_active = true` and `w_current > 0` (08 §5b-2). The generator
derives a **sell of the whole position** from that pair, whatever `w_target` says
(`w_target` is the lock-clamped display value: 0 when the sell is allowed today,
`w_current` when a counted buy blocks it). It calls the gate: `Ok` -> normal
sequence today; `BlockedUntil(d)` -> a `Pending` sell with `earliest_execution =
d`; `BlockedOpenOrder` -> nothing is queued, an alert is raised, and the next
night's targets repeat the attempt (an open order has no first permitted day to
queue against). A veto also cancels the name's *working buy orders* and its pending buys
(`veto_triggered`); a cancel is not a placement and cannot create a block, and
a buy cancelled with zero fills stops counting, so it does not delay the sell.
The queued sell is never cancelled by the veto, by the regime or by a weak
signal comparison (§4).

## 3. The Fire Rule (first permitted day)

The timer runs at the **start of the execution window** on each trading day
(10:00 ET; orders are placed only in-session, nothing outside 09:30-16:00,
`10-execution-schwab/EXECUTION-WINDOW.md`), not at the 09:30 open
(`lock-monitor.rs`). Pending rows that are due are **released to the normal
order generator** at the window start, after re-authorization. "Fire" below means
"release", not "send". If `TRADING_HALTED` is present (`SKILL.md` §8b) the
timer releases nothing and leaves every row `Pending`.

The generator then treats the released intent like any other target: it
**recomputes the quantity** from the fresh quote and the current NAV and holding
(`EXECUTION-WINDOW.md` §1.3: the share difference to the target, clamped so that
rounding never turns a hold into an opposite-side order), runs the stale-move
rule, the spread gate and the ladder, and assigns the randomized window arm. The
arm may start the sequence later than 10:00 (arm B at 11:30, arm C at 14:00); the
release at 10:00 and the placement are separate moments, and the placement goes
through the per-ticker gate and `authorize` again (authoritative). If the
sequence reaches its deadline unplaced, the row stays `Pending` for the next
session (a due sell is re-decided tonight), and `Expired` applies after 5 trading
days (§5). The queued `target_quantity` plays no part.

1. Load all `Pending` rows with `earliest_execution <= today`. The unlock
   condition is **`today >= first_permitted`**. (An earlier version of this
   file unlocked on `today > expiry`, one day late; that boundary is gone.)
2. Recompute the verdict with `authorize(ticker, side, today, &history, ...)`
   against an `OrderHistory` freshly read over REST for this check.
   - `Err(Halted(_))`: leave the row `Pending`, release nothing, alert.
   - `Err(NonTradingDay)`: leave the row `Pending` (a closure was appended
     today); it is recomputed on the next session.
   - `Err(BlockedOpenOrder(_))`: an opposite-side order is open with no end
     date; do not fire, keep `Pending` and alert.
   - `Err(BlockedUntil(d))` with `d > earliest_execution`: the block was
     extended (a new same-direction counted order, possibly placed outside
     the system). Set `earliest_execution = d`, `status = Pending`, audit the
     extension. Do not fire.
   - `Err(Restricted)` / `Err(NothingToSell)`: cancel the row
     (`cancelled_reason` = `restricted` / `nothing_to_sell`).
   - `Err(HistoryIncomplete(_) | Unmapped)`: leave the row `Pending`, do not
     fire, alert. Retry next scheduled run.
   - `Ok(_)`: the pure verdict allows it (no permit is reserved at release); continue to re-validation.
3. Re-validate the signal (§4). If valid, release the intent to the generator
   and set `status = Fired` (the row records that the intent was released; the
   orders it produces are in `orders.parquet`); otherwise cancel with the reason.
Re-validation is load-bearing: a signal that was correct when queued may
have reversed. Firing blindly executes a stale decision. Re-authorization is
equally load-bearing: the queue never carries a permit across days, and the
pre-check at release does not replace the authoritative check at placement.

## 4. Signal Comparison

The `signal_snapshot` captures the GSPQR quantiles at queue time:

```json
{
  "q_05": -0.042,
  "q_50": 0.081,
  "q_95": 0.198,
  "conviction": 1.089,
  "regime_factor": 1.0
}
```

| Current vs. Snapshot | Pending **Buy** | Pending **Sell** |
|---|---|---|
| `q_50` sign unchanged, magnitude >= 50% of snapshot | Fire | Fire |
| `q_50` sign unchanged, magnitude < 50% of snapshot | Cancel (`signal_weakened`) | Cancel (`signal_weakened`), unless vetoed (below) |
| `q_50` sign reversed | Cancel (`signal_reversed`) | Cancel (`signal_reversed`), unless vetoed (below) |
| Veto active | Cancel (`veto_triggered`) | **Not affected.** A veto never cancels a queued sell; the signal comparison is skipped for a vetoed name and the sell fires |
| Regime flipped to crisis | Cancel (`regime_crisis`) | Not affected (the regime never forces or blocks a sale) |

A cancelled sell is only ever cancelled because the system no longer wants
to sell. No rule here lets the system sell inside a block, and none lets a
veto or regime remove the queued exit of a held name. Because a veto sets the
target weight to 0, the vetoed held name keeps a sell queued (or re-queued)
until it is gone.

## 5. Order Lifecycle

```
                        ┌── Pending ──┬── Fired
                        │             │
Target generated ───────┤             ├── Cancelled
(authorize: BlockedUntil)│            │
                        │             └── Expired
                        │
                        └── (if the block extends, update earliest_execution)
```

- **Pending:** queued, waiting for the first permitted day.
- **Fired:** authorized, re-validated and released to the order generator.
- **Cancelled:** signal reversed/weakened, buy vetoed, regime crisis (buys
  only), restricted, nothing to sell.
- **Expired:** `earliest_execution` passed by more than 5 trading days
  without firing or being re-dated. A bug or incident indicator; log and
  alert (`WARN`).

## 6. What Not to Do

- Do not fire a pending order without re-validating the signal and calling
  `authorize` again on the day, and do not submit the queued quantity: the
  generator recomputes it.
- Do not unlock on `today > expiry`. Unlock on `today >= first_permitted`.
- Do not store a permit in the queue.
- Do not queue an order that is not blocked by the lock.
- Do not cancel a pending sell because of a veto or a regime flip.
- Do not cancel a pending order because the signal is unchanged.
- Do not skip the `Expired` status. It catches stuck orders.
- Do not queue `Restricted`, `NothingToSell` or `HistoryIncomplete` results.
- Do not store the signal snapshot as anything other than JSON.

## 7. Verification

- [ ] A test queues a blocked sell with `earliest_execution = first_permitted`
      and fires it exactly on that day (not the day after, not the day before)
- [ ] A test asserts a buy placed after queuing extends the block and the
      pending row is re-dated, not fired
- [ ] A test asserts a veto on a held name leaves its queued sell intact and
      the sell fires
- [ ] A test asserts `regime_crisis` cancels a pending buy and not a pending
      sell
- [ ] A test asserts the queued order cancels when the signal reverses
      (non-vetoed name)
- [ ] A test asserts `HistoryIncomplete` and `Halted` leave the row `Pending`
      and send nothing
- [ ] A test asserts the released intent's quantity is recomputed from the fresh
      price and NAV (a changed quote or NAV changes the order; the queued
      `target_quantity` is ignored) and that arms B and C place after 10:00
- [ ] A test asserts expired orders are flagged
- [ ] A test asserts the signal snapshot round-trips through JSON
