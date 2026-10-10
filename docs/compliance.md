# Compliance: the 5-day lock and restricted securities

Plain-language summary for the account owner. The normative text is the constitution
(§1 and §6, [`../.claude/skills/00-construction/SKILL.md`](../.claude/skills/00-construction/SKILL.md));
the mechanics are in [`09-lock-state-machine`](../.claude/skills/09-lock-state-machine/SKILL.md).
If this page and the constitution ever disagree, the constitution wins.

## The 5-day lock

The owner may not place an opposite-sided order within 5 days of an order, counting the day
the order was placed. It is a contractual obligation, so the system treats it as absolute
and fails closed: when in doubt, it does not trade.

**The rule, as implemented:**

- An order **counts** if it is still open or has any fill. An order cancelled, rejected,
  expired or replaced with **zero fills does not count**.
- A counted order blocks the **opposite side** from its placement day. Once filled, the block
  lasts through 4 trading days after its **last fill day**; the 5th trading day after it is
  the first permitted day. An order that is still open blocks with no end date.
- Orders on the **same side** are always allowed and extend the block.
- **Trading days**, not calendar days: weekends, NYSE holidays and ad-hoc closures are skipped.
- **Every account** counts: all Schwab accounts, plus orders the owner records for other
  accounts (for example Fidelity) in `$QUANT_STATE/lock_external.csv`.
- **Share classes of one company are one lock group:** buying GOOGL blocks selling GOOG.
- **No exceptions:** no override, no veto exit, no stop order can break a block.

**Examples:**

| Event | First permitted opposite order |
|---|---|
| Buy Monday 12 Oct 2026 | Monday 19 Oct |
| Buy Monday 12 and Wednesday 14 Oct | Wednesday 21 Oct |
| Buy Monday 23 Nov 2026 (Thanksgiving week) | Tuesday 1 Dec |
| Order placed Monday, filled Thursday 15 Oct | Thursday 22 Oct |
| Buy order placed Monday, cancelled with no fill | Not blocked |
| Buy in Fidelity on Wednesday 14 Oct | Sell in Schwab blocked until Wednesday 21 Oct |

These and about 3,000 more cases are golden fixtures
(`09-lock-state-machine/fixtures/lock_cases.json`); every implementation must reproduce them.

**How it is enforced (once execution exists):**

- Before every order, the system re-reads order history from Schwab (no cached answers),
  checks the lock, the restricted list and the halt flag, and records the intent in the audit
  journal before sending.
- Every evening a compliance audit rescans all order history and writes a dated report. A
  violation (for example from a manual order in the Schwab app) halts trading until the owner
  reviews it.
- The NYSE calendar is computed from the holiday rules and a list of ad-hoc closures that is
  maintained automatically and can only grow. A missed closure can only make a block longer.

## Restricted securities

- **ICE is never ordered, buy or sell** (`config/restricted.toml`). The owner holds ICE only
  in Fidelity. ICE found in a Schwab account halts trading and is never sold automatically.
- Adding or removing a restricted ticker is a change the owner approves.

## Reading a compliance report

Reports are written daily to `$QUANT_STATE/compliance/`. The outcome is `COMPLIANT`,
`VIOLATION` or `INCOMPLETE` (data could not be verified, for example an expired login).
`INCOMPLETE` is not a violation; it halts until a clean re-run. A `VIOLATION` halts until the
owner acknowledges it.
