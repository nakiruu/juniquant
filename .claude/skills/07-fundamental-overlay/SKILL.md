---
name: fundamental-overlay
description: Veto and conviction layer for fundamentals. Load when touching
  conviction/ (src/veto.jl, f_score.jl, altman_z.jl, components.jl,
  standardize.jl), scripts/compute-overlay.jl, or any code computing F-Score,
  Altman Z, or the conviction multiplier.
---

# Fundamental Overlay

The fundamental overlay has two independent jobs. They are separate
functions, separate outputs, and separate database tables.

## 1. The Two Jobs

| Job | Output | Type | Applied |
|---|---|---|---|
| **Veto** | `veto_active` | Boolean | Before optimizer |
| **Conviction** | `conviction` | Float [0.50, 1.25] | After optimizer |

**Veto is survival.** A stock with an active veto is removed from the
eligible universe before optimization. It is not sized down and never bought.
Conviction is irrelevant for vetoed stocks. If the name is **held**, its target
is 0 and the position is sold like any other sell: on the first permitted day
if the 5-day lock blocks it today (queued, never cancelled by the veto); the
veto never overrides the lock. A vetoed name that is not held has no target
row.

**Conviction is quality.** A stock with high conviction gets a larger
position. A stock with low conviction gets a smaller one. Conviction
never eliminates a position.

## 2. Why They Are Separate

Merging them would be a mistake. A single "fundamental score" that both
vetoes and sizes creates two problems:

1. **The cliff problem.** A stock at F-Score 4 is approved; F-Score 3 is
   rejected. There is no economic discontinuity at that boundary.

2. **The exposure problem.** A soft multiplier cannot protect against
   gap-down risk. If a company has a going-concern warning, a 0.7×
   multiplier still leaves you exposed to a 60% overnight drop for as long
   as the position is held. A veto stops new buys at once. A held name that
   is vetoed is sold on its first permitted day under the 5-day lock; the
   lock is a contractual obligation and **no exception exists** for a veto
   (constitution §1, amendment 1.4).

The separation solves both. Veto is binary and absolute. Conviction is
smooth and bounded.

## 3. Veto Rules

See `VETO-RULES.md`. Seven triggers, any one sufficient:

| Trigger | Threshold | Source |
|---|---|---|
| Altman Z-Score | < 1.8 | 10-Q / 10-K |
| Going-concern qualification | Any | 10-K audit opinion |
| Negative book + negative OCF | Both | 10-Q / 10-K |
| Pending delisting notice | Any | Exchange filing |
| Material restatement | Trailing 12 months | 8-K |
| Auditor resignation | Trailing 12 months | 8-K |
| Debt covenant breach | Any | 10-Q / 10-K |

Vetoes are checked on every filing event and at every daily close. Once
triggered, a veto persists until a subsequent filing explicitly clears
it.

## 4. Conviction Components

See `CONVICTION.md`. Six components, all quality signals:

| Component | Direction | Weight |
|---|---|---|
| Piotroski F-Score | Higher = better | 0.25 |
| ROIC − WACC spread | Higher = better | 0.20 |
| Accruals ratio | Lower = better | 0.20 |
| Gross margin trend | Improving = better | 0.15 |
| Revenue growth persistence | Higher = better | 0.10 |
| Analyst revision breadth | Higher = better | 0.10 |

**Valuation is excluded.** `sp_ratio` is already a GSPQR feature. Adding
valuation to conviction would double-count it.

## 5. The Conviction Formula

For each stock on each rebalance date:

1. Compute each component's cross-sectional z-score (median/MAD,
   winsorized at ±3).
2. Weighted sum: `composite_z = Σ w_c · z_c`
3. Map to multiplier:
   `conviction = clip(1.0 + 0.12 · composite_z, 0.50, 1.25)`

The 0.12 coefficient means a 1-standard-deviation improvement in
composite quality translates to roughly a 12% position size increase.

## 6. Application Order

```
GSPQR → TCP → EVT → VETO FILTER → Optimizer → CONVICTION SCALE → Regime Gate → Lock Gate → Execution
```

Veto runs **before** the optimizer: a vetoed name is not an optimizer variable.
The portfolio layer then keeps each **held** vetoed name in the output with
target 0 plus the lock clamp (`08-portfolio-optimization` §5b-2): buys are
blocked immediately, the sell goes through the normal generator and is queued
for the first permitted day if the lock blocks it today; a veto never cancels a
queued sell and never forces a sale inside the window
(`09-lock-state-machine/PENDING-ORDERS.md` §4). Conviction runs **after**.
The order matters:

- Veto removes stocks entirely, so the optimizer does not allocate to them.
- Conviction scales the optimizer's output, so the optimizer's constraints
  (5% cap, 25% sector) are applied to the unscaled weights.

If conviction were applied before optimization, a high-conviction stock
could push past the 5% cap after renormalization.

## 7. What Not to Do

- Do not merge veto and conviction into one score.
- Do not use valuation as a conviction component.
- Do not compute conviction with mean/std. Use median/MAD.
- Do not apply conviction before the optimizer.
- Do not skip the veto check on filing events.
- Do not let a veto persist after a filing that explicitly clears it.
- Do not sell a vetoed held name inside the 5-day lock. There is no veto
  exception; queue the sell for the first permitted day.

## 8. Verification

- [ ] A test asserts a vetoed stock is not an optimizer variable, gets no buy,
      and has target 0 unless lock-bound; when lock-bound the target is clamped
      to the current weight and the sell is queued for the first permitted day
      (a veto never cancels it)
- [ ] A test asserts conviction stays in [0.50, 1.25]
- [ ] A test asserts conviction components use median/MAD
- [ ] A test asserts valuation is not a conviction component
- [ ] `conviction_scores.parquet` and `veto_flags.parquet` schemas match
- [ ] A test asserts vetoes clear on subsequent filings

## 8a. Scripts

`scripts/compute-overlay.jl` is a SKETCH (not compiled, not normative; the real
code lives in `conviction/src/`, `scaffold.md`). It writes `veto_flags.parquet`
and `conviction_scores.parquet` with the column names of
`13-parquet-lake/SCHEMAS.md` #11 and #12 (`rev_persist_z`, `revision_z`,
`filing_date`). Several veto triggers and two conviction components are
placeholders in it; they are listed at the top of the file.

## 9. Related Skills

- `00-construction` §7 — veto vs conviction separation
- `03-gspqr-model` — the 12-feature list (what conviction must not duplicate)
- `08-portfolio-optimization` — consumes both outputs
- `13-parquet-lake` — schemas for both tables