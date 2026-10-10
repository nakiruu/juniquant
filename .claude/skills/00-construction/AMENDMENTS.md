# Constitution Amendment Log

Append-only. Every change to SKILL.md adds a row. Never edit a past row.

| Version | Date       | Invariant | Change                          | Rationale                          | PR  |
|---------|------------|-----------|---------------------------------|------------------------------------|-----|
| 1.0     | 2026-10-10 | all       | Initial ratification            | Project inception                  | #1  |
| 1.1     | 2026-10-10 | §5, §8    | Permit in-place update of `tax_lots.parquet` on full close; permit reduction of `open_quantity` on partial close with a new row for the closed portion | FIFO lot tracking requires mutating the open lot when partially closed. This is the only sanctioned update path for a non-audit file. `audit_log.parquet` remains strictly append-only. `portfolio_ledger.parquet` is append-only for new days. | #2 |
| 1.2     | 2026-10-10 | §6        | Replace "Limit orders only" with a three-tier NAV-based execution model. Under $2,500 NAV: fractional market orders only. $2,500–$10,000 NAV: limit orders are the majority; market orders permitted for fractional shares only. Above $10,000 NAV: limit orders only. | Schwab rounds limit orders up to whole shares and enforces minimums that a sub-$1,000 account cannot meet. The tiers make the system operable at small capital while preserving the limit-order discipline at scale. | #3 |
| 1.3     | 2026-10-10 | §6        | Clarify the tier model ratified in 1.2, no change of intent: Transition is $2,500 ≤ NAV < $10,000 and Standard is NAV ≥ $10,000 (the 1.2 table said "> $10,000" while its rules said exactly $10,000 is Standard); Standard-tier quantities round down. | The 1.2 text contradicted itself at exactly $10,000. Rounding to nearest could round a Standard-tier order past the 5% cap or into the 2% cash buffer; rounding down keeps both and returns the residual to cash, as skill 08 §6b assumes. | owner-approved 2026-10-10 |
| 1.4     | 2026-10-10 | §1        | Replace the five-state lock model by a rule over order history: counted orders (open or with a fill) block the opposite side from their placement day through 4 trading days after the placement date, or after the last fill date if the fill is later, across every account; recomputed at submission; fail closed; no exceptions. Remove the proposed veto-forced-liquidation override. | The lock is a contractual obligation of the account owner. The state model had no definition for partial sells, started the clock at the first fill, unlocked one trading day later than the contract (so it was too conservative, not too lax), could not see manual or external-account orders, and derived trading days from prices, which cannot contain future holidays. Trading days are now computed from the NYSE holiday rules plus automated overrides, so the schedule never expires and never halts trading. | owner-approved 2026-10-10 |
| 1.5     | 2026-10-10 | §6        | Add restricted securities: the system never places an order, in either direction, for a ticker on `config/restricted.toml`. Initial entry: `ICE`. | The account owner is barred from trading ICE and has blackout periods. A breach is the owner's, so the exclusion must be mechanical. | owner-approved 2026-10-10 |
| 1.6     | 2026-10-10 | §6        | Wording corrections, no change of intent: the universe is the individual stocks held by IWF and IWP (not the ETFs); fractional quantities fill only as market orders in regular hours (owner-confirmed), limited to 2 decimal places (owner-reported); the unverified claim that Schwab rounds limit orders up to whole shares is dropped. | The 1.2 text said "the universe is ETFs", which contradicts the outline and the stock-level model, and stated a Schwab rounding behavior nothing confirms. | owner-approved 2026-10-10 |
| 1.7     | 2026-10-10 | §9        | Wording: `q_hat_lower`/`q_hat_upper` are the endpoints of a 95%-coverage TCP interval built on the raw 90% base interval, not the 5%/95% quantiles. | The old wording was wrong and led to scoring the endpoints with pinball loss. | owner-approved 2026-10-10 |
| 1.8     | 2026-10-10 | §7        | A vetoed stock is never bought; a held vetoed stock is sold on its first permitted day under the lock. A trigger with no point-in-time source is inactive and reported as inactive, never simulated. | "Not traded" read literally forbade the exit; four triggers have no free point-in-time source. | owner-approved 2026-10-10 |
| 1.9     | 2026-10-10 | §4        | Frontend row loses "manual override" (read-only dashboard). Julia is the sole writer of `gspqr_predictions`; Rust model loading is for parity tests and non-persisted use. No Python in the repository. | Contradictions with §1 rule 8 and with the single-writer rule; owner decision (no Python). | owner-approved 2026-10-10 |
| 1.10     | 2026-10-10 | §3        | Vendor snapshot wording replaces the yfinance `Ticker.info` sentence; TCP and EVT read stored as-of predictions from models trained only on matured rows. | yfinance is no longer used; the as-of rule was implicit. | owner-approved 2026-10-10 |
| 1.11     | 2026-10-10 | §8        | The audit journal is the system of record; the `audit_log` dataset is the export of closed days. "Manual override" replaced by halt and kill-switch actions. Companion documents linked from §10. | The audit design moved to a journal plus export; no manual override exists. | owner-approved 2026-10-10 |
| 1.12     | 2026-10-10 | §1        | Rule 6 gains an add-only exception: a weekday with no daily bar for the broad-market reference set in the last 10 sessions is added as a closure if the automated check missed it. | Owner decision: fill missed closure data. Adding a closure can only lengthen a block. | owner-approved 2026-10-10 |
| 1.13    | 2026-10-10 | §6a       | Reduce-only exits: (a) in the Standard tier a Market order may sell only a legacy fractional remainder (0 < qty < 1 share) as part of a full exit; (b) in the Bootstrap tier any reduce-only exit of a held position may be a Market order. Lock, restricted list, halt flag and audit still apply; never a buy. | Without it, fractional remainders in Standard and whole-share holdings in Bootstrap had no legal order and could never be exited (including vetoed names). | owner-approved 2026-10-10 |

## Amendment Template

Copy this into a new row when proposing a change:

| Version | Date | Invariant | Change | Rationale | PR |
|---|---|---|---|---|---|
| x.y | YYYY-MM-DD | §N | one-line description | why the old rule failed | #NN |

## Rules for Amending

- One invariant per amendment. Do not bundle.
- The PR must update every skill listed in CROSS-REFERENCE.md for that invariant.
- If the amendment removes a rule, state what replaces it. No silent deletions.
- If more than 2 amendments land in a quarter, the constitution is carrying
  detail that belongs in layer-specific skills. Move it.

## Deprecation Log

Rules that were once constitutional but were demoted to skills. Keep this
list so future readers understand why the constitution is short.

| Rule (removed) | Date demoted | Now lives in |
|---|---|---|
| (none yet) | — | — |

## Amendment Detail: 1.1

**Invariants affected:** §5 (Parquet schema immutability, extended to append-only), §8 (Audit log).

**Change:** `tax_lots.parquet` may be updated in place on a full close and may have `open_quantity` reduced on a partial close, with a new row written for the closed portion.

**Scope:** Only `tax_lots.parquet`. `audit_log.parquet` remains strictly append-only. `portfolio_ledger.parquet` is append-only for new days, with the idempotency overwrite being a defensive measure on re-run, not a normal update path.

**Skills updated in the same PR:**
- `16-portfolio-ledger` (defines the update pattern)
- `13-parquet-lake` (schema for `tax_lots.parquet`)

## Amendment Detail: 1.2

**Invariant affected:** §6 (Execution constraints).

**Change:** The rule "Limit orders only. Never submit a market order to Schwab" is replaced by a three-tier NAV-based execution model.

**Tiers:**
- Bootstrap (< $2,500): fractional market orders only
- Transition ($2,500–$10,000): limit orders default; market orders for fractional remainder only
- Standard (> $10,000): limit orders only

**Scope:** The tier is determined by the trailing 5-day mean NAV, read at order submission time, not cached. The 5-day lock applies in all tiers.

**Skills updated in the same PR:**
- `10-execution-schwab` (§1, §2, §2a, §6)
- `10-execution-schwab/ORDER-TYPES.md` (§1, §1a, §8, §9)
- `10-execution-schwab/SHADOW-ENGINE.md` (§2, §5, §6, §7)
- `08-portfolio-optimization` (§6, new §6a and §6b)
- `14-backtest-protocol` (§6, new §6a, §6b, §6c)
- `16-portfolio-ledger` (§1, §5, §8)
- `11-rust-event-bus/EVENT-TYPES.md` (`OrderSubmitted`, `OrderFilled`)

## Amendment Detail: 1.3

**Invariant affected:** §6 (Execution constraints), §6a (NAV-tiered order execution).

**Change:** Clarification of amendment 1.2. (a) The Transition/Standard boundary is inclusive at $10,000 on the Standard side in the table as well as in the rules. (b) The Standard tier rounds quantities down.

**Scope:** `00-construction/SKILL.md` §6a only. No order type gains or loses permission. The tiers' trailing 5-day mean NAV input, the lock, and the audit requirements are unchanged.

**Skills updated in the same change:**
- `10-execution-schwab` (§1, §2, §2a, §6–§9, `ORDER-TYPES.md`)
- `08-portfolio-optimization` (§6a, §6b)
- `14-backtest-protocol` (§6a)
- `11-rust-event-bus/EVENT-TYPES.md` (`OrderSubmitted`, `OrderFilled`)
- `13-parquet-lake/SCHEMAS.md` (fractional quantity types, `orders.tier`)
- `16-portfolio-ledger` (lot quantities)
- `00-construction/ENFORCEMENT.md` §6 and `scripts/pre-commit-constitution.sh`

**Status:** merged 2026-10-10 on the account owner's instruction (the owner is the contract-bound party and the sole reviewer of this repository, so this stands in for the §10 branch and two approvals).

## Amendment Detail: 1.4

**Invariant affected:** §1 (The 5-Day Transaction Lock), with pointers in the Verification Checklist.

**Trading days (added before merge):** rule 6 now computes the NYSE schedule from its holiday rules for any year and unions it with automation-maintained overrides (`calendar-sync`, `09-lock-state-machine` §4); the earlier checked-in table with a coverage range is withdrawn.

**Change:** The state diagram (IDLE / LONG_LOCKED / LONG_UNLOCKED / FLAT_LOCKED / FLAT_UNLOCKED) is replaced by a rule over order history. See §1 for the text. Compliance answers that fixed the details (2026-10-10): the contract counts the placement day, and a fill later than the placement counts from the fill; trading days; an order placed after the close counts as that day; cancelled and rejected orders do not count; every account the owner trades is covered.

**Withdrawn:** the veto-forced-liquidation override proposed in the earlier 1.4 draft. It would breach the contract.

**Skills updated in the same change:** `09-lock-state-machine`, `10-execution-schwab`, `11-rust-event-bus`, `07-fundamental-overlay`, `08-portfolio-optimization`, `13-parquet-lake`, `14-backtest-protocol`, `15-ai-dev-workflow`, `18-trade-summary`, `ENFORCEMENT.md`, `CROSS-REFERENCE.md`, `outline.md`, `scaffold.md`.

**Status:** merged 2026-10-10 on the account owner's instruction (see 1.3).

## Amendment Detail: 1.5

**Invariant affected:** §6 (Execution constraints).

**Change:** New bullet, "Restricted securities". `config/restricted.toml` lists tickers the system must never order in either direction. Initial entry: `ICE`.

**Scope:** enforced at three layers: the portfolio eligibility filter (weight 0), the Rust pre-trade check (reject and audit CRITICAL), and a test that `ICE` is present in the file. Data ingestion and the model panel are unchanged; the exclusion is at the tradable layer so cross-sectional ranks are not altered.

**Status:** merged 2026-10-10 on the account owner's instruction.

## Amendment Detail: 1.6

**Invariant affected:** §6 (Execution constraints, universe sentence) and §6a (tier rationale bullet).

**Change:** Wording only. (a) "The universe is ETFs of growth equities" becomes "the individual stocks held by IWF and IWP". (b) "Schwab rounds limit orders up to whole shares" is replaced by the owner-confirmed rules that fractional quantities fill only as market orders in regular hours, with a 2-decimal precision limit that is owner-reported and still to be verified. No order type gains or loses permission.

**Skills updated in the same change:** `10-execution-schwab` (SKILL, ORDER-TYPES), `08-portfolio-optimization`, `14-backtest-protocol`, `outline.md`.

**Status:** merged 2026-10-10 on the account owner's instruction (see 1.3).

## Amendment Detail: 1.7-1.12

Approved together by the account owner on 2026-10-10 (the owner is the sole reviewer; this stands in for the §10 branch and approvals). Each row amends one section. The note in "Rules for Amending" about more than 2 amendments per quarter applies: the next change should move detail (for example the §6a tier table) out of the constitution into skill 10 and record it in the deprecation log.

Skills updated in the same change: 03, 04, 05, 07, 09, 11, 12, 14, 17, CROSS-REFERENCE.md, ENFORCEMENT.md, outline.md.

## Amendment Detail: 1.13

Approved by the account owner on 2026-10-10 after an explanation of the two gaps (decision A1). Implemented in `10-execution-schwab` §2a (`Tier::market_permit(reduce_only = true)`), `ORDER-TYPES.md`, `14-backtest-protocol` (simulator), and `outline.md`.
