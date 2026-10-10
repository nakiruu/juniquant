# Training Protocol

The step-by-step procedure for training a GSPQR model generation.

A generation is trained at a **cutoff** `m` (a month-end close) on the
rows matured at `m`. The same procedure serves the live monthly retrain and
every historical month-end of the walk-forward backfill
(`PREDICTION-BACKFILL.md`). A second mode, `--mode research`, trains one
static model on `split == train` for the held-out research evaluation only.

## 1. Inputs

- `panel.parquet` — the 12-feature panel with forward return targets
- `registry.json` — the previous generation's metrics (for comparison)
- the cutoff `m` (`--cutoff`; default: the previous trading day's close)

## 2. Splitting

**Production and backfill generations (default).** The candidate rows for
horizon `H` at cutoff `m` are the **matured** rows: panel rows with
`date <= m - H` on the global trading-day index and a non-NaN
`y_fwd_Hd`, from the panel as of `m`. The `split` column is not read. (The
target of a later row exists in the full panel but is not known at `m`; the
`date + H <= m` mask is what keeps it out.) Within the candidate rows a
further chronological split creates a validation set:

    1. Sort the distinct candidate dates.
    2. Find the 80th percentile date, val_boundary.
    3. Training set: candidate rows with date <= val_boundary - H trading days.
    4. Validation set: candidate rows with date > val_boundary.
    5. Rows with date in (val_boundary - H, val_boundary] are dropped for
       this horizon's model.

**Research mode.** The panel has a `split` column with values `train`,
`purge`, `cal`, `test`, assigned by global calendar date, with a 252-day
`purge` band (see `02-panel-construction/TARGETS.md` §4). `--mode research`
uses `split == train` as the candidate rows (the `purge` band is excluded)
and the same 80/20 procedure. Its model is evaluated on `split == test`
once. It is not a source of predictions for TCP or EVT, and nothing
production-bound is trained this way.

The drop in step 5 is a purge: a training label at date `t` ends at
`t + H`, so without it the last training labels overlap the validation
period and early stopping sees information it should not. `H` is the
horizon of the model being fit (63, 126 or 252).

This is per-horizon. A 63-day model uses rows where `y_fwd_63d` is
non-NaN. A 252-day model uses rows where `y_fwd_252d` is non-NaN. The
two sets overlap but are not identical.

**Why 80/20 within the candidate rows:** Early stopping needs a held-out set.
The validation set is used only for early stopping and overfitting
detection, not for calibration or final evaluation. The last 20% of dates
are therefore not fitted on by that generation; they are in the next one.

## 3. Feature Matrix

For horizon H:

    X_train = panel[candidate & date <= val_boundary - H, FEATURES]
    y_train = panel[same, y_fwd_Hd]
    X_val   = panel[candidate & date > val_boundary, FEATURES]
    y_val   = panel[same, y_fwd_Hd]

Where `FEATURES` is the frozen 12-column list in order. NaN values are
preserved, not imputed.

Drop rows where `y_train` or `y_val` is NaN. Keep rows where features
are NaN.

## 4. Training One Model

For each (horizon, quantile) pair:

```julia
params = Dict(
    "objective"        => "quantile",
    "alpha"            => tau,
    "metric"           => "quantile",
    "num_leaves"       => num_leaves,
    "learning_rate"    => learning_rate,
    "min_data_in_leaf" => min_data_in_leaf,
    "feature_fraction" => 0.8,
    "bagging_fraction" => 0.8,
    "bagging_freq"     => 5,
    "lambda_l1"        => 0.1,
    "lambda_l2"        => 0.1,
    "verbose"          => -1,
    "seed"             => 42,
    "num_threads"      => NUM_THREADS,   # a fixed integer, recorded in the registry
    "deterministic"    => true,
)

# The LightGBM.jl calls below are a sketch: the Dataset/fit signatures and
# the keyword names (early_stopping_rounds, verbose_eval) are UNVERIFIED
# against the pinned LightGBM.jl version (verify). The parameter names are
# LightGBM's own. As remembered, LightGBM.jl exposes an estimator interface
# (`LGBMRegression(...)` with `fit!`), not a `LightGBM.fit(train_set, val_set)`
# function: treat the call shape below as a placeholder (verify). `num_threads = -1` is not used: the thread count must not
# vary between runs (§9).
train_set = LightGBM.Dataset(X_train, label = y_train, free_raw_data = false)
val_set   = LightGBM.Dataset(X_val,   label = y_val,   reference = train_set)

model = LightGBM.fit(
    train_set, val_set;
    params = params,
    num_boost_round = n_estimators,
    early_stopping_rounds = 50,
    verbose_eval = false,
)
```

The `alpha` parameter is set to the target quantile level. Everything
else is from `HYPERPARAMETERS.md`, adjusted per-quantile as specified.

## 5. Saving the Model

```julia
qtag = lpad(string(round(Int, tau * 100)), 2, '0')            # 0.01 -> "01", 0.5 -> "50"
save_path = "models/generations/$(generation)/gspqr_$(generation)_$(horizon)_q$(qtag).txt"
LightGBM.save(model, save_path)   # verify: the save/load function names in LightGBM.jl
```

`generation` is the cutoff date `m` (ISO). The file name and the registry
key use the single convention of `EXPORT.md` §2 (`63d_q05`).
A non-padded `q5` or `q1` is wrong.

The `.txt` format is LightGBM's native text serialization. It is
human-readable, versionable in Git, and portable across LightGBM
versions with forward compatibility guarantees.

## 6. Evaluating the Model

For each model, compute on the validation set:

**Pinball loss** (the primary metric):

    pinball(y, yhat, tau) = mean(
        tau * (y - yhat) if y >= yhat
        (1 - tau) * (yhat - y) if y < yhat
    )

**Coverage** of the quantile (the fraction of validation rows where
`y <= yhat`; this is the quantile's own hit rate, not an interval
coverage):

    coverage = mean(y_val .<= yhat_val)

For a well-calibrated model at quantile τ, coverage should be close to
τ. A coverage of 0.10 for a q_05 model indicates the model is
under-predicting the tail; 0.02 indicates over-predicting.

**Record both metrics** in the registry. They are the primary inputs
to the deploy/don't-deploy decision.

## 7. Abort Conditions

Abort the retrain and keep the previous generation if ANY of:

- Any model's validation pinball loss is more than 10% worse than the
  previous generation's (live retrain only; in the walk-forward backfill the
  two generations' validation sets cover different dates, so the comparison
  is a recorded WARN, not an abort)
- Any model's validation coverage deviates from nominal by more than
  10 percentage points (the abort threshold; a deviation of more than 5
  points but at most 10 is a WARN recorded as `val_coverage` in the
  registry and reviewed, not an abort. One tolerance per purpose, no
  second ±5 gate)
- More than 3 of the 21 models fail to train (converge, produce finite
  predictions)
- Training runs longer than 6 hours for the full generation
- The crossing rate on the validation set exceeds 1% of rows, for any
  horizon: after all seven models of a horizon are trained, predict the
  horizon's validation rows with each model (raw, not rearranged), and
  count rows where the seven values are not non-decreasing in quantile
  order. Rearrangement at inference hides crossing, so this check is what
  tells you the seven models disagree too often to be trusted. Write the
  counts to `registry.json` under `crossing` (`SKILL.md` §6) whether or
  not the retrain aborts.

In the backfill an aborted generation is recorded (`status = aborted` in
the manifest) and its month is served by the last successful generation
(`PREDICTION-BACKFILL.md` §3).

On abort, write an audit event with `severity = "WARN"` and
`event_type = "model_retrain_aborted"` through the audit spool directory
(`11-rust-event-bus` §5; Julia never writes the audit log directly), with
`details.reason` naming the condition (for example `crossing_rate`). The
live model remains the previous generation.

## 8. Deployment

After successful training and evaluation:

1. Write the new `.txt` files to `models/generations/<generation>/`.
2. Update `registry.json` with the new generation entry and set `live`.
3. Previous generations stay in place under their own directories (they are
   the as-of history; never delete or overwrite one).
4. Commit the registry to Git (the `.txt` files of a 15-year backfill are
   large; commit the registry with the sha256 of each, and keep the files
   on the array).
5. Julia batch inference reads the live generation and writes the day's
   `gspqr_predictions` rows. A Rust component that loads models for the
   parity test or on-demand prediction polls the registry's mtime and
   reloads on change.

## 9. Reproducibility

Given the same as-of panel slice (hash it before training), the same seed
(42, constant across generations) and the same fixed `num_threads`,
training must produce byte-identical model files. If it does not:

- Check that `deterministic = true` is set in the LightGBM params (whether
  `force_row_wise = true` is also needed is to verify)
- Check that `num_threads` does not vary (LightGBM's threading can be
  non-deterministic on some operations); it is a fixed integer recorded in
  the registry, never -1
- Check that the input panel slice is identical

Any non-determinism is a bug. Investigate before deploying.

## 10. Verification Checklist

Before deploying:

- [ ] For each horizon, no training row has `date > val_boundary - H`, and
      no row has `date + H > m` (cutoff)
- [ ] Production mode never reads the `split` column; research mode reads
      nothing else
- [ ] All 21 `.txt` files exist and are non-empty
- [ ] Each file's first line lists the feature names in the frozen order
- [ ] Validation pinball loss is recorded for each model
- [ ] Validation coverage is recorded for each model
- [ ] No model triggered an abort condition
- [ ] The registry is updated with the new generation
- [ ] The previous generation is archived (not deleted)
- [ ] A Rust inference test reproduces Julia predictions on 100 rows
      to within 1e-6
- [ ] The commit message references the generation date and the
      validation metrics
