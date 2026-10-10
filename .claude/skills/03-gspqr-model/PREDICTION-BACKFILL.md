# Walk-Forward Generations and the As-Of Prediction Backfill

TCP and EVT read **stored as-of predictions**: for every calibration date
`s` the raw quantiles that a model produced using only rows whose labels
had matured by `s` (`04-tcp-calibration/CALIBRATION.md` §1). Nothing else
in the system produces them. A single static model trained once on the
first 60% of the panel cannot: its predictions on the first 60% are
in-sample, so the conformity scores TCP and the residuals EVT read would be
too small. This file defines how 15 years of as-of predictions are made.

Everything here is a specification. Nothing in it has been run: no Julia is
installed in the authoring environment, and every number in the cost and
timeline sections is arithmetic or an estimate, labelled as such.

## 1. Definitions

- **Trading-day index `d`.** The global index of the panel's sorted distinct
  dates (`d = 1` is the first panel date). Offsets such as `s + H` always
  count on this global index, never on one ticker's own rows, which can have
  gaps. The panel's distinct dates must equal the computed NYSE calendar
  over the panel range (asserted when the panel is built).
- **Horizon `H`** is 63, 126 or 252 trading days.
- **Matured row for cutoff `m`.** A panel row `(ticker i, date s)` is matured
  for horizon `H` at cutoff `m` when `s + H <= m` and `y_fwd_Hd(i, s)` is
  non-NaN. Its label uses only prices up to `m`.
- **Generation `g(m)`.** The set of GSPQR models (up to 3 horizons x 7
  quantiles) trained after the close of **month-end `m`** on the panel
  **as of `m`**: only rows with `date <= m`, and for horizon `H` only
  matured rows (`s <= m - H`). The generation id is the ISO date of `m`
  (for example `2019-03-29`). A live retrain run on the first trading day
  of a month uses `m` = the previous close and is named by that `m`; this
  is the one naming convention for the registry, model files and the
  `generation` column.
- **Minimum history `L_min` (decided, owner C2, 2026-10-10).** `L_min = 504`
  matured, feature-bearing training dates (two years) makes a month-end the
  **earliest candidate** for a horizon's first generation. The panel starts
  at the first trading day with at least 50 eligible names, and every
  eligible row there already has 756 bars of price history
  (`02-panel-construction/SKILL.md`, owner decision C3), so features exist
  from panel day 1 (`F0 = 1`) and
  `m_cand(H) = first month-end >= F0 + L_min - 1 + H = 504 + H`.
- **Acceptance rule (owner C2).** The first generation of a horizon,
  `m_first(H)`, is the first candidate month-end `m >= m_cand(H)` at which
  the generation (a) passes every abort criterion of `TRAINING.md` §7 and
  (b) beats the **naive baseline** on its validation window: the mean
  pinball loss over the seven quantiles of the model is strictly lower than
  that of the unconditional rolling empirical quantiles of the same horizon
  (§2a). Until a candidate is accepted, **no predictions are emitted** for
  that horizon (no rows, not NaN rows), and each rejected candidate writes a
  registry entry with `status = rejected_baseline` and a `data_quality`
  INFO row. Once accepted, later generations are governed by the abort and
  staleness rules of §3, not by the baseline test (it is still computed and
  reported). Horizons enter one by one: the first generations hold 7 models
  (63d only), then 14, then 21. `m_first(H)` is data-driven; the timeline
  of §8 uses the earliest candidate `m_cand(H)` and is a lower bound.

## 2. One Generation

For cutoff `m` and each horizon `H` with `m >= m_cand(H)`:

1. **As-of panel.** Take `panel.parquet` rows with `date <= m` and keep only
   rows whose `(date, ticker)` is `eligible = true` in `universe`
   (`13-parquet-lake/SCHEMAS.md` #1) at that date: train on what you would
   trade (owner decision C3). Ineligible panel rows stay in the panel for
   diagnostics only and never enter a fit, a validation set or a baseline.
   Mask the
   target: a row is used only if `s + H <= m` on the global index. The stored
   `y_fwd_Hd` of later rows is known in the full panel but is not known at
   `m`; the mask, not the stored value, is what enforces this. No
   re-computation of the panel: features at `s` use only data up to `s`
   (constitution §2/§3), so filtering by date reproduces the as-of panel.
   A verification test asserts this (§10).
2. **Chronological validation split** inside the matured rows exactly as
   `TRAINING.md` §2: the last 20% of the matured training dates are the
   validation set, and training rows within `H` days of the validation
   boundary are purged.
3. **Fit** the 7 quantile models (`TRAINING.md` §4) with the fixed seed.
4. **Evaluate, abort checks** (`TRAINING.md` §7, including the comparison
   with the previous generation, which aborts in the backfill as in live),
   the **baseline test** (§2a, decisive only until the horizon's first
   generation is accepted), write the registry entry.
5. **Predict** the dates the generation serves (§3) for eligible names only
   and write them (`EXPORT.md` §5, Julia is the only writer).

The **fits** of a generation do not depend on the previous one (no warm
start, no shared state): they are independent jobs that can run in any
order, in parallel, and a failed one can be re-run alone. The **acceptance**
of a generation does: the previous-generation comparison of `TRAINING.md` §7
and the serving rule of §3 read the outcome of the earlier generations, so
the accept/abort decisions and the prediction writes run in month order per
horizon (cheap; only the fits are expensive).

### 2a. The Naive Baseline

For horizon `H` at cutoff `m`, the baseline predicts, for every row of the
generation's validation window, the same seven numbers: the empirical
quantiles at `tau` = 0.01, 0.05, 0.25, 0.50, 0.75, 0.95, 0.99 of `y_fwd_Hd`
over all eligible matured rows in the trailing `L_min` = 504 dates that end
before the validation window starts (purged by `H` like the training rows).
It is unconditional (no features) and rolling. A generation passes when its
mean pinball loss over the seven quantiles on the validation window is
strictly below the baseline's on the same rows; both numbers go to the
registry (`val_pinball_mean`, `baseline_pinball_mean`).

## 3. Which Dates a Generation Predicts

Generation `g(m)` predicts the dates `s` in `(m, m_next]`, where `m_next` is
the next month-end: the first trading day after `m` through the next
month-end, inclusive. These are exactly the dates of one calendar month, so
the `year/month` partition of `gspqr_predictions.parquet` maps 1:1 to a
generation per horizon. Every `(ticker, date, horizon)` is therefore
produced by **exactly one** generation, and the primary key
`(ticker, date, horizon)` stays unique. The `generation` column
(`SCHEMAS.md` #7, appended, nullable) records which one, and
`panel_version` (appended, nullable) records the as-of panel it saw (the
cutoff `m`, or its hash, see §6).

A date earlier than `m_first(H) + 1` has no prediction for horizon `H`. This
is not a gap to fill; the TCP and EVT timelines start after it (§8).

**Abort and staleness (decided, owner C4, 2026-10-10).** The abort criteria
of `TRAINING.md` §7 apply in the historical backfill exactly as in the live
retrain, including "validation pinball more than 10% worse than the previous
generation": that check **aborts**, it is not downgraded to a warning. When
generation `g(m)` aborts for a horizon, the previous accepted generation of
that horizon stays live and serves the month `(m, m_next]` (it was trained
on earlier data only, so there is no lookahead); the `generation` column
shows the older id. A serving generation may be at most **3 months** stale
(its cutoff at most three month-ends before `m`). Beyond that, no prediction
rows are emitted for that horizon until a generation is accepted again, and
each unserved month writes a `data_quality` WARN row (`check =
stale_generation`, horizon, last accepted generation). The backfill continues
with the next month-end; it does not stop. The same rule applies live
(`SKILL.md` §4). The comparison is with the previous **accepted** generation,
so one aborted month does not move the reference. The previous-generation
check is defined only once a horizon has an accepted generation; before that
the baseline test of §2a decides.

## 4. The Production Model and the `split` Column

Decision (binding for this system):

- The **production/live model trains on all matured, point-in-time eligible
  rows** at its cutoff `m`, as a generation of exactly the procedure above. It is the latest
  generation of the same sequence; there is no separate "first 60%" model.
  The live monthly retrain (`SKILL.md` §4) is `scripts/train-gspqr.jl
  --cutoff <m>`.
- The panel's `split` column (global date cut, 252-day purge, `TARGETS.md`
  §4) is used **only by the held-out research evaluation** (`scripts/
  train-gspqr.jl --mode research`, which trains one static model on
  `split == train` and scores it on `split == test` once). It must not be
  used by TCP, by EVT, or to select what the production model trains on.
- CPCV (`14-backtest-protocol`) does not use `split` either; it has its own
  groups.

## 5. Why Monthly, and Why Expanding

Monthly matches the live cadence (`SKILL.md` §4), so the backfilled
predictions are produced by the same process the live system will run, which
is what makes TCP's calibration scores valid for live use. The window is
expanding, not rolling: the 252-day horizon needs the longest history and
there is no regime-forgetting argument for GSPQR's cross-sectional model.
Changing either is a skill amendment (the generation count and cost below
change with it).

## 6. Sequencing, Resumability, Determinism

Order of work (each step reads only the outputs of the steps above it):

1. **Panel** complete for the whole range; record its content hash.
2. **Generations** (`models/generations/<generation>/`): fits in any order,
   parallel; then the accept/abort decisions (§1 acceptance rule, §3 abort
   and staleness) in month order per horizon.
3. **Predictions**: per generation, month partition written to a temp path
   and renamed atomically. A partition is complete or absent.
4. **TCP**: strictly in date order within a horizon (`04` §6: the
   Robbins-Monro recursion is sequential per ticker and horizon).
5. **EVT residuals**: written at maturity, in date order (`05`).
6. **EVT tail fits**: one per `(fit_date, horizon, tail)`, independent;
   parallel by date.

**Resumability.** A manifest `models/backfill_manifest.json` (a log record,
JSON is allowed) lists per generation: status (`done`, `aborted`,
`rejected_baseline`), the serving generation of each horizon for its month,
the sha256 of each `.txt`, the as-of panel hash, and the seed. A restart
skips generations whose files match the manifest. Prediction partitions are
skipped if present and their generation id matches. TCP resumes from the
last complete month partition, reading `rm_offset` and `rm_n` of the last
row of each `(ticker, horizon)` (`04` §6); EVT residuals resume from the last
complete month.

**`panel_version`.** For a backfilled generation, the cutoff date `m` (the
registry's `panel_version`) identifies the as-of slice; the manifest stores
the hash of that slice. If a later panel rebuild changes the hash of an
existing slice (for example a corrected filing), the generation is flagged
stale and it and everything downstream of its month must be recomputed.

**Determinism.** Same as-of panel slice, same seed (42), same fixed
`num_threads` recorded in the registry, `deterministic = true`: byte-identical
model files (`TRAINING.md` §9). The backfill re-run check compares sha256s.
The seed is constant across generations on purpose: a different seed per
generation would make generation-to-generation differences partly noise.

## 7. Compute-Cost Estimate (an estimate, not a measurement)

Assumptions: 15 years = 3,780 trading days, about 450 tickers (the
IWF/IWP union is 450 to 600; verify), 21-day months.

| Quantity | Arithmetic | Result |
|---|---|---|
| Generations per horizon | `floor((3780 - m_first(H)) / 21) + 1`, with `m_first` = 819 / 882 / 1008 | 142 / 139 / 133 |
| Model fits | `7 x (142 + 139 + 133)` | 2,898 |
| Prediction rows | `450 x ((3780-819) + (3780-882) + (3780-1008))` | about 3.9 million rows (x 7 quantile columns) |
| Training rows per fit | `450 x (m - H - 252)`, average over the expanding window | about 0.2 to 1.6 million (first generation of a horizon: 504 dates x 450 = 0.23 million; last: about 3,465 dates x 450 = 1.56 million); mean roughly 0.9 million |

Wall-clock: assume 10 to 60 s per LightGBM fit at this size on an 8-core
machine (500 rounds, 31 leaves, 12 features; **unmeasured**). That gives
about 8 to 48 hours for the 2,898 fits, one to two days in the plausible
range, and it parallelizes across (horizon, quantile) and generation.
Inference is minutes. **Measure the first 5 fits and extrapolate before
committing the machine.** The TCP and EVT stages are small next to
training (TCP: about 7.8 thousand `(horizon, origin)` pairs, 2,836 + 2,647 + 2,269, each a partial sort of at most 113 thousand scores, 450 x 252).

## 8. Timeline for a 15-Year Panel (arithmetic)

Day index from the first panel date, `T = 3780` (15 x 252). Years are
`d / 252`. Parameters: `L_min = 504`, TCP pooled window `M_d = H`
(`04-tcp-calibration/PARAMETERS.md`), EVT `W_min = 252 / 378 / 756` and
`W_full = 504 / 756 / 1008` (`05-evt-tails/PARAMETERS.md`). All placeholders,
so every row moves if they change.

| Event | 63d | 126d | 252d | Rule |
|---|---|---|---|---|
| First usable features | 253 (1.0y) | 253 | 253 | `mom_12m_1m` needs 253 days |
| First generation | 819 (3.25y) | 882 (3.5y) | 1008 (4.0y) | `756 + H`, rounded up to a month-end |
| First stored prediction | 820 | 883 | 1009 | day after the first generation |
| First TCP origin `t0` | 945 (3.75y) | 1134 (4.5y) | 1512 (6.0y) | `(m_first + 1) + H + M_d - 1`: all `M_d` window dates need a stored prediction |
| First RM update | 1008 | 1260 | 1764 | `t0 + H`: needs a matured interval |
| First EVT fit, partial window | 1259 (5.0y) | 1637 (6.5y) | 2519 (10.0y) | `t0 + H + W_min - 1` |
| First EVT fit, full window | 1511 (6.0y) | 2015 (8.0y) | 2771 (11.0y) | `t0 + H + W_full - 1` |
| Years of full-window EVT left | 9.0 | 7.0 | 4.0 | `(3780 - full) / 252` |
| Last evaluable TCP origin | 3717 | 3654 | 3528 | `T - H` (the label must mature) |

(`first prediction` is `m_first + 1`; `t0 = (m_first + 1) + H + M_d - 1`, so
945 = 820 + 63 + 63 - 1, 1134 = 883 + 126 + 126 - 1, 1512 = 1009 + 252 +
252 - 1.)

**Flags.**

- The 252-day horizon is the binding one: TCP starts at year 6 and a full
  EVT window only at year 11. About 8 years of TCP evaluation and 4 years of
  full-window EVT remain in a 15-year panel.
- On a **10-year panel (2,520 days)** the 252d horizon has no usable EVT at
  all: its partial window opens at day 2519 and its full window (2771)
  never arrives; 63d and 126d are fine (full at 1511 and 2015). That is a
  reason for the 15-year collection target.
- A per-ticker TCP window with `m = 60` would start earlier (942 / 1068 /
  1320) but its effective sample for `H >= 63` is about one observation
  (`04` CALIBRATION §4); it is not a faster start, it is a worse
  calibration.

**Sensitivity to the first panel row `F0` (cross-skill flag).** The table
assumes the panel has rows, with a possibly NaN feature set, from the first
date with 253 days of history (`F0 = 253`). If the panel keeps only
tickers that are `eligible` in `universe` (`SCHEMAS.md` #1) and `eligible`
requires `history_days >= 756` as currently written, then no ticker is
eligible before day 757 of a history that starts at the panel start, the
first panel row is `F0 = 757`, and every row above shifts by 504 days:

| Event (`F0 = 757`) | 63d | 126d | 252d |
|---|---|---|---|
| First generation | 1323 (5.25y) | 1386 (5.5y) | 1512 (6.0y) |
| First TCP origin `t0` | 1449 (5.75y) | 1638 (6.5y) | 2016 (8.0y) |
| First EVT fit, partial / full window | 1763 / 2015 | 2141 / 2519 | 3023 / 3275 |
| Years of full-window EVT left | 7.0 | 5.0 | 2.0 |

(arithmetic as in §8). The 252d horizon would then have two years of
full-window EVT in a 15-year panel. Whether pre-eligibility rows stay in the
panel, with `eligible` applied as a tradable-set filter downstream instead of
as a panel-presence filter, is a data/panel decision (OPEN-3).

## 9. Tables Written

`gspqr_predictions.parquet` (`13-parquet-lake/SCHEMAS.md` #7) gains two
appended nullable columns: `generation` (the cutoff id above) and
`panel_version`. Julia is the only writer. The row count for a 15-year
backfill is about 3.9 million, not the 500 thousand once listed.

## 10. Verification

- [ ] A test builds the as-of panel for cutoff `m` by filtering a full panel,
      and separately from data truncated at `m`; the training matrix is
      identical (features and masked targets)
- [ ] A test asserts no training row has `s + H > m` for any horizon, and
      that mutating prices after `m` leaves every model file byte-identical
- [ ] A test asserts every `(ticker, date, horizon)` appears under exactly one
      `generation`, and that `generation`'s cutoff `m < date`
- [ ] A test asserts the month partition of each generation holds exactly the
      dates `(m, m_next]`
- [ ] A re-run of one generation reproduces the manifest sha256s
- [ ] A test asserts no generation exists for a horizon before `m_first(H)`
- [ ] The timeline table in §8 is regenerated from the parameters by a test
      (so a changed placeholder cannot leave the table stale)
- [ ] A test asserts neither TCP nor EVT code reads the `split` column

## 11. What Not to Do

- Do not train one model and use its predictions for calibration dates. It is
  in-sample for part of the range.
- Do not use today's generation to produce history.
- Do not use `split` for TCP, EVT, or the production model.
- Do not read a target for a row with `s + H > m` when training generation
  `m`, even though the full panel stores it.
- Do not warm-start a generation from another: generations are independent.
- Do not write predictions for a month without a complete generation behind
  them.
- Do not write predictions from Rust (`EXPORT.md` §5).

## 12. Open Items for the Owner

- **OPEN-1: `L_min`.** 504 matured dates is a placeholder. A larger value
  delays every row of §8; a smaller one trains the first generations on very
  little data. Option A: 504 (default). Option B: 756 (three years; first
  generations at days 1071 / 1134 / 1260, i.e. 4.25 / 4.5 / 5.0 years, so
  every later row of §8 moves by 252 days). Option C: 252 (first generations
  at 567 / 630 / 756, 2.25 / 2.5 / 3.0 years; earlier start, noisier first
  windows).
- **OPEN-3: first panel row.** Keep pre-eligibility rows in the panel
  (`F0 = 253`, §8 table) or drop them (`F0 = 757`, sensitivity table).
  A point for keeping them: the history is the scarce resource here, and
  `eligible` can filter the tradable set after prediction. A point against:
  cross-sectional ranks would then include names the system would never
  trade. The decision belongs with `02-panel-construction` and the
  universe spec; this file only records the effect on the timeline.
- **OPEN-2: cadence.** Monthly is fixed to match live. A quarterly
  generation would cut the cost by two thirds but would differ from live.
