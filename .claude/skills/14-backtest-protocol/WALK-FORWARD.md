# Walk-Forward

Walk-forward in this protocol has two roles: the **walk-forward track**, the
production pipeline replayed over history, and the 80/20 chronological split
used for early stopping inside one GSPQR fit. Neither is the portfolio
evaluation; that is CPCV (`SKILL.md` §2a).

## 1. What It Is

Walk-forward analysis divides the timeline into sequential folds. Each
fold trains on all data before the test block and tests on the next
block. The window can be **expanding** (training set grows) or
**sliding** (training set stays a fixed size).

```
Fold 1: [====TRAIN====][TEST]
Fold 2: [=======TRAIN=======][TEST]
Fold 3: [==========TRAIN==========][TEST]
```

## 2. Purging and Embargoing

Simple walk-forward still has a leak: if the label horizon extends into
the test period, training observations near the boundary "see" test
data. Purge removes training rows whose label interval `[t, t+H]` overlaps
a test label interval, on both sides of the block (`H` = horizon of the
model being trained). Embargo removes a further `h = max(21, ceil(0.01 * T))`
training rows after the back-side purge, for residual serial correlation
(`SKILL.md` §3, owner decision C1). In walk-forward there are no post-test
training rows, so the back-side purge and the embargo are no-ops; in CPCV
they are not.

In the walk-forward track the purge is built into the generation: a
generation trained at cutoff `m` uses only rows with `date + H <= m`, so no
label reaches past `m` (`03-gspqr-model/PREDICTION-BACKFILL.md` §2). Its
training rows are restricted to point-in-time eligible names
(`03-gspqr-model/TRAINING.md`, owner decision C3).

`PortfolioOptimisers.jl`'s `WalkForwardEstimator` accepts a `purged_size`
parameter (default 5) for the pre-test boundary (unverified, verify); the
pipeline does not rely on it.

Walk-forward here scores quantile loss, interval coverage and tail
exceedances, not trading P&L: it has no execution simulator and no lock rule.
The lock, the cost model and the restricted list belong to the CPCV
evaluation (`SKILL.md` §6a-6f).

## 3. The Walk-Forward Track

The track is the monthly generation sequence: for each month-end `m`, a
GSPQR generation trained on rows matured at `m`, predictions for the
following month stored with their `generation`, then TCP and EVT read those
stored predictions in date order, and the regime is refit monthly with
filtered probabilities. It is the only source of:

- the as-of predictions TCP and EVT require,
- the evaluation of the calibrated interval (coverage, Winkler) and of the
  tails (exceedance rates), because those layers are only valid when fed
  out-of-sample predictions,
- the raw-quantile pinball and rank IC over the full history, comparable to
  CPCV's.

It produces **one path**. Its metrics are single-path numbers: report them by
calendar year and regime, with HAC standard errors (lag >= H), and reserve
the last 20% of origins as a confirmation window for any placeholder checked
on the track (`SKILL.md` §2a). It is not a Sharpe distribution and is not used
for the portfolio evaluation, because a single path has high variance and
cannot support significance testing.

## 4. The Training Loop (early stopping)

Inside each GSPQR fit (see `03-gspqr-model/TRAINING.md`):

1. Take the candidate rows (matured at the cutoff, or `split == train` in the
   research mode).
2. Split them into training and validation by date (last 20% of dates for
   validation), purging `H` days at the boundary.
3. Train with early stopping on the validation set.

This is a simple 80/20 chronological split, not full walk-forward: it selects
the number of boosting rounds. CPCV fits do **not** early-stop (fixed
`n_estimators`, `SKILL.md` §6).

## 5. What Not to Do

- Do not use the walk-forward track for the portfolio evaluation or a Sharpe
  distribution. Use CPCV.
- Do not evaluate TCP or EVT on CPCV paths. Use the walk-forward track.
- Do not use `KFold` or `train_test_split` for time series (the CI check
  greps for them). Use the purged splitter of this protocol
  (`scripts/run-backtest.jl`, `cpcv_splits`).
- Do not skip purging in walk-forward. Overlapping labels leak.
- Do not assume `shuffle=False` makes `KFold` safe. Every fold but the
  last still trains on future rows.
