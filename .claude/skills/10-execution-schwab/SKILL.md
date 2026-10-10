---
name: execution-schwab
description: Schwab API client and order lifecycle (later phase). Load when
  touching execution/src/schwab/, execution/src/risk/, order submission,
  OAuth, the halt flag or position reconciliation.
---

# Schwab Execution

> **Phase: later.** Execution and live trading come last in the build order
> (`scaffold.md` Build Order; `outline.md` "Project phase and build order").
> The project is currently collecting data and training GSPQR + TCP + EVT.
> This skill is the specification for that later phase; do not build or
> refine it unless asked.

The Schwab adapter is the last code between a portfolio target and a real
order. It is Rust-only (Julia never holds a token, places an order or halts
trading; constitution §4). It runs in the execution hot path. It is the
narrowest, most rule-bound module in the system.

Sketch policy: every code block here and every file under `scripts/` is a
SKETCH, not compiled, not normative; the crate files named in `scaffold.md` are
the real homes (`15-ai-dev-workflow` §4).

## 1. The Hard Constraints

From the constitution (§6, §6a):

- **NAV-tiered order types.** Limit orders are the default. Market orders
  exist only in the Bootstrap tier and, for fractional remainders, in the
  Transition tier. In the Standard tier no market order is ever
  submitted. See §2a.
- **No shorts.** Long-only. `SELL_SHORT` is not representable.
- **Position cap.** Resulting weight ≤ 5% per name. Applies to orders that
  increase the position; a **reduce-only** order (a sell that does not take the
  holding below zero) is exempt, otherwise a name above the cap could not be
  sold down.
- **Sector cap.** Resulting weight ≤ 25% per GICS sector (same reduce-only
  exemption).
- **Cash buffer.** Resulting cash ≥ 2% of portfolio value (a sell only raises
  cash and is exempt).
- **Sells before buys.** A buy's cap and cash check counts a sell only once it
  has filled (§6a step 5).
- **Lock.** The 5-day lock is a contractual obligation and applies in every
  tier (`09-lock-state-machine`). It is recomputed from order history at
  submission time, over a REST read made for that very authorize (§6a), never
  read from a cached row. No exceptions.
- **Halt.** `$QUANT_STATE/TRADING_HALTED` stops `authorize`, every `submit_*`,
  the pending timer and the ladder (`09-lock-state-machine` §8b).
- **Restricted securities.** No order, either side, for a ticker on
  `config/restricted.toml` (initial entry `ICE`). Rejected in
  `risk/pre_trade.rs` and again by `lock::authorize`; CRITICAL audit row
  `restricted_security_blocked` (§6a).
- **Tier.** The order type must be permitted by the current tier (§2a).
- **Fractional.** Fractional quantities are market orders only, placed only
  in regular hours, at most 2 decimals (§2b); API acceptance is
  OWNER-CONFIRMED, the 2-decimal limit is OWNER-REPORTED and still to be
  verified (live test 5).
- **Daily reconciliation.** Local positions vs Schwab positions, in Rust
  (`risk/reconciliation.rs`, §6c). A mismatch that survives the tolerance and
  retry halts trading.

These are pre-submit runtime checks, each witnessed by a permit type
(`MarketPermit`, `BuyPermit`/`SellPermit`) so that no submission path
exists without the check having run. An order cannot reach Schwab without
passing them.

## 2. Order Types Supported

v1 submits exactly two order types. The Rust enum is `OrderType { Limit, Market }`
and `Market` is constructible only with a `MarketPermit` (§2a). Nothing else can
be built; the pre-trade guard allowlist is `{Limit, Market(with MarketPermit)}`
(`00-construction/ENFORCEMENT.md` §6).

| Type | v1 status | Notes |
|---|---|---|
| `Limit` | submitted | Primary order type in the Transition and Standard tiers; whole shares only |
| `Market` | submitted, tier-gated | Only with a `MarketPermit` (§2a): Bootstrap, or a Transition fractional remainder |
| `StopLimit`, `TrailingStop`, `OCO` (same-side legs only), `LimitOnClose` | **reserved, disabled** | Documented in `ORDER-TYPES.md` for the API shape only. Not in the enum, never produced. Disabled until a lock analysis exists: a resting stop is an open sell from its placement and blocks buys of that ticker for as long as it works (nothing in the system needs one) |
| `Bracket`, `OTO` | **not usable, ever** | Opposite-side legs placed with the entry violate the lock (the sell leg counts from placement) |
| `Stop` (plain) | not supported | Triggers a market order. Not supported in any tier |
| `MarketOnClose` | not supported | A market order with a different trigger. Not supported in any tier |

Enabling a reserved type is a deliberate change: a written lock analysis in
`09-lock-state-machine` (what the resting order blocks, for how long, and which
permit gates it), a new `OrderType` variant, an update of the pre-trade guard
allowlist, and a review by the owner. A stop-type order that fires is a sell and
counts as an open sell **from the day it is placed** (not from the day it
triggers), which is why none is enabled.

The system submits `DAY` orders by design (same-session fills, slippage);
a `GoodTillCancel` order is lock-compliant (an open order blocks with no end
date and a late fill counts from its last fill date) but is not used
(`09-lock-state-machine` §3).

See `ORDER-TYPES.md` for construction patterns.

## 2a. NAV-Tiered Order Generation

The order generator is the only place that converts the optimizer's
**weights** into **quantities** (`08-portfolio-optimization` §6a).

**Tier input.** The tier is chosen from the trailing 5-day mean of
`portfolio_ledger.total_value`, read at submission time and never
cached. If the ledger has fewer than 5 rows the mean of the available
rows is used; with no rows (first day), use the broker-reported NAV.
The ledger row changes once per day, so every order in one rebalance
sees the same tier; a boundary crossing takes effect at the next
rebalance's first order.

| Tier | Trailing 5-day mean NAV | Order policy |
|---|---|---|
| Bootstrap | NAV < $2,500 | Fractional market orders only, fractional-eligible names only |
| Transition | $2,500 ≤ NAV < $10,000 | Whole-share limit for the integer part; fractional market for the remainder only, and only for fractional-eligible names |
| Standard | NAV ≥ $10,000 | Whole-share limit only |

The lower bound is inclusive: exactly $2,500 is Transition and exactly
$10,000 is Standard.

**Sizing NAV.** `quantity = weight × NAV / price` uses the current
portfolio value (cash + positions), not the trailing mean. The mean only
selects the tier.

**Weight to quantity.**

| Tier | Conversion |
|---|---|
| Bootstrap | `qty = floor(w × NAV / price, 2 decimals)`, one market order; only if `fractional_eligible(ticker)` is `Yes` (`Unknown` = `No` = no trade, `gate_reason = not_fractional_eligible`) |
| Transition | `whole = floor(w × NAV / price)` as a limit; `rem = floor((w × NAV / price − whole), 2 decimals)` as a market order if `rem > 0` **and** the name is fractional-eligible. A non-eligible name trades `whole` only and the sub-share remainder stays in cash; a weight worth less than one share yields no trade |
| Standard | `qty = floor(w × NAV / price)` as a limit; the residual stays in cash (unchanged) |

Fractional quantities are rounded **down** to 2 decimals (the 2-decimal limit
is OWNER-REPORTED, UNVERIFIED; §2b). The whole-share part floors exactly as
before. The resulting `qty` or `rem` may be zero, in which case no order is
built. How the 0.01-share granularity interacts with the minimum notional:
`EXECUTION-WINDOW.md` §4.

Standard rounds **down**. Rounding to nearest could round up past the
5% cap or into the 2% cash buffer, which the pre-trade check would then
reject. Rounding down makes the residual return to cash, as
`08-portfolio-optimization` §6b assumes.

**Market orders require a permit.** The tier is runtime data, so the type
system cannot know it. What it can enforce is that a `MarketOrder` cannot
be built without a `MarketPermit`, and that only the tier check can
issue one.

```rust
pub enum Tier { Bootstrap, Transition, Standard }

impl Tier {
    /// Boundaries are inclusive at the lower bound.
    pub fn from_nav(mean_nav_5d: Decimal) -> Tier {
        if mean_nav_5d < dec!(2500) { Tier::Bootstrap }
        else if mean_nav_5d < dec!(10000) { Tier::Transition }
        else { Tier::Standard }
    }

    /// The only constructor of `MarketPermit`.
    /// `fractional_part` is the share quantity that cannot be a whole-share limit.
    /// `reduce_only` is Some only on the sell path (amendment 1.13).
    pub fn market_permit(self, fractional_part: Decimal, reduce_only: Option<&ReduceOnly>)
        -> Result<MarketPermit, ExecutionTierError>
    {
        if let Some(r) = reduce_only {
            if r.qty <= Decimal::ZERO || r.qty > r.shares_held {
                return Err(ExecutionTierError::NotReduceOnly);
            }
            return match self {
                // (b) Bootstrap: any reduce-only exit, whole or fractional
                Tier::Bootstrap => Ok(MarketPermit(self)),
                Tier::Transition if fractional_part > Decimal::ZERO
                                 && fractional_part < Decimal::ONE => Ok(MarketPermit(self)),
                // (a) Standard: only the legacy fractional remainder of a full exit
                Tier::Standard if r.qty < Decimal::ONE && r.completes_full_exit => Ok(MarketPermit(self)),
                _ => Err(ExecutionTierError::MarketOrderNotPermitted(self)),
            };
        }
        match self {
            Tier::Bootstrap => Ok(MarketPermit(self)),
            Tier::Transition if fractional_part > Decimal::ZERO
                             && fractional_part < Decimal::ONE => Ok(MarketPermit(self)),
            _ => Err(ExecutionTierError::MarketOrderNotPermitted(self)),
        }
    }
}

/// Facts for a reduce-only exit. Only constructible from a lock `SellPermit`
/// (09 §6a), so a buy can never carry it.
pub struct ReduceOnly {
    qty: Decimal,
    shares_held: Decimal,
    completes_full_exit: bool,
}

impl ReduceOnly {
    pub fn from_sell(_p: &SellPermit, qty: Decimal, shares_held: Decimal, completes_full_exit: bool) -> Self {
        ReduceOnly { qty, shares_held, completes_full_exit }
    }
}

/// Private field: cannot be constructed outside this module.
pub struct MarketPermit(Tier);

impl MarketOrder {
    pub fn builder(permit: MarketPermit) -> MarketOrderBuilder { /* ... */ }
}
```

`Market` references live only in `schwab/orders.rs`, `risk/pre_trade.rs`
(which reads the tier) and the shadow engine. The pre-commit hook
(`00-construction/scripts/pre-commit-constitution.sh`) fails any other
`execution/` file that names a market order.

**Reduce-only exits (constitution §6a, amendment 1.13; owner decision A1).**
Two exits that the buy-oriented tier rules left without a legal order:

1. *Legacy fractional remainder in the Standard tier.* A position bought as a
   fraction in an earlier tier (say 2.47 shares) is fully exited by a
   whole-share limit for the integer part (2) plus one Market order for the
   remainder (0.47). The Market part is allowed only when `0 < qty < 1`, the
   order reduces the position, and it completes a full exit of that position.
2. *Any reduce-only exit in the Bootstrap tier.* A held position, whole or
   fractional, may be sold with a Market order, regardless of fractional
   eligibility.

Both obtain the permit from `Tier::market_permit(fractional_part, reduce_only =
true)`, which checks that the side is SELL, the quantity does not exceed shares
held, and (case 1) that the order completes a full exit. They pass through the
same lock gate, restricted check, halt flag, spread gate and audit as any order,
and never permit a buy. Tests: a Standard 2.47-share exit builds Limit 2 +
Market 0.47; a Standard Market BUY or a partial-exit Market is rejected; a
Bootstrap 3-share exit builds Market 3; a Bootstrap buy of an ineligible name is
still rejected.

Execution timing and slippage: see EXECUTION-WINDOW.md

## 2b. Fractional Orders (owner-confirmed rules; 2-decimal limit UNVERIFIED)

Source labels follow `SCHWAB-API-REFERENCE.md` §3.

- **Fractional orders are `MARKET` only and are placed and filled only during
  regular trading hours** (09:30-16:00 ET). This is the owner's statement and it
  matches Schwab's Stock Slices terms for purchases (COMMUNITY quoting the
  terms: "You will not be permitted to enter limit or other conditional orders";
  market "not held" orders, best-efforts, executed at market; after-hours
  orders queued for the next open). The execution window (10:00-15:30 ET) is
  already inside regular hours; Bootstrap trades and Transition fractional
  remainders are placed only inside it, after the spread gate. A queued
  after-hours fractional order would be an open order counted by the lock from
  its placement, which is a further reason the system never places one.
- **Do not rely on a fractional LIMIT sell.** A Schwab social-media reply
  reports that limit orders are accepted when selling fractions; that is a
  search-snippet statement only, **UNVERIFIED**. The design uses market orders
  for every fractional quantity, buy or sell.
- **The Trader API accepts fractional orders: OWNER-CONFIRMED (2026-10-10;
  the owner has used it).** This supersedes the earlier doubt (the schema
  allows it, `quantity` is `double`, `quantityType` is `ALL_SHARES` /
  `DOLLARS` / `SHARES`; Schwabdev's "not supported" and a search summary are
  outdated or wrong). Fractional orders are market orders in regular hours
  only (OWNER-CONFIRMED). **Precision: at most 2 decimals** (OWNER-REPORTED,
  UNVERIFIED; Schwab's Stock Slices press release says four decimals, so the
  API limit may differ from the product's). The system rounds down to 2
  decimals, which is valid under either limit. `previewOrder` should still
  confirm precision and sell-side behavior (live test 5, §8).
- **Stock Slices eligibility and size (reference §3).** Schwab Stock Slices
  covers S&P 500 stocks ("any of America's leading companies from the S&P
  500"; CONFIRMED-OFFICIAL press release, 2020), no ETFs (COMMUNITY), "as little
  as $5" per stock, shares rounded to four decimal places. Whether the
  eligible list has changed since 2020 is UNVERIFIED. **Consequence: in the
  Bootstrap tier only fractional-eligible names are tradable**, and a Transition
  remainder in a non-eligible name is dropped back to cash (the name trades
  whole shares only).
- **Eligibility hook in the order generator.** `fractional_eligible(ticker) ->
  Eligibility { Yes, No, Unknown }`, read from the owner-approved list
  `config/fractional_eligible.toml` (initially empty; the owner seeds it from
  Schwab's current Stock Slices list; how to obtain that list programmatically is
  UNVERIFIED) and refined by `previewOrder` results cached per name (a rejection
  records `No`). **`Unknown` is `No` in Bootstrap** (fail closed). The order
  generator calls the hook for every fractional quantity; a `No` skips the
  trade and writes `gate_reason = not_fractional_eligible`.
  **Note for `08-portfolio-optimization` (owned elsewhere, not edited here):**
  the eligibility filter should, in the Bootstrap tier, restrict the tradable
  set to fractional-eligible names (a held non-eligible name is still sold by a
  whole-share limit where possible; see the Standard-tier fractional-remainder
  open decision above).
- **Fills may appear late.** Schwab's terms (COMMUNITY) say there may be a
  delay between execution and appearance in the account. A fractional order
  stays an open order (counted by the lock, no end date) until its status turns
  terminal; reconciliation compares positions with a tolerance and a retry
  before it halts.


## 3. OAuth 2.0

See `OAUTH.md`. The critical facts:

- Access token valid ~30 minutes (COMMUNITY-REPORTED, Schwabdev docs; not in
  the OpenAPI spec; verify live).
- Refresh token valid ~7 days, fixed, no sliding extension reported
  (COMMUNITY-REPORTED, same source; verify live). The system records the time of
  the owner's authorization and warns at 48 h, 24 h and 6 h before the assumed
  expiry (`broker_auth_expiring`, `09-lock-state-machine` §8c).
- **No paper environment** found: no sandbox or paper endpoint appears in any
  source consulted (`SCHWAB-API-REFERENCE.md`, UNVERIFIED as an absolute; the
  portal is login-gated). The adapter is designed as if it trades real money. A
  shadow engine simulates fills for validation.
- Refresh token expiry requires re-authorization by the account owner in a
  browser (COMMUNITY-REPORTED).

The token refresh is a single POST to Schwab's token endpoint. The
adapter stores the refresh token in the state directory with restrictive
permissions. When the refresh token expires, `BrokerAuthExpired` is
raised and trading halts (halt kind `AUTH_EXPIRED`, which clears automatically
once the owner has re-authorized and a clean audit passes) until then.

## 4. Rate Limits

Numeric limits are **not in the OpenAPI spec**. COMMUNITY-REPORTED
(Schwabdev docs): 120 API requests/minute, 120 order requests/minute, 4,000
order-related calls/day, HTTP 429 beyond (actuals and `Retry-After` are live
test items). Streamer: one connection per user (CONFIRMED-OFFICIAL, code 12
otherwise); 500 concurrently streamed symbols (COMMUNITY; the official doc has
code 19 `REACHED_SYMBOL_LIMIT` without a number). Budget (placeholder): order
history read per authorize = 2 x accounts requests (orders window and 14-day
transactions, `N = 0`); quote polling <= 6/min; calendar-sync <= 31 queries/day
(today + 30 days) at the 09:00 refresh; all inside 120/minute because the client's
rate limiter paces the order generator (a large rebalance takes minutes).


**Policy:** Exponential backoff on 429. Honor `Retry-After` header.
Do not retry order submissions blindly; check order status first. The Order
schema has **no client order id** (no idempotency key): at most the free-text
`tag` field exists ("client tag"; whether it can be set on POST and is echoed on
GET is UNVERIFIED, live item), so a retry after an ambiguous result (timeout,
5xx) is never a blind re-POST: the journal records `send_unknown` and the
adoption rule of `09-lock-state-machine` §3.7 decides whether an order exists.

## 4a. Order History, Streamer and Quote Data (summary)

Full specification: `09-lock-state-machine/SKILL.md` §3 (order history),
`EXECUTION-WINDOW.md` §2.1 (quote recorder); facts and confidence labels:
`SCHWAB-API-REFERENCE.md`. In short:

- **Order history.** Prefill by REST (`/accounts/accountNumbers`, then
  per-account `/accounts/{hash}/orders` over a 45-day entered-time window, plus
  open-status queries) at startup and at the 09:00 pre-open refresh. The
  Streamer `ACCT_ACTIVITY` subscription marks the history dirty and raises early
  warnings; it is never a source for a decision.
  At submission:
  `N = 0` (owner decision): **a REST read of every linked account's orders and of the
  last 14 days of `transactions` before EVERY `authorize`**, inside the per-ticker
  gate; if REST fails, deny. The stream is only a dirty trigger and early warning.
  Fill time is from REST `executionLegs[].time`. Account hashes only, never raw
  account numbers. **Freshness and coalescing:** the snapshot must have been
  fetched starting after the authorize call began; no cache of history or verdicts
  (the added ~100-300 ms, unmeasured, is about 0.8 bps of zero-mean noise versus
  2-10 bps half-spreads, so a cache buys nothing measurable and risks an unseen
  manual order). Concurrent authorizes share one in-flight fetch per
  `(account_hash, epoch)` that started after their own start (single flight; Rust
  `moka` `future::Cache::try_get_with`, verify), invalidated on resolve, to
  respect the reported 120 requests/minute at the 10:00 burst and ladder
  reprices. Allowed TTL caches (moka, placeholders) for non-compliance data only:
  account hashes (hours), symbol map, fractional eligibility, restricted list
  (reload on file change), calendar/market-hours (minutes-hours), cost model
  (per session); never order history, positions used in authorize, the halt flag
  or verdicts. Details and tests: `09-lock-state-machine` §3.2, §9.
- **Streamer session.** `GET /trader/v1/userPreference` gives
  `streamerInfo` (CONFIRMED-OFFICIAL; array vs object shape is a live test),
  LOGIN with the access token (code 0 before any SUBS), one connection, forced
  re-login with a fresh token after the 30-minute access token (survival of an
  established stream across expiry is UNVERIFIED), reconnect with backoff 2 s
  doubling to 120 s (the Schwabdev client's choice), re-send subscriptions on
  reconnect, heartbeat gap or codes 3/12/30 mark the history dirty and the quote
  recorder gapped. Request `ADD` for incremental subscriptions; `SUBS` overwrites.
- **Quotes.** `LEVELONE_EQUITIES` for the recorder and the spread gate, REST
  `/marketdata/v1/quotes` for the remainder; no historical bid/ask endpoint
  exists, so spread history is built forward only.

## 5. The Shadow Execution Engine


See `SHADOW-ENGINE.md`. Because no paper environment was found (UNVERIFIED), the
adapter runs a shadow engine that:

1. Simulates fills using the current quote + slippage model
2. Compares simulated P&L against real P&L after live deployment
3. Flags divergence between simulation and reality

Slippage is measured against the **arrival mid** (the mid quote at the moment
the order is sent), not the last price or the close; see `SHADOW-ENGINE.md`.


The shadow engine is not a substitute for real testing. It is a guardrail
against deploying a code change that produces obviously wrong fills.

## 6. Order Lifecycle

```
CREATED ──submit──▶ PENDING ──fill──▶ FILLED
                       │
                       ├──partial──▶ PARTIAL ──fill──▶ FILLED
                       │
                       ├──reject──▶ REJECTED
                       │
                       └──cancel──▶ CANCELLED
```

Community reports (not in the reference, UNVERIFIED) say Schwab can accept an
order and show it as `REJECTED` afterwards. The design therefore never treats
acceptance as a fill: the adapter keeps the order visible and polls its status
over REST until it reaches a terminal state, and the lock counts the order as
open until then (`09-lock-state-machine` §3.1).

**Critical:** An order that appears accepted may still end rejected or
cancelled. The adapter polls order status until a terminal state.

**Fractional quantities.** Quantities are `Decimal` in Rust and `float64`
in Parquet. `OrderSubmitted` and `OrderFilled` carry the `tier` the order
was generated under (`11-rust-event-bus/EVENT-TYPES.md`). Any comparison
of a local quantity with a broker quantity (fills, reconciliation) uses a
tolerance, not equality. A market order normally fills in one execution,
but the adapter still handles partial fills.

## 6a. Lock, Restricted List, Halt and Reconciliation in the Submission Path

Every order, from the order generator, from the ladder (each post, reprice,
cross, cancel/replace) or from the pending-order release, goes through **one**
entry point, the `OrderGate`, and this sequence inside one critical section.
Nothing is cached between steps.

0. **Take the ticker's gate** (`09-lock-state-machine` §6a): one async mutex per
   canonical ticker, held from here until the outcome row is written. The
   critical section is `halt check -> ready history -> authorize -> intent append
   -> send -> outcome append`; no second path can read the history in between
   (TOCTOU). Different tickers do not contend.
1. **Halt check.** If `$QUANT_STATE/TRADING_HALTED` is present, unreadable or
   unparseable, deny (`Denied::Halted`); nothing is sent.
2. **Restricted check** (`risk/pre_trade.rs`, before anything else is evaluated
   and before the history is read). If the ticker (canonicalised through the
   symbol map, `09` §3.8) is on `config/restricted.toml`, reject, write CRITICAL
   `restricted_security_blocked`, stop. If the file cannot be read or parsed,
   reject **every** order (fail closed). An unmapped symbol is not tradable,
   and a superseded-security ticker `SYMBOL~CIK10` is rejected (it fails
   `is_tradable` of the shared `symbols` crate, which `execution/src/symbols.rs`
   reuses rather than re-implements; `01-data-ingestion/UNIVERSE.md` §3, `09` §3.8).
3. **Ready the order history** (`OrderHistory::ready_for_submission`) with the
   REST freshness bound **`N = 0`** (owner decision, 2026-10-10): a REST read of
   **every** linked account's orders (45-day window; fill times from
   `executionLegs[].time`) and of the last 14 days of `transactions` (type
   `TRADE`) is made **before every authorize**, merged with the audit journal
   (Schwab authoritative, `09` §3.7) and `lock_external.csv`. If REST fails, or
   any source is unreadable, stale, unparseable, truncated or short of the 21-day
   requirement => `HistoryIncomplete` => no order, **even if the stream looks
   healthy**. A single unknown enum value with a parseable symbol blocks only its
   ticker (`09` §3.6). Details: `09-lock-state-machine` §3.
4. **Positions.** `shares_held` from the broker in the same step. A sell with
   nothing held is `NothingToSell`, a rejected request, not a lock event.
5. **`lock::authorize(ticker, side, today, &history, ...)`** recomputes the
   block from history (an open order blocks with no end date; a filled one
   through 4 trading days after its last fill date; `REPLACED` with zero fills is
   not counted) and, via `authorize_and_reserve`, **writes the intent row at permit
   issue** and returns a `BuyPermit`/`SellPermit` (private fields, no
   `Clone`/`Copy`, bound to ticker and date, owning the ticker's guard) or
   `Denied::*`. `BlockedUntil(d)` is queued as a pending order with
   `earliest_execution = d` (`09-lock-state-machine/PENDING-ORDERS.md`). Nothing
   else is queued. The permit cannot be sent through a channel before the intent
   exists, because the intent is written when it is issued.
6. **Other pre-trade checks** (tier permit, order-type allowlist, position cap,
   sector cap, cash buffer). The position/sector/cash check for a **buy** may
   count a **sell** only after that sell has **filled**, not merely been placed;
   reduce-only sells are exempt from the caps. Buys that depend on sell proceeds
   or sector room are therefore sequenced after those sells fill within the same
   session; if the sells have not filled by the deadline, the dependent buys are
   **not placed** (they are not queued as lock orders; the next rebalance
   regenerates them). Then `submit_buy`/`submit_sell` consumes the permit.
7. **After sending**, the outcome row (with the `orderId` from the `Location`
   header, or `send_unknown`/`rejected_by_broker`) is appended before the guard is
   released, so the next `authorize` for the ticker sees the order whether or not
   Schwab's list has caught up.

There is **no override** and no switch that skips any step. A veto, regime
change, stop, optimizer output or operator instruction cannot place an order
inside a block; a vetoed held name is sold on its first permitted day, by the
normal generator, through this same sequence.

## 6c. Reconciliation (`execution/src/risk/reconciliation.rs`, Rust)

Daily at the close (16:15 ET slot), in Rust. **Julia does not hold tokens, call
Schwab or halt trading** (constitution §4); the earlier Julia script
`reconcile-positions.jl` is removed and this section replaces it.

- **Accounts.** Every linked account (`GET /accounts/accountNumbers`, hashes
  only). The system's *trading* accounts (config `trading_accounts`, labels) are
  compared with `positions.parquet`; every linked account is also scanned for
  restricted tickers and for shorts. Positions are read with
  `GET /accounts/{hash}?fields=positions`: `positions[].longQuantity`,
  `shortQuantity`, `instrument.symbol` (field names to verify against the spec;
  every field `Option`).
- **Checks.**
  1. A **restricted** ticker in any linked Schwab account (long or short, any
     quantity): halt kind `RESTRICTED`, CRITICAL `restricted_security_blocked`,
     **no auto-sell**.
  2. Any `shortQuantity > 0` (the system is long-only): halt kind `RECONCILIATION`,
     CRITICAL.
  3. Local vs broker shares per canonical ticker in the trading accounts (symbols
     canonicalised): a missing ticker either way, or a quantity difference above
     the tolerance, is a mismatch.
- **Tolerance.** `abs(local - broker) <= max(0.005, 1e-6 x broker)` shares
  [placeholder: half the 0.01-share step; not equality]. Fractional fills can
  appear in the account late (COMMUNITY): a mismatch explained by an order with
  fills not yet reflected (a market order sent today whose fill is not in the
  position yet) is **retried** at +5, +15, +30 and +60 minutes before it is a
  mismatch.
- **Outcomes.** Clean: `position_reconciled` (INFO). A REST or token failure:
  `INCOMPLETE` (halt kind `INCOMPLETE`, auto-clearing; `lock_audit_incomplete`-style
  WARN row `reconciliation_incomplete`), never a mismatch. A surviving mismatch:
  halt kind `RECONCILIATION` (owner-only clear), CRITICAL `position_reconciled`
  with `details.status = mismatch`. The halt is written through the `halt` module
  (`09-lock-state-machine` §8b), not by an ad-hoc file write.
- **Order.** Reconciliation (16:15) precedes the lot update, the ledger write
  (16:25) and the compliance audit (16:40, `09` §8); the ledger is written only
  after a clean reconciliation (`16-portfolio-ledger`).

## 6d. Kill Switch and Owner Actions

All owner actions are Rust commands on the host or in the container (`quant-exec
...`); none is in the frontend (the frontend is a viewer, `17-frontend`) and none
is in Julia.

- **Kill switch** (`quant-exec kill`): cancels every working order (cancels only;
  a cancel is not a placement) and sets the halt flag with kind `KILL_SWITCH`.
  Audit rows `kill_switch_activated` (CRITICAL) and `trading_halted`. It **never
  liquidates** and never places an order. Owner-only clear (`quant-exec halt
  clear`). Orders that cannot be cancelled stay open and are counted by the lock.
- **`manual_override`** is an audit event type for owner actions that supply a
  fact or restart the system, with `details.kind` one of `halt_clear`,
  `calendar_add`, `journal_resolve`. It **never** refers to the lock: no override
  of the lock exists, and no `kind` can be added that would bypass `authorize`.
- The constitution §4 table lists "manual override" among the frontend's
  responsibilities; `17-frontend` forbids it. This skill follows `17-frontend`
  (the work report records the contradiction for the owner to resolve in the
  constitution).

## 7. What Not to Do

- Do not build a `MarketOrder` without a `MarketPermit`. Only
  `Tier::market_permit` issues one.
- Do not cache the tier. Read the trailing 5-day mean NAV at submission.
- Do not round Standard-tier quantities to nearest. Round down.
- Do not submit a fractional limit order. Fractional quantities fill only as
  market orders in regular hours (owner-confirmed).
- Do not submit a fractional quantity with more than 2 decimals (round down).
- Do not assume an order is filled because it was accepted. Poll until
  terminal.
- Do not enable a reserved order type (`StopLimit`, `TrailingStop`, `OCO`,
  `LimitOnClose`) without a written lock analysis; do not build a `Bracket` or an
  entry-plus-exit `OTO`.
- Do not release the ticker gate between the history read and the send, and do
  not pass a permit through a channel.
- Do not reuse a REST read for a second authorize (`N = 0`).
- Do not write the halt flag from anywhere but the `halt` module, and do not
  clear a `VIOLATION` halt automatically.
- Do not reconcile in Julia, and do not hold a Schwab token outside Rust.
- Do not retry a rejected order without understanding why.
- Do not skip daily reconciliation. The local state drifts.
- Do not authorize an order from a cached lock row, a stored expiry, or a
  previous day's history. Load the order history at submission.
- Do not submit an order for a restricted ticker, and do not sell a restricted
  position found in a Schwab account. Halt and report.
- Do not add an override, a bypass flag or a "force" parameter to
  `submit_*`. A veto never forces a sale inside the window.
- Do not submit an order for a ticker after an unreadable order-history
  source. Fail closed.
- Do not authorize from the Streamer. A stream message is a dirty trigger and an
  early warning; the REST read is the anchor, and if REST fails the order is
  denied even when the stream looks fine.
- Do not take a fill date from the stream's `ExecutionTimeStamp` (timezone
  UNVERIFIED) or from `closeTime`. Use REST `executionLegs[].time`.
- Do not rely on the aggregate `GET /orders` alone, or assume
  `ACCT_ACTIVITY` covers every linked account (UNVERIFIED). Read each account.
- Do not store raw account numbers; store hashes.
- Do not submit a fractional limit order or rely on a fractional limit sell
  (UNVERIFIED), and do not place a fractional order outside regular hours.
- Do not promote an UNVERIFIED API fact in `SCHWAB-API-REFERENCE.md` to a
  design assumption; test it live (§8) and update the reference first.

- Do not store the refresh token in plaintext.
- Do not run the adapter in a container without the state mount.
- Do not call the bridge from the order submission path.

## 8. Verification

- [ ] A `MarketOrder` cannot be built without a `MarketPermit` (the
      permit field is private)
- [ ] `Tier::market_permit` without `reduce_only` returns `Err` for Standard, and for Transition
      unless `0 < fractional_part < 1`; with `reduce_only` (amendment 1.13) it allows
      any Bootstrap exit, the Transition remainder, and in Standard only a `qty < 1`
      remainder that completes a full exit; `ReduceOnly` is only built from a `SellPermit`
- [ ] A test asserts the boundaries: 2,499.99 → Bootstrap, 2,500 →
      Transition, 9,999.99 → Transition, 10,000 → Standard
- [ ] A test asserts the tier is re-read on every submission (a changed
      ledger changes the next order's tier)
- [ ] A test asserts Standard-tier quantities round down and never exceed
      the 5% cap or the cash buffer
- [ ] A test asserts the pre-commit hook rejects a market-order reference
      in a file outside the allow-list
- [ ] No `SELL_SHORT` instruction exists
- [ ] **Before any live order (a later phase): the five live tests** (`SCHWAB-API-REFERENCE.md`
      §8, run on the real account(s) in shadow mode; failing any keeps the
      system out of live trading until the reference and these docs are
      updated with the observed behaviour):
      (1) order-history window and truncation: windows of 30/59/61/90/365/400
      days on both `/accounts/{hash}/orders` and `/orders`, omitted
      parameters, > `maxResults`, ordering (resolves the 60-day vs 1-year
      conflict; the design stays inside 60 days until then);
      (2) `ACCT_ACTIVITY` scope: one subscription, a tiny order in a second
      linked account, does the event arrive?;
      (3) partial fill, cancel, replace and reject: REST and stream
      representation (old order `REPLACED` or `CANCELED`,
      `replacingOrderCollection`, `closeTime`, `filledQuantity > 0` with
      `CANCELED`, the stream event names);
      (4) fill-time field and time zone: stream `ExecutionTimeStamp` vs REST
      `executionLegs[].time`, offset form of `enteredTime`/`closeTime`;
      (5) confirm the 2-decimal limit and sell-side fractional behavior via
      `POST .../previewOrder` (`quantity: 0.25`, `0.255`, and a sell), and how
      fractional fills appear (acceptance itself is owner-confirmed).
- [ ] Added live items: `transactions` TRADE row shape and same-day availability;
      whether `Order.tag` can be set on POST and is returned; `Location` header
      order id on POST; market-hours endpoint for *today* on a closure morning and
      for half days; the refresh token's real lifetime and whether the token
      response carries an expiry
- [ ] Remaining live items (not gate-blocking but logged): rate-limit actuals
      and `Retry-After`; streamer heartbeat cadence, token-expiry survival,
      symbol cap, ACCT_ACTIVITY outside market hours; LEVELONE size units vs
      REST, field 32 spellings, absence of a LULD field; `/markets` on holidays
      and early closes; CHART_EQUITY numbering; `userPreference` shape;
      `Order.accountNumber` population and hash stability
- [ ] Recorded-fixture tests parse the redacted real payloads cited in the
      reference; an unknown enum value is counted and marks the history
      incomplete
- [ ] A stream message never authorizes; a REST read of all accounts' orders and
      of the 14-day transactions precedes **every** `authorize` (`N = 0`, even two
      in a row); REST down denies
- [ ] The ticker gate makes check-then-send atomic: concurrent opposite-side
      submissions for one ticker yield one permit; the intent is written at
      permit issue; ladder replacements, the pending release and the generator all
      enter through the gate
- [ ] `TRADING_HALTED` (present, unreadable or unparseable) denies in
      `authorize`, in `submit_*`, in the pending release and in the ladder
- [ ] The order-type allowlist is exactly `{Limit, Market(MarketPermit)}`; the
      guard rejects every other type; `Bracket` is absent from the allowlist
- [ ] A reduce-only sell of a name above the 5% cap is not rejected by the cap
      guard
- [ ] Reconciliation (Rust) is multi-account, flags `shortQuantity` and
      restricted tickers, applies the tolerance and the retry, and treats a REST
      outage as `INCOMPLETE`, not a mismatch; there is no Julia Schwab client
- [ ] `kill` cancels working orders and halts, and never places an order
- [ ] `fractional_eligible` is `No`/`Unknown` => no Bootstrap fractional trade;
      no fractional order is placed outside 09:30-16:00 ET
- [ ] A test asserts fractional quantities are floored to 2 decimals (never
      rounded up, never more than 2 decimals), and that a Transition name
      that is not fractional-eligible gets the whole-share limit only (the
      remainder stays in cash; a sub-share weight yields no order)

- [ ] `authorize` runs on a freshly loaded `OrderHistory` for every
      submission; no code path reads `position_locks` to authorize
- [ ] A restricted ticker (`ICE`) is rejected before the history is loaded,
      with CRITICAL `restricted_security_blocked`; an unreadable
      `restricted.toml` rejects all orders
- [ ] An unreadable Schwab account, journal or `lock_external.csv` blocks
      the order
- [ ] A restricted position or order found in a Schwab account halts trading,
      writes CRITICAL, and generates no sell
- [ ] The daily lock-compliance audit (16:40) and its dated report run after
      reconciliation (16:15)
- [ ] Pre-submit checks for position, sector, and cash caps
- [ ] Reconciliation runs daily and halts (`RECONCILIATION`) on a mismatch that
      survives tolerance and retry
- [ ] OAuth token refresh is tested with a mock server
- [ ] Shadow engine produces fills within expected slippage
- [ ] A test submits a limit order far from market and cancels it

## 9. Related Skills

- `00-construction` §6, §6a — execution constraints and NAV tiers
- `08-portfolio-optimization` §6a — the weights this section converts to quantities
- `14-backtest-protocol` §6a — the simulated version of §2a
- `10-execution-schwab/SCHWAB-API-REFERENCE.md` — API facts with confidence labels (UNVERIFIED stays UNVERIFIED)
- `09-lock-state-machine` — order gating, the ticker gate, order-history data flow, restricted tickers, the halt flag, the daily compliance audit

- `10-execution-schwab/EXECUTION-WINDOW.md` — execution timing and slippage (window, deadline, sequencing)

- `11-rust-event-bus` — where order events are dispatched
- `12-julia-rust-bridge` — the bridge that is not used here
