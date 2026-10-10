---
name: project-constitution
description: Non-negotiable rules for the quant trading system. Always loaded.
  Every other skill inherits from this one. If code violates any rule here,
  it is wrong regardless of what any other document says.
---

# Project Constitution

This file defines invariants. They do not change without an explicit
amendment committed to Git. When any other skill, comment, or instruction
conflicts with this file, this file wins.

## 1. The 5-Day Transaction Lock

The lock is a **contractual obligation** of the account owner, not a risk
preference. No opposite-sided order may be placed within 5 trading days of a
counted order, counting the day that order was placed. Buy on Monday: the
earliest sell is the following Monday. Violating it is a breach of contract,
so it outranks latency, slippage and profit, and the system fails closed.

The lock is a pure function of order history, not a state machine that is
mutated on fills:

1. An order is **counted** if it is open (working) or has at least one
   fill. Cancelled, rejected or expired orders with zero fills are not
   counted.
2. `D(order)` is the order's placement date in America/New_York, regardless
   of time of day, rolled forward to the next session if it was placed on a
   non-trading day. An order that fills later than it was placed (for
   example a resting order) counts from its **last fill date** instead: the
   clock never runs from an earlier day than the fill.
3. A counted order of one side blocks the opposite side from its placement
   day while it is open, and, once filled, through the 4 trading days after
   `D`. The first permitted day is `D + 5 trading days`. An order still open
   blocks with no end date.
4. An opposite-side order may be placed only if no counted order in **any
   account** the owner trades blocks it. A same-direction order is always
   allowed and, once counted, extends the block (buy Monday and Wednesday:
   first sell the following Wednesday).
5. The system is long-only. A sell requires shares held; selling with
   nothing held is a rejected request (`NothingToSell`), not a lock event.
6. Trading days follow the NYSE holiday rules, computed for any year and
   unioned with an overrides set for ad-hoc closures that an automated check
   against Schwab's market-hours data maintains. A date in doubt is treated
   as closed (that can only lengthen a block). The schedule never expires and
   its upkeep never halts trading. Trading days never come from the dates
   present in `prices.parquet`, with one add-only exception: if the
   automated check missed a day, a weekday on which no daily bar exists for
   the broad-market reference set (for example IWF, IWP and SPY) in the
   last 10 sessions is added to the overrides as a closure. It can only add
   a closure (lengthen a block), never remove one.
7. The check is recomputed from order history at submission time, over the
   union of every Schwab account's order history, the audit journal, and the
   owner-maintained external-account file. It is never answered from a
   cached lock row. If any of these cannot be read, is stale, or cannot be
   parsed, every opposite-side order for the affected ticker is blocked.
8. **No exceptions.** No override exists. A veto, regime change, stop,
   optimizer output or manual instruction in the system cannot place an
   order inside a block.

Daily after the close, all order history is rescanned for violations of
rules 1-4. A violation (including one caused by an order placed outside the
system) halts trading and writes a CRITICAL audit row.

See skill `09-lock-state-machine` for the mechanics, examples and tests.

## 2. Point-in-Time Integrity

Every fundamental datum must carry its `filing_date`. The `period_end` is
metadata, never a decision variable.

- A 10-Q filed 2024-02-15 for the quarter ending 2023-12-31 becomes usable
  on 2024-02-15, not 2023-12-31.
- Macro series are forward-filled from their release date, not their
  observation date.
- Cross-sectional ranks are computed per date using only that date's data.
- Any join between price data and fundamental data is an as-of join on
  `filing_date <= date`, never an equality join on `period_end`.

## 3. No Lookahead

No feature, target, calibration window, or evaluation metric may use data
that was not available at the forecast origin.

- Train/validation/test splits are chronological. Never random.
- The TCP calibration window is strictly prior to the forecast date.
- Forward-return targets use only future prices for label construction,
  never for feature construction.
- Backtests use purged and embargoed folds. Overlapping horizons require
  HAC standard errors with lag >= H.
- Any vendor snapshot (for example a quote-summary page) is a snapshot, not a
  time series. It must be stamped at fetch time and used only forward from
  that point.
- TCP and EVT read stored as-of predictions, made by models trained only on
  rows whose labels had matured at the model's cutoff. Predictions are never
  recomputed for past dates with a later model.

## 4. Language Boundaries

Each language owns a specific layer. Do not cross the boundary without
updating the relevant skill.

| Layer                          | Language | Owns                                      |
|--------------------------------|----------|-------------------------------------------|
| Model training, calibration,   | Julia    | GSPQR, TCP, EVT, HMM, conviction, PMPT    |
| portfolio optimization         |          |                                           |
| Execution, risk, lock state,   | Rust     | Order lifecycle, Schwab client, event bus |
| data ingestion                 |          |                                           |
| Frontend, API, visualization   | Bun/TS   | Read-only dashboard, WebSocket push       |

- Rust never trains models. Julia never places orders.
- Data exchange between Julia and Rust uses Arrow IPC. Never JSON for arrays.
- LightGBM models are trained in Julia and exported as `.txt`. Julia is the
  sole writer of `gspqr_predictions`; Rust reads that dataset. Rust may load
  the `.txt` models (for example via `bosk`) only for parity tests and
  on-demand predictions that are never persisted. Never re-train in Rust.
- The frontend has no manual override and places no orders.
- No Python in the repository (code, scripts, tests or tooling).

See skills `12-julia-rust-bridge` and `15-ai-dev-workflow`.

## 5. Parquet Schema Immutability

Once a Parquet schema is written and used, column names and types are frozen.

- Additive changes only. New columns are appended, never inserted.
- Renaming a column requires a migration file and a version bump.
- Changing a dtype (e.g., float32 to float64) requires a migration.
- Every schema change is a separate commit with a clear message.
- The full schema inventory lives in skill `13-parquet-lake`.

If you need a different shape, write a new file. Do not mutate the old one.

## 6. Execution Constraints

- **NAV-tiered order execution.** See §6a. The execution tier is determined
  by the trailing 5-day mean NAV.
- **No shorts.** Long-only. The universe is the individual stocks held by
  the iShares growth ETFs (IWF, IWP), not the ETFs themselves.
- **Cash buffer.** At least 2% of portfolio value remains in cash at all times.
- **Restricted securities.** The system never places an order, buy or sell,
  for a ticker on `config/restricted.toml`. The initial entry is `ICE`,
  which the account owner may not trade because of employer rules and
  blackout periods. A restricted ticker is excluded from the portfolio,
  is never ordered, and a blocked attempt is audited as CRITICAL. Adding or
  removing an entry is a commit approved by the account owner.
- **Position cap.** No single name exceeds 5% of portfolio value.
- **Sector cap.** No GICS sector exceeds 25% of portfolio value.
- **Reconciliation.** Every day at market close, local positions are compared
  against Schwab's reported positions. Mismatch halts trading.

## 6a. NAV-Tiered Order Execution

Three tiers, checked at every order submission:

| Tier | Trailing 5-day mean NAV | Order policy |
|---|---|---|
| **Bootstrap** | < $2,500 | Fractional market orders only. Limit orders are not submitted. |
| **Transition** | $2,500 ≤ NAV < $10,000 | Limit orders are the default. Market orders permitted only for fractional share quantities that cannot be expressed as whole-share limits. |
| **Standard** | NAV ≥ $10,000 | Limit orders only. Market orders are not submitted. |

Rules:
- The tier is checked at order submission time, not cached. A NAV crossing
  a tier boundary takes effect on the next order.
- The tier boundary is inclusive at the lower bound. NAV of exactly $2,500
  is Transition. NAV of exactly $10,000 is Standard.
- Fractional quantities can only be filled as market orders, and only
  during regular trading hours (owner-confirmed). Whole-share quantities
  may use limit orders. Fractional quantities are limited to 2 decimal
  places (owner-reported; verify).
- In the Transition tier, the order generator produces a whole-share limit
  for the integer portion and a fractional market for the remainder.
- In the Standard tier, quantities are rounded down to whole shares and
  the residual is returned to cash.
- The 5-day lock applies in all tiers. A fractional market order is still
  a buy or sell that triggers or resets the lock.
- The order-type enum in Rust contains both `Limit` and `Market` variants.
  The `Market` variant is only constructible inside the Bootstrap tier, the
  Transition tier's fractional path, or one of the two reduce-only exits
  below.
- **Reduce-only exits (amendment 1.13).** (a) In the Standard tier a Market
  order is permitted solely to sell a legacy fractional remainder
  (0 < quantity < 1 share) of a position bought in an earlier tier, as part
  of a full exit of that position. (b) In the Bootstrap tier a reduce-only
  exit of a held position, of any quantity and regardless of fractional
  eligibility, may be placed as a Market order. Both are issued only through
  `Tier::market_permit(reduce_only = true)`, stay subject to the 5-day lock,
  the restricted list, the halt flag and the audit, and never permit a buy.

See skill `10-execution-schwab` §2a for the implementation.

## 7. Veto vs. Conviction

Fundamentals act in two distinct ways. They are never mixed.

- **Veto is binary.** A stock with any active veto trigger is removed from
  the eligible universe before optimization. It is not sized down and it is
  never bought. A held vetoed stock is sold on its first permitted day (the
  5-day lock still applies; there is no exception). Conviction is
  irrelevant.
- **Conviction is bounded.** The multiplier is `clip(1.0 + 0.12 * z, 0.50, 1.25)`.
  It scales post-optimization weights. It never eliminates a position.
- **Valuation is excluded from conviction.** `sp_ratio` is already a GSPQR
  feature. Using valuation again double-counts it.

Veto triggers (any one is sufficient):
- Altman Z-Score < 1.8
- Going-concern qualification in latest 10-K
- Negative book value AND negative operating cash flow
- Pending delisting notice
- Material restatement in trailing 12 months
- Auditor resignation in trailing 12 months
- Debt covenant breach disclosed

Once triggered, a veto persists until a subsequent filing explicitly clears it.

A trigger with no point-in-time data source is inactive and is reported as
inactive in every run. It is never simulated or approximated without saying
so (skill `07-fundamental-overlay` lists which triggers are active).

See skill `07-fundamental-overlay`.

## 8. Audit Log

The audit log is append-only. Never update, never delete. Its system of
record is the append-only audit journal (skill `11-rust-event-bus`); the
`audit_log` Parquet dataset is the export of closed days. The journal is the
record for any day not yet exported.

Every order, lock transition, veto trigger, regime flip, halt and kill-switch
action writes a row. The row contains `event_id`, `timestamp`, `event_type`,
`ticker`, `actor`, `details` (JSON), and `severity`.

If the audit log is unwritable, halt trading. Do not proceed without it.

`portfolio_ledger.parquet` is append-only for new trading days. The
idempotency overwrite on a re-run of the same day is a defensive measure,
not a normal update path. `tax_lots.parquet` permits in-place update on
full close and reduction of `open_quantity` on partial close, as
documented in skill `16-portfolio-ledger`.

## 9. Model Output Contracts

The prediction pipeline produces three outputs per ticker per horizon.
Their names and semantics are fixed.

- `q_hat_lower`, `q_hat_upper` — endpoints of the TCP-calibrated prediction
  interval targeting 95% coverage, built on the raw GSPQR `q_05` and `q_95`
  (a 90% base interval). They are not the 5% and 95% quantiles.
- `q_extreme_lower`, `q_extreme_upper` — EVT 1% and 99% bounds
- `conviction` — bounded multiplier in [0.50, 1.25]

The median forecast used by the optimizer is the GSPQR 50th percentile.
Never substitute the midpoint of the TCP interval. They are not the same.

## 10. When in Doubt

Companion documents: `AMENDMENTS.md` (the amendment log), `ENFORCEMENT.md`
(how each invariant is enforced) and `CROSS-REFERENCE.md` (which skills
implement which invariant).


1. Check this file first.
2. Check the specific skill for the layer you are working on.
3. If neither answers, stop and ask. Do not invent a rule.
4. If a rule here is wrong, propose an amendment as a Git commit. Do not
   silently violate it.

## Verification Checklist

Before committing any change, confirm:

- [ ] No new code uses `period_end` as a decision variable
- [ ] No new code creates a random train/test split on time series
- [ ] No new code submits a market order outside the Bootstrap tier or
      the Transition tier's fractional path
- [ ] No new code places an opposite-side order inside the lock window, or
      answers the lock check from anything but order history (§1)
- [ ] No new code can place an order for a ticker on the restricted list (§6)
- [ ] No new code changes a Parquet schema without a migration
- [ ] No new code mixes veto and conviction logic
- [ ] No new code uses valuation as a conviction component
- [ ] Any new language-boundary crossing is documented in the relevant skill
- [ ] The audit log is written for every order and state change
- [ ] The execution tier is read from `portfolio_ledger.parquet` at
      submission time, not cached
- [ ] The change is committed with a message that references this file if it
      touches any invariant

## Amendment Process

To change this file:

1. Open a branch named `constitution/amendment-<short-description>`.
2. Edit only this file.
3. Commit with a message explaining what invariant changed and why.
4. Review against skills that reference the changed invariant.
5. Merge only after the affected skills are updated in the same PR.

Constitutional changes are rare. If you are proposing one weekly, the
constitution is too large. Move detail into layer-specific skills instead.