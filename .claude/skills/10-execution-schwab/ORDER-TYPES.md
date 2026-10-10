# Order Construction

> **Phase: later.** Execution and live trading come last in the build order
> (`outline.md` "Project phase and build order"). Kept as the specification
> for that phase; do not build or refine it unless asked.

> **v1 submits only `Limit` and `Market` (the latter only with a `MarketPermit`).**
> Sections 2 to 6 document the API shape of types that are **reserved and
> disabled** (`StopLimit`, `TrailingStop`, `OCO`, `LimitOnClose`) or **not usable
> at all** (`Stop`, `Bracket`, `OTO`, `MarketOnClose`). They are not in the Rust
> `OrderType` enum and nothing in the system produces them; a resting sell blocks
> buys for as long as it works (`09-lock-state-machine`), so each needs a written
> lock analysis before it can be enabled (`SKILL.md` §2). Code blocks here are
> SKETCHES (not compiled, not normative).

How to build each supported order type. The order type is determined by
the current NAV tier (see `SKILL.md` §2a: trailing 5-day mean NAV, boundaries
inclusive at the lower bound). Fractional quantities are only expressible as
market orders in regular hours (owner-confirmed), rounded down to 2 decimals
(OWNER-REPORTED, UNVERIFIED); limit orders are always whole shares.

When an order is placed, at what price and by which ladder is specified in
`EXECUTION-WINDOW.md`: orders are placed only during the regular session
inside the execution window, after the spread gate, and never as an
evening placement for the next open. This file says only how each order
type is *built*. Every builder below is called only after the lock permit
(`09-lock-state-machine`), the tier check and the restricted-list check
(`00-construction` §6, `config/restricted.toml`) have passed for **that
submission**; a cancel/replace is a new submission and needs its own.

## 1. Limit Order (Transition and Standard tiers)

The default order type at or above $2,500 NAV. Buy or sell at a specified price
or better. Limit quantities are whole shares; the builder rejects a
fractional quantity.

```rust
let order = LimitOrder::builder()
    .symbol("AAPL")
    .instruction(Instruction::Buy)
    .quantity(25)
    .limit_price(Decimal::new(18750, 2))  // $187.50
    .duration(Duration::Day)
    .session(Session::Normal)
    .build()?;
```

In the Transition tier, a limit order is used for the integer portion of
a target quantity. The fractional remainder is submitted separately as a
market order (see §1a).

### 1.1 Limit prices used by the ladder

The ladder in `EXECUTION-WINDOW.md` §3 uses three kinds of limit price,
all `LimitOrder` with `Duration::Day`, `Session::Normal`, whole shares:

| Leg | Buy limit | Sell limit |
|---|---|---|
| Passive post | arrival mid rounded down to the tick, or `bid + 1 tick` when the spread is two ticks or wider | mirrored |
| Repriced | between the post price and the far touch, on the schedule | mirrored |
| Capped cross (marketable limit) | `ask + X bps × mid`, rounded **down** to the tick | `bid − X bps × mid`, rounded **up** to the tick |

The cap `X` is the slippage budget: a marketable limit can never pay more
than `X` bps through the touch, which is why the Standard tier needs no
market order. A cap that cannot be honoured (the book is thinner) leaves
the remainder unfilled; the remainder is cancelled, not chased.

**Cancel/replace.** Changing a working limit price is a cancel of the old
order and a new `LimitOrder` for the remaining quantity (a replace
creates a **new order id**: "the old order will be canceled and a new order
will be created", CONFIRMED-OFFICIAL, `PUT .../orders/{id}`; the old order ends
`REPLACED` or `CANCELED`, which is UNVERIFIED, and the link between them is
UNVERIFIED; the code treats it as a new submission, and the lock counts each
order on its own). Lock consequences: the
replacement is a same-side order and is always lawful with respect to its
own predecessor; it carries the same placement date; an order cancelled
with zero fills is not counted; an order that was partly filled and then
cancelled is counted, which is harmless on the same side on the same day.

## 1a. Market Order (Bootstrap tier, and Transition fractional)

The only order type below $2,500 NAV (Bootstrap). Supports fractional quantities.
The spread is paid in full.

```rust
// The permit is issued only by the tier check (SKILL.md §2a).
let permit = tier.market_permit(fractional_part)?;
let order = MarketOrder::builder(permit)
    .symbol("AAPL")
    .instruction(Instruction::Buy)
    .quantity(Decimal::new(150, 2))   // 1.50 shares
    .duration(Duration::Day)
    .session(Session::Normal)
    .build()?;
```

A `MarketOrder` cannot be built without a `MarketPermit`. That is the
compile-time guarantee: there is no way to name a market order without
holding a permit. The tier itself is read at runtime, so the check that
issues the permit is a runtime one: `Tier::market_permit` returns
`ExecutionTierError::MarketOrderNotPermitted` in the Standard tier, and in
the Transition tier unless the quantity is a fractional remainder
(`0 < remainder < 1`), except for the two reduce-only exits of amendment
1.13 (a Standard-tier legacy fractional remainder that completes a full exit;
any Bootstrap exit), which need a `ReduceOnly` built from a lock `SellPermit`.
The error occurs when the permit is requested, before any order exists.

**Fractional rules (`SKILL.md` §2b).** A fractional quantity is a `MARKET`
order, `Duration::Day`, `Session::Normal`, placed and filled only in regular
hours (09:30-16:00 ET; the execution window is inside it). Never a fractional
limit; a fractional limit *sell* is reported only in a social post (UNVERIFIED)
and is not used. The Trader API accepting a decimal `quantity` is
OWNER-CONFIRMED (2026-10-10). The quantity has **at most 2 decimals**
(OWNER-REPORTED, UNVERIFIED) and is **rounded down** (`floor` at 0.01), never
up; a quantity that floors to zero builds no order. `previewOrder` should
still confirm precision and sell-side behavior (live test 5). The builder asks
`fractional_eligible(ticker)` first (`config/fractional_eligible.toml`; Stock
Slices: S&P 500 stocks, no ETFs; `Unknown` = no). The response `Location`
header carries the new order id; the fill may appear in the account with a
delay (COMMUNITY).

In the Transition tier, a market order is only used for the fractional
remainder after a whole-share limit has been submitted for the integer
portion, and only for a fractional-eligible name; for a non-eligible name the
remainder stays in cash. It is never used for the full quantity when the full
quantity is expressible as whole shares.

## 2. Stop Order (not supported)

Triggers a market order when the stop price is reached. **Plain stop
orders are not supported in any tier.** The resulting market order would
bypass the tier check, and Bootstrap holds fractional shares that stops
cannot express. Use `StopLimit` instead. A stop that fires is a sell and
is subject to the lock.

## 3. Stop-Limit Order (RESERVED, disabled)

Triggers a limit order when the stop price is reached. **Reserved, disabled in
v1** (not in the enum; nothing produces it). It is the form a future risk exit
would take at or above $2,500 NAV, once a lock analysis exists.

```rust
let order = StopLimitOrder::builder()
    .symbol("AAPL")
    .instruction(Instruction::Sell)
    .quantity(25)
    .stop_price(Decimal::new(18000, 2))   // trigger at $180
    .limit_price(Decimal::new(17950, 2))  // limit at $179.50
    .duration(Duration::Day)
    .build()?;
```

A resting stop-limit sell is a **sell order** under the lock. Once placed
it is a counted (open) order and blocks buys of that ticker; it may be
placed only when no counted buy blocks sells of that name. The rebalancing
flow does not rely on resting stops. How long an open order keeps blocking
is a `09-lock-state-machine` question; do not assume a resting stop stops
counting after five days.

In the Bootstrap tier, `StopLimit` is not used because it requires whole
shares. There are no resting stop orders in the Bootstrap tier; an exit is
an ordinary market sell generated from the optimizer, subject to the lock.

## 4. Trailing Stop (RESERVED, disabled)

A dynamic stop that follows the price. Requires `stopPriceBasis`
(`LAST`, `BID`, `ASK`), `stopPriceType` (`VALUE` or `PERCENT`), and
`stopPriceOffset`. Not used in the Bootstrap tier.

```rust
let order = TrailingStopOrder::builder()
    .symbol("AAPL")
    .instruction(Instruction::Sell)
    .quantity(25)
    .stop_price_basis(PriceBasis::Bid)
    .stop_price_type(PriceType::Percent)
    .stop_price_offset(Decimal::new(10, 0))  // 10% trail
    .build()?;
```

## 5. OCO (One-Cancels-Other) (RESERVED, disabled; same-side legs only)

Two linked orders. When one fills, the other cancels. Reserved and disabled in
v1; a future use would be a sell pair at or above $2,500 NAV. Not used in the
Bootstrap tier. **Both legs must be the same side** (two sells on a long position, for example a stop
and a profit target). An OCO with one buy leg and one sell leg is never
built: it would put an opposite-side order into the market on the same
date as its counterpart, and the lock counts open orders. Like any sell,
an OCO sell pair needs a `SellPermit` and cannot be placed inside a
counted buy's block.

```rust
let oco = OcoOrder::builder()
    .leg(StopLimitOrder::builder()
        .instruction(Instruction::Sell)
        .stop_price(Decimal::new(18000, 2))
        .limit_price(Decimal::new(17950, 2))
        .build()?)
    .leg(LimitOrder::builder()
        .instruction(Instruction::Sell)
        .limit_price(Decimal::new(22000, 2))
        .build()?)
    .build()?;
```

## 6. Bracket Order (NOT USABLE)

Entry + stop-loss + take-profit in one submission. Requires whole-share
quantities. **Never built by this system and not in the allowlist of the
pre-trade guard.**

**Not usable under the lock.** A bracket's entry is a buy and its stop and
take-profit are sells, submitted together on the same date. The buy is a
counted order that blocks the opposite side from its placement day (while
open) through four trading days after its last fill date, so the sell legs are opposite-side orders placed
inside the block. The same applies to `OTO` (entry triggers an exit). The
builder below is kept only as a reference for the API shape; the order
generator must not call `BracketOrder::builder()` or an entry-plus-exit
`OTO`. (`SKILL.md` §2 lists both as "not usable: opposite-side legs".)

```rust
let bracket = BracketOrder::builder()
    .entry(LimitOrder::builder()
        .instruction(Instruction::Buy)
        .limit_price(Decimal::new(18750, 2))
        .build()?)
    .stop_loss(StopLimitOrder::builder()
        .instruction(Instruction::Sell)
        .stop_price(Decimal::new(17500, 2))
        .limit_price(Decimal::new(17450, 2))
        .build()?)
    .take_profit(LimitOrder::builder()
        .instruction(Instruction::Sell)
        .limit_price(Decimal::new(21000, 2))
        .build()?)
    .build()?;
```

## 7. Duration and Session

| Field | Values | Default |
|---|---|---|
| `duration` | `Day`, `GoodTillCancel`, `FillOrKill` | `Day` |
| `session` | `Normal`, `AM`, `PM` | `Normal` |

The system uses `Day` and `Normal`. Extended hours are not used because
liquidity is thin and the lock constraint makes after-hours fills risky.
Orders are placed only during the regular session (America/New_York), so
the placement date equals the session date and every `Day` order is
terminal by the close; no order is carried into the next session. Placing
in the evening "for the open" is not done (`EXECUTION-WINDOW.md` §1).

## 8. Tier-to-Order-Type Matrix

| Tier | Trailing 5-day mean NAV | Primary order type | Fallback | Fractional |
|---|---|---|---|---|
| Bootstrap | < $2,500 | Market (spread gate and window first) | — | Yes |
| Transition | $2,500 ≤ NAV < $10,000 | Limit (ladder) | Capped marketable limit; Market (fractional only) | For fractional only |
| Standard | ≥ $10,000 | Limit (ladder) | Capped marketable limit; Market only for a legacy fractional remainder of a full exit (amendment 1.13) | Remainder exits only |

The tier is derived from `portfolio_ledger.parquet` at submission time.
The order generator consults the tier before constructing the order. A
`MarketOrder` needs a `MarketPermit`, which `Tier::market_permit` issues
only in the Bootstrap tier, or in the Transition tier for a fractional
remainder (`SKILL.md` §2a).

## 9. What Not to Do

- Do not use `Market` orders outside the Bootstrap tier or the Transition
  tier's fractional path.
- Do not use `Stop` (plain) orders in any tier. The resulting market
  order bypasses the tier check.
- Do not use extended hours sessions.
- Do not use `FillOrKill` without a specific reason.
- Do not build OCO or bracket orders. OCO is reserved and disabled; bracket is
  never built.
- Do not submit a fractional limit order. Fractional quantities fill only as
  market orders in regular hours (owner-confirmed). Use a market order for
  fractional quantities, with at most 2 decimals rounded down, and never rely
  on a fractional limit sell (UNVERIFIED).
- Do not cache the execution tier across orders. Read it at submission
  time.
- Do not build a `Bracket`, an entry-plus-exit `OTO`, or an OCO with legs
  on opposite sides. They place opposite-side orders on the day of the
  entry.
- Do not place orders outside the regular session or carry a `Day` order's
  remainder into the next session.
- Do not submit a replacement without its own lock, tier and
  restricted-list checks.
- Do not use a price cap wider than the tier's calibrated `X`; the cap is
  the slippage budget.