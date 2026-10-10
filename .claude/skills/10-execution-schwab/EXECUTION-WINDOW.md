# Execution Window and Least-Slippage Design

> **Phase: later.** Execution and live trading come last in the build order
> (`outline.md` "Project phase and build order"). Kept as the specification
> for that phase; do not build or refine it unless asked.

How the order generator turns a target weight into fills with the least
slippage the system can achieve, without ever touching the 5-day lock.

**Priority order (the user's, in this order):** (1) compliance with the
5-day opposite-side lock (`00-construction` §1), (2) least slippage, (3)
latency, (4) profit. Nothing in this file may trade a lower priority for
a higher one: every design choice below is legal under the lock in every
branch, and slippage is minimised only inside that envelope. The
restricted-securities rule (`00-construction` §6, `config/restricted.toml`)
applies to every order sequence described here.

**Status of every number.** No slippage benefit is measured yet and none
is assumed anywhere in this skill tree. Every parameter below marked
`[placeholder]` is a starting configuration, not a finding. Every claimed
gain (window choice, passive posting, spread gate, no-trade band) is a
**hypothesis** until the randomized experiment in §10 and the shadow
engine (`SHADOW-ENGINE.md`) report. Schwab behaviours that are not
documented in this tree are marked **verify** or UNVERIFIED; the confidence labels of `SCHWAB-API-REFERENCE.md` apply.
Code blocks are SKETCHES (not compiled, not normative; `15-ai-dev-workflow` §4).

## 1. Decide and Execute Are Separate Steps

| Step | When | Who | Output |
|---|---|---|---|
| Decide | After the close of session `t` (optimization 17:00 ET) | Julia | `portfolio_targets` for session `t+1` |
| Plan | After the optimizer, before midnight | Rust | Per-name intents for session `t+1`; nothing is sent to Schwab |
| Execute | Session `t+1`, inside the execution window (§1.2) | Rust | Orders, only inside the window, only with permits |

Orders are **not** placed in the evening for the next open, and nothing
is placed at the open.

### 1.1 Why not "place after the close, fill at the open"

- The open auction and roughly the first 15 to 30 minutes have the widest
  quoted spreads and the highest short-term volatility of the session.
  (Hypothesis from market-microstructure practice, to be checked on this
  system's own quote data in §10; do not treat it as established.)
- Placing in the session makes the **placement date equal the fill date**.
  The lock runs from the placement day while an order is open and, once
  filled, through 4 trading days after `D`, the **last fill date**
  (`00-construction` §1 rules 2-3), so for a same-session fill the
  contractual clock and the economic trade coincide. An evening
  placement is dated `t` while it fills on `t+1`, which would make the
  placement and fill dates differ for no benefit.
- Orders are placed only during the regular session (America/New_York,
  Normal session, `Duration::Day`). There are no after-hours placements,
  so the placement date is never decided by a midnight or time-zone edge,
  and no order can be carried past the close. `Duration::GoodTillCancel` is
  lock-compliant (an open order blocks with no end date and the clock then
  runs from its last fill) but the execution design does not use it: it
  uses `Day` orders only.

### 1.2 The default window

    open of regular session   09:30 ET   no placements
    quiet period              09:30-10:00  no placements (first ~15-30 min)  [hypothesis]
    execution window          10:00-15:30  default placements and ladder
    deadline fallback         15:30-15:50  only the capped-cross fallback (§3 step iv, §2 deadline rule)
    last-10-minutes quiet     15:50-16:00  no placements, working orders cancelled at 15:50

The times above are for a normal 16:00 close. They are **defined relative to the
actual close** `C` of the day, which Rust derives from the market-hours data
(`regularMarket.end`, fetched at the 09:00 refresh; `09-lock-state-machine` §4):
the window ends at `C - 30 min` (the deadline), working orders are cancelled at
`C - 10 min` ("15:50" below means `C - 10 min`), and nothing is placed after
`C - 10 min`. On a 13:00 early close these are 12:30 and 12:50 and the window
ends there (§9). If the close for the day cannot be read, the day is treated as
a possible early close: no sequence is started after 12:30 ET until the close is
confirmed, and working orders are cancelled at 12:50 unless it is. The lock is
unaffected: it counts sessions, not hours.

**Halt and closure.** If `TRADING_HALTED` is set (`09` §8b), the ladder places and
replaces nothing and cancels its working orders best-effort; if today is
appended to the calendar as closed (same-day signal, `09` §4), no sequence
starts (`authorize` denies `NonTradingDay`).

The 10:00-15:30 window is a **hypothesis** (the opening and closing
extremes are avoided). The randomized experiment in §10 replaces it with
the measured best window per tier.

### 1.3 Staleness and cheap re-validation

Targets are computed from the close of `t` and used up to 22.5 hours
after the optimization run (17:00 to 15:30 the next day; 18 to 23.5 hours
from the close). The GSPQR horizon is 63 days and up, so a signal half-life
of weeks makes that delay small next to the spread. It is still checked
before each sequence starts, using only quote-cache data (no Julia, no
bridge in the order path):

    move_sigma = ln(mid_now / decision_close) / sigma_daily

where `decision_close` is the close price the optimizer saw and
`sigma_daily` is the name's trailing daily volatility (carried to Rust
with the targets in `execution_costs.parquet`, §11.1; until it exists the check is skipped and
only step 1 applies).

1. **Always:** the quantity is recomputed from the fresh price and the
   current NAV: `target_shares = f(w_target × NAV / mid_now)` with the
   tier rounding of `SKILL.md` §2a. The order is the share **difference**
   to the current holding. This is "the same weights logic with the fresh
   price".
2. **If `|move_sigma| > k` (`k = 1.5` [placeholder]):** a **buy** is
   skipped (not placed) and logged `gate_reason = stale_move`; the name is
   re-decided by tonight's optimization. A **sell** that is due proceeds
   with the recomputed quantity. Rationale (hypothesis): skipping a buy
   costs a day of delay; a gap of that size may be news the fundamental
   overlay has not seen; a sell reduces risk and is not made less
   justified by a move.
3. A target is valid for **one session only**. Nothing unfilled is
   carried over (§3 compliance notes).

Skipping or deferring before placement has **no compliance effect**: an
order that is not placed does not exist, so it is not counted.

**Pending (queued) orders re-enter here.** A queued order released by the timer
at 10:00 (`09-lock-state-machine/PENDING-ORDERS.md` §3) is an ordinary intent:
step 1 recomputes its quantity from the fresh price and NAV (never the quantity
queued), the stale-move rule applies, then the spread gate and the ladder. The
release at 10:00 and the placement are separate moments: with the randomized
window arms of §10.2 the sequence of arm `B` or `C` starts at 11:30 or 14:00 and
is gated again by `authorize` at placement. A veto exit (a held vetoed name,
whole position) is "due" in the sense of §2 and bypasses the stale-move skip.

### 1.4 What the order generator may and may not do with a target

- It trades the **difference** between the recomputed target shares and
  the current holding, in whole shares in the limit tiers.
- If the sign of that difference is opposite to a counted block (lock
  check at placement, `09-lock-state-machine`), nothing is placed.
  Rounding must never turn a hold into an opposite-side order: a
  buy-blocked-sells name whose rounded target is one share below its
  holding because the price moved is **not** sold; the difference is
  clamped to zero.
- A ticker on `config/restricted.toml` gets no sequence at all.
- At most one side per ticker per session (§6).

## 2. Spread Gate

Immediately before starting a sequence (and before every retry) the Rust
quote cache is read: bid, ask, bid size, ask size, quote timestamp.

    spread_bps = (ask - bid) / mid × 10000,  mid = (bid + ask) / 2

**The quote is unusable** (sequence deferred, `gate_reason = bad_quote`)
if any holds: quote older than `max_quote_age_s` [placeholder 5 s]; `bid`
or `ask` not positive; `ask <= bid` (locked or crossed); either size is
zero; the quote's **security status** (LEVELONE field 32, REST
`securityStatus`) is anything other than `Normal` (the documented values are
"Normal, Halted, Closed"; other spellings UNVERIFIED, so any other string
defers); the record is `delayed: true` (REST `quoteType` `NFL` or
`realtime: false`); the stream is down or gapped for the name (§2.1). The
quote stream has **no LULD / limit-up-limit-down field** in the official table
(absence; the doc has no such field), so that part of the check cannot be done
from the stream and is inert; a halted or abnormal status defers the sequence
(fails safe).

**Spread gate.** Defer if

    spread_bps > gate_bps_i
    gate_bps_i = min( hard_cap_bps , max( m × median_spread_bps_i(bucket) , floor_bps ) )

- `median_spread_bps_i(bucket)` is the name's trailing median quoted
  spread **for the same time-of-day bucket** (30-minute buckets), because
  spreads are strongly time-of-day dependent; a single daily median would
  defer everything in the morning.
- `m` [placeholder 2.0] is the relative multiple; `floor_bps`
  [placeholder 3] stops a very tight name from being gated by one-tick
  noise (this is the "absolute" term the max protects); `hard_cap_bps`
  [placeholder 40] is the ceiling beyond which the system does not trade
  that name that session regardless of its own history.
- Medians come from `quote_spread_1m.parquet` (the quote recorder, §2.1), by
  30-minute bucket, over the trailing window. Until at least `N_sessions`
  [placeholder 20] sessions of recorded quotes exist for a name, there is no
  median: only `hard_cap_bps` applies and the row is tagged
  `gate_reason = no_history` on passing. Recording starts **before live
  trading** so that the medians exist when trading starts.

**Retry.** A deferred sequence is retried every `retry_s` [placeholder 30 s]
until the arm's deadline (§10 gives each arm its own deadline; outside the
experiment it is 15:30). Each deferral increments `deferral_count`.

**At the deadline** (only reached if the gate never passed or the ladder did
not complete):

| Intent | At the deadline |
|---|---|
| Sell that is due (target lower than holding, target not a rounding artefact) | Capped-cross fallback (§3 step iv), if the quote is usable and the **price cap** can be honoured; otherwise cancelled and re-decided tonight |
| Buy that is not due, or any buy | Cancelled / never placed (`gate_reason = deadline_not_due` or `deadline_gate`). **Never** crossed at a bad price |
| Fractional market remainder | Dropped (returned to cash) |

"Due" means the optimizer's target requires the trade for a hard
constraint (a cap, the cash buffer, or a veto exit) or
the trade is a sell whose size exceeds the no-trade band (§7). Buys are never "due"; a missed buy is
re-decided tonight at the cost of one day.

A cancelled or never-placed order has zero fills and is not counted by
the lock; nothing here can create a block. A sell that was due and is
deferred to tomorrow starts no new block (it was never placed), so the
sell remains lawful tomorrow provided no counted buy has appeared; the
optimizer sees the lock status again tonight.

### 2.1 Quote Recorder (the intraday spread history)

The system has no intraday bid/ask history, and none can be fetched: the
Market Data `pricehistory` endpoint is OHLCV only (CONFIRMED-OFFICIAL SPEC-M),
so spread history can only be **built forward** from the stream or REST polls.
The recorder builds it. It needs no order and no permit and runs in shadow mode
from the first day. Facts and labels: `SCHWAB-API-REFERENCE.md` §2.6, §4.2.

**Module sketch (Rust, not compiled; verify):**
`execution/src/schwab/streamer.rs` (WebSocket session, LOGIN, SUBS/ADD,
reconnect, heartbeat watch, dirty/gap signalling; shared with the `ACCT_ACTIVITY`
handler of `09-lock-state-machine` §3.2) and `execution/src/schwab/quotes.rs`
(per-symbol state, 1-minute aggregation, REST polling, the Parquet writer).

```rust
// quotes.rs (sketch)
pub struct SymbolState {                 // merged "Change" deltas: a missing field = unchanged
    bid: Option<Decimal>, ask: Option<Decimal>, last: Option<Decimal>,
    bid_size_shares: Option<u64>,        // stream field 4 is in LOTS of 100: multiply by 100
    ask_size_shares: Option<u64>,        // field 5, same
    total_volume: Option<u64>,           // field 8
    security_status: Option<SecurityStatus>, // field 32: Normal | Halted | Closed | Unknown(String)
    quote_time_ms: Option<i64>, trade_time_ms: Option<i64>,   // fields 34, 35
    bid_time_ms: Option<i64>, ask_time_ms: Option<i64>,       // fields 37, 38
    delayed: bool,
}
pub struct MinuteAgg { n_updates: u32, spreads_bps: Vec<f64> /* per update, valid quotes only */,
                       last: SymbolState, volume_first: u64, flags: StatusFlags, source: Source }
```

**Subscription set (LEVELONE_EQUITIES).** One streamer connection per user
(CONFIRMED-OFFICIAL). Reported cap 500 symbols (COMMUNITY; code 19 on excess).
The universe is about 600 names, so the stream set is prioritised and the rest
is polled over REST:

1. *Always streamed:* held names; names with a target, a queued pending order or
   an open order today; the candidates the planner may trade this session.
2. *Then by turnover:* the highest-turnover names (trailing median dollar
   volume from `prices.parquet`), filling up to `stream_cap` = reported cap
   minus a reserve of 20 [placeholder], so that urgent `ADD`s do not hit the cap.
3. *The remainder* (about 100-150 names, the lowest turnover and not otherwise
   needed) is polled over REST: `GET /marketdata/v1/quotes?symbols=...` in
   batches of `rest_batch` [placeholder 100; the batch limit is UNVERIFIED] every
   `rest_poll_s` [placeholder 60] seconds. That is a few requests per minute,
   far inside 120/minute (COMMUNITY).
4. *Rotation.* The set is rebuilt at the 09:00 pre-open step from the previous
   close's holdings/targets and turnover rank, with hysteresis (a name leaves
   the stream set only when it falls more than 20 ranks below the cut), and
   changes intraday only by `ADD` for newly held/targeted names, evicting the
   lowest-priority streamed name with `UNSUBS`. If code 19 arrives, `stream_cap`
   is lowered to the number accepted and the overflow moves to REST. If the stream is down, **all** names are polled over REST
   at `rest_poll_s` (rows tagged `source = rest`, one sample per poll).
5. Request fields `0,1,2,3,4,5,8,32,34,35,37,38` only (fields 52-54 are not
   defined in the official table: UNVERIFIED, never requested). REST uses the
   `quote` root node only (`fields=quote`).

**Per update (stream) or per poll (REST):** merge into `SymbolState`; during
regular hours only (09:30:00-16:00:00 ET by `quoteTime`/arrival time; nothing
before the open or after the close is aggregated). Drop an update from the
spread statistics (but set a flag) if: `bid <= 0` or `ask <= 0`; `ask <= bid`
(locked or crossed); a size is zero; `delayed`/`NFL`; security status not
`Normal`. `spread_bps = (ask - bid) / mid * 10000`.

**1-minute rows.** For each (ticker, ET minute) with at least one update (or
one poll) write one row; a name with no update in a minute while the feed is
healthy gets a carried row (`n_updates = 0`, last quote, flag `STALE_CARRY`,
null spread statistics) so that "no update" is distinguishable from "recorder
down". A minute with the stream down and no REST fallback writes **no** row;
the gap is recorded in an audit row `quote_recorder_gap` (WARN; start, end,
reason). Normalised units: sizes in **shares** (stream lots x 100). The REST
`bidSize`/`askSize` are described as "number of shares" but whether they are
also lots is UNVERIFIED: until the live test compares both for a known stock,
REST-sourced size columns carry the flag `SIZE_UNITS_UNVERIFIED`.

**New dataset: `quote_spread_1m`** (dataset #27, `13-parquet-lake`
`SCHEMAS.md` #27; Rust writer; partitioned by `session`, `13-parquet-lake/IO-CONTRACT.md` §2). Columns:
`date`, `minute_et`, `ticker`, `n_updates`, `bid_last`, `ask_last`,
`spread_bps_median`, `spread_bps_p90`, `bid_size_last`, `ask_size_last`,
`last_price`, `volume_delta`, `status_flags`, `source`. Rows are buffered on
disk during the session in an append-only spool (Arrow IPC segments or SQLite;
Parquet cannot be appended) and written once per day after the close
(~16:05 ET) to `$QUANT_ROOT/lake/quote_spread_1m/session=YYYY-MM-DD/data.parquet` via temp file,
fsync and atomic rename; a crash keeps the spool and the file is written at
restart. **Size (estimate, not measured):** ~600 names x 390 minutes is about
234,000 rows/day, about 59 million rows/year; at an assumed 10-20 bytes/row
after ZSTD on sorted columns that is roughly 0.6-1.2 GB/year. Treat as an
order-of-magnitude figure until a week of real data exists.

**Uses (in this skill and its neighbours).**

- The spread gate's time-of-day medians (§2): `median_spread_bps_i(bucket)` =
  median over trailing sessions of the minute rows' `spread_bps_median` in the
  30-minute bucket (rows with `STALE_CARRY`, crossed or delayed flags and
  `source = rest` rows may be weighted or excluded; placeholder: stream rows only
  when at least 50% of a bucket's rows are stream rows).
- The spread-bucket quintiles of the experiment (§10) and the per-name cost
  model (§5, §8).
- The backtest spread source (`14-backtest-protocol`, owned elsewhere: it
  should read this file once `N_sessions` sessions exist and use the prior
  before then; not edited here).
- **Prerequisite and ramp.** Start recording before live trading; require at
  least `N_sessions` [placeholder 20] recorded sessions before the gate relies
  on medians. Until then the `no_history` path applies (`hard_cap_bps` only) and
  the §8 spread filter is off.

**Failure behaviour (fails safe).** Stream down, heartbeat gap, parse failure or
code 3/12/30: the recorder marks a gap and the quote cache for the affected
names is *unusable*, so the spread gate **defers** (the sequence is not started
or is paused; nothing is crossed at a bad price). Recovery: reconnect with a new
token, re-send the subscriptions, resume. No fill is ever inferred from a gap.

## 3. The Order Ladder (Standard and Transition Whole-Share Limits)

All legs for a ticker happen in **one** session, one side, one direction.

1. **Arrival.** Record `arrival_mid` from the quote read that passed the
   gate. It is the benchmark for implementation shortfall (§10).
2. **Post.** Submit a `Limit` order at the passive price:
   buy: `P_0 = arrival_mid` rounded **down** to a legal tick, or one tick
   inside the touch (`bid + 1 tick`) when the spread is at least two
   ticks; sell: mirrored. If the spread is one tick, post at the touch on
   the near side (join the bid for a buy, join the ask for a sell).
   Hold for `T1` [placeholder 10 minutes].
3. **Reprice** toward the touch on a schedule by cancel/replace
   (placeholder fractions of the distance from `P_0` to the far touch:
   0.5 at `T1`, 1.0 at `2·T1`; i.e. join the touch at `2·T1`), each leg
   with the remaining quantity only.
4. **Cross.** At the arm deadline (or when the schedule is exhausted) cross
   with a marketable limit **capped at `X` bps through the touch**: buy
   `limit = ask + X bps × mid` rounded **down** to the tick, sell
   `limit = bid − X bps × mid` rounded **up**. The cap is the slippage
   budget: if the book is thinner than the cap allows, the remainder is
   left unfilled and cancelled at 15:50, never chased. `X` is calibrated
   per tier from realized fills (initial value `[placeholder]`: the
   per-tier shadow-engine median slippage plus one half-spread, to be
   replaced as data accumulate).

The control arm (`ladder_arm = cross`) does step 4 only, immediately.

**Whole shares.** The ladder never submits a fractional limit (fractional
quantities fill only as market orders in regular hours, `ORDER-TYPES.md` §9). Partial fills reduce the remaining
quantity for the next leg. The sum of filled and working quantity never
exceeds the target share count, so the 5% cap and the 2% cash buffer
cannot be breached by a ladder overlapping its own fills.

**Permits.** Every cancel/replace submission is a new order and needs its
own permit (the lock permit is one-shot, `09-lock-state-machine`; the
tier check re-runs, `SKILL.md` §2a; the restricted check re-runs). A
replacement that cannot obtain a permit leaves the previous order
cancelled and is not retried that session.

### 3.1 Compliance notes for the ladder

- A cancel/replace of a same-side order is lawful: the replacement is a
  same-direction order, always allowed, and carries the same placement
  date (the session date).
- An order that is cancelled with **zero fills** is not counted by the
  lock. An order that is partly filled and then cancelled **is** counted:
  harmless here, because it is the same side on the same day.
- A race (a fill arrives while a cancel is in flight) cannot change the
  side; it can only make the order counted, again same side, same day.
- **Never carry an unfilled remainder into the next session** unless the
  signal re-validates through tonight's optimization. `Duration::Day`
  orders expire at the close; the generator does not re-place them. The
  reason is the exit date: a remainder that fills on a later day moves the
  earliest opposite order to that later last-fill day plus 5 trading days,
  so finishing in one session keeps the exit date earliest. A new placement
  on a later day is a same-direction order that only delays the earliest
  opposite order; it cannot create a violation.
- The lock check at placement is authoritative and is never answered from
  this file's intents or from the optimizer's lock snapshot (§8.1).

### 3.2 The trade-off, stated plainly

Passive posting can capture part of the half-spread, but it risks
non-fill (the price moves away, an unfilled buy is a missed return) and
adverse selection (passive orders tend to fill when the price is about to
move against them). Crossing immediately pays about the half-spread with
certainty. Which is cheaper is an empirical question about this account,
these names and these prices. The fill probability of a passive order is
unknown: the shadow engine brackets it (`SHADOW-ENGINE.md` §2) and the
experiment in §10 measures it. No benefit of posting is assumed.

## 4. Bootstrap Tier (Fractional Market Orders)

A market order has no price cap, so the **spread gate and the window do
all the work** (§1, §2); there is no ladder. Additional rules:

- **Minimum notional.** Skip a Bootstrap trade whose notional is below
  `N_min_bootstrap` [placeholder; also at least the broker's fractional
  minimum, UNVERIFIED; the Stock Slices minimum is "as little as $5"]. Reasons: a tiny trade carries the same lock
  consequence as a large one (every counted buy blocks sells in that name
  for five trading days), pays the full half-spread, and moves the
  position negligibly.
- **Per-name spread filter.** Skip entries into a name whose estimated
  round-trip cost (`RT_i`, §5) exceeds `entry_edge_fraction × q_50` of
  that name [placeholder 0.25]. This filter matters most in Bootstrap
  because the spread is paid in full on every trade.
- **Fractional rules (owner-confirmed; `SKILL.md` §2b).** Fractional
  quantities are market orders only, placed and filled only in regular hours
  (inside this window by construction); no fractional limit (a fractional limit
  *sell* reported in a social post is UNVERIFIED and is not relied on).
  The Trader API accepting a fractional quantity is OWNER-CONFIRMED; the
  quantity has at most 2 decimals and is rounded down (OWNER-REPORTED,
  UNVERIFIED; live test 5 confirms with `previewOrder`). **Eligibility:**
  Stock Slices covers S&P 500 stocks only, no ETFs (COMMUNITY), from about $5;
  in Bootstrap only `fractional_eligible` names are traded (`Unknown` = not
  tradable), and `N_min_bootstrap` is at least the $5 Stock Slices minimum
  ("as little as $5", Schwab press release, CONFIRMED-OFFICIAL via a summary;
  the API's own minimum is UNVERIFIED).
- **Quantity granularity and the minimum notional.** The quantity step is
  0.01 share, so the smallest expressible trade is `0.01 × price` dollars and
  a quantity is `floor(target_qty, 2 decimals)`. Flooring can lose up to
  0.01 share (`≤ 0.01 × price`), which returns to cash. The minimum-notional
  rule above is applied **to the floored quantity's notional**
  (`qty_floored × price`), after flooring; a trade whose floored quantity is
  zero, or whose notional is below `N_min_bootstrap`, is skipped. The step
  never overrides the minimum (a legal 0.01-share quantity can still be below
  `N_min_bootstrap` and is then skipped), and the minimum never permits a
  finer step. For a high-priced stock the step itself exceeds `N_min_bootstrap`
  when `0.01 × price > N_min_bootstrap`, so the effective minimum is one step.
  No numeric value is fixed here; `N_min_bootstrap` and `N_min_frac` stay
  placeholders.
- **Schwab fractional / Stock Slice execution timing and price are
  unverified.** The terms describe "not held" market orders executed on a
  best-efforts basis with possible delay before they appear in the account
  (COMMUNITY quoting the terms); whether they execute immediately at the
  quote, in batches at specific times, and at what price relative to the NBBO is
  **not** documented (live item). Until learned, Bootstrap fills are
  logged with timestamps and compared with the quote path
  (`SHADOW-ENGINE.md`), `window_arm` is recorded as null for Bootstrap
  rows if fills turn out not to be time-of-day sensitive, and no
  Bootstrap conclusion is carried over to the whole-share tiers.
- **Transition fractional remainder.** The remainder (`0 < rem < 1` share,
  floored to 2 decimals) is a market order under a `MarketPermit`
  (`SKILL.md` §2a). It is only for **fractional-eligible** names
  (`Unknown` = not eligible); a non-eligible name trades its whole-share
  limit only and the remainder stays in cash, and a target worth less than
  one share in such a name yields no trade. It is submitted only after the
  whole-share limit leg is finished, only if the gate passes, and only if
  `rem_floored × price ≥ N_min_frac` [placeholder]; else it returns to cash.
  The 0.01-share granularity interacts with `N_min_frac` exactly as in the
  minimum-notional bullet above (floor first, then the notional test). It
  does not use the ladder.

## 5. The Per-Name Cost Model (single source for 08 and 14)

The optimizer's cost vectors (`08-portfolio-optimization`) and the
backtest's cost model (`14-backtest-protocol` §6a-6e) both read this
model; they do not restate it.

For name `i`, tier `τ`, ladder arm `a`, spread bucket `b`:

    half_spread_i      = median_spread_bps_i / 2        (trailing, §8; the prior)
    c_entry_i(τ,a,b)   = EWMA of realized slippage_vs_arrival_bps of BUY fills in the cell
    c_exit_i(τ,a,b)    = EWMA of realized slippage_vs_arrival_bps of SELL fills in the cell
    RT_i               = c_entry_i + c_exit_i                   (round trip)

`slippage_vs_arrival_bps` already contains the half-spread when an order
crosses, so the half-spread is a **prior**, not an extra term: with too few
fills the cell estimate is shrunk toward `half_spread_i`
(`cost = (n × EWMA + n0 × half_spread_i) / (n + n0)`, `n0 = 50`
[placeholder]). A passive fill can make a cell's realized slippage
smaller than the half-spread (or negative); that is the effect the
experiment measures, and the model is allowed to learn it.

In the optimizer the vectors are used in horizon units
`c = bps / 10000` (a fraction of traded notional). Rules:

- Estimates are per tier and per arm: Bootstrap market fills, Transition
  limit fills, Transition market fills and Standard limit fills are never
  pooled (`SHADOW-ENGINE.md` §6).
- Spread buckets are **quintiles of the universe's median quoted spread**
  (so every bucket is populated), not fixed bps edges.
- A cell with fewer than 50 fills is shrunk as above and falls back to the
  next coarser cell (tier and arm, then tier); with no fills at all the
  estimate is the half-spread alone, flagged `cost_source = prior`.
- EWMA half-life: 30 fills per cell [placeholder].
- Realized slippage, not the quoted spread alone, drives the estimate:
  quotes understate cost for orders larger than the displayed size and
  for fast markets.
- The model is a **proportional** cost. At this account size market
  impact is negligible (§7.4); a square-root impact term is out of scope
  until the participation check in §7.4 starts to bind.

## 6. Netting: One Order Sequence per Name per Session

The order generator nets all intents for a ticker into a single signed
share difference before any order exists. Therefore:

- a ticker never has a buy and a sell in the same session;
- the Transition tier's limit leg and fractional-market remainder are the
  same side by construction;
- no ladder leg for a name is ever started while another sequence for
  that name is working.

This is also a compliance property: with one side per name per session, a
sequence cannot place an order that an earlier order of the same session
blocks.

## 7. Trade-Size and Turnover Control

Cutting the number and size of trades is the surest slippage reduction:
every avoided round trip saves a half-spread twice, and every counted
order costs optionality under the lock.

### 7.1 No-trade band (hysteresis)

After the optimizer, conviction, caps and the regime gate (`08` §5), a
name is traded only if

    |w_target − w_current| > τ_i,    τ_i = max( N_min / NAV , c × RT_i )

where `N_min` is the minimum notional of the tier (§4, and one whole share
in the limit tiers), `RT_i` is the round-trip cost as a fraction of
notional (§5), and `c` is a dimensionless multiplier [placeholder: scan
`{0, 0.5, 1, 2, 4}` in the backtest, choose on CPCV paths, never on a
single path]. Otherwise `w_target := w_current` and the name is not
traded this session.

Honest limits: this band is a hysteresis heuristic; it is not derived from
a utility. The cost-aware optimizer (`08` §5c) already refuses trades whose
marginal gain is below cost, but conviction scaling, renormalization, the
caps and the regime gate move weights afterwards, so the band is applied
last. The band never suppresses a hard-constraint trade: exits forced by
veto, by the cash buffer, or by a cap breach at placement are exempt (a
restricted holding is not an exit: it is never sold by the system).
A position that has drifted slightly above 5% without a lock is held by
the band; it is not a violation (caps apply to orders at placement).

### 7.2 Minimum rebalance interval / event-triggered rebalancing

Per-name minimum interval `rebalance_min_days` [placeholder 5] between
discretionary trades in a name (the lock already makes opposite-side
trades impossible for 5 trading days; this also removes same-side
dribbling). Event triggers bypass it: veto, cap/cash breach, regime gate going
to zero, and a re-validated signal reversal. A **restricted ticker found in the
account is never a trigger**: it is never ordered, in either direction, by any
path (`09-lock-state-machine` §5: halt, CRITICAL, no auto-sell). Whether a longer interval (for example weekly or
fortnightly portfolio-level rebalancing) lowers cost net of tracking error
is a backtest question (`14` §6e), not assumed.

### 7.3 Netting
See §6.

### 7.4 Participation limit

Cap each sequence at `participation_max` [placeholder 1%] of the expected
volume of the window (trailing median volume of the same time bucket).
**At the current account size the limit does not bind and market impact
is negligible**: a full 5% position equals 1% of the $5M median daily
dollar-volume floor only when NAV is about $1M, and below that every order
is a small fraction of a percent of the day's dollar volume. It would
change when NAV grows by one to two orders of magnitude, or when names
near the dollar-volume floor become a large share of the book; then an
impact term in §5 and a multi-session slicing policy become necessary
(and multi-session slicing has lock consequences: a sell slice and a
later buy slice in the same name are opposite-side orders and must be
planned together).

## 8. Universe Liquidity and Spread Filter

In addition to the $5M median daily dollar-volume floor (`02-panel-construction`):

- **Maximum median quoted spread** (bps) `max_median_spread_bps`
  [placeholder 30], measured from quote history, applied as an
  **entry filter**: it removes a name from the set of names that may be
  *bought*. It must not remove a held name from the optimizer: a held name
  that fails the spread filter would otherwise be assigned weight zero and
  liquidated at the worst spread. Held names stay in the optimization, where
  the cost vector (`c_exit` high) prices the sale (`08` §5c).
- **Per-name cost estimate** maintained from quotes and realized fills
  (§5), written to `execution_costs.parquet` (§11.1) after each
  session and read by Julia at 17:00.
- Until at least `N_sessions` [placeholder 20] sessions of recorded quotes
  exist (`quote_spread_1m.parquet`, §2.1), there is no spread filter and cost
  vectors use the prior (`cost_source = prior`). The recorder (§2.1) is the
  prerequisite for this section and for §10; it starts before live trading.
  The median here is the median of the per-minute `spread_bps_median` over the
  trailing sessions.

### 8.1 What the optimizer may know about the lock

The optimizer receives a lock snapshot (blocked side and
`first_permitted` per ticker) so that it does not choose weights the
lock will refuse. The snapshot is **advisory**: it can be stale (an
external-account order after 17:00), so the authoritative check is the
placement-time recomputation from order history in Rust. A target that
fails the placement-time check is dropped, never retried through a
different path.

## 9. Special Sessions (Heuristics to Test, Not Facts)

Each row is a hypothesis that may reduce cost; none is established. Each
is recorded in `gate_reason` / a session tag so it can be tested later
with the pooled regression of §10 once enough tagged orders exist (rare
days will be under-powered; treat their defaults as unvalidated
conservative choices, never as results).

| Situation | Default action to test | Why it might help |
|---|---|---|
| Early close (day after Thanksgiving, Christmas Eve; 13:00 ET, detected from the market-hours data, below) | Window 10:00-12:00, deadline `C - 60 min`, cancel at `C - 10 min` | Thin, shortened session |
| Index rebalance / quarterly expiry (triple witching) / reconstitution close | Window ends 14:30; non-due buys deferred | Closing auction volume dominated by non-discretionary flow |
| FOMC decision day | Deadline 13:45 ET; no placements 13:45-15:00 | Announcement at 14:00 |
| Major scheduled data release at 08:30 | Window opens 10:30 | Open reflects the release |
| Major scheduled data release at 10:00 | No placements 09:55-10:10 | Spike around the print |
| Name-level scheduled event (earnings reported today or the previous evening, **if a calendar exists; verify**) | Defer non-due buys in that name | Gap and wide spread risk |
| First ~15 and last ~10 minutes of any session | No placements (§1.2) | Widest spreads/volatility |

Exception: the deadline fallback may run inside the closing quiet period
only for a due sell and only until 15:50.

Sources of these dates: **early closes and the day's real close time come from
the market-hours data** (`regularMarket.end` from `GET /marketdata/v1/markets/equity?date=`
at the 09:00 refresh for today, cached for the following 30 days; the endpoint's
early-close rendering is UNVERIFIED, live item 10), not from an advisory January
calendar and not from a price file. The scheduled-event rows (FOMC, data
releases, index rebalances, name-level events) have no source yet: until one is
added they are not applied (they are hypotheses to test, listed so that a
source can be chosen later). An error in any of this changes execution timing
only, never the lock (the lock calendar is computed, `09` §4).

## 10. Measurement and the Randomized Window Experiment

### 10.1 Benchmarks

- **Primary: implementation shortfall vs arrival mid.** For a filled
  quantity: `slippage_vs_arrival_bps = dir × (fill_price − arrival_mid) /
  arrival_mid × 10000`, quantity-weighted over all fills of the sequence;
  positive is worse (same sign as `orders.slippage_bps`, `16`/`18`).
- **Unfilled quantity.** The experiment's decision metric is the
  fill-rate-aware shortfall
  `shortfall_bps = fill_rate × slippage_vs_arrival_bps + (1 − fill_rate) × opp_cost_bps`,
  where `opp_cost_bps = dir × (close_of_session − arrival_mid) /
  arrival_mid × 10000` is the opportunity cost of the quantity not
  executed. Opportunity cost is noisy and depends on the return forecast,
  so the filled-part slippage, the fill rate and the shortfall are always
  reported **together**.
- **Secondary: interval VWAP**, the volume-weighted price over the arm's
  window (`slippage_vs_vwap_bps`), from the recorded minute rows
  (`quote_spread_1m.parquet`, §2.1: `last_price` and `volume_delta` per
  minute give a minute-VWAP proxy; no tick data exists, and the stream's
  `CHART_EQUITY` bars are an alternative whose field numbering conflicts between
  Schwab's doc and schwab-py, a live item).
- Optional markout `markout_5m_bps` (mid 5 minutes after each fill vs the
  fill) to see adverse selection.

### 10.2 The arms

Each order sequence is randomly assigned, before its first submission, to
one cell of a balanced 3 × 2 factorial:

| Factor | Levels |
|---|---|
| `window_arm` | `A` 10:00-11:00; `B` 11:30-13:30; `C` 14:00-15:30 |
| `ladder_arm` | `cross` (cross immediately, §3 step 4); `post` (post-then-cross, §3 steps 2-4) |

The arm's window end is its deadline (§2), bounded by the day's `C - 30 min`; on an early-close session the arms are not applied (default window, `gate_reason = special_session:early_close`). One attempt per sequence: a
sequence is not moved to another arm after a deferral.

**Randomization never touches compliance.** The arm decides only the
time of day and the price path of an order that the targets, the lock
check, the restricted check and the gate have *already* approved. Every
arm is legal under the lock: all arms are the same side, in the same
session; none places an opposite-side order; none carries an order over a
session. The assignment is a function of a logged seeded PRNG only,
stratified by tier, side and spread bucket, never of lock status, and
is written to the audit log (`OrderArmAssigned`, seed and draw index)
**before** the first submission. If the arm service fails, the default
arm is used and the failure is logged; execution is never blocked by it,
and the lock check is never skipped by it. Bootstrap uses `window_arm`
only (market orders have no ladder).

### 10.3 Pre-registered decision rule (fixed before the first fill)

- **Unit of analysis:** the order sequence; paired within `(session date,
  side, spread bucket)` strata, with standard errors clustered by session
  date (same-day orders share market-wide moves).
- **Statistic:** difference in mean `shortfall_bps` between levels, from
  a regression of per-sequence shortfall on the arm indicators, with
  stratum fixed effects and the arrival half-spread as covariate
  (cross-name comparability); main effects of the factorial; the
  interaction is examined only when each cell has at least the minimum
  count below.
- **Minimum evidence:** at least `n_min = 100` filled sequences per arm
  level (`[placeholder]`) spread over at least 30 distinct sessions before
  any decision; interim looks every 100 new sequences, at most 5 looks,
  each tested at `0.05 / 5` (so peeking does not inflate error).
- **Decide:** a level wins when the two-sided confidence interval of its
  advantage over each competitor excludes zero (Holm adjustment across the
  three window levels). If the interval lies within `±δ` [placeholder
  1 bp] the levels are declared equivalent and the lower-variance /
  earlier-deadline one is used. If the maximum duration (12 months
  [placeholder]) is reached without a decision, the best point estimate
  is used and reported as undecided.
- **Exploit:** after a decision, 90% of sequences use the winning cell and
  10% continue uniform exploration over all cells (still logged; the
  residual keeps the estimate alive and detects drift). The decision is
  re-tested every quarter from the pooled data.
- **Stratified by tier.** A conclusion drawn in one tier is never applied
  to another.
- **No assumed benefit.** Until the rule fires, the default is the §1.2
  window and the `cross` ladder is the control. No document in this tree
  assumes that any arm is better.

### 10.4 Telemetry

All columns are proposed additions to `orders.parquet` (§11). The
assignment, the gate outcome and the benchmark are computed in Rust at
submission and at the terminal state.

## 11. Merged Proposals (status)

### 11.1 Columns for `orders.parquet` and the `execution_costs.parquet` file

**Merged** into `13-parquet-lake/SCHEMAS.md` #16 (`orders`) and #26
(`execution_costs`), which are normative. The nullable columns and their
meanings are: `arrival_mid`, `spread_bps_at_submit`, `window_arm`, `ladder_arm`,
`time_to_first_fill_s`, `time_to_complete_s`, `n_reprices`, `fill_rate`,
`slippage_vs_arrival_bps`, `slippage_vs_vwap_bps`, `deferral_count`,
`gate_reason` (`ok` / `no_history` / `bad_quote` / `spread` / `stale_move` /
`deadline_gate` / `deadline_not_due` / `special_session:<tag>`), `sequence_id`,
`markout_5m_bps`. Where this file and SCHEMAS.md differ, SCHEMAS.md wins.

Consequence for the other skills (already reflected there):
`orders.slippage_bps` is measured against the mid at **that order's own**
submission, so a replaced order hides drift that accumulated while the ladder
worked. `16-portfolio-ledger` and `18-trade-summary` use
`slippage_vs_arrival_bps` when it is present for cost attribution.

`execution_costs.parquet` is written by Rust after each session and read by
Julia at 17:00 (`date`, `ticker`, `tier`, `ladder_arm`, `spread_bucket`,
`median_spread_bps`, `ewma_slip_bps`, `n_fills`, `cost_buy_bps`, `cost_sell_bps`,
`cost_source`, `sigma_daily`). **Before execution exists the file does not
exist**; the portfolio phase then uses zero cost vectors (`cost_source =
prior`, with the half-spread prior only if a spread estimate exists): see
`08-portfolio-optimization` §5f for the bootstrap defaults.

### 11.2 The daily schedule

The "Daily schedule" paragraph of `outline.md` already carries the decide /
execute split. The slots that the audit added are now in the outline (merged 2026-10-10;
`outline.md` is not edited from this skill):

> Decide after the close of session `t`: prices at 16:10, reconciliation at 16:15,
> tax lots at 16:20, trade summary at 16:22, ledger at 16:25, model inference at
> 16:30, **lock-compliance audit at 16:40, lock snapshot at 16:50**, portfolio
> optimization at 17:00 (targets for session `t+1`), next-session order plan at
> 17:30 (intents only; nothing is sent to Schwab), audit export then backup at
> 18:00. Execute in session `t+1`: pre-open refresh at 09:00 (token, calendar-sync
> including today, lock history refresh, cost and quote-cache warm-up), no
> placements from 09:30 to 10:00, same-day closure check and execution window
> from 10:00 to `C - 30 min`, deadline fallback to `C - 10 min`, no placements
> after that.

`16-portfolio-ledger/LEDGER-PROTOCOL.md` §2 carries these slots.

## 12. Gaps and Things Not Verified

- No intraday quote history exists before the recorder (§2.1) has run;
  `pricehistory` is OHLCV only, so it cannot be backfilled. The spread gate's
  medians, the spread-bucket quintiles, VWAP and markout all need it. Tick-level
  trades are not recorded (no TIMESALE service exists in Schwab's streamer);
  VWAP/markout use the minute rows' `last_price` and `volume_delta`.
- Schwab, all UNVERIFIED and listed in `SCHWAB-API-REFERENCE.md` §8: the
  representation of a replaced order (the new order id is confirmed by the spec
  text, the old order's final status and the link are not), the rate-limit cost
  of a replace, fractional / Stock Slice execution timing and pricing and the
  API minimum notional, the 2-decimal fractional limit (OWNER-REPORTED), REST size units,
  `Security Status` spellings and the absence of a LULD field.
- All `[placeholder]` values are untested.
- The experiment's power depends on the variance of per-order shortfall,
  which has not been measured.

## 13. What Not to Do

- Do not place an order outside the regular session, and do not place
  orders for "tomorrow's open" in the evening.
- Do not cross a bad spread to complete a buy. Cancel it.
- Do not carry an unfilled remainder into the next session.
- Do not let an experiment arm, a retry, or a ladder step produce an
  opposite-side order for the same ticker in the same session.
- Do not submit any order, including a replacement, without its own lock,
  tier and restricted-list checks.
- Do not turn a hold into an opposite-side order by rounding.
- Do not remove a held name from the optimizer because of the spread
  filter; the filter is for entries.
- Do not claim a slippage benefit that the experiment has not reported.
- Do not pool tiers or arms when estimating costs.

## 14. Verification

- [ ] A test asserts no code path submits an order outside 09:30-16:00 ET
      or before the window opens, and none uses `GoodTillCancel` (lock-compliant but not part of this design)
- [ ] A test asserts a spread above `gate_bps_i`, a stale, crossed or
      locked quote, and a zero size each defer the sequence, and that a
      deferred sequence has no order (so none is counted by the lock)
- [ ] A test asserts at the deadline a due sell uses the capped cross and a
      buy is cancelled/never placed
- [ ] A test asserts the cross price never exceeds `X` bps through the
      touch
- [ ] A test asserts a ladder places only one side per ticker per session
      and every replacement obtains a fresh permit
- [ ] A test asserts a rounding artefact is not turned into an
      opposite-side order (blocked side clamps the share difference to 0)
- [ ] A test asserts a restricted ticker never starts a sequence, and a
      blocked attempt is audited CRITICAL
- [ ] A test asserts the arm assignment is logged before the first
      submission, is independent of lock status, and that every arm
      yields only same-side orders
- [ ] A test asserts the stale-move rule skips a buy beyond `k` sigma and
      still sells a due sell with a recomputed quantity
- [ ] A test asserts the no-trade band exempts forced exits
- [ ] A test asserts costs are estimated per tier and arm and never pooled
- [ ] A test asserts the quote recorder writes only regular-hours rows, drops
      locked/crossed/zero-size/delayed/non-`Normal` updates from the spread
      statistics, converts stream sizes from lots to shares, writes carried rows
      with `STALE_CARRY`, records a gap (`quote_recorder_gap`) when the stream is
      down, and falls back to REST polling at `source = rest`
- [ ] A test asserts a stream gap, a security status other than `Normal` or an
      absent quote defers the sequence (the spread gate fails safe), and that
      with fewer than `N_sessions` recorded sessions only `hard_cap_bps`
      applies (`no_history`)
- [ ] A test asserts the stream set never exceeds `stream_cap`, always contains
      held and targeted names, and that the rest is polled over REST
- [ ] A test asserts no fractional order is built outside 09:30-16:00 ET, none
      is a limit order, and a non-eligible or `Unknown` name is skipped in
      Bootstrap (`not_fractional_eligible`)
- [ ] A test asserts fractional quantities are floored to 2 decimals, the
      minimum-notional test uses the floored quantity, and a non-eligible
      Transition name places no remainder order
- [ ] A test asserts Bootstrap uses market orders only through
      `Tier::market_permit`, with the gate applied first
