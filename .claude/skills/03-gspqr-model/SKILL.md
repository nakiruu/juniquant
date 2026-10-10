---
name: gspqr-model
description: LightGBM quantile regression for the GSPQR base model.
  Load when touching models/src/gspqr/, training code, hyperparameter tuning,
  or the model export path.
---

# GSPQR Model

GSPQR is the base quantile model. It takes the 12-feature panel (see
`02-panel-construction`) and produces conditional quantile estimates of
forward excess returns at three horizons.

## 1. What GSPQR Produces

For each ticker, date, and horizon, GSPQR emits seven quantiles:

| Quantile | Purpose |
|---|---|
| `q_01` | 1st percentile — extreme downside |
| `q_05` | 5th percentile — the TCP lower bound input |
| `q_25` | 25th percentile — lower quartile |
| `q_50` | 50th percentile — **median, used by the optimizer** |
| `q_75` | 75th percentile — upper quartile |
| `q_95` | 95th percentile — the TCP upper bound input |
| `q_99` | 99th percentile — extreme upside |

These are **raw** quantiles, before TCP calibration. "Raw" means before
TCP, not before rearrangement: see the next subsection.

**Monotone rearrangement.** The seven quantiles come from seven
independent models (§2), so nothing forces `q_01 <= q_05 <= ... <= q_99`.
After inference, the seven values for each `(ticker, date, horizon)` are
**sorted ascending** before `gspqr_predictions.parquet` is written. The
writer is Julia (`models/src/inference/predict.jl`), the only writer of
that file (`EXPORT.md` §5). The Rust loader (`execution/src/inference`,
parity test and optional on-demand prediction, never persisted) applies the
same sort, and the two must produce identical output (`EXPORT.md` §7). The stored values are the rearranged
ones, and TCP conformity scores are computed on them
(`04-tcp-calibration/CALIBRATION.md` §1). The boolean column `crossed`
(`13-parquet-lake/SCHEMAS.md` #7) is true when the raw output was not
already non-decreasing. If any of the seven raw values is not finite, the
row is not written and the event is logged; sorting never papers over a
NaN. Sorting is applied at inference only; training, validation metrics
per model, and the registry's crossing rate use the raw per-model outputs.

`[q_05, q_95]` is a 90% base interval (5% in each tail). TCP widens it
toward a **95%-coverage** interval, so `q_hat_lower` and `q_hat_upper` are
the endpoints of that 95% interval, not the 5% and 95% quantiles
(`04-tcp-calibration/SKILL.md` §1).

The calibrated layer inherits the ordering: since `q_05 <= q_95` after
rearrangement, `Q̂ >= 0` keeps `q_hat_lower <= q_hat_upper`. The calibration
layer (see `04-tcp-calibration`) turns `q_05` and `q_95` into valid
prediction intervals. EVT (see `05-evt-tails`) extends `q_01` and `q_99`
using extreme value theory.

## 2. Architecture

**One LightGBM model per (horizon, quantile) pair.** That is 3 horizons
× 7 quantiles = 21 models.

Why not one model with multiple outputs: LightGBM's quantile objective
is defined per quantile. Training a single multi-output model requires
either a custom objective or a wrapper, both of which complicate
inference and export. Separate models are simpler, faster to train in
parallel, and each model's output is independently interpretable.

**The models do not share trees.** Each is fit independently on the
same features against the same target, differing only in the quantile
level.

## 3. Training Discipline

Three non-negotiables:

- **Chronological, as-of training only.** Production GSPQR (including the
  live model) trains on **all rows matured at the cutoff** `m`: rows with
  `date + H <= m` (global trading-day index) from the panel as of `m`
  (`PREDICTION-BACKFILL.md`). The panel's `split` column
  (`train`/`purge`/`cal`/`test`, a global date cut) is **only** for the
  held-out research evaluation (`--mode research`): one static model on
  `split == train`, scored once on `split == test`. TCP and EVT never use
  it. Never shuffle.
- **NaN is a valid feature value.** LightGBM handles NaN natively. Do
  not impute, do not drop rows with NaN features. The only rows dropped
  are those where the target for the horizon being modeled is NaN. That
  excludes every `survivorship_gap` row (its target is NaN by definition,
  `02-panel-construction/TARGETS.md` §3); the count of excluded rows is
  reported per year and horizon, and every generation's registry entry and
  report carry the survivorship caveat (`01-data-ingestion/SURVIVORSHIP.md`).
  The panel has 21 columns (`02-panel-construction` SKILL); training reads
  the 12 features and the target, never the flags as features.
- **Fixed feature order.** The 12 features are always in the order
  specified in `02-panel-construction/FEATURES.md` §7. Reordering
  invalidates every trained model. The export step embeds the feature
  names, and Rust inference asserts them on load.

## 4. Training Cadence

| Trigger | Action |
|---|---|
| Monthly (first trading day) | Full retrain of all 21 models: a new **generation** with cutoff `m` = the previous close, on all matured rows (`PREDICTION-BACKFILL.md` §1-§2) |
| Daily after market close | Inference only, no retraining. Julia batch inference writes the day's `gspqr_predictions` rows (sole writer, `EXPORT.md` §5) |
| Historical backfill | The same generation procedure for every past month-end (`PREDICTION-BACKFILL.md`) |
| Manual | Full retrain on demand via `scripts/train-gspqr.jl --cutoff <m>` |

Retraining monthly, not daily, because:

- The panel updates daily, but the feature distribution shifts slowly.
- Monthly retrain prevents the model from chasing noise.
- Each retrain is a fresh set of 21 models; the previous generation is
  archived, not overwritten. A generation is identified by its cutoff date
  `m` (ISO date of the last panel date it saw), the same id used in file
  names, the registry and the `generation` column of
  `gspqr_predictions.parquet`.

**Model registry:** `models/registry.json` tracks every trained model
generation, its training date, training set size, and validation metrics.
See §6. Model artifact paths in this skill (`models/registry.json`,
`models/generations/`, `models/backfill_manifest.json`) are relative to the
data root, i.e. `$QUANT_ROOT/models/...` (`13-parquet-lake/IO-CONTRACT.md`
§1), not the repository's `models/` code directory. Julia writes the registry;
the Rust loader reads it for parity tests and on-demand use only.

## 5. Hyperparameters

See `HYPERPARAMETERS.md`. The defaults are:

    objective        = "quantile"
    alpha            = <the quantile level>
    num_leaves       = 31
    learning_rate    = 0.05
    n_estimators     = 500
    min_data_in_leaf = 100
    feature_fraction = 0.8
    bagging_fraction = 0.8
    bagging_freq     = 5
    lambda_l1        = 0.1
    lambda_l2        = 0.1
    max_depth        = -1

These are **defaults**. A tuning pass may revise them. The structure
(one model per quantile, fixed feature order, chronological training)
is frozen.

## 6. Model Registry

`models/registry.json` names the live generation and holds one entry per
generation (live and walk-forward backfill alike):

```json
{
  "live": "2026-09-30",
  "generations": {
    "2026-09-30": {
      "generation": "2026-09-30",
      "role": "live",
      "trained_at": "2026-10-01T07:14:22Z",
      "panel_version": "2026-09-30",
      "train_rows": 312847,
      "train_tickers": 612,
      "seed": 42,
      "num_threads": 8,
      "models": {
        "63d_q05": {
          "path": "models/generations/2026-09-30/gspqr_2026-09-30_63d_q05.txt",
          "sha256": "...",
          "feature_names": ["mom_20d", "mom_60d", "..."],
          "train_metric": 0.041,
          "val_metric": 0.044,
          "val_coverage": 0.062,
          "best_iteration": 212
        },
        "63d_q50": { "...": "..." },
        "...": {}
      },
      "crossing": {
        "threshold": 0.01,
        "by_horizon": {
          "63d":  { "val_rows": 41200, "val_crossed": 118, "rate": 0.00286, "max_violation": 0.0071 },
          "126d": { "...": "..." },
          "252d": { "...": "..." }
        },
        "max_rate": 0.00286
      }
    }
  }
}
```

The example is illustrative (values are not results). `role` is `live`
or `backfill`. A generation early in the backfill holds only the horizons
that have enough history (`PREDICTION-BACKFILL.md` §1), so `models` has 7
or 14 keys until all three horizons are available. Model keys are
`<horizon>_q<NN>` with a two-digit quantile (`63d_q05`, `63d_q50`,
`252d_q99`): the same string as the file-name suffix (`EXPORT.md` §2).
Parquet columns keep their frozen names `q_05`, `q_50`, and so on; the
two conventions differ on purpose (file names and registry keys versus
column names) and are never mixed.

`crossing` is the per-generation crossing report. For each horizon, the
seven models are evaluated on that horizon's validation rows; `val_rows`
is the number of `(ticker, date)` rows, `val_crossed` how many have a raw
output that is not non-decreasing, `rate = val_crossed / val_rows`, and
`max_violation` the largest `q_k - q_{k+1}` over adjacent quantiles
(a measure of how bad a crossing is). `max_rate` is the largest rate over
horizons. A generation with `max_rate > threshold` (1%) is aborted
(`TRAINING.md` §7). The field is written even when the retrain passes, so
the rate can be tracked across generations. The same rate is monitored in
live inference as the share of `crossed = true` rows per day.

Each entry includes the model path, the exact feature names (in order),
and the training and validation metrics. Julia inference (and the Rust
loader used for parity and on-demand prediction) reads the registry to know
which generation is live and asserts the feature names match the expected
order.

## 7. Export Format

See `EXPORT.md`. The short version:

- LightGBM's native `.txt` model format.
- One `.txt` file per model.
- No serialization via pickle, joblib, or any language-specific format.
- Rust loads via `bosk` (pure-Rust parser) or `lightgbm-rust` (C API); the
  existence, API and `.txt` compatibility of both crates are **unverified
  (verify)** before anything depends on them. Rust loading exists for the
  round-trip parity test and optional on-demand prediction that is never
  persisted; it does not write `gspqr_predictions.parquet`.

The `.txt` format is portable, human-readable, versionable in Git, and
reproducible across machines. It is the only sanctioned export format.

## 8. Training Protocol

See `TRAINING.md` for the step-by-step procedure. The summary:

1. Load `panel.parquet`; keep rows with `date <= m` and, per horizon, only
   matured rows (`date + H <= m`, global index). In `--mode research`
   instead filter to `split == train`.
2. Split those rows (matured rows; `split == train` rows in research mode)
   into a **training set** and a **validation set** by date. The validation
   set is the last 20% of their dates, and
   training rows within `H` trading days of the validation boundary are
   dropped (purge, `TRAINING.md` §2). This is a further chronological
   split, not random.
3. For each (horizon, quantile), fit a LightGBM model with early
   stopping on the validation set.
4. Save each model as `.txt`.
5. Evaluate each model on the validation set: pinball loss, coverage
   of the nominal quantile.
6. If any model's validation metric is worse than the previous
   generation by more than 10%, abort the retrain and keep the old
   generation (live only; in the backfill the two validation sets differ, so
   this is recorded as a WARN, `TRAINING.md` §7).
7. Evaluate the crossing rate on each horizon's validation set (raw
   outputs of the seven models). If it exceeds 1% of rows for any horizon,
   abort (`TRAINING.md` §7).
8. Update `registry.json`, including the `crossing` report (§6).

## 9. Feature Matrix Construction

The 12 features come from the panel in this exact order:

    mom_20d, mom_60d, mom_12m_1m, vol_60d_res, sp_ratio, turn_20d,
    r_mom_20d, r_mom_60d, r_mom_12m_1m, r_vol_60d, r_sp, r_turn

The feature matrix `X` has shape `(n_rows, 12)`. The target vector `y`
for horizon H is `y_fwd_Hd` from the panel, filtered to non-NaN rows.

**NaN handling:** Do NOT fill NaN. Pass the matrix with NaN directly to
LightGBM. The `use_missing=true` default handles it.

**Do not standardize or normalize.** The raw features and rank features
are already on comparable scales (ranks are [0, 1]; raw features are
unbounded but LightGBM is scale-invariant on splits).

**Training speed (no new infrastructure).** The panel is small (about 3M
rows by 12 features, about 145 MB as float32) and is read once per
generation; I/O is negligible next to tree building. A vector store or an
in-process cache between the lake and LightGBM buys nothing and adds a
second copy of the data (owner, 2026-10-10). Where training time matters
(the prediction backfill: one generation per month-end, 21 models each),
use these, in this order:

1. **One binned Dataset per horizon.** The seven quantile models of a
   horizon share `X` and `y_fwd_Hd`; construct the LightGBM Dataset once
   and train all seven `alpha` values on it instead of re-binning seven
   times.
2. **Arrow straight to the matrix.** Read the panel columns from Arrow into
   the feature matrix without per-row conversion. Float32 features only if
   the Rust parity test (§11) passes with the same dtype on both sides.
3. **Generations in parallel.** Month-end generations are independent:
   run several at once with `num_threads` split between them so the cores
   are not oversubscribed. Each generation keeps its own fixed `seed` and
   `num_threads`, recorded in the registry, so results stay reproducible.
4. **Coarser histograms last.** `max_bin` and related binning settings are
   hyperparameters: change them only through a tuning pass
   (`HYPERPARAMETERS.md`), never as a speed tweak.

## 10. What Not to Do

- Do not train a single multi-output model. One model per quantile.
- Do not shuffle rows. Chronological only.
- Do not impute NaN features. Pass them through.
- Do not standardize features. LightGBM does not need it.
- Do not use a random validation split. The validation set is the last
  20% of train dates.
- Do not export via pickle or joblib. `.txt` only.
- Do not overwrite a previous model generation. Archive and version.
- Do not tune hyperparameters on the test set. Tune on validation.
- Do not train a production generation on a row whose label has not matured
  at the cutoff (`date + H > m`), and do not use the `split` column for
  production training, TCP or EVT. The `split` column is for the research
  evaluation only.
- Do not write `gspqr_predictions.parquet` from Rust.
- Do not reorder the feature matrix. The order is frozen.
- Do not store raw crossed quantiles. Rearrange (sort) before writing
  `gspqr_predictions.parquet` (Julia writes; the Rust loader sorts
  identically for parity and on-demand use).
- Do not sort in only one language. A Julia/Rust mismatch is a failed
  round-trip.
- Do not compute TCP scores on the raw (possibly crossed) values.

## 11. Verification

Before deploying a new model generation:

- [ ] All 21 models exist and are readable `.txt` files
- [ ] Each model's feature names match the frozen order
- [ ] Each model's validation pinball loss is within 10% of the
      previous generation
- [ ] Coverage of the nominal quantile on the validation set is within
      ±10 percentage points of nominal (the abort threshold,
      `TRAINING.md` §7); outside ±5 points is recorded as a WARN in the
      registry (`val_coverage`) and reviewed, not a blocker
- [ ] A Rust inference test loads the models and reproduces Julia's
      rearranged predictions on 100 held-out rows to within 1e-6 (and the
      same `crossed` flags), see `EXPORT.md` §7
- [ ] A test feeds a deliberately crossed raw vector and asserts the
      stored quantiles are non-decreasing and `crossed = true`, in both
      Julia and Rust
- [ ] Every stored `(ticker, date, horizon)` row satisfies
      `q_01 <= q_05 <= q_25 <= q_50 <= q_75 <= q_95 <= q_99`
- [ ] `registry.json` carries the `crossing` report, and the retrain
      aborts when the validation crossing rate exceeds 1% of rows
- [ ] `registry.json` is updated and committed; file names and registry
      keys use the one convention of `EXPORT.md` §2 (`63d_q05`)
- [ ] No training row has `date + H > m` for the generation's cutoff `m`
- [ ] The previous generation is archived, not deleted

## 12. Related Skills

- `02-panel-construction` — produces the input features
- `04-tcp-calibration` — consumes `q_05` and `q_95`
- `05-evt-tails` — consumes `q_01` and `q_99`
- `08-portfolio-optimization` — consumes `q_50` as expected return
- `03-gspqr-model/PREDICTION-BACKFILL.md` — generations and the as-of
  prediction history that TCP and EVT read
- `12-julia-rust-bridge` — the export/load path
- `13-parquet-lake` — schema for `gspqr_predictions.parquet`
- `14-backtest-protocol` — evaluates the model (CPCV: one raw model per
  split; walk-forward track: these generations)
