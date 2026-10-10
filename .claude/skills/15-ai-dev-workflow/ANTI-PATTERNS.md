# Anti-Patterns

Common mistakes on this codebase, with the fix. If you catch yourself
writing one of these, stop.

## Data

### AP-1: Using `period_end` as a decision variable
**Why it is wrong:** It is lookahead. The data was not public until
`filing_date`.
**Fix:** Always join on `filing_date <= date`. Linter catches this in
`features/`, `models/`, `portfolio/`, `conviction/`. If you are outside those
directories and about to use `period_end`, stop.

### AP-2: Random train/test splits
**Why it is wrong:** Time series data is not exchangeable. Random splits
leak future information into the training set.
**Fix:** Use `features/src/splits.jl` (chronological splits) and
`backtest/src/cpcv.jl` (purged, embargoed CPCV). Never call `train_test_split`
or `KFold` directly; the pre-commit hook allows those identifiers only there.

### AP-3: Forward-filling fundamentals across a filing
**Why it is wrong:** Fills the gap between `period_end` and `filing_date`
with data that was not yet public.
**Fix:** Forward-fill from `filing_date` forward, not from `period_end`.

### AP-4: Computing cross-sectional ranks with fewer than 50 names
**Why it is wrong:** Ranks become coarse and unstable. With Mag7 alone,
the rank is 1–7, which is not a meaningful cross-sectional signal.
**Fix:** Compute ranks only on the full eligible universe (IWF + IWP,
~600 names). Flag any subset computation for review.

## Modeling

### AP-5: Non-chronological calibration windows
**Why it is wrong:** TCP requires the calibration set to be strictly prior
to the forecast origin. Including same-day data breaks coverage guarantees.
**Fix:** Use `date < forecast_date` for calibration. Not `<=`.

### AP-6: Fitting GPD to raw returns
**Why it is wrong:** The EVT layer operates on calibrated residuals, not
raw returns. The residuals are what remains after the model has explained
the central tendency.
**Fix:** Compute residuals from the TCP calibration step. Fit GPD to those.

### AP-7: Using mean/std for cross-sectional standardization
**Why it is wrong:** Fundamentals have fat tails and outliers. Mean/std is
not robust.
**Fix:** Median/MAD, winsorized at ±3.

### AP-8: Adding valuation to conviction
**Why it is wrong:** `sp_ratio` is already a GSPQR feature. Adding valuation
again double-counts it and distorts the interval.
**Fix:** Conviction uses only quality signals (F-Score, ROIC spread, accruals,
margin trend, revenue persistence, revisions). No valuation.

## Execution

### AP-9: Submitting market orders outside the tier rules
**Why it is wrong:** Violates constitution §6a. Market orders accept any
fill price, which is unacceptable for a system with a 5-day lock, so they
are allowed only in the Bootstrap tier and for Transition-tier fractional
remainders.
**Fix:** Build orders through `Tier::market_permit`. Standard-tier orders
are limit orders only; a `MarketOrder` cannot be built without a
`MarketPermit` (`10-execution-schwab` §2a).

### AP-10: Answering the lock from a cached or stored row
**Why it is wrong:** The lock is a contractual obligation. A stored row
(`position_locks`, an in-memory flag, a "locked until" date) can be stale,
cannot see orders placed in the Schwab app or in another account, and a miss
is a breach of contract.
**Fix:** Recompute from order history (all Schwab accounts, the audit journal,
`lock_external.csv`) at submission time via `lock::authorize`, over a REST read
made for that authorize (no TTL cache of history or verdicts). If any source is
unreadable, stale or malformed, block. `position_locks` is a display view only.

### AP-21: Selling inside the window
**Why it is wrong:** The 5-day lock has no exceptions. A veto, a regime
crisis, a stop, a drawdown or an optimizer target does not entitle the system
to sell within 5 trading days of a counted buy (and vice versa). A forced exit
inside the window is a breach of contract, however sensible the risk argument.
A late fill (a GTC order) counts from its last fill date, and an open order
blocks with no end date, so waiting for the clock is the only safe move.
**Fix:** Block the new buys, set the target to 0 (clamped to the current weight
while the lock binds), queue the sell for the first permitted day, and let
`authorize` decide. Never add an override. A veto never cancels a queued sell.

### AP-22: Treating the lock as a risk parameter
**Why it is wrong:** Risk parameters (caps, buffers, regime factors) can be
tuned, traded off against profit, or relaxed in an emergency. The lock cannot:
it is an obligation to a third party. Weakening it for latency, slippage or
P&L, "just for the backtest" or "just in shadow mode", trains the habit that
causes a breach.
**Fix:** Keep it out of configuration and out of the optimizer's objective. It
is a hard gate with fail-closed behaviour; every ambiguity resolves to
"block".

### AP-23: Trading a restricted ticker, or auto-selling one
**Why it is wrong:** The owner is barred from trading the names on
`config/restricted.toml` (`ICE`). An order in either direction is a breach, and
so is a clever "cleanup" sell of a restricted position found in a Schwab account.
**Fix:** Weight 0 in the portfolio, reject in the pre-trade check, CRITICAL
audit row. If a restricted position or order appears in a Schwab account,
halt and report; the owner acts.

### AP-11: Firing pending orders without signal re-validation
**Why it is wrong:** A signal that was correct at queue time may have
reversed by lock expiry. Firing blindly executes a stale decision.
**Fix:** Re-fetch the latest signal on the first permitted day and call
`authorize` again. Cancel if direction reversed (a veto never cancels a queued
sell).

## Infrastructure

### AP-12: Polling where events belong
**Why it is wrong:** Wastes CPU, introduces latency, and violates the
event-driven architecture.
**Fix:** Subscribe to the event bus. Polling is only for reconciliation
jobs, which run on a fixed schedule by design.

### AP-13: Unbounded channels
**Why it is wrong:** Masks backpressure. Under load, memory grows until
OOM.
**Fix:** Bounded channels with explicit overflow policy (drop, block, or
fail). The policy is documented at the call site.

### AP-14: `unwrap()` in library code
**Why it is wrong:** Panics on unexpected input. In an execution system,
a panic is a trading halt at best and a partial fill at worst.
**Fix:** `?` with typed errors via `thiserror`. `unwrap()` is only allowed
in `main.rs` and tests.

### AP-15: JSON for array exchange across FFI
**Why it is wrong:** Slow, lossy, and loses dtype information.
**Fix:** Arrow IPC. Both Julia (`Arrow.jl`) and Rust (`arrow`) support it
natively. Zero-copy where possible.

## Working with Claude

### AP-16: Editing files without reading them
**Why it is wrong:** The file may have changed since your last context.
Editing based on memory produces incoherent diffs.
**Fix:** Read before edit. Always.

### AP-17: Claiming a task is done without running tests
**Why it is wrong:** The verification protocol exists because LLM-generated
code is often plausible-looking but wrong.
**Fix:** Run the tests. If you cannot run them, say so and mark the task as
unverified, not done.

### AP-18: Weakening a check to make a test pass
**Why it is wrong:** The check encodes an invariant. Weakening it to pass
your code inverts the purpose of the test suite.
**Fix:** Fix the code. If the check is genuinely wrong, propose a skill
amendment. Do not edit the check inline.

### AP-19: Improvising when a task does not fit the skills
**Why it is wrong:** The skills cover the system. A task that does not fit
is either miscategorized or requires a new skill. Improvising produces code
with no rules and no review.
**Fix:** Stop and ask. The user decides whether the skill is missing or the
task is out of scope.

### AP-20: Reading skills after writing code
**Why it is wrong:** The skill defines the constraints. Writing first and
checking later means rewriting or, worse, shipping a violation.
**Fix:** Read the layer skill before writing. If you forgot, read it now
and audit the code you already wrote.

### AP-24: Treating an UNVERIFIED Schwab fact as certain
**Why it is wrong:** The Schwab developer portal is login-gated; the facts in
`10-execution-schwab/SCHWAB-API-REFERENCE.md` come from mirrors, client source and
community reports. Several are open (the order-history window of 60 days vs 1
year, whether `ACCT_ACTIVITY` covers every linked account, whether fractional
quantities are accepted, how replace and partial-fill-then-cancel are
represented). Code built on a guess can under-count a counted order, which is a
breach of the lock.
**Fix:** Keep the label. Count unknown values, re-read over REST, fail closed, and
add the question to the live tests.

### AP-25: Authorizing from the stream
**Why it is wrong:** The Streamer can miss, reorder or duplicate messages, its
account coverage is unverified, and its timestamps have an unknown zone.
**Fix:** The stream is a dirty trigger and early warning. REST is the anchor; a REST failure denies the
order, whatever the stream shows.
