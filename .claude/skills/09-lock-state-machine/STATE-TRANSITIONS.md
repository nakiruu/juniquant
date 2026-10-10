# Lock Semantics (formerly: Lock State Transitions)

The file name is kept so that links do not break. There are **no stored
states and no transition table** any more. The lock is a pure function of
order history (constitution §1, amendment 1.4). This file holds the detailed
semantics that `SKILL.md` summarises.

## 1. The Function

```
counted(o)            = o.is_open or o.filled_qty > 0      # open = the 15 open statuses OR any unknown/missing status
D(o)                  = roll_forward(ET date of o's LAST FILL if it filled, else of its placement)
block_end(o)          = Open if o.is_open else FirstPermitted(add_trading_days(D(o), 5))   # first permitted day
blocked(side, today)  = exists counted o on the same ticker, any account,
                        o.side == opposite(side)
                        and (block_end(o) is Open or today < block_end(o))
allowed(side, today)  = restricted(ticker) == false
                        and not halted                          # TRADING_HALTED (SKILL.md 8b)
                        and today is a trading day
                        and history is complete (REST read for this authorize)
                        and (side == Buy or shares_held > 0)
                        and not blocked(side, today)
```

The executable definition is the golden fixture pair
`fixtures/lock_cases.json` and `fixtures/nyse_calendar_cases.json`, with the
Julia oracles `scripts/lock_rule_oracle.jl` and `scripts/nyse_calendar_oracle.jl`
(run by `scripts/run_fixtures.jl`); the Rust tests read the same fixtures
(`SKILL.md` §9). The earlier Python reference has been removed from the repository.

## 2. Derived Display Labels

The old state names survive only as labels computed when the dashboard is
rendered. They are never stored and never read by an order path.

| Label | Derived when |
|---|---|
| `IDLE` | No shares held and no counted order in the lookback window |
| `LONG_LOCKED` | Shares held and `blocked(Sell, today)` |
| `LONG_UNLOCKED` | Shares held and not `blocked(Sell, today)` |
| `FLAT_LOCKED` | No shares held and `blocked(Buy, today)` (a recent counted sell) |
| `FLAT_UNLOCKED` | No shares held, a counted sell in the window but not blocking today |

The distinction between `IDLE` and `FLAT_UNLOCKED` is cosmetic. Nothing
depends on it.

## 3. What Used to Be a Transition

| Old concept | Now |
|---|---|
| Buy while `LONG_LOCKED` resets the timer | A counted buy is just another order; the block is the `max` over counted opposite-side orders, so a later buy extends it. Never computed from the previous expiry |
| Timeout at `today > expiry` (monitor flips the state) | Nothing flips. `today >= first_permitted` is the unlock; the monitor only fires pending orders and emits derived events |
| First-fill (or last-fill) date starts the clock | The block starts at placement (open: no end date); once filled, the clock runs from the **last fill date** (equal to the placement date for a same-day fill) |
| Cancelled / rejected: no state change | Not counted if zero fills; counted if open or any fill |
| `IDLE` + Sell = no-op sell of 0 shares | `NothingToSell`, a rejected request |
| Forbidden: Short | Unchanged: long-only; `SELL_SHORT` is not representable and is an audit anomaly if seen in history |

## 4. Edge Cases

**Partial fills.** One order, one `D`, the date of its **last fill**. A
partially filled order that is still working is open: no end date. Partial
fills on Mon and Wed, then cancelled: counted, `D` = Wed, first permitted day
the following Wed.

**Late fills (GTC).** Placed Mon, filled Thu: blocks the opposite side from
Mon (no end date while open) and, once filled, through 4 trading days after
Thu; first permitted day the following Thu. Compliant for the lock. The
execution design still uses `DAY` orders (slippage, same-session fills).

**Partial sells.** A partial sell is a counted sell: it blocks buys of that
ticker for 5 trading days. (The old state model had no definition for this.)

**Cancel after counting.** If a working buy is cancelled with zero fills, it
stops being counted and the block lifts at the next recomputation. The system
never relies on that: a cancel that races a fill is why `PENDING_CANCEL`
counts. An order that was open while an opposite order was placed, and was
then cancelled with zero fills, is not a retrospective breach and is not
flagged by the audit (owner decision B2, 2026-10-10; `SKILL.md` §8 step 2a).

**Replaced orders.** `REPLACED` with zero fills is **not counted**, exactly like
`CANCELED` (owner decision, 2026-10-10); with any fill it counts from the last
fill day. The replacement is a new order with its own `orderId` and `D`; between
`PENDING_REPLACE` and `REPLACED` the old order is still open and counts.

**Orders from the Schwab app.** Counted exactly like system orders (they are
in the broker history). The daily audit is what notices a violation they
cause.

**Dated on a non-trading day.** `D` is the next session. A weekend or holiday
placement or fill never gains a day. Example: a Saturday-dated `D` equals
Monday.

**After the close, or overnight.** The ET date of the placement (or the last
fill), regardless of time. 23:30 ET Friday is Friday (UTC says Saturday).

**Ticker removed from the universe / vetoed while held.** The veto sets the
target weight to 0, blocks new buys, and generates a sell. If a sell is
blocked, it is queued for `first_permitted` and is released to the order
generator then (the generator recomputes the quantity). **No veto
exception exists** (the override that earlier drafts proposed was withdrawn,
amendment 1.4). A veto never cancels a queued sell
(`PENDING-ORDERS.md` §4).

**Restricted ticker in a Schwab account.** Not a lock event. See `SKILL.md`
§5: halt, CRITICAL, no auto-sell.

**Stops and resting orders.** A resting stop is a sell order from its
placement; it blocks buys, and it cannot be placed inside a buy's window.

## 5. Trading-Day Edge Cases

- Thanksgiving week 2026: buy Mon 23 Nov -> first sell Tue 1 Dec.
- Christmas: buy Wed 23 Dec 2026 -> first sell Thu 31 Dec (Fri 25 Dec closed).
- Year end: the rules compute the following January's closures (New Year's
  Day, Martin Luther King Jr. Day) directly; there is no table to run out.
- 3 July 2026 (Independence Day observed) is closed; a buy on Wed 1 Jul ->
  first sell Thu 9 Jul (Fri 3 Jul, Sat, Sun skipped); the rules and the
  calendar fixtures reproduce it. Verify the closure itself against NYSE's page.

## 6. What Not to Do

- Do not add a stored state, an enum of states, or a "lock row" that anything
  authorizes against.
- Do not reset or compute a timer from a previous expiry.
- Do not start the clock at the placement date of an order that filled later,
  or at its first fill: use the last fill date. An open order has no end date.
- Do not treat a cancelled zero-fill order as counted, or a partially filled
  cancelled order as uncounted.
- Do not drop a status you do not recognise. Count it.
- Do not override the lock for a veto exit. No exception exists, and none is
  pending.
