# Shadow Execution Engine

> **Phase: later.** Execution and live trading come last in the build order
> (`outline.md` "Project phase and build order"). Kept as the specification
> for that phase; do not build or refine it unless asked. Code blocks are
> SKETCHES (not compiled, not normative).

Because no Schwab paper or sandbox environment was found in any source
consulted (UNVERIFIED as an absolute; the developer portal is login-gated), the
adapter runs a shadow engine that simulates fills and compares them against real fills
after deployment.

## 1. What It Does

For every order the adapter is about to submit:

1. Read the current quote (bid, ask, last) from the quote cache (Streamer
   `LEVELONE_EQUITIES`, or REST for names outside the stream set;
   `EXECUTION-WINDOW.md` §2.1) and record the **arrival mid**
   (`EXECUTION-WINDOW.md` §3 step 1) and the quoted spread in bps. The shadow
   engine and the quote recorder share the same quote cache, and the slippage
   model's spread-bucket quintiles come from `quote_spread_1m.parquet`.
2. Simulate the fill using a tier-specific, arm-specific, spread-bucket
   slippage model (§2).
3. Record the simulated fill in `shadow_fills.parquet`.
4. Submit the real order.
5. When the real fill arrives, compare.

## 2. The Slippage Model

**Benchmark.** All slippage is measured against the **arrival mid**
(`EXECUTION-WINDOW.md` §10.1), the mid at the quote read that started the
sequence, not against the mid at the submission of each individual leg.
For a single immediate order the two coincide. For a ladder they do not:
a leg resubmitted later is measured against a later mid, which hides the
drift that accumulated while the ladder worked. `orders.slippage_bps` is
the per-order figure; `slippage_vs_arrival_bps` is the benchmark.
Positive is worse than the benchmark (same sign as `16`/`18`).

**Model key.** The parameters are indexed by `(tier, order_kind,
ladder_arm, spread_bucket)`. Spread buckets are quintiles of the
universe's median quoted spread (`EXECUTION-WINDOW.md` §5). A cell with
fewer than 50 real fills uses the next coarser cell (drop the bucket,
then the arm) and, with none, the prior below.

    mid        = arrival mid
    dir        = +1 buy, -1 sell
    half_sp    = (ask_arrival - bid_arrival) / 2    (price units)

### Standard tier (limit orders)

`ladder_arm = cross` (cross immediately with the capped marketable limit):

    simulated_fill = mid + dir × (half_sp + adj(cell))

`adj(cell)` is the calibrated median of `(real slippage - half spread)`
in the cell, in price units. The prior, before any fills in the cell, is
`adj = 0`: the order is assumed to pay the half-spread.
(The previous fixed `slippage_bps = 5` default is retired as the prior
because it ignored the name's own spread. It stays available as
`legacy_fixed_5bps` for comparison.)

`ladder_arm = post` (post, reprice, then cross):

    E[simulated_fill] = mid + dir × ( p × passive_offset + (1 - p) × (half_sp + drift + adj) )

where `p` is the probability that the passive stage fills before the
deadline, `passive_offset` is the passive limit's distance from the mid
(`<= 0` for a price at or inside the mid, in the direction that favours
us), and `drift` is the adverse move of the mid while working (adverse
selection). **`p` and `drift` are not known.** Until the experiment
(`EXECUTION-WINDOW.md` §10) provides fills in the `post` arm, the shadow
engine reports a **bracket**, never a single optimistic number:

| Variant | p | Reading |
|---|---|---|
| pessimistic | 0 | The passive stage never fills: same as `cross` |
| optimistic | 1, `drift = 0` | Always fills at the passive price: an upper bound on the benefit, not an expectation |

The real fill must lie inside the bracket; a fill outside it by more than
the divergence threshold of §3 is a data or adapter problem. When the arm
has 50 real fills, `p` and `drift` are estimated per cell and the engine
reports the calibrated number next to the bracket.

### Transition tier (limit orders)

Same keys and arms as Standard, calibrated on Transition-tier limit fills
only.

### Transition tier (fractional market orders)

    simulated_fill = direction == Buy ? ask : bid

The market order crosses the spread. The fill is at the quoted ask for
buys and the quoted bid for sells. No slippage parameter is applied
beyond the spread itself, because the spread is the cost. (Whether
Schwab's fractional execution honours the NBBO at the moment of
submission is unverified, `EXECUTION-WINDOW.md` §4; the divergence check
is how it is learned. Fractional orders are market "not held" orders
placed and filled only in regular hours; the terms (COMMUNITY) allow time and
price discretion and a delay before the fill shows in the account, so the
shadow engine records the **fill time** from REST `executionLegs[].time` and
compares it with the submission time; a long, variable latency is a
finding, not an error. The API accepting fractional quantities is
OWNER-CONFIRMED; the shadow engine floors simulated fractional quantities to
2 decimals like the real order generator (`SKILL.md` §2a, §2b), and simulates
Transition remainders and Bootstrap trades only for fractional-eligible names.)

### Bootstrap tier (market orders)

    simulated_fill = direction == Buy ? ask : bid

Same as the Transition fractional case. The full spread is the cost.

## 3. Divergence Detection

After the real fill:

    divergence_bps = dir × (real_fill - simulated_fill) / arrival_mid × 10000

If `abs(divergence_bps) > 20`, append a `WARN` row to the audit log. If
`abs(divergence_bps) > 50` for 3 consecutive orders, halt trading and
require manual review.

Divergence is also reported **per arm and per spread bucket**: a model
that is right on average but wrong in the widest-spread bucket is the
failure the spread gate is meant to prevent. The threshold is the same
for all tiers. A market order that diverges by
more than 50 bps from the quoted spread is unusual and indicates either a
stale quote or a data problem.

## 4. What the Shadow Engine Is Not

- It is not a backtest. It runs on live quotes.
- It is not a substitute for real testing. It is a guardrail.
- It does not simulate partial fills. Real fills may be partial; the
  shadow engine assumes full fill at the simulated price.
- It does not model queue position for limit orders. A limit order at the
  bid may or may not fill depending on order flow. For the `post` arm the
  engine therefore reports the pessimistic/optimistic bracket of §2 rather
  than assuming a fill; for the `cross` arm it assumes the capped
  marketable limit fills at the touch plus the calibrated adjustment.

## 5. Schema

`shadow_fills.parquet`:

| Column | Type | Notes |
|---|---|---|
| `order_id` | `utf8` | FK to orders |
| `ticker` | `utf8` | |
| `side` | `utf8` | `Buy` / `Sell` |
| `quantity` | `float64` | Fractional permitted |
| `order_type` | `utf8` | `Limit` / `Market` |
| `tier` | `utf8` | `Bootstrap` / `Transition` / `Standard` |
| `bid_at_submit` | `float64` | |
| `ask_at_submit` | `float64` | |
| `mid_at_submit` | `float64` | |
| `simulated_fill` | `float64` | |
| `slippage_bps` | `float64` | Realized slippage vs mid |
| `real_fill` | `float64` | Null until filled |
| `divergence_bps` | `float64` | Null until filled |
| `arrival_mid` | `float64` | Appended; nullable. Benchmark of §2 |
| `spread_bps_at_submit` | `float64` | Appended; nullable |
| `spread_bucket` | `int32` | Appended; nullable. Quintile index 1-5 |
| `window_arm` | `utf8` | Appended; nullable. `A` / `B` / `C` or null |
| `ladder_arm` | `utf8` | Appended; nullable. `cross` / `post` / `market` |
| `model_variant` | `utf8` | Appended; nullable. `calibrated` / `bracket_pessimistic` / `bracket_optimistic` / `legacy_fixed_5bps` |

The six columns above are **appended** after `divergence_bps` (additive
schema change, constitution §5) and are proposed to
`13-parquet-lake/SCHEMAS.md`, which this file does not edit. In
`shadow_fills` the `slippage_bps` column is the simulated slippage against
`arrival_mid` (positive is worse); before the change it was against the
mid at submission, which are the same number for a single immediate order.

The `quantity` is `float64` rather than `int64` to accommodate fractional
orders in the Bootstrap and Transition tiers.

The `tier` column records which slippage model was applied. This allows
post-hoc comparison: the average slippage in the Bootstrap tier is
expected to be higher than in the Standard tier, and the divergence
analysis must be done per tier.

## 6. Tier-Specific and Arm-Specific Calibration

The model of §2 is calibrated from real fills, separately per tier, per
order kind, per ladder arm and per spread bucket:

| Tier | Calibration source | Prior before data (not a finding) |
|---|---|---|
| Bootstrap | Bootstrap-tier market fills only | The quoted half-spread |
| Transition (limit) | Transition-tier limit fills only, per arm | The quoted half-spread (`cross`); the bracket (`post`) |
| Transition (market) | Transition-tier market fills only | The quoted half-spread |
| Standard | Standard-tier limit fills only, per arm | The quoted half-spread (`cross`); the bracket (`post`) |

Calibrating across tiers would mix market-order spread cost with
limit-order fill cost, and calibrating across arms would mix passive and
aggressive fills. The separation ensures the simulator reflects the
actual cost structure of each cell. The previous "expected ~5 bps / ~5-20
bps" table values were placeholders and are not carried forward; expected
slippage is whatever the cells measure.

After 50 real fills in a cell, the cell's median is recomputed and the
parameter updated. The shadow engine uses the **median** (robust, drives
the divergence check); the execution cost model uses an **EWMA** of the
same realized slippage (`EXECUTION-WINDOW.md` §5), which tracks drift. The
two are computed from the same fills and may differ; they serve different
purposes.

## 7. What Not to Do

- Do not use the shadow engine to decide whether to submit. It runs
  alongside, not before.
- Do not skip the divergence check. It catches adapter bugs.
- Do not assume simulated = real. Measure.
- Do not calibrate slippage across tiers. Each tier has its own cost
  structure.
- Do not report a single optimistic number for the `post` arm before it
  has fills. Report the bracket.
- Do not benchmark a ladder against the mid at each leg's own submission;
  use the arrival mid.
- Do not apply a fixed slippage parameter to market orders. The cost is
  the spread, not a fixed bps value.
- Do not record a fractional order with an integer quantity column. The
  schema uses `float64` for a reason.