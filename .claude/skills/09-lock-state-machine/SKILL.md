---
name: lock-state-machine
description: The 5-day transaction lock, a contractual rule computed from order
  history. Load when touching execution/src/locks/, order history, pending
  orders, the compliance audit, the halt flag, restricted tickers, or any order
  gating code. (The directory name is historical; there is no state machine
  any more.)
---

# The 5-Day Lock

> **Phase: later.** Execution and live trading come last in the build order
> (`outline.md` "Project phase and build order"). This skill is the
> specification for that phase; do not build or refine it unless asked. The
> oracles and fixtures under `scripts/` and `fixtures/` are the exception:
> they are language-neutral test assets and may be used at any time.

The lock is a **contractual obligation of the account owner**, not a risk
preference. A breach is a breach of contract, so this skill outranks latency,
slippage and profit, and every ambiguity resolves to "block". The rule is the
constitution's §1; this skill is its mechanics. If this file and the
constitution disagree, the constitution wins.

The skill directory keeps its old name (`09-lock-state-machine`) so that
references do not break. There is **no stored lock state**: the lock is a pure
function of order history.

Files in this skill: `STATE-TRANSITIONS.md` (semantics and edge cases),
`PENDING-ORDERS.md` (the queue), `scripts/lock-monitor.rs` (pending timer
sketch), `scripts/lock_rule_oracle.jl`, `scripts/nyse_calendar_oracle.jl`,
`scripts/run_fixtures.jl` (Julia test oracles), `fixtures/lock_cases.json` and
`fixtures/nyse_calendar_cases.json` (golden cases). The compliance audit
sketch is `10-execution-schwab/scripts/audit-lock-compliance.rs`.

**Sketch policy.** Every `.rs` / `.jl` file under a skill's `scripts/` and every
code block in a skill is a SKETCH: not compiled, not normative. The crate
files named in `scaffold.md` are the real homes. The fixtures are the only
executable contract (§9).

## 1. The Rule

Restated from constitution §1 (normative text lives there):

1. A **counted order** is open (working) or has at least one fill. Cancelled,
   rejected, expired or replaced orders with zero fills are not counted. A
   partially filled order that was then cancelled **is** counted.
2. `D(order)` is the order's **last fill date** if it filled later than it
   was placed (a resting or GTC order; partial fills on several days use the
   last fill day), otherwise its placement date. Dates are in
   **America/New_York** regardless of time of day (23:30 ET counts as that ET
   day even if the UTC date is the next one). A `D` on a non-trading day rolls
   forward to the next session.
3. A counted order blocks the **opposite** side from its placement day. While
   it is still **open** it blocks with **no end date**. Once it has stopped
   working it blocks through the 4 trading days after `D`;
   `first_permitted = D + 5 trading days`. Buy on Monday, filled Monday: the
   first sell is the next Monday. Placed Monday, filled Thursday: the next
   Thursday. Partially filled Mon and Wed, then cancelled: the next Wednesday.
4. Same-direction orders are always allowed and, once counted, extend the
   block: the block ends at the `max` over counted opposite-side orders of
   their end (and never ends while any of them is open).
5. Applies across **every account the owner trades** (see §3).
6. Long-only: a sell requires shares held, else `NothingToSell` (a rejected
   request, not a lock event).
7. **No exceptions.** No override, permit, flag or manual instruction exists.

The executable definition is the pair of language-neutral fixture files
(`fixtures/lock_cases.json`: 19 named + 3000 random cases;
`fixtures/nyse_calendar_cases.json`: 22 years of holidays + 1500
`add_trading_days` cases) together with the Julia oracles
`scripts/lock_rule_oracle.jl` and `scripts/nyse_calendar_oracle.jl`, which
implement the same functions and are run against the fixtures by
`scripts/run_fixtures.jl`. The Rust `counted`, `block_end`, `blocked` and
calendar functions must reproduce the fixtures exactly (§9,
`execution/tests/lock_fixtures.rs`). The fixtures were generated from a
verified Python reference that has since been removed from the repository
(no Python anywhere in the repo); the fixtures are now frozen goldens.

```
D(o)         = roll_forward( et_date(o.last_fill_time) if o.filled_qty > 0
                             else et_date(o.placed_time) )       # oracle: last_fill, else placed
block_end(o) = Open if o is open else FirstPermitted(add_trading_days(D(o), 5))
blocked(side, today) = any counted o with o.side == opposite(side)
                           and (block_end(o) is Open or today < block_end(o))
```

An order that is still working, even with partial fills, is open: no end date
until it stops working. The oracle's `last_fill` field is the order's last
fill date; a placed-only order uses its placement date.

The oracle's status handling is the full 21-status table of §3.1: the 15 open
statuses block with no end date; `FILLED` counts from its last fill day;
`CANCELED`, `REJECTED`, `EXPIRED` and `REPLACED` are not counted with zero
fills and count from the last fill day with any fill; any unknown, missing or
non-string status is treated as open.

## 2. Derived Display Labels (not state)

The five names `IDLE`, `LONG_LOCKED`, `LONG_UNLOCKED`, `FLAT_LOCKED`,
`FLAT_UNLOCKED` may still appear on the dashboard as **labels derived at
render time** from the pure function (`LONG_LOCKED` = shares held and a sell
is blocked today, and so on). They are never stored as authority, never read
by `authorize`, and never fed back into an order decision. The
`position_locks` file (`13-parquet-lake` #14) is such a derived view. A null
`lock_expiry_date` in that view means an **open order with no end date**, not
"unlocked": the dashboard must render it as "blocked, open order".

## 3. Inputs: the Order History

`authorize` takes an `&OrderHistory`. It is the merge of the sources below and
is **rebuilt from a fresh REST read before every `authorize`** (freshness
bound `N = 0`, §3.2), never answered from a cached row. The in-memory history
is a REST-anchored copy of the orders, never a stored lock verdict.

| Source | Content | Failure means |
|---|---|---|
| Schwab Trader API orders, **each linked account** (REST per account) | Every order, any origin (including manual trades in the Schwab app) | Block |
| Schwab `transactions` (`types=TRADE`), each linked account, last 14 calendar days | Fills of orders that the entered-time window cannot show (§3.1 caveat); read at authorize time | Block |
| Audit journal (`audit.db`) | Intents and outcomes written by this system, including orders just sent that the API may not list yet. Merged with Schwab as in §3.7 | Block |
| `lock_external.csv` (owner-maintained) | Orders in accounts the API cannot see (for example the owner's Fidelity account) | Block |

**Fail closed.** If any source cannot be read, is stale, cannot be parsed, or
does not cover the lookback window, the loader returns an
`Incomplete` history and `authorize` denies the order. Because an unreadable
history means an opposite-side order might exist for any ticker, this blocks
both sides for every ticker until the history loads again. A loud audit row is
written (`WARN` once per episode, `CRITICAL` if it persists past the next
scheduled order time). **Exception, deliberately narrow (§3.6):** a single
order carrying an unknown enum value whose symbol parses degrades to a
block on **that ticker only**; it does not stop trading in the other names.

### 3.1 Schwab facts used (labels from `10-execution-schwab/SCHWAB-API-REFERENCE.md`)

Labels: CONFIRMED-OFFICIAL (Schwab spec file via mirror), CLIENT-SOURCE
(named open-source client), COMMUNITY, UNVERIFIED (must be tested live; never
treated as certain here). Anything not labelled below is UNVERIFIED.

- **Accounts.** `GET /trader/v1/accounts/accountNumbers` returns
  `{accountNumber, hashValue}` pairs (CONFIRMED-OFFICIAL). The URL path
  parameter of `/accounts/{accountNumber}/orders` is literally named
  `accountNumber` but must be the **encrypted `hashValue`** (CONFIRMED-OFFICIAL
  and CLIENT-SOURCE). Hash stability across sessions is UNVERIFIED: re-fetch at
  every startup. **Store hashes (or a salted id), never raw account numbers**;
  `Order.accountNumber` (int64, plain) is used only to attribute orders from the
  aggregate endpoint and is mapped to a hash immediately; population on every
  order is UNVERIFIED.
- **Orders.** `GET /trader/v1/accounts/{hash}/orders?fromEnteredTime=..&toEnteredTime=..&maxResults=..&status=..`
  and the aggregate `GET /trader/v1/orders` (same parameters; CONFIRMED-OFFICIAL).
  `fromEnteredTime` and `toEnteredTime` are marked **required** in the spec
  (format `yyyy-MM-dd'T'HH:mm:ss.SSSZ`, e.g. `2024-03-29T00:00:00.000Z`, UTC;
  CONFIRMED-OFFICIAL). `maxResults` default 3000. `status` is a single optional
  enum value (21 values, below). The date filter is on **`enteredTime` only**
  (CONFIRMED-OFFICIAL text; the consequence below is our deduction).
- **Window limits (conflict, live test item).** Spec: per-account "maximum date
  range is 1 year"; aggregate `/orders`: "within 60 days from today's date".
  schwab-py documents 60 days for both (CLIENT-SOURCE). The effective
  per-account window is UNVERIFIED. **Design: stay inside 60 days always.**
  Entered-time lookback **45 calendar days** (`from = now - 45d`, `to = now`)
  in a single window per account, which is safe under either reading and covers
  the 21-day requirement; a window that returns `maxResults` rows (or more) is
  `Incomplete` (split by day and retry; truncation behaviour above 3000 is
  UNVERIFIED). Result ordering is UNVERIFIED: sort by `enteredTime`, `orderId`.
- **History-window caveat and the `transactions` net.** Because the filter is
  on `enteredTime`, an order entered before the window and filled or still
  working inside it is invisible to a window read. The system submits `DAY`
  orders only, so its own orders are always inside the window; the open-status
  query (3.3) covers working orders entered within its (widest safe) window; a
  long-lived order entered earlier (a manual GTC from the Schwab app) can still
  be missed. **(m7) Mitigation at authorize time:** every `authorize` is
  preceded by `GET /accounts/{hash}/transactions?types=TRADE` for each linked
  account over the **last 14 calendar days** (`startDate`/`endDate`, required,
  CONFIRMED-OFFICIAL spec; the longest 5-trading-day span from 2018 to 2044 is
  11 calendar days, so 14 covers every block; "maximum 3000 transactions, 1 year
  range" CONFIRMED-OFFICIAL). Each TRADE transaction yields a *fill witness*
  (`orderId`, ET trade date from `time`, symbol, side):
  - if its `orderId` is in the merged history, the order's last fill date is
    raised to the witness date when that is later (never earlier than any known
    timestamp);
  - if its `orderId` is **not** in the history, a synthetic counted order is
    added (source `Transactions`, `D` = the ET date of the witness, status
    `FILLED`), and a `WARN` `fill_without_order` row is written (`CRITICAL` if
    it falls inside a block);
  - the side of a `TRADE` transaction is read from `transferItems[]`
    (`amount` sign / `positionEffect`; their exact semantics are UNVERIFIED): if
    the side cannot be established with certainty, the synthetic order blocks
    **both** sides of that ticker.
  Same-day availability of TRADE rows is UNVERIFIED (settlement lag), so this
  is a net under the order read, never a replacement for it. The transactions
  read failing (error, timeout, truncation, unparsable) is an `Incomplete`
  history like any other source. The residual risk (a pre-window order whose
  fill is also invisible) is stated in the compliance report.
- **Time.** `enteredTime`, `closeTime` and `executionLegs[].time` are strings
  like `2021-01-01T12:01:00+0000` in client samples (UTC, `+0000` without a
  colon; CLIENT-SOURCE). Parse leniently (`+0000`, `+00:00`, `Z`), assume UTC
  (UNVERIFIED for non-UTC offsets), **convert to `America/New_York`** and take
  the date. Never the UTC date.
- **Fill time (CONFIRMED-OFFICIAL).** There is no order-level fill time.
  `D` uses the latest `orderActivityCollection[].executionLegs[].time`
  (`ExecutionLeg.time`). `closeTime` is the order's terminal time and is **not**
  a fill time. If `filledQuantity > 0` and no `executionLegs` time can be read,
  use the later of the `enteredTime` and `closeTime` dates (never earlier than
  any known timestamp); if neither parses, the history is `Incomplete`. Fills
  are always taken from **REST**; the stream's `ExecutionTimeStamp` (timezone
  UNVERIFIED) is never used as a fill date (3.4).
- **Fields used (CONFIRMED-OFFICIAL names):** `orderId`, `accountNumber`,
  `status`, `enteredTime`, `closeTime`, `duration`, `session`, `quantity`,
  `filledQuantity`, `remainingQuantity`, `tag`, `orderLegCollection[].instruction`
  (`BUY` / `SELL`; every other value, including `SELL_SHORT`, `BUY_TO_COVER`
  and the option instructions, is an anomaly: the order is counted as
  blocking **both** sides of its ticker, flagged `WARN`, and reviewed by the
  owner), `orderLegCollection[].instrument.symbol` and `.assetType`,
  `orderActivityCollection[].executionLegs[]`, `childOrderStrategies`,
  `replacingOrderCollection`. Every field is `Option` in the Rust model; every
  enum has an `Unknown(String)` capture (3.6).

**Status mapping to the counted rule.** The 21 `apiOrderStatus` values are
CONFIRMED-OFFICIAL (SPEC-T; identical in schwab-py and Schwabdev). Real values
may exceed the list: any value not in the table counts as open.

| Schwab `status` | Counted? |
|---|---|
| `WORKING`, `QUEUED`, `ACCEPTED`, `NEW`, `PENDING_ACTIVATION`, `PENDING_ACKNOWLEDGEMENT`, `PENDING_CANCEL`, `PENDING_REPLACE`, `PENDING_RECALL`, `AWAITING_PARENT_ORDER`, `AWAITING_CONDITION`, `AWAITING_STOP_CONDITION`, `AWAITING_MANUAL_REVIEW`, `AWAITING_UR_OUT`, `AWAITING_RELEASE_TIME` | **Open: counted, no end date** |
| `FILLED` | Counted, from its last fill date |
| `CANCELED`, `REJECTED`, `EXPIRED`, `REPLACED` | Counted **only if `filledQuantity > 0`** or any `executionLegs` quantity > 0 (a partial fill then cancel counts, from its last fill date). The rendering of partial-fill-then-cancel is UNVERIFIED, so never infer "no fill" from the status: sum `executionLegs[].quantity` and read `filledQuantity`, take the larger. **`REPLACED` with zero fills is NOT counted, exactly like `CANCELED`** (owner decision 2026-10-10; the replacement is a new `orderId` with its own `D`; the old order ends `REPLACED` or `CANCELED`, UNVERIFIED which; the link `replacingOrderCollection` is UNVERIFIED so no linkage is relied on) |
| `UNKNOWN`, any other string, or missing `status` | **Open: counted, no end date** (fail closed) and flagged (3.6) |

`PENDING_CANCEL` is still working until Schwab confirms the cancel. A cancel
that races a fill can leave a fill the system has not seen: that is why the
status is re-read on every check and a `PENDING_CANCEL` order counts. A replace
is the same: between `PENDING_REPLACE` and `REPLACED` the old order is open and
counts; once `REPLACED` with zero fills only the new order counts, and the new
order is a same-side order carrying its own open status.

### 3.2 Data flow: prefill, live updates, REST before every authorize

The history is a **live copy of the orders, REST-anchored**. It is never a
stored verdict: `authorize` recomputes the block from it every time
(constitution §1 rule 7: "recomputed from order history at submission time",
"never answered from a cached lock row").

1. **Prefill by REST** at startup and at the 09:00 pre-open refresh (and on
   every dirty event): `GET /accounts/accountNumbers`; then **per account**
   `GET /accounts/{hash}/orders` over the 45-day window (all statuses); then the
   open-status queries (3.3) and the 14-day `transactions` read. The aggregate
   `GET /orders` is **not relied on alone**; it is used only as a cross-check
   (a mismatch with the per-account union marks the history dirty and WARNs).
   Every response is logged with its `Schwab-Client-CorrelId` header.
2. **Live updates from the Streamer** `ACCT_ACTIVITY` subscription
   (`keys: "Account Activity"`, `fields: "0,1,2,3"`, CONFIRMED-OFFICIAL request
   shape; one streamer connection per user, code 12 otherwise) are a **dirty
   trigger and early warning only**. A message gives account (`"1"`, plain
   number, mapped to a hash and not stored), type (`"2"`) and a JSON string
   (`"3"`) with `SchwabOrderID`. The handler marks that order "stale" and raises
   an alert (a fill or cancel seen in the stream before REST shows it WARNs and
   re-reads); it never changes an `authorize` result by itself. De-duplicate on
   `seq` (uniqueness across batches UNVERIFIED, so a duplicate `seq` is ignored
   only if the payload is identical).
3. **Dirty rules.** The history is marked **dirty** (and a full REST re-read is
   triggered) on: any unknown `"2"` message type (the observed set is
   `OrderCreated`, `OrderAccepted`, `OrderMonitor{Created,Updated,Completed}`,
   `ExecutionRequested`, `ExecutionRequestCreated`, `ExecutionRequestCompleted`,
   `ExecutionCreated`, `OrderFillCompleted`, `CancelAccepted`,
   `OrderUROutCompleted`; CLIENT-SOURCE, Schwabdev docs; partial-fill, reject and
   replace event names are UNVERIFIED, so none is keyed on), a `seq` gap, a
   disconnect, a missed heartbeat (cadence UNVERIFIED; the threshold is a
   placeholder, default 3x the observed cadence), a parse failure, a response
   code 30 or 12, a LOGIN denial, or a position change not explained by a known
   order. Because every `authorize` re-reads over REST (next point), dirtiness
   matters for the compliance snapshot, the dashboard and the alerting, not for
   the correctness of an authorize.
4. **The REST read is the anchor, and `N = 0`.** *Decision (owner,
   2026-10-10):* the REST freshness bound is **zero** for every account
   relevant to the order: **a REST read of every linked account's order window
   and of the 14-day `transactions` is performed before EVERY `authorize`**,
   after the per-ticker gate (§6) is held, and its result is not reused for any
   other authorize. Constitution §1 rule 7 requires the check to be recomputed
   at submission; with `N = 0` the stream cannot be what a permit rests on. If
   any REST call fails (HTTP error, timeout, 429 after bounded retries,
   truncation, unparsable body), `authorize` **denies** (fail closed), **even if
   the stream looks healthy**. Correctness never depends on the stream. The
   constant exists in code (`ANCHOR_MAX_AGE`, default `Duration::ZERO`) so tests
   can exercise it; raising it is an owner decision, not a tuning knob.
   *Cost:* per authorize, `accounts + accounts` requests (orders and
   transactions) against the reported 120 requests/minute (COMMUNITY); a
   rebalance of 20 names with 2 accounts is about 80 requests and is paced by
   the client's rate limiter (a 429 is retried with backoff, never skipped). The
   order generator therefore takes minutes, not seconds, for a large rebalance;
   that is accepted.
   **Freshness guarantee, exactly (owner follow-up, 2026-10-10).** `authorize`
   may only use an `OrderHistory` snapshot whose REST fetch **started after that
   authorize call began** (per relevant account). There is **no TTL cache of order
   history and no cache of lock verdicts, ever.** *Why a cache buys nothing:* the
   latency of a fresh read (about 100-300 ms, unmeasured) is about 0.8 bps of
   one-sigma price noise with zero mean for a stock with 2-3% daily volatility,
   against 2-10 bps half-spreads and multi-minute ladder waits; a cache would buy
   no measurable bps and would open a window for an unseen manual order (a
   contract breach).
   **Request coalescing (single flight) protects the 120 requests/minute limit**
   at the 10:00 burst and during ladder reprices: concurrent `authorize` calls
   share one in-flight fetch per `(account_hash, fetch_epoch)` **only if that
   fetch started after their own start**; a caller that arrives after a fetch
   began waits for the next fetch (a new epoch). Implementation: the Rust `moka`
   crate (`future::Cache`, `try_get_with`; verify the API) or an equivalent
   single-flight map, used **solely for in-flight de-duplication**: the entry is
   invalidated the moment the fetch resolves (or is keyed by epoch so it can never
   be hit by a later epoch). A resolved result is never served to a later caller.
   **TTL caches that are allowed (NON-compliance data only; indicative
   placeholders):** account hash list (hours), symbol map, fractional eligibility,
   restricted list (reloaded on file change; a "not restricted" answer is never
   cached across a file change), NYSE calendar / market-hours answers (minutes to
   hours; an override append invalidates), per-name cost model (per session).
   **Never cached:** order history, positions used in `authorize`, the halt flag,
   lock verdicts.
5. **ACCT_ACTIVITY coverage of all linked accounts is UNVERIFIED** (the key is
   arbitrary; the scope is per user/token; no doc says every linked account is
   covered). Per-account REST is therefore mandatory regardless, and the stream
   is treated as covering nothing in particular.
6. **Replace.** `PUT .../orders/{id}` cancels the old order and creates a new one
   with a new `orderId` (CONFIRMED-OFFICIAL text); the old order ends `REPLACED`
   or `CANCELED` (UNVERIFIED). Both are handled the same way: not counted with
   zero fills, counted from the last fill day with any fill; each order with its
   own `D`, no reliance on `replacingOrderCollection`.
7. **No raw account numbers** are stored in the cache, the snapshot, the audit
   rows or the `order_history` file; hashes (or a salted id) only. The stream's
   plain account number is mapped to a hash on receipt and discarded.

### 3.3 Open-status queries

`status` is a single enum value (a comma-separated list is UNVERIFIED), so the
pre-open refresh issues one query per open status per account (`WORKING`,
`QUEUED`, `ACCEPTED`, `NEW`, `PENDING_ACTIVATION`, `PENDING_ACKNOWLEDGEMENT`,
`PENDING_CANCEL`, `PENDING_REPLACE`, `PENDING_RECALL`, `AWAITING_PARENT_ORDER`,
`AWAITING_CONDITION`, `AWAITING_STOP_CONDITION`, `AWAITING_MANUAL_REVIEW`,
`AWAITING_UR_OUT`, `AWAITING_RELEASE_TIME`) over the **widest window known to be
safe** (59 days by default; the per-account 1-year window may be used only after
the live test confirms it). The filter is still on `enteredTime`, so this query
finds working orders entered within that window; it does **not** find an open
order entered earlier (history-window caveat above). The 15 queries per account
run only at startup, at 09:00 and when dirty, not at each submission (the
submission-time window read already includes open orders entered within 45 days,
and the `transactions` net covers pre-window fills).

### 3.4 Stream versus REST

REST is authoritative for status, fills and fill dates. If a stream-derived
update and a REST read disagree, REST wins, the history is marked dirty, an
audit `WARN` `order_history_disagreement` row is written (a stream message
reporting a fill that REST does not yet show means the REST read was early:
re-read after a short delay, and if it still disagrees the order counts as
filled today). A stream message can only make the lock stricter before the next
REST read, never looser; since every `authorize` re-reads over REST, "before the
next REST read" is at most the time to the next authorize.

### 3.5 Pre-go-live gate: the five live tests (reference section 8)

These must pass on the real account(s) before any live order; failing any keeps
the system in shadow mode. Details in `10-execution-schwab/SKILL.md` §8.

1. Order-history window and truncation: windows of 30/59/61/90/365/400 days on
   both endpoints; counts; omitted parameters; > `maxResults`; ordering.
2. `ACCT_ACTIVITY` scope: does one subscription deliver events for all linked
   accounts (a tiny order in a second account while subscribed)?
3. Partial fill, cancel, replace, reject: the REST and stream representation of
   each (status of the old order, `closeTime`, `replacingOrderCollection`,
   `filledQuantity > 0` with `CANCELED`).
4. Fill-time field and time zone: stream `ExecutionTimeStamp` vs REST
   `executionLegs[].time`; the offset form of `enteredTime`/`closeTime`.
5. Fractional order acceptance by the API via `previewOrder`, and the sell-side
   order types (10 SKILL §2b).

Added by this revision (live items, also in `10-execution-schwab/SKILL.md` §8):
the `transactions` TRADE row shape (side semantics, same-day availability);
whether `Order.tag` can be set on POST and is returned on GET; the market-hours
endpoint's behaviour for *today* on a closure morning (§4).

### 3.6 Rust types and unknown-variant handling

Per reference section 9: every REST field `Option<_>` except identity keys;
`#[serde(rename_all = "camelCase")]`; every enum has `#[serde(other)] Unknown`
(or a `Raw(String)` capture); quantities as `Decimal`; ids `i64`;
`#[serde(flatten)] extra: Map` on `Order` to log new fields; the stream's
fixed-point `{lo, signScale}` is kept raw (`value = lo / 10^6` is inferred,
UNVERIFIED) and is never used for the lock (only REST quantities are). Stream
messages feed the existing event classes (`11-rust-event-bus`): an `OrderFilled`
or `OrderCancelled` event is emitted only after the REST re-read confirms it.

**An unknown enum value must not stop all trading for 45 days.** One order
with an unrecognised value in a decision field (status, instruction, asset
type) lives in the 45-day window for 45 days; making the whole history
`Incomplete` for that long would freeze the account. The rule is graded:

| Case | Effect |
|---|---|
| Unknown **status** string, `instrument.symbol` parses to a ticker | The order counts as **open, no end date, for THAT ticker only**; `WARN` `unknown_enum_value` (once per order id per day); the history stays `Complete` |
| Unknown **instruction** (not `BUY` / `SELL`) or unknown **asset type**, symbol parses | The order blocks **both sides of that ticker**; `WARN`; the history stays `Complete` |
| Missing / empty / unparseable `instrument.symbol` (cannot attribute the order to a ticker), a non-string `status` field that cannot be read at all, an unparseable order body | History is **globally `Incomplete`**: every `authorize` denies until it loads again; `CRITICAL` if it persists past the next scheduled order time |
| Any unreadable, truncated or unparsable *source* (HTTP error, timeout, page truncation, bad external file) | Globally `Incomplete` (as in §3) |

"Parses" means the symbol maps to a canonical ticker through the symbol map
(§3.8) **or** is a well-formed equity symbol (`[A-Z0-9./-]{1,12}`) that is not in
the map; an order in a symbol outside the map is carried under its normalised
raw symbol so that, if the universe later adds it, the order already blocks it.
The owner reviews every `unknown_enum_value` WARN and updates the status table
and the oracle if it is a real new value.

**Test fixtures.** Recorded-fixture tests use the redacted real payloads cited
in the reference (Schwabdev `docs/pages-raw/stream.md` ACCT_ACTIVITY samples;
schwab-rs `src/models/trader.rs` fixtures; schwab-py test payloads), checked in
under `execution/tests/fixtures/schwab/`, with account numbers replaced by
hashes. They prove parsing, not behaviour; behaviour is the live tests above.

OCO/OTO/bracket child legs are separate orders for this purpose. A resting
sell leg (a stop) counts as an open sell from its placement and blocks buys.
Therefore a bracket whose exit legs are sells entered together with the entry
buy is itself a violation (the sell legs are placed on the buy's `D`). Such
orders are not submitted: v1 allows only `Limit` and `Market` orders
(`10-execution-schwab/SKILL.md` §2).

**Order duration.** A late fill counts from the fill (rule 2), so a GTC order
is compliant for the lock: it blocks while open and from its last fill date.
The system nevertheless submits `DAY` orders by design (same-session fills,
slippage, a simpler open-order picture). A working order older than one
session is an anomaly (audit `WARN`), typically a manual GTC order in the
Schwab app. Because an open order blocks with no end date, a forgotten GTC
sell blocks buys of that ticker until it is cancelled or fills.

### 3.7 Merging the audit journal with Schwab

Schwab is **authoritative** for every order it lists; the journal exists to
cover the gap between "we sent it" and "Schwab lists it". The merge is
deterministic:

1. **Identity.** A Schwab order is identified by `(account_hash, orderId)`.
   Every journal intent has an `intent_id` (UUID, the audit `event_id`); once
   the broker call returns, the outcome row records the `orderId` parsed from
   the `Location` header of the 201 response (CONFIRMED-OFFICIAL for replace;
   assumed the same for POST: live item). The system also sets `Order.tag =
   intent_id` on every POST if the API accepts it (the field exists in the
   schema as "client tag"; whether it can be set and is echoed back is
   UNVERIFIED); the tag is a hint, never relied on.
2. **Journal row with an `orderId` that Schwab lists** (same account): the
   journal row is **dropped**; Schwab's status, fills and dates are used.
   Schwab saying `CANCELED` with zero fills wins over a journal that says
   "sent".
3. **Journal row with an `orderId` that Schwab does NOT list:** it counts as
   **OPEN** (no end date, `D` = the intent's ET date). A `WARN`
   `journal_order_not_in_broker` is written if the intent is older than the
   **lag grace** (placeholder 120 s: just-sent orders may not be listed yet);
   within the grace it is `INFO`. It stays open until Schwab lists it or the
   owner resolves it (point 6).
4. **Journal intent with no `orderId`** (crash between intent and send; or a
   send that timed out with unknown outcome): counts as **OPEN** the same way.
   The loader first tries to **adopt** a Schwab order: a Schwab order in the
   same account with the same symbol, side, quantity, `enteredTime` at or after
   the intent time (and, if the tag came back, an equal tag) and no journal row
   yet. Exactly one candidate: link it (the Schwab row now governs). More than
   one candidate: link all of them (fail closed: they all count). None, after
   the lag grace: stays open and WARNs.
5. **Definite negatives.** A journal outcome `rejected_by_broker` (the broker
   answered with a 4xx and no order was created) or `not_sent` (the permit was
   released without a send) is **not counted**. Anything ambiguous (timeout, 5xx,
   connection reset) is `send_unknown`, which is case 4.
6. **Resolution.** A journal-only open order is resolved by one of: Schwab lists
   it (then point 2); the owner runs `quant-exec journal resolve <intent_id>
   --never-sent | --found <orderId>` after checking the Schwab app, which writes
   an audit row (`manual_override`, `details.kind = journal_resolve`, actor
   `manual`) and is not a lock override: it states a fact about whether an
   order exists. Until resolved the order blocks the opposite side of its
   ticker; startup also blocks trading on any unresolved intent
   (`11-rust-event-bus` §6).
7. **Schwab orders the journal never saw** (manual trades in the Schwab app)
   count from Schwab alone; nothing in the journal is needed.
8. The journal never *reduces* what Schwab shows: it can only add an open order
   or leave Schwab's view untouched.

### 3.8 Symbol mapping (one canonical map)

The same company appears under different spellings in different sources
(Yahoo `BRK-B`, iShares holdings `BRKB`, Schwab `BRK/B`; also `BF.B`, class
shares such as `GOOG` / `GOOGL`, and renames such as `FB` to `META`; every
spelling here is a **verify** item, never a fact). A lock check that compares
spellings as strings can miss a block, so there is **one canonical symbol map**,
implemented in Rust (`execution/src/symbols.rs`) over the data file
`config/symbol_map.toml` (shape in that file; seeded from the `universe` dataset and
reviewed by the owner). The normalization function itself (`normalize`,
`render`, `is_tradable`) is **not** re-implemented here: `execution/src/symbols.rs`
is a thin layer over the shared Rust crate `symbols/` that `ingest/` also uses
(`01-data-ingestion/UNIVERSE.md` §3; test vectors in
`schemas/fixtures/symbol_normalization.csv`). A superseded-security ticker of
the form `SYMBOL~CIK10` (a delisted security whose symbol was reused) fails
`is_tradable` and is **rejected** at order entry, by `authorize` and by the
restricted and fractional lists; in order history it is still carried and
counted, never dropped. The map is used by:

- the lock history loader (every Schwab, journal, transaction and external-file
  symbol is canonicalised before it is compared);
- the restricted list (`config/restricted.toml` tickers are canonicalised, so
  `ICE` cannot be missed through a spelling);
- the fractional list (`config/fractional_eligible.toml`);
- the order generator and the snapshot writer (they emit canonical tickers; the
  Schwab spelling is produced only at the API boundary by `canonical -> schwab`).

Map shape: one entry per canonical ticker with its `yahoo`, `ishares` and
`schwab` spellings, an optional `aliases` list (old tickers after a rename, with
`until` dates), and a `lock_group`. An old ticker listed in `aliases` maps to
the new canonical ticker for the lookback window, so an order placed under the
old symbol still blocks the new one.

**Share classes are one lock group (decided, owner, 2026-10-10, DECISIONS B1).**
All share classes of one issuer form **one** `lock_group`: a counted order in
any class blocks the opposite side in **every** class of the same issuer, and a
same-direction order in any class extends the block for all of them. The lock
check runs over the group, not the ticker: `authorize(ticker, side)` collects the
history of every canonical ticker whose `lock_group` equals the ticker's group.
The issuer identity comes from the shared `symbols/` crate's map, whose
`lock_group` is derived from the `security_master` issuer key (the SEC CIK of
the issuer, `13-parquet-lake` `security_master`; two classes of one company share
the CIK); a class pair the map does not group but `security_master` gives one
issuer key fails the map load (fail closed, as a map conflict). Named examples
(dates as in §11):

- Buy `GOOGL` placed Mon 12 Oct 2026 and filled: a sell of `GOOG` is blocked
  Mon 12 through Fri 16 Oct; **first sell of `GOOG` (or `GOOGL`) Mon 19 Oct**
  (example 21).
- Sell `BRK.B` placed Wed 14 Oct: a buy of `BRK.A` is blocked through Tue 20 Oct;
  first buy of either class Wed 21 Oct.
- Buy `GOOG` Mon 12 Oct and buy `GOOGL` Wed 14 Oct: same direction across classes
  extends the block; first sell of either class Wed 21 Oct.

The contract does not define "the same security"; one group per issuer is the
fail-closed reading and the owner chose it.

**Unmapped symbol = fail closed.** A ticker that the order generator or the
universe wants to trade but the map cannot resolve is **not tradable** (no
order is built), and a `WARN` `symbol_unmapped` audit row is written. A history
order whose symbol is outside the map is carried under its normalised raw
symbol (§3.6) and never dropped. A *conflict* (two canonical tickers claiming
one Schwab spelling) fails the map load: every `authorize` denies until it is
fixed. Tests: every universe ticker maps to exactly one canonical ticker and
round-trips through the Schwab spelling.

### `lock_external.csv`

`$QUANT_STATE/lock_external.csv` (host `/mnt/cache/appdata/quant/`; the
container variable `QUANT_STATE` is `/state`). Owner-maintained, not part of
the lake, not Parquet.

```
account,ticker,side,placed_date,status,filled_qty
FIDELITY,MSFT,BUY,2026-10-12,FILLED,10
```

- The file **must exist**. A missing file is a fail-closed condition. An
  empty file with only the header is valid ("no external orders").
- `placed_date` is the `America/New_York` date (`YYYY-MM-DD`). The file has
  no fill-date column: for an external order that filled on a later day than
  it was placed, enter the **last fill date** here (that is the date the lock
  counts from); for an order still working, enter the placement date with a
  working status (it blocks with no end date until the owner updates the row). `side` is
  `BUY` or `SELL`. `status` uses the same vocabulary as the mapping table
  (any unknown value counts). `filled_qty` is a non-negative number. `ticker`
  is canonicalised through the symbol map.
- Any malformed row, bad header, bad date, unknown side, or a row dated in
  the future => the whole file is rejected => block (do not skip bad rows).
- The owner appends by hand. The file is read at every check. There is no
  staleness check and no reminder (owner's decision): the file only has to
  exist and parse.
- Fidelity is an external account for the lock: the owner's Fidelity trades
  in **any other ticker** count and go here with `account=FIDELITY`. The
  owner's Fidelity ICE activity is irrelevant to the system and need not be
  logged (ICE is restricted, §5).

## 4. Trading Days

Trading days come from the **NYSE holiday rules, computed for any year**
(the shared Rust crate `calendar/` and the Julia package `julia/NYSECalendar`, both checked
against `fixtures/nyse_calendar_cases.json` and the Julia oracle `scripts/nyse_calendar_oracle.jl`), **unioned with an
overrides set** for ad-hoc closures that rules cannot predict (national days of
mourning, disasters). They never come from `prices.parquet` (it cannot contain
future holidays, and a missing price row is a data gap, not a closure). The one
price-based input is the add-only price-bar backfill of `calendar-sync` (below;
constitution §1 rule 6, amendment 1.12), which can only add a closure. The
calendar rules module (rules + overrides) is **the single calendar** used by the
lock, the audit, the pending timer, the snapshot, the execution window and the
dashboard; no other component keeps its own holiday list.

- **Rules.** New Year's Day (a Saturday 1 January closes nothing; a Sunday
  moves to Monday), Martin Luther King Jr. Day (3rd Monday of January),
  Washington's Birthday (3rd Monday of February), Good Friday, Memorial Day
  (last Monday of May), Juneteenth (from 2022), Independence Day, Labor Day
  (1st Monday of September), Thanksgiving (4th Thursday of November),
  Christmas. Juneteenth, Independence Day and Christmas move to the
  preceding Friday when they fall on a Saturday and to the following Monday
  when they fall on a Sunday. The oracle reproduces the published lists
  for 2022-2027 (verify against NYSE's pages).
- **No expiry, no halt.** Because the rules apply to any year, there is no
  schedule that can run out and no review that can lapse. Upkeep of the
  overrides never halts trading.
- **Overrides.** The seed `config/nyse_calendar_overrides.toml` (historical
  ad-hoc closures: 2001-09-11 to 2001-09-14 September 11 attacks; 2004-06-11
  National Day of Mourning for President Reagan; 2007-01-02 National Day of
  Mourning for President Ford; 2012-10-29 and 2012-10-30 Hurricane Sandy;
  2018-12-05 and 2025-01-09 National Days of Mourning; each historical date to
  be verified against NYSE notices) and the runtime file
  `$QUANT_STATE/nyse_calendar_overrides.toml` are unioned with the rules. The
  seed covers the backfill (prices from 2005-01-01, macro from 2004-01-01;
  owner decision D7) and the September 2001 closures; the one-time historical
  price-bar backfill (below) cross-checks it. The oracle's `OVERRIDES_SEED`
  mirrors the seed file and `scripts/run_fixtures.jl` fails if they differ;
  the golden fixtures (2018-2040) do not cover the 2007 and 2012 dates, so
  `run_fixtures.jl` checks them separately (`add_trading_days(2012-10-26, 1)
  == 2012-10-31`). Runtime file entries carry `date`, `name`,
  `added_on`, `first_seen`, `source` (`schwab_marketdata` | `same_day_signal` |
  `price_bar_backfill` | `manual`). The file is **append-only**: no code removes a line.
- **Errors err toward closed.** Treating an extra date as a closure can only
  make `D + 5 trading days` later (property-tested in the oracle and the
  fixtures' monotonicity cases), never earlier. The reverse error, treating a
  closed day as a session, is the unsafe one, so the automation never removes a
  closure and **writes on the first sighting**.
- **Early closes are not closures.** A 13:00 close day is a session. The
  execution window needs the real close time, so Rust derives each day's session
  times **from the market-hours data** (`regularMarket.start/end`, fetched at
  the 09:00 refresh for today and cached for the next 30 days; the endpoint's
  early-close rendering is UNVERIFIED, live item). There is no advisory January
  calendar. The cancel/deadline rule is relative to the actual close:
  `cancel_at = close - 10 minutes`, `deadline = close - 30 minutes` (15:50 and
  15:30 on a normal day; 12:50 and 12:30 on a 13:00 close). If the session times
  for today cannot be read, the day is treated as a **normal** 16:00 close for
  the lock (irrelevant to it) but the execution window **fails safe**: no
  placement is started after 12:30 ET unless the close is confirmed
  (`10-execution-schwab/EXECUTION-WINDOW.md` §1.2, §9).

### `calendar-sync` (what it queries, when, and what it writes)

An Unraid User Script running `docker exec quant-core quant-exec calendar-sync`:
weekly Sunday 08:00 ET, **daily at the 09:00 pre-open refresh**, at startup, and
as the same-day check at 10:00 ET (below).

**The failure that this design closes.** An unplanned closure announced the
evening before is first seen on the closure morning. If `today` were excluded
from the query, or a second sighting were required, the closure day would pass
unrecorded; past dates are never queried (the endpoint documents "from current
date to 1 year ahead", CONFIRMED-OFFICIAL SPEC-M), so it would never be seen at
all, and a sell could be placed one trading day too early. Therefore:

- **Dates queried: `[today, today + 30 days]`**, one request per weekday that
  the rules treat as a session. **Today is included at the 09:00 refresh.** The
  documented range makes the current date valid; what the endpoint returns for
  *today* on a closure morning (`isOpen`, empty `sessionHours`) is **UNVERIFIED**
  (reference §4.1, open item 10: test it on the next closure or half day). Past
  dates are never queried and unsupported.
- **A date is "a mismatch"** when the source reports closed (`isOpen=false`, or no
  `regularMarket` session) for a weekday the rules treat as a session.
- **Write on FIRST sighting.** A mismatch is appended to the runtime overrides
  immediately (atomic temp + fsync + rename; record `added_on`, `first_seen`,
  `source`), with a `WARN` `calendar_override_added` audit row. There is no
  two-consecutive-runs rule any more: a false closure can only *lengthen* a block
  (property-tested: adding any set of closures never moves a `first_permitted`
  earlier), while a missed closure is a breach. The cost of a false positive is a
  longer block (a delayed sale), which the `WARN` makes visible to the owner.
- **Suspect source is a WARN only.** More than 3 mismatches in a single run still
  writes every one of them; it additionally raises `calendar_sync_suspect_source`
  (`WARN`) so the owner can check whether the source, not the calendar, is wrong
  (a wrongly added closure is removed only by the owner, §below). It never
  suppresses a write.
- **Same-day signal (10:00 ET, before the first placement of the day).** For a
  weekday the rules treat as a session, `quant-exec calendar-sync --same-day`
  reads REST quotes (not the stream) for three reference symbols (`SPY` and two
  index-heavy large caps, placeholder `AAPL`, `MSFT`) and treats the day as
  **closed** when, for the reference set, the `securityStatus` is `Closed` (the
  documented values are "Normal, Halted, Closed", other spellings UNVERIFIED) or
  the `quoteTime` has not advanced past 09:30 ET today by 10:00 ET. It then writes
  an override for today (`source = same_day_signal`) on that first sighting. If
  the REST quotes cannot be read the signal is *unknown*, writes nothing and
  `WARN`s (the spread gate then defers every sequence on unusable quotes anyway).
  Once the day is in the calendar as closed, `authorize` denies on it
  (`Denied::NonTradingDay`) and the generator places nothing.
- **Owner's manual-add path.** `quant-exec calendar add --date YYYY-MM-DD --name
  "..."` (a Rust CLI on the host, not the frontend, not Julia) validates the
  date, appends it to the runtime overrides atomically (`source = manual`) and
  writes an audit row (`manual_override`, `details.kind = calendar_add`, actor
  `manual`). It can only add. A line that appears in the runtime file without a
  matching audit row (hand-edited) is detected at load by comparing the file's
  entries with the audit rows, accepted (it can only lengthen a block) and
  `WARN`ed. The owner may also add a line to the seed file by commit.
- **Removal is an owner act and never automatic.** The automation never removes
  a closure. If the owner wants a wrongly added date removed, they edit the
  runtime file and write the audit row; the next compliance audit recomputes
  with the new calendar and reports any consequence.
- **Price-bar backfill (add-only; constitution §1 rule 6 as amended in 1.12;
  owner decision B3).** Closes the gap the forward query cannot: if
  `calendar-sync` was down on a closure day and nobody added it, the past date
  can no longer be queried from the market-hours endpoint. At the 09:00 refresh,
  `calendar-sync --bars` fetches **daily bars** for the broad-market reference set
  **`IWF`, `IWP`, `SPY`** from Schwab `pricehistory` (`periodType=month`,
  `frequencyType=daily`; parameters per `10-execution-schwab/SCHWAB-API-REFERENCE.md`,
  verify), falling back to the Yahoo chart API (`interval=1d`, the same client as
  `01-data-ingestion`) when Schwab is unavailable. It never reads
  `prices.parquet`. For each of the **last 10 sessions** before today that the
  current calendar (rules + overrides) treats as open:
  - **No bar from any of the three** (each request succeeded and parsed, and none
    has a bar dated that day) => the date is a missed closure: append it to the
    runtime overrides at once (`source = price_bar_backfill`, atomic write as
    above), write a `WARN` `calendar_override_added` audit row with
    `details.kind = price_bar_backfill` and the three responses' coverage, and a
    `data_quality` row (check `calendar_closure_added_from_bars`, severity
    `WARN`, the date and the reference tickers) so the owner reviews it.
  - **A bar from at least one** => open; nothing is written. A **single** (or a
    second) ticker missing its bar is a data gap, not a closure: `INFO` only.
  - **Any of the three requests failed, was truncated or did not parse** =>
    unknown for every date of that run: nothing is written, `INFO`, `WARN` after
    2 consecutive daily failures (as the other upkeep failures; never a halt).
  - It **only adds**. A bar on a date the calendar already treats as closed
    (rules or overrides) never removes the closure; it is reported as a `WARN`
    `calendar_bar_on_closed_day` for the owner (it may be a vendor artefact).
  - Today is excluded (today's daily bar exists only after the close); the
    10-session lookback covers a `calendar-sync` outage of up to two weeks.

  **One-time historical run.** After the price backfill, `calendar-sync --bars
  --from 2001-01-01` runs the same test over every rules-session weekday from
  2001 to yesterday, using the Yahoo chart API daily history of the reference
  set (Schwab's daily depth is unverified). A ticker counts in the set only from
  its first bar (`IWP` began trading in 2001, verify), and at least `SPY` must be
  present. It must **rediscover** the seed closures (2001-09-11 to 2001-09-14,
  2004-06-11, 2007-01-02, 2012-10-29 and 2012-10-30, 2018-12-05, 2025-01-09):
  each one found is logged as a confirmation of that seed `verify` item; a seed
  date on which bars exist is reported (`calendar_bar_on_closed_day`), never
  removed. Every **new** date it finds is appended as above and gets its own
  `data_quality` row. The run writes a summary to the next compliance report.

  **Trade-off.** A false closure (vendor bars missing on a day the market was
  open) only lengthens lock blocks, which is the safe direction for the
  contract. But the same calendar defines the panel's trading-day windows (the
  21/63/252-day features and the `H`-day targets of `02-panel-construction`),
  so a false closure also shifts those windows by one session. That is why
  every added date is a `data_quality` row for review, and why the trigger
  needs all three reference tickers to be missing.
- **Raw responses are logged** (date, HTTP status, body) for the first runs
  (placeholder: first 10 runs, then only mismatches), so the holiday and
  early-close behaviour can be learned.
- **Upkeep failures.** If `calendar-sync` fails (source unavailable, unparseable),
  nothing changes: `INFO`, `WARN` after 2 consecutive daily failures, an
  alert-only `CRITICAL` after 7 days. It never halts trading (a halt here would
  make a calendar outage cost more than the risk it covers, and the owner's
  manual path exists).

| Source says | Rules say | Action |
|---|---|---|
| closed (any date in `[today, today + 30 days]`, first sighting) | open | Append to the runtime overrides at once; `WARN calendar_override_added` |
| closed by the same-day signal | open | Append an override for today; `WARN`; no placements today |
| closed, more than 3 mismatches in this run | open | Same as above for every date, plus `WARN calendar_sync_suspect_source` |
| no daily bar for any of `IWF`, `IWP`, `SPY` on a past session (last 10, or the one-time historical run) | open | Append (`source = price_bar_backfill`); `WARN calendar_override_added`; `data_quality` row `calendar_closure_added_from_bars` |
| daily bar for one or two of the three only | open | Nothing written (data gap); `INFO` |
| daily bar exists | closed | Keep it closed (add-only); `WARN calendar_bar_on_closed_day` |
| open | closed | Keep it closed (conservative); `WARN` naming the date so the owner can check the rule (the source's behaviour on closures is UNVERIFIED, so this may be the source) |
| agrees | agrees | Nothing |
| unavailable or unparseable | any | No change; `INFO`; `WARN` after 2 consecutive daily failures |

A closure declared after orders were placed against it is a compliance
question: the nightly audit recomputes with the updated calendar and reports it;
the owner decides.

- Count **trading days**, never weekdays: a weekday count is never later and
  can be earlier, which is a breach (buy Mon 23 Nov 2026 -> weekday count
  gives Mon 30 Nov, trading-day count gives Tue 1 Dec).
- `is_trading_day(d)` = weekday and not a holiday. `add_trading_days(d, n)`
  matches the oracle: roll `d` forward to a session, then advance `n`
  sessions.
- Half days are sessions.

## 5. Restricted Tickers

Constitution §6 / amendment 1.5: the system never places an order, either
side, for a ticker on `config/restricted.toml` (initial entry: `ICE`).

`lock::authorize` **rejects restricted tickers first**, before the halt flag
and the order history are consulted, with `Denied::Restricted` and a CRITICAL
audit row `restricted_security_blocked`. Tickers in `restricted.toml` and the
ticker being ordered are both canonicalised through the symbol map (§3.8), so a
spelling cannot slip through; an unmapped symbol is not tradable at all. Enforcement is layered:

| Layer | Where | Behaviour |
|---|---|---|
| Portfolio eligibility filter | `08-portfolio-optimization` (Julia; owned by that skill) | Restricted ticker gets weight 0. **Contract:** read `config/restricted.toml`, drop every ticker listed with `rule = "never_trade"` from the tradable set before optimization; **error** if the `restricted` key is absent or the list is empty, and assert that `ICE` is present with `rule = "never_trade"`. The data/model **panel is not changed**: exclusion is at the tradable layer, so cross-sectional ranks are unaffected |
| Rust pre-trade check | `execution/src/risk/pre_trade.rs` and `lock::authorize` | Reject, CRITICAL `restricted_security_blocked`. A parse failure of `restricted.toml` fails closed: **block all orders** |
| Test | `execution/tests/restricted.rs` | `restricted.toml` contains `ICE` with `rule = "never_trade"` (deleting the entry fails CI) |
| Daily audit | §8 | Any order (including a zero-fill, rejected or cancelled one), fill or position in a restricted ticker in a Schwab account (or the audit journal) halts trading and writes CRITICAL |

**The Schwab account never holds ICE.** The owner's Fidelity account is the
only account that owns ICE. Therefore:

1. The system never orders ICE, and ICE is never expected in any Schwab
   account.
2. If reconciliation or the daily audit finds an ICE position, or any ICE
   order or fill, in a Schwab account, it is an **anomaly**: halt trading
   (kind `RESTRICTED`, owner-only clear), write a CRITICAL audit row, and **do
   not auto-sell**. Any action is the owner's, subject to the owner's blackout
   rules. A restricted name found in the account is **never a forced-exit
   trigger**: nothing in the portfolio, the generator or the pending queue sells
   it.
3. Fidelity ICE activity is irrelevant to the system and need not be logged
   in `lock_external.csv`. (Fidelity trades in other tickers still count, §3.)
4. NAV, tier and reconciliation need no special ICE handling: ICE is simply
   absent.

## 6. Enforcement Points

**Layer 0: the halt flag (§8b).** `authorize`, every `submit_*`, the pending
timer and the ladder each check `$QUANT_STATE/TRADING_HALTED`. A present,
unreadable or unparseable flag halts everything.

**Layer 1: permit gate (compile-time presence, run-time truth).**
Order submission takes a `BuyPermit` or `SellPermit` (private fields, no
`Clone`/`Copy`, ticker and date bound, and holding the ticker's gate guard, §6a).
Only `lock::authorize_and_reserve` issues one. `authorize` recomputes from the
history. The compiler guarantees that no order reaches the broker without a
permit; the correctness of `authorize` is covered by the fixtures and the
property tests (§9), not by the type system.

**Layer 2: the per-ticker gate and the pre-submit order-history read.** The
permit is issued against an `OrderHistory` read over REST in the same critical
section (§3.2, §6a). There is no cached lock row to cross-check.

**Layer 3: daily post-close compliance audit** (§8). Rescans all order history
for violations of rules 1-4 and for restricted-ticker activity. It catches what
the permit gate cannot see: orders placed outside the system. Position
reconciliation also runs at close (`10-execution-schwab` §6c).

### 6a. The per-ticker gate (TOCTOU)

Between "the history says this side is free" and "the order is on Schwab's book"
another path (the ladder's replacement, the pending timer, the generator, a
second rebalance) could place an opposite-side order for the same ticker. The
check-then-send must therefore be **atomic per ticker**:

- `OrderGate` holds one `tokio::sync::Mutex<()>` per canonical ticker (created on
  first use, in a `DashMap`). **Every** path that can place an order, the
  generator, each ladder step (post, reprice, capped cross, each cancel/replace),
  the pending-order timer and any retry, enters through the gate; there is no
  other entry point to `submit_*`.
- The critical section for one submission is, in order: acquire the ticker's
  guard -> halt check -> `ready_for_submission` (the REST read of §3.2, `N = 0`)
  -> `authorize` -> **intent append (durable, `append_durable`)** -> `send` ->
  outcome append -> release the guard. The guard is held across the send and the
  outcome so that no second path can read the history in between.
- **The permit cannot outlive the critical section or move before the intent.**
  `authorize_and_reserve(...)` takes the guard, performs the checks and **writes
  the intent at permit issue**, then returns the permit, which *owns* the
  `OwnedMutexGuard`. The permit is neither `Clone` nor `Copy`; it is consumed by
  `submit_*`, which drops the guard only after the outcome row. A permit
  dropped without a send writes the outcome `not_sent` (via an explicit
  `release_unsent().await`, since `Drop` cannot await; a panicking path is
  covered by the unresolved-intent scan, which blocks trading until resolved).
  Because the intent is already in the journal, even a path that ignored the
  guard would be seen by the next authorize as an open order of that side.
- Different tickers do not contend. The guard is a correctness device, not a
  throughput limit: the REST read, not the lock, dominates its duration.
- What the gate cannot cover: orders placed by the owner in the Schwab app or in
  another account during the section. That is the daily audit's job (§8) and the
  reason the lock reads Schwab's history rather than the system's own.

## 7. Permit Enforcement (Rust)

What is and is not guaranteed:

- **Compile time:** no `submit_buy` / `submit_sell` without a
  `BuyPermit` / `SellPermit`; a permit cannot be built outside the `lock`
  module; a `BuyPermit` cannot be passed where a `SellPermit` is required.
- **Run time, tested:** `authorize` allows exactly when the fixtures say the
  order is allowed (§9) and no other deny applies.
- **Not guaranteed by types:** that the history is complete. That rests on
  fail-closed loading (§3), the daily audit (§8) and the owner keeping
  `lock_external.csv` current. The last one is a human control and is the
  weakest link: say so in the compliance report.

```rust
// SKETCH, not compiled, not normative. Crate home: execution/src/locks/*.rs (scaffold.md).
// execution/src/locks/order_history.rs
pub enum BlockEnd { Open, FirstPermitted(NaiveDate) }   // Open = may still fill: no end date

pub struct HistoricalOrder {
    pub account: AccountId,          // hash or label, never a raw account number
    pub ticker: Ticker,              // canonical (symbols.rs)
    pub side: Side,
    pub placed_et: NaiveDate,        // America/New_York date of enteredTime
    pub last_fill_et: Option<NaiveDate>, // ET date of the last fill (REST executionLegs[].time), None if no fill
    pub status: OrderStatus,         // 21 values + Unknown(String); unknown => open
    pub filled_qty: Decimal,
    pub source: HistorySource,       // Schwab | Journal | External | Transactions
    pub blocks_both: bool,           // unknown instruction/asset type, undeterminable side
}

impl HistoricalOrder {
    /// Mirrors the oracle's `counted`: open (incl. unknown) or any fill.
    pub fn counted(&self) -> bool {
        self.status.is_open_or_unknown() || self.filled_qty > Decimal::ZERO
    }

    /// Mirrors the oracle's `block_end`. Open (incl. unknown status) blocks with no end date;
    /// otherwise first permitted day = D + 5 trading days, D = last fill date (placement date
    /// if there was no fill). REPLACED is treated exactly like CANCELED.
    pub fn block_end(&self, cal: &TradingCalendar) -> BlockEnd {
        if self.status.is_open_or_unknown() { return BlockEnd::Open; }
        let d = self.last_fill_et.unwrap_or(self.placed_et);
        BlockEnd::FirstPermitted(cal.add_trading_days(d, 5))   // calendar rules never run out
    }
}

/// Private fields; built only by the loader. `Incomplete` makes authorize deny.
pub struct OrderHistory { /* orders, per-account rest_ok_at, source health, covered_from */ }

impl OrderHistory {
    /// Full REST read: accountNumbers, then per account the 45-day window (all statuses) and the
    /// 14-day TRADE transactions; plus journal (merged as in section 3.7) and the external file.
    /// Any failure => Err (never a partial history).
    pub async fn rest_refresh(&mut self, cal: &TradingCalendar) -> Result<(), RefreshError> { /* ... */ }

    /// The stream only marks dirty / raises alerts; it never changes a decision.
    pub fn mark_dirty(&mut self, why: DirtyReason) { /* sets dirty; never clears itself */ }

    /// Called inside the ticker gate, immediately before `authorize`. ANCHOR_MAX_AGE is zero:
    /// the history is always re-read over REST first (owner decision, section 3.2).
    pub async fn ready_for_submission(&mut self, now: DateTime<Utc>, cal: &TradingCalendar)
        -> Result<&OrderHistory, Incomplete>
    {
        self.rest_refresh(cal).await.map_err(Incomplete::from)?;   // never papered over
        self.is_complete_for(now.with_timezone(&New_York).date_naive())?;
        Ok(self)
    }

    pub fn is_complete_for(&self, today: NaiveDate) -> Result<(), Incomplete> { /* all sources ok,
        every account read this call, no truncated window (rows < maxResults), no unattributable
        order, covered_from <= today - 21 days. An unknown enum value with a parseable symbol does
        NOT make the history incomplete (section 3.6); it only adds an open / both-sided order */ }
}

// execution/src/locks/authorize.rs  (module `lock`)
pub struct BuyPermit  { ticker: Ticker, as_of: NaiveDate, guard: OwnedMutexGuard<()> }  // private fields
pub struct SellPermit { ticker: Ticker, as_of: NaiveDate, guard: OwnedMutexGuard<()> }  // no Clone / Copy
pub enum Permit { Buy(BuyPermit), Sell(SellPermit) }

#[derive(Debug)]
pub enum Denied {
    Halted(HaltKind),                    // TRADING_HALTED present, unreadable or unparseable (section 8b)
    Restricted,                          // ticker on config/restricted.toml (checked first)
    Unmapped,                            // symbol not in the canonical map: not tradable
    NonTradingDay,                       // today is not a session in the calendar (incl. an override)
    NothingToSell,                       // long-only: no shares held (not a lock event)
    BlockedUntil(NaiveDate),             // first permitted day
    BlockedOpenOrder(Option<NaiveDate>), // an opposite-side order is still open: no end date (latest other end, if any)
    HistoryIncomplete(Incomplete),       // fail closed
}

/// The single runtime lock check. Pure over its arguments: no cached lock row, no clock reads,
/// no I/O. (`halt` is read by the caller in the same critical section and passed in.)
pub fn authorize(
    ticker: &Ticker,
    side: Side,
    today: NaiveDate,                    // ET session date
    history: &OrderHistory,
    restricted: &RestrictedList,         // Err/unparsed list => deny all
    halt: &HaltState,                    // unreadable/unparsed => Halted
    shares_held: Decimal,                // from the broker, same critical section
    cal: &TradingCalendar,
) -> Result<AuthorizedOrder, Denied> {
    if restricted.contains(ticker) { return Err(Denied::Restricted); }   // 1. first
    if let Some(k) = halt.active() { return Err(Denied::Halted(k)); }   // 2. halt flag
    if !cal.is_trading_day(today) { return Err(Denied::NonTradingDay); }
    history.is_complete_for(today).map_err(Denied::HistoryIncomplete)?;   // 3. fail closed
    if side == Side::Sell && shares_held <= Decimal::ZERO { return Err(Denied::NothingToSell); }
    let opposite = side.opposite();
    let mut until: Option<NaiveDate> = None;
    let mut open_block = false;
    for o in history.orders_for(ticker).filter(|o| o.counted()) {
        let blocks = if o.blocks_both { true } else { o.side == opposite };
        if !blocks { continue; }
        match o.block_end(cal) {
            BlockEnd::Open => open_block = true,                         // open: no end date
            BlockEnd::FirstPermitted(fp) if today < fp => until = Some(until.map_or(fp, |u| u.max(fp))),
            BlockEnd::FirstPermitted(_) => {}
        }
    }
    if open_block { return Err(Denied::BlockedOpenOrder(until)); }
    if let Some(d) = until { return Err(Denied::BlockedUntil(d)); }
    Ok(AuthorizedOrder { ticker: ticker.clone(), side, as_of: today })   // permit minted by authorize_and_reserve
}

/// The only issuer of permits. Called by `OrderGate` while holding the ticker's guard.
/// Order of effects: halt check -> REST read (ready_for_submission) -> authorize -> intent
/// append (durable) -> return the permit, which owns the guard. A denial releases the guard and
/// writes `lock_violation_blocked` (WARN) or `restricted_security_blocked` (CRITICAL).
pub async fn authorize_and_reserve(/* gate, ticker, side, order spec, ... */) -> Result<Permit, Denied> { /* ... */ }

// execution/src/schwab/orders.rs: the only submission entry points.
pub async fn submit_buy(order: BuyOrder, permit: BuyPermit) -> Result<OrderAck, SubmitError> {
    halt::assert_not_halted()?;                    // defence in depth: re-read the flag
    check_permit(&order.ticker, permit.ticker(), permit.as_of())?; // same ticker, as_of == today
    /* tier permit (10 §2a), risk checks (incl. restricted), then send; outcome append; drop guard */
}
pub async fn submit_sell(order: SellOrder, permit: SellPermit) -> Result<OrderAck, SubmitError> { /* same */ }
```

Notes:

- `authorize` compares `today` itself, not a rolled-forward date, and **denies
  on a non-trading day** (`NonTradingDay`). The system does not place orders on
  non-trading days.
- Two orders in one rebalance see each other: the first order's intent is in the
  journal before the second `authorize` reads it (and Schwab's list usually has
  it too).
- The same pattern as `MarketPermit` in `10-execution-schwab` §2a: the permit
  is a witness that the runtime check ran.
- A grep test asserts `authorize`, `authorize_and_reserve` and `submit_*` have no
  `force`, `override`, `bypass` or `skip` parameter.

## 8. The Daily Compliance Audit and Report

Runs every trading day **after the close, at the 16:40 ET slot** (after
reconciliation 16:15, lots 16:20, trade summary 16:22, ledger 16:25 and inference
16:30; before the 16:50 lock snapshot, the 17:00 optimization and the 18:00 audit
export; the slots are in `16-portfolio-ledger/LEDGER-PROTOCOL.md` §2 and in the
`outline.md` daily schedule). Sketch:
`10-execution-schwab/scripts/audit-lock-compliance.rs` (not compiled).

1. Load the full history (45-day entered-time window per account, never above 60;
   plus the `transactions` TRADE cross-check over the same 45 days, §3.1) from all
   sources with the same loader as `authorize`. **A failed read does not halt at
   once** (§8c): the audit retries for a bounded period first.
2. For every ticker, for every pair of **counted** orders `(a, b)` with opposite
   sides (or a `blocks_both` order and any other order): a violation exists if `b`
   was placed on or after `a`'s placement day and before `block_end(a)`
   (`BlockEnd::Open` while `a` is open at the time of the audit, else last fill
   date + 5 trading days), and symmetrically with the roles swapped. An
   opposite-side order placed while `a` was still open, or after `a`'s fill but
   inside the 4 trading days after it, is a violation. **An open `a` is not
   skipped**: it blocks every later placement (the earlier sketch `continue`d on
   it and compared the wrong types). The audit uses each order's dates (placement
   day, last fill day), whatever their origin, including orders placed outside the
   system (manual trades in the Schwab app or in an external account).
2a. **Orders that ended with zero fills are not locked (decided, owner,
   2026-10-10, DECISIONS B2).** An order cancelled, rejected, expired or
   replaced with zero fills is not counted (§1 rule 1), and the audit evaluates
   `counted` at audit time. It therefore does **not** flag an opposite-side order
   `b` placed while such an order `a` was open, nor a `b` that itself ended with
   zero fills: neither is a violation, and neither is a `WARN` anomaly (the
   earlier `open_overlap_zero_fill` WARN is withdrawn). The live `authorize`
   still denies an opposite order while `a` is open, because an open order is
   counted at that moment; this case can only arise from an order placed outside
   the system. The report carries one `INFO` count, `zero_fill_overlaps`
   (number of such pairs, computed from `enteredTime`/`closeTime`), for the
   owner's information only: no audit row, no event, no halt.
3. Any order, fill or position in a restricted ticker in a Schwab account or in
   the journal => violation. This includes **zero-fill, cancelled and rejected**
   orders: *any ICE order*, whatever its status, is flagged, because an order
   attempt is the breach. Only the `External` source is excluded (Fidelity ICE
   activity is irrelevant). Schwab account activity in ICE is an anomaly: halt,
   CRITICAL, no auto-sell.
4. Any order with `SELL_SHORT` / `BUY_TO_COVER`, an unknown enum value, a working
   order older than one session, a fill whose `orderId` is absent from the
   history, or an unknown account => `WARN` (anomaly; `CRITICAL` if it falls
   inside a block).
5. **A violation halts trading.** It writes `TRADING_HALTED` kind `VIOLATION`
   (§8b) and one `lock_violation_detected` row per violation, severity `CRITICAL`,
   with the two orders in `details`. Clearing the halt is the owner's act (§8b);
   the violation is recorded by its identity (§8b) so that the next audit does
   not re-halt on the same pair while it remains in the 45-day window.
6. **Could not verify is not a violation.** If the history cannot be read (REST
   or token outage, truncation), the outcome is `INCOMPLETE`: the audit writes
   `lock_audit_incomplete` (`WARN` at the first failure, `CRITICAL` when the
   retry window ends), **never** `lock_violation_detected`, and sets a halt of
   kind `INCOMPLETE` (§8c). A clean automatic re-run clears an `INCOMPLETE`-only
   halt; it never clears a `VIOLATION` halt.
7. **Report.** A dated report is always produced, clean or not:
   `$QUANT_STATE/compliance/lock-compliance-YYYY-MM-DD.json` plus a short `.md`.
   Contents: audit date and time, `OrderHistory` as-of per source, accounts
   covered (by label), lookback window, number of orders scanned and counted per
   source, the calendar rules version (git hash of `calendar.rs`), the overrides
   hash and entries and the last successful `calendar-sync`, the SHA-256 of
   `lock_external.csv`, the restricted list and its hash, the symbol map hash,
   every violation and anomaly, the `INFO` count `zero_fill_overlaps` (step 2a),
   the closures added by the price-bar backfill since the last report (§4),
   acknowledged violations, the outcome
   (`COMPLIANT` / `VIOLATION` / `INCOMPLETE`), and a statement of the known limit
   (external accounts depend on the owner's file; pre-window orders are covered
   only by the `transactions` net). Reports are never edited; a re-run writes
   `.../lock-compliance-YYYY-MM-DD.rerun-N`. The reports are backed up with the
   rest of the state directory (the evidence the owner could show compliance).
8. The report and the raw order snapshot (`order_history.parquet`,
   `13-parquet-lake` #25, append-only) are written before the audit row that
   says the audit completed. An audit that cannot write its report is an
   `INCOMPLETE` audit and halts trading (kind `INCOMPLETE`, after the retry).

## 8a. How Long a Resting Order Blocks, and the Lock Snapshot

**Resting orders.** An open order blocks the opposite side, with no end date,
until it reaches a terminal state (filled, cancelled, rejected, expired). With
the system's `DAY` orders that is at the latest the close of the session in
which it was placed (the execution window also cancels working orders at
`close - 10 minutes`, 15:50 ET on a normal day; see
`10-execution-schwab/EXECUTION-WINDOW.md`, read-only for this skill). After
that it blocks only if it filled, and then through 4 trading days after its
last fill date. An order still working after the session it was placed in is an
anomaly (audit `WARN`): a manual GTC order, or an unseen terminal state.

**The snapshot the optimizer reads (one contract).** Julia never computes the
lock and never reads order history. Rust writes a small derived file,
**advisory only**, so the optimizer does not size trades the lock will refuse:

- **Path: `$QUANT_STATE/lock_snapshot.json`** (container `/state/lock_snapshot.json`;
  host `/mnt/cache/appdata/quant/lock_snapshot.json`), JSON, not in the lake, not
  under the schema registry. Julia reads it through `QUANT_STATE` (the
  `QUANT_ROOT` / `QUANT_STATE` / `QUANT_CONFIG` convention of `13-parquet-lake`).
  There is no Parquet or other form of this file; the optimizer script reads this
  JSON and nothing else.
- **Written by Rust at about 16:50 ET** (after the 16:40 audit), atomically (temp,
  fsync, rename), evaluated for the **next** session so that Julia's 17:00
  optimization reads a snapshot for the session the orders will be placed in. It
  may be rewritten at 09:00 ET for the day's session (dashboard and tools; Julia's
  planning does not read that rewrite).
- Fields (`blocked_side`, `first_permitted`, `complete`, `for_session`,
  `generated_at` are the contract; others are informational):

```json
{
  "generated_at": "2026-10-12T20:50:03Z",
  "for_session": "2026-10-13",
  "history_as_of": "2026-10-12T20:50:01Z",
  "complete": true,
  "calendar_overrides_sha256": "...",
  "tickers": [
    {"ticker": "AAPL", "blocked_side": "SELL", "first_permitted": "2026-10-19", "open_no_end_date": false},
    {"ticker": "MSFT", "blocked_side": "BUY",  "first_permitted": null,         "open_no_end_date": true},
    {"ticker": "NVDA", "blocked_side": "BOTH", "first_permitted": null,         "open_no_end_date": true}
  ]
}
```

  `blocked_side` is the side that **cannot be traded** on `for_session`: `SELL`
  (a counted buy blocks sells; the optimizer's `no_sell`), `BUY` (a counted sell
  blocks buys; `no_buy`) or `BOTH` (counted orders of both sides, or a
  `blocks_both` order). `first_permitted` is the `max` block end of the blocking
  orders, or null with `open_no_end_date: true` while any blocking order is open;
  for `BOTH` it is the later of the two sides' ends (null if either is open).
  A ticker absent from a `complete: true` snapshot is not blocked. It lists every
  ticker with a counted order in the lookback that is still blocking on
  `for_session`. A block whose `first_permitted <= for_session` is not listed.
- **Julia fails closed**: a missing, unreadable, unparseable, `complete: false`,
  or stale snapshot (`for_session` is not the session being planned, or
  `generated_at` older than 26 hours) means **blocked both ways for every
  ticker**: no new orders are planned, and the planner logs why. The planned
  session is given to Julia by the scheduler (`--for-session`, from
  `quant-exec next-session`), so Julia needs no calendar of its own. Julia may
  size a target for a blocked ticker (the order generator queues it); it must
  not assume it can trade it.
- **Rust re-checks at submission regardless** (`authorize`, §7). The snapshot
  can never grant a permit; it is not an input to `authorize`.

## 8b. The Halt Flag (`TRADING_HALTED`)

The audit, the reconciliation, the audit writer and the token layer set it; this
section is the one place that says who reads it and who clears it.

- **File:** `$QUANT_STATE/TRADING_HALTED`, JSON, written only by the Rust `halt`
  module (flock + temp + rename): `{"halts": [{"id", "kind", "reason",
  "set_at", "set_by", "details"}]}`. **Absent = not halted.** Present but
  unreadable, unparseable or with an unknown shape = **halted** (kind `UNKNOWN`,
  owner-only clear).
- **Kinds.** `VIOLATION` (audit found a breach), `RESTRICTED` (restricted
  activity or position), `RECONCILIATION` (a position mismatch that survived the
  tolerance and retry, §8c / `10-execution-schwab` §6c), `AUDIT_WRITE` (the audit
  journal is unwritable), `UNRESOLVED_INTENT` (startup found an intent with no
  outcome), `KILL_SWITCH` (the owner's kill switch, `10-execution-schwab` §6d),
  `INCOMPLETE` (could not verify: REST / token outage; §8c), `AUTH_EXPIRED`
  (refresh token expired), `UNKNOWN`.
- **Who reads it (every enforcement point):** `lock::authorize`
  (`Denied::Halted`), every `submit_buy` / `submit_sell` (re-read, defence in
  depth), the pending-order timer (a halted day leaves the row `Pending`, sends
  nothing, alerts), and the order ladder (no new placement, replacement or cross
  while halted; working orders are cancelled best-effort at the halt, because a
  cancel is not a placement and cannot create a block, and any that cannot be
  cancelled stay open and are counted). The order generator checks it before
  planning; the dashboard displays it.
- **Who clears it, and how.** Only these rules clear a halt:
  - **Owner action:** `quant-exec halt clear --id <id> --reason "<text>"`, a Rust
    CLI on the host (never the frontend, never Julia). It writes an audit row
    `halt_cleared` (actor `manual`) and `manual_override` (`details.kind =
    halt_clear`). For a `VIOLATION` halt it additionally requires
    `--ack <violation_id>...` for every open violation: the ids go into
    `$QUANT_STATE/acknowledged_violations.json` (append-only), so the next audit
    reports them as `ACKNOWLEDGED` and does not re-halt on the same pair. The
    violation identity is `sha256(ticker | a.source | a.account | a.order_id |
    b.source | b.account | b.order_id | b.placed_et)`. Clearing does **not** bypass
    the lock: it only restarts a system whose owner has acted on the breach; a
    sell inside a block is still denied by `authorize`.
  - **Automatic clear, `INCOMPLETE` and `AUTH_EXPIRED` only:** a clean automatic
    re-run of the compliance audit and reconciliation (outcome `COMPLIANT`, all
    sources read, token valid) clears a halt whose only entries are of those two
    kinds, with an audit row `halt_cleared` (actor `system`, `details.auto =
    true`). **Never** a `VIOLATION`, `RESTRICTED`, `RECONCILIATION`,
    `AUDIT_WRITE`, `UNRESOLVED_INTENT`, `KILL_SWITCH` or `UNKNOWN` halt. If any
    non-auto-clearable entry is present, nothing is cleared automatically.
- Setting a halt writes `trading_halted` (`CRITICAL`, or `WARN` for
  `INCOMPLETE` until the retry window ends) *and then* the file; if the audit
  journal is unwritable the file is written anyway (the halt does not depend on
  the audit).

## 8c. Retry Before Halting, and "Could Not Verify"

A transient outage must not be mistaken for a breach, and must not freeze trading
for longer than it has to.

- **Audit.** A failed history read is retried with backoff for a bounded period
  (placeholder: attempts at +0, +5, +15, +30 and +60 minutes after 16:40, i.e. to
  17:40). During the retries the 16:50 snapshot is written `complete: false` (so
  Julia holds). When the window ends with the history still unreadable the audit
  outcome is `INCOMPLETE`: audit row `lock_audit_incomplete` (`CRITICAL`), halt
  kind `INCOMPLETE`. The hourly re-run continues until a clean pass or the 09:00
  pre-open refresh, which also re-runs it; a clean pass clears the halt
  automatically (§8b). A halt that is still set at the window start (10:00 ET)
  simply means no placements that day.
- **Reconciliation** (`10-execution-schwab` §6c): positions are compared with a
  tolerance; a mismatch that is explained by an order with a fill not yet
  reflected (a fractional fill can appear late, COMMUNITY) is retried at +5, +15,
  +30 and +60 minutes before it halts as `RECONCILIATION`. A REST or token outage
  during reconciliation is `INCOMPLETE`, not a mismatch.
- **Refresh-token expiry.** The refresh token is reported to live about 7 days
  (COMMUNITY, `OAUTH.md`); the system stores the time the owner authorized it and
  writes `broker_auth_expiring` (`WARN`) at 48 h, 24 h and 6 h before the
  assumed expiry, and the dashboard shows it. At expiry (`BrokerAuthExpired`)
  the halt kind is `AUTH_EXPIRED` (auto-clearing once the owner has authorized and
  a clean audit passes). The owner is notified on the dashboard only (owner
  decision B4, 2026-10-10; no email or push channel).

## 9. Tests (`execution/tests/lock_rule.rs`, `lock_fixtures.rs`, `restricted.rs`, `compliance_audit.rs`)

**The fixtures are the shared contract.** `execution/tests/lock_fixtures.rs`
reads the same two files as the Julia runner, through the path
`concat!(env!("CARGO_MANIFEST_DIR"), "/../.claude/skills/09-lock-state-machine/fixtures/…")`
(no copy, so the two languages cannot drift): for every case in
`lock_cases.json` (`named` and `random`) it builds the orders and asserts
`blocked(orders, side, today) == expected_blocked`; for `nyse_calendar_cases.json`
it asserts the computed holidays of each year (rules + the seed overrides) equal
the listed ones and every `add_trading_days(start, n)` equals `expected`. A
mismatch in either language fails CI. The fixtures are frozen goldens; a new case
is appended to `named` with its expected value computed by the Julia oracle **and
checked by hand**, in the same commit as the code change that needs it. The Julia
side runs `julia --project scripts/run_fixtures.jl`.

`execution/tests/lock_rule.rs` adds table-driven and property tests of the full
`authorize` (halt, restricted, history, merge, symbol and gate behaviour) whose
core verdict must agree with the oracle.

| Group | Cases |
|---|---|
| Fixtures | All 19 named + 3000 random lock cases and all calendar cases reproduce exactly (Rust and Julia) |
| Worked examples (§11) | All of them as a table: `(history, side, today) -> allowed / first_permitted` |
| Property tests vs oracle | Random histories (2-4 accounts, 0-20 orders, random sides, statuses, fills, dates over a year including holidays) and random `(side, today)`: Rust `authorize` allows iff the oracle `blocked` is false and no other deny applies. Also: adding a counted same-direction order never shortens a block; adding an uncounted order never changes the result; order of input does not matter; adding any closure to the calendar never moves a `first_permitted` earlier |
| Trading days | Weekend, each 2026 holiday, Thanksgiving week, Christmas week, year-end into January (rules compute it), every published 2022-2027 closure, the 2018 and 2025 override seeds, a far-future year (2040), weekday-count < trading-day-count cases, an unplanned closure added by `calendar-sync` (examples 19) |
| Late fill | GTC placed Mon filled Thu: first sell next Thu; still working: blocked with no end date; partials on Mon and Wed then cancel: next Wed; fill time missing: fallback to later of entered/close date, none parseable: `Incomplete`; fill timezone/DST edge; property tests include `last_fill != placed` and open-with-partial-fill orders |
| Recorded fixtures | The redacted real payloads cited in the reference parse into `Order` / `AcctActivityMsg`; every one of the 21 statuses maps as in 3.1 (incl. `REPLACED` zero-fill not counted, `REPLACED` with a fill counted); an **unknown enum value** with a parseable symbol is counted for that ticker only, history stays complete (3.6); an unparseable symbol makes it incomplete; an unknown extra field is logged, not fatal |
| Stream / REST | An unknown message type, a `seq` gap, a disconnect, a missed heartbeat or a parse failure marks the history dirty; a stream-reported fill that REST does not show: REST re-read, disagreement WARN; the stream's `ExecutionTimeStamp` never sets `D` |
| Coalescing | Concurrent `authorize` calls produce at most one in-flight REST read per account per epoch; a call that starts after a fetch began does not reuse it (it waits for the next); a resolved fetch or cache entry is never used by a later `authorize`; the halt flag, positions and verdicts are never cached; a restricted-list file change invalidates its cache |
| Freshness (N = 0) | **Every** `authorize` is preceded by a REST read of all accounts' orders and the 14-day transactions (a counting mock asserts it, even immediately after a previous authorize); REST down (error, timeout, 429, truncated, unparsable) and the stream looks healthy: **deny**; one account's REST failing denies orders for all tickers (history incomplete) |
| Transactions net | A TRADE fill whose `orderId` is absent from the order window adds a counted order for its ticker and WARNs; a fill dated later than the known last fill raises `D`; a side that cannot be read blocks both sides; the transactions read failing denies |
| Journal merge | Journal row with an id Schwab lists is dropped (Schwab status wins); with an id Schwab does not list: counted open, INFO inside the grace, WARN after; intent with no id adopts exactly one matching Schwab order; two matches adopt both; `rejected_by_broker` / `not_sent` not counted; `send_unknown` counted; `journal resolve` writes an audit row |
| Per-account | Prefill reads every account from `accountNumbers` (not only the aggregate `/orders`); an account hash that disappears or a new account appears mid-session marks the history dirty; aggregate vs per-account mismatch: WARN and dirty |
| Window | Window of 45 days is used (never more than 60); a response with `maxResults` rows is `Incomplete`; the status-filtered open queries run at startup/09:00/dirty only |
| Share-class lock group (B1) | Buy `GOOGL` filled Mon 12 Oct 2026: `authorize(GOOG, SELL)` denied Mon 12-Fri 16 Oct with `first_permitted` Mon 19 Oct, allowed Mon 19; sell `BRK.B` Wed 14 Oct blocks a buy of `BRK.A` through Tue 20 Oct; buys of `GOOG` Mon and `GOOGL` Wed extend the group block to Wed 21 Oct; an open `GOOGL` order blocks a `GOOG` opposite order with no end date; two classes with one `security_master` issuer key but different `lock_group` in the map fail the map load (every `authorize` denies); a single-class issuer is unaffected |
| Symbols | Every universe ticker maps once and round-trips; `BRK.B` / `BRK/B` / `BRKB`, a renamed ticker and a share-class pair block each other as the map says; an unmapped universe ticker is not tradable and audited; a map conflict makes every `authorize` deny; `ICE` is caught through every spelling |
| calendar-sync | Dates queried are `[today, today + 30 days]` and **include today**; a first sighting writes the override at once (no second run needed); a closure first seen on the closure morning is in the calendar before the day's first order; more than 3 mismatches still write every date and WARN `calendar_sync_suspect_source`; early-close days are never written; the overrides are never removed by code; the same-day signal (status `Closed` or no quote advance by 10:00) writes today and `authorize` then denies `NonTradingDay`; an unreadable quote makes the signal unknown (no write, WARN); `calendar add` appends and audits; a hand-edited line is accepted and WARNed; raw responses are logged for the first runs; calendar-sync failures never halt trading |
| Price-bar backfill (B3) | **Add-only:** a session with no bar from any of `IWF`, `IWP`, `SPY` in the last 10 sessions is appended with `source = price_bar_backfill`, a `WARN` audit row and a `data_quality` row; **never removes:** a bar on a seed or rules closure leaves the closure in place (property: the overrides after a run are a superset of those before) and WARNs `calendar_bar_on_closed_day`; **a single missing ticker does not trigger it** (nor two): `INFO` only; a failed or unparseable request for any reference ticker writes nothing; today is never tested; `prices.parquet` is never opened (grep test); the historical run over a recorded 2001-2025 bar fixture rediscovers every seed date (2001-09-11..14, 2004-06-11, 2007-01-02, 2012-10-29..30, 2018-12-05, 2025-01-09) and adds nothing else; before `IWP`'s first bar the set is `IWF` and `SPY` only |
| Counting | CANCELED/REJECTED/EXPIRED/REPLACED with zero fills not counted; same with `filled_qty > 0` counted; WORKING, QUEUED, PENDING_CANCEL counted; unknown status counted |
| Fail closed | Schwab fetch error, timeout, 401/403/429/5xx, truncated page, empty body for an account that had orders, missing account, journal unreadable, external file missing, bad header, bad date, unknown side, future date, lookback shorter than 21 days, `restricted.toml` unparseable, halt flag unparseable: `authorize` denies **both** sides |
| Halt | `TRADING_HALTED` present: `authorize` -> `Halted`; `submit_*` refuses even with a permit; the pending timer sends nothing and the row stays `Pending`; the ladder stops placing and replacing; a halted state with an unreadable file still halts; owner clear writes the audit rows; auto-clear works for an `INCOMPLETE`-only halt after a clean re-run and never for `VIOLATION`; an acknowledged violation is reported but does not re-halt |
| Gate / TOCTOU | Two concurrent opposite-side submissions for one ticker: exactly one gets a permit; the second sees the first's intent; a permit cannot be obtained without the guard; a dropped permit writes `not_sent`; ladder replacements, the timer and the generator all take the same gate; different tickers do not block each other |
| Multi-account | Buy in account A blocks sell in account B and in the external file; external buy blocks a Schwab sell; a sell in one account blocks a buy in another; same-direction in two accounts extends |
| Timezone | 22:00 ET on Mon (UTC is Tuesday) is `D` = Monday; 00:30 ET is that ET day; 23:30 ET Friday (UTC Saturday); placement on Saturday/Sunday rolls to Monday; placement on a holiday rolls to the next session |
| DST | Dates around 8 Mar 2026 and 1 Nov 2026 (US DST changes): enteredTime near 02:00 local and near midnight UTC give the right ET date; the 01:30 ET ambiguity on 1 Nov 2026 is handled (the API form is `+0000`, UTC, in client samples; a non-UTC offset is UNVERIFIED and is converted via its offset) |
| NothingToSell | Sell with zero shares: `NothingToSell`, no lock event, no audit CRITICAL |
| Restricted | `ICE` buy and sell denied with `Restricted` before any history is loaded (a failing history source does not change the reason); `restricted.toml` contains `ICE`/`never_trade`; unparsable file denies all; ICE in a mock Schwab history (any status, incl. a zero-fill order) => audit halts, CRITICAL, **no sell is generated** |
| Permit | Permit for another ticker or date rejected by `submit_*`; compile-fail test (`trybuild`, verify) that permits cannot be built outside `lock`; grep test that every submission entry point takes a permit and none takes an override |
| Audit | Clean history => `COMPLIANT` report written; injected manual sell inside a window => violation, halt, CRITICAL row, report; **an open order is not skipped** (an opposite order placed while `a` is open is flagged); report is written even when clean; unreadable history => retry, then `INCOMPLETE` with `lock_audit_incomplete` (not `lock_violation_detected`) and an `INCOMPLETE` halt that a clean re-run clears; a zero-fill order that was open while an opposite order was placed (either one ending with zero fills) => no violation, no WARN, no audit row, no halt; only the report's `INFO` count `zero_fill_overlaps` increases (decision B2, example 22) |
| Veto / pending | Veto on a held name: buy blocked, sell queued for `first_permitted`, fires then; veto never cancels the queued sell; `regime_crisis` cancels buys only; firing re-enters the generator with a fresh quantity |
| Snapshot | Written at about 16:50 for the next session; contents equal `authorize` on the same history for `for_session`; open order shows `open_no_end_date`; `BOTH` handled; Julia stand-in reader treats missing/stale/incomplete/unparseable as blocked both ways; a snapshot never changes an `authorize` result |
| No cache | Static check: no read of `position_locks` on any order path (grep test) |

## 10. Decisions and Open Items

Decided by the owner on 2026-10-10 (`DECISIONS.md` B1-B4):

- **B2, zero-fill orders open during an opposite placement: not locked.** The
  retrospective audit does not flag them (no WARN, no halt); the report carries
  only the `INFO` count `zero_fill_overlaps` (§8 step 2a, example 22).
- **B1, share classes: one lock group per issuer** (§3.8, example 21).
- **B3, past-closure backfill: yes, add-only** (amendment 1.12; §4
  "Price-bar backfill").
- **B4, owner notification channel: dashboard only.** Halts,
  `broker_auth_expiring` and the calendar `WARN`s reach the owner through the
  dashboard (`17-frontend`); no email or push channel is built.

Still open (not owner decisions; residual risks or live items):

1. **Window conflict** (per-account 1 year vs 60 days): the 45-day window is
   safe either way; widening it needs the live test (3.5 test 1).
2. **Old open orders** entered before the window are invisible to an
   `enteredTime`-filtered read (3.1); the `transactions` net catches their fills
   but not a still-open pre-window order with no fill. Residual risk, stated in
   the report.
3. **Stream scope and message types** (ACCT_ACTIVITY covering all accounts,
   partial-fill/reject/replace event names) are UNVERIFIED; correctness does not
   rely on them.

- Pre-clearance or disclosure: not required (owner, 2026-10-10).
- `REPLACED` zero-fill orders: **decided** (owner, 2026-10-10): not counted, like
  `CANCELED`.

## 11. Worked Examples

Examples 1-22 assume the order filled on its placement day unless stated.
All dates verified against the retired Python reference's fixtures
(`fixtures/lock_cases.json`, `fixtures/nyse_calendar_cases.json`) and, for the
timezone cases, an independent `zoneinfo` check; the Julia oracle reproduces them
(run `scripts/run_fixtures.jl`).

| # | History | Question | Answer |
|---|---|---|---|
| 1 | Buy placed Mon 12 Oct 2026 | Sell on Fri 16 Oct / Mon 19 Oct | Blocked Mon-Fri; **first sell Mon 19 Oct** |
| 2 | Buys placed Mon 12 and Wed 14 Oct | First sell | Same-direction extends: blocked through Tue 20 Oct; **first sell Wed 21 Oct** |
| 3 | Buy placed Mon 23 Nov 2026 (Thanksgiving Thu 26 Nov is closed) | First sell | Blocked Mon 23, Tue 24, Wed 25, Fri 27, Mon 30; **first sell Tue 1 Dec** (a weekday count would say Mon 30 Nov: a breach) |
| 4 | Buy placed Mon 12 Oct, cancelled with zero fills | Sell Tue 13 Oct | Allowed (not counted) |
| 5 | Same buy, still WORKING | Sell Tue 13 Oct | Blocked |
| 6 | Same buy, 40% filled then cancelled | Sell Tue 13 Oct | Blocked (counted) |
| 7 | Buy in the owner's Fidelity account (in `lock_external.csv`) Wed 14 Oct | Schwab sell Tue 20 Oct / Wed 21 Oct | Blocked / allowed |
| 8 | Buy placed Mon 12 Oct 22:00 ET (= Tue 13 Oct 02:00 UTC) | First sell | `D` is Mon 12 (ET date); **Mon 19 Oct**, not Tue 20 |
| 9 | Buy placed Fri 30 Oct 2026 23:30 EDT (= Sat 31 Oct 03:30 UTC) | First sell | `D` = Fri 30 Oct; **Fri 6 Nov** |
| 10 | Buy placed Sat 31 Oct 2026 (non-trading day) | First sell | `D` rolls to Mon 2 Nov; **Mon 9 Nov** |
| 11 | Buy placed Sun 1 Nov 2026 19:30 EST (= Mon 2 Nov 00:30 UTC; DST ended that morning) | First sell | `D` rolls to Mon 2 Nov; **Mon 9 Nov** |
| 12 | Sell placed Mon 12 Oct | Buy Fri 16 / Mon 19 Oct | Symmetric: blocked / allowed |
| 13 | Buy placed Mon 12 Oct | Another buy Tue 13 Oct | Always allowed |
| 14 | Sell with zero shares held | Sell | `NothingToSell` (rejected request) |
| 15 | Any order in `ICE` | Either side | `Restricted`, CRITICAL `restricted_security_blocked` |
| 16 | GTC buy placed Mon 12 Oct, filled Thu 15 Oct | Sell Mon 19 / Wed 21 / Thu 22 Oct | Blocked / blocked / **allowed Thu 22 Oct** (clock from the fill) |
| 17 | Same GTC buy, still working on Tue 1 Dec | Sell | Blocked (open: no end date) |
| 18 | Buy partially filled Mon 12 and Wed 14 Oct, cancelled Thu 15 | First sell | Last fill Wed 14: blocked Tue 20, **allowed Wed 21 Oct** |
| 19 | Buy placed Mon 12 Oct 2026; an unplanned closure on Wed 14 Oct is announced Tue evening and appended to the overrides on the 09:00 refresh of Wed 14 Oct (first sighting) | First sell | Without the closure Mon 19 Oct; with it **Tue 20 Oct** (Wed 14 is not a session). If the closure were missed, a sell on Mon 19 would breach: this is why the write happens on the first sighting and today is queried |
| 20 | Buy placed Mon 12 Oct, then cancel/replaced (old order `REPLACED` with zero fills, new buy placed Mon 12 and filled) | First sell | The old order is not counted; the replacement counts from its fill: **Mon 19 Oct** |
| 21 | Buy `GOOGL` placed and filled Mon 12 Oct (share classes are one lock group, §3.8, decision B1) | Sell `GOOG` Fri 16 / Mon 19 Oct | Blocked / **allowed Mon 19 Oct** (the same as a sell of `GOOGL`) |
| 22 | Buy placed Mon 12 Oct 10:05, cancelled 10:20 with zero fills; a sell of the same ticker placed 10:10 while the buy was open (decision B2) | Retrospective audit | Not a violation and no WARN: the buy ended uncounted. The live `authorize` at 10:10 denied the sell (an open order is counted), so this arises only from an order placed outside the system; the audit's report carries it in an `INFO` count only |

## 12. Pending Orders

See `PENDING-ORDERS.md`. When the target portfolio wants a trade that the
lock blocks, the order is queued with `earliest_execution = first_permitted`
(the day `D + 5 trading days`, recomputed from history). On that day the
timer re-validates the signal (a veto never cancels a queued sell) and
**releases the intent to the normal order generator** at the start of the
execution window (10:00 ET): the generator recomputes the quantity from the
fresh price and NAV, applies the spread gate and the ladder, and the placement
goes through the gate (`authorize` is asked again, authoritative, at
placement). The queued quantity is never submitted as it was at queue time.
Timers fire at the start of the execution window of the first permitted day,
not at the 09:30 open: orders are placed only in-session (nothing outside
09:30-16:00). The monitor's morning run only re-dates and emits derived events.

## 13. What Not to Do

- Do not store a lock state, expiry or "locked until" as authority. Do not
  read `position_locks` to decide an order.
- Do not answer the lock from a cached row or an in-memory flag. Recompute
  from order history at submission time, over a REST read made for that
  authorize.
- Do not count calendar days or weekdays. Count NYSE trading days from the
  computed NYSE calendar (rules plus overrides), not from `prices.parquet`.
- Do not exclude today from the calendar-sync query, require a second sighting
  before writing a closure, or let the suspect-source rule suppress a write.
- Do not take the UTC date of `enteredTime`. Convert to America/New_York.
- Do not skip an unreadable, stale or unparseable source. Block. Do not let one
  unknown enum value block the whole account for 45 days either: degrade it to its
  ticker (§3.6).
- Do not skip a malformed row of `lock_external.csv`. Reject the file.
- Do not add an override, a permit-bypass, an environment variable, a
  "dry-run" path that submits, or a manual escape hatch. There are no
  exceptions: a veto, regime change, stop, optimizer output or instruction
  cannot place an order inside a block.
- Do not auto-sell a position to "fix" a violation or an ICE anomaly. Halt,
  write CRITICAL, and let the owner act.
- Do not auto-clear a `VIOLATION` halt, and do not label an outage a violation.
- Do not release the per-ticker gate between the history read and the send, and
  do not carry a permit through a channel.
- Do not submit a sell leg (stop, bracket exit) together with a buy.
- Do not start the clock at placement for an order that filled later; use the
  last fill date. Do not treat an open order as having an end date.
- Do not count a `REPLACED` zero-fill order.
- Do not submit non-`DAY` orders without a reason: `DAY` is the design default
  (slippage, same-session), not a lock requirement (see §3).
- Do not construct `BuyPermit`/`SellPermit` outside `lock::authorize_and_reserve`.
- Do not describe the lock as a risk parameter or compile-time enforced.
  The compiler enforces the permit requirement; `authorize` and its tests
  enforce the rule.
- Do not fire pending orders without re-validating the signal, and do not
  skip `authorize` when firing; do not submit the queued quantity.
- Do not compare ticker spellings as strings; canonicalise (§3.8).
- Do not mix the lock gate with the regime gate. They are separate controls.

## 14. Verification

- [ ] `julia --project scripts/run_fixtures.jl` passes (oracles match the fixtures)
- [ ] `execution/tests/lock_fixtures.rs` passes on the same fixture files
- [ ] Property tests show Rust `authorize` equals the oracle on random
      multi-account histories
- [ ] Every worked example in §11 is a test
- [ ] Trading days come from the computed NYSE calendar (rules + overrides);
      a test checks the rules against the published 2022-2027 lists and the
      seed overrides; adding a closure never makes `first_permitted`
      earlier; `calendar-sync` queries today, writes on the first sighting,
      never removes a closure and never halts trading; no import of
      `prices.parquet` in `locks/`
- [ ] An unreadable Schwab source, journal or external file denies both sides;
      a single unknown enum value with a parseable symbol blocks only its ticker
- [ ] A REST read precedes every `authorize` (`N = 0`)
- [ ] A malformed `lock_external.csv` row rejects the whole file; a missing
      file blocks; a header-only file is valid
- [ ] An ET-date test: 22:00 ET Monday is Monday; 23:30 ET Friday is Friday
- [ ] A GTC order placed Mon and filled Thu allows a sell from the following
      Thu; a still-working order blocks with no end date; partial fills on
      Mon and Wed then cancel allow a sell from the following Wed
- [ ] A cancelled or replaced zero-fill order does not block; a
      partial-fill-then-cancel does; unknown status blocks
- [ ] `authorize` rejects `ICE` (restricted) before reading history, and
      `config/restricted.toml` contains `ICE`
- [ ] The halt flag is honoured by `authorize`, `submit_*`, the pending timer and
      the ladder; only the owner clears a `VIOLATION` halt
- [ ] The per-ticker gate makes check-then-send atomic (concurrent opposite
      submissions: one permit)
- [ ] `NothingToSell` is a rejected request, not a lock event
- [ ] A compile-fail test asserts a permit cannot be constructed outside the
      `lock` module and `submit_*` cannot be called without one
- [ ] A permit for another ticker or another date is rejected
- [ ] A pending sell is released on `today >= first_permitted`, recomputed by the
      generator, and a veto does not cancel it
- [ ] The daily audit writes a dated report every trading day, halts and
      writes CRITICAL on an injected violation, flags ICE in a Schwab
      account without selling it, does not skip open orders, and reports an
      outage as `INCOMPLETE`
- [ ] No code path reads `position_locks.parquet` to authorize an order

## 15. Related Skills

- `00-construction` §1, §6 — the lock and restricted securities
- `10-execution-schwab` — pre-submit history check, permit flow, reconciliation, halt
- `11-rust-event-bus` — `lock_violation_detected`, `lock_audit_incomplete`, `restricted_security_blocked`; derived `LockTransition`/`LockExpiry`
- `13-parquet-lake` — `position_locks` (derived view), `pending_orders`, `order_history`
- `07-fundamental-overlay` — veto: blocks buys, queues a sell, never overrides
- `08-portfolio-optimization` — owns the restricted-ticker eligibility filter; reads the lock snapshot
