# Model Export and Cross-Language Inference

The contract between Julia training and Rust loading. The model file
is the only artifact that crosses the language boundary.

**Inference ownership (decision).** Julia is the **sole writer** of
`gspqr_predictions.parquet`: daily batch inference and the historical
backfill (`PREDICTION-BACKFILL.md`) both run in Julia
(`models/src/inference/predict.jl`). Rust **reads the dataset**. The Rust
model loader (§4) exists for two things only: the round-trip parity test
(§7) and optional on-demand prediction for a single ticker, which is never
persisted and never feeds TCP or EVT. There is no second writer, and Rust never
invokes Julia for the daily inference. The jlrs and RustCall options in
`12-julia-rust-bridge` are documented later-phase options, not the daily
inference path.

## 1. The Export Format

**LightGBM `.txt`.** Not pickle. Not joblib. Not ONNX. Not PMML.

Why `.txt`:

- **Portable.** The LightGBM text format is read by LightGBM's own C
  library and, if they behave as described, by `bosk` (pure Rust) and
  `lightgbm-rust` (C bindings). Both crates are **unverified (verify):**
  existence, maintenance, API and compatibility with the pinned LightGBM
  version are to be checked before any Rust loader is built.
- **Human-readable.** You can open the file and inspect the trees. This
  matters when debugging why a prediction changed.
- **Versionable.** Git diffs of `.txt` model files show exactly which
  trees changed between generations.
- **Stable.** LightGBM guarantees forward compatibility of the text
  format across minor versions.
- **No Python dependency.** A `.txt` model does not require Python,
  LightGBM's Python bindings, or any runtime beyond the parser itself.

## 2. File Naming (the one convention)

    gspqr_<generation>_<horizon>_q<NN>.txt
    registry key:  <horizon>_q<NN>

`<generation>` is the cutoff date `m` in ISO format (the last panel date the
generation saw, `PREDICTION-BACKFILL.md` §1). `<horizon>` is `63d`, `126d`
or `252d`. `<NN>` is the **two-digit** integer percentile: `q01`, `q05`,
`q25`, `q50`, `q75`, `q95`, `q99`. Files live in
`models/generations/<generation>/`.

Example:

    models/generations/2026-09-30/gspqr_2026-09-30_63d_q05.txt
    models/generations/2026-09-30/gspqr_2026-09-30_63d_q50.txt
    models/generations/2026-09-30/gspqr_2026-09-30_252d_q99.txt

with registry keys `63d_q05`, `63d_q50`, `252d_q99`. This is the only form.
Variants such as `63d_q5`, `63d_q1` (unpadded) and `63d_q_05` (underscore)
are all wrong. The Parquet columns `q_01 ... q_99`
(`SCHEMAS.md` #7) keep their frozen underscore names; file names and
registry keys are a different namespace and use the form above.

## 3. Feature Name Embedding

LightGBM `.txt` files begin with a header block that includes
`feature_names=...`. This is critical: Rust inference reads the feature
names and asserts they match the frozen order.

If the feature names in the file do not match what Rust expects, the
load fails. This prevents the "I retrained with a different feature
order" bug that would otherwise produce silently wrong predictions.

**Do not strip the feature names from the export.** Do not rename
features on export. The feature names must match the panel column
names exactly.

## 4. Rust Model Loading (parity test and on-demand use only)

Two options. **Both are unverified (verify): the crate names, versions and
APIs below are a sketch** and must be confirmed against the actual crates
(and that they parse the pinned LightGBM's `.txt` output, including NaN
handling) before the loader or the parity test is written. If neither works,
the parity test cannot run and the Rust loader does not exist; Julia
inference is unaffected.

### `bosk` (pure Rust)

```rust
use bosk::lightgbm::{LightGBMModel, Predictor};

let model = LightGBMModel::from_file("models/gspqr_2026-10-01_63d_q50.txt")?;
assert_eq!(model.feature_names(), &EXPECTED_FEATURE_ORDER);

let x: Vec<f64> = /* 12 features in frozen order */;
let y_hat: f64 = model.predict(&x)?;
```

Advantages:
- No C dependency
- Cross-compiles trivially (any target, any OS)
- Small binary size
- Pure Rust safety

Disadvantages:
- Slightly slower than the C API for large batch predictions
- May lag LightGBM feature support (e.g., custom objectives)

For parity checks and occasional on-demand calls, "slower" is irrelevant.
`bosk` would be the default if it exists as described (verify).

### `lightgbm-rust` (C bindings)

```rust
use lightgbm::{Booster, Dataset};

let booster = Booster::from_file("models/gspqr_2026-10-01_63d_q50.txt")?;
let y_hat = booster.predict(&x, true)?;
```

Advantages:
- Full LightGBM feature set
- Same code path as training
- Faster on large batches

Disadvantages:
- Requires `lib_lightgbm.so` at runtime
- Complicates cross-compilation
- Adds a C dependency to the Rust build

Use only if `bosk` is missing or unusable. For standard quantile regression
`bosk` should suffice (verify).

## 5. Batch Inference (Julia writes; this Rust sketch is on-demand only)

Daily inference across the universe is a Julia job: load the live
generation, build `X` per horizon from `panel.parquet` for the day, predict
the seven models, `rearrange` (sort ascending, set `crossed`), write the
rows to `gspqr_predictions.parquet` with `generation` and `panel_version`
filled. The same job, pointed at a past generation and its date range,
is the backfill. Julia is the sole writer.

The Rust function below mirrors that logic for the **parity test and for
optional on-demand prediction that is never persisted**. It does not write
`gspqr_predictions.parquet`.

```rust
pub async fn predict_all(
    models: &ModelSet,
    panel: &Panel,
) -> Result<Vec<PredictionRow>, InferenceError> {
    let mut rows = Vec::with_capacity(panel.len() * 21);

    for horizon in [Horizon::D63, Horizon::D126, Horizon::D252] {
        for ticker in panel.tickers() {
            let x = panel.feature_vector(ticker, horizon);
            for (tau, model) in models.for_horizon(horizon) {
                let q = model.predict(&x)?;
                rows.push(PredictionRow {
                    ticker: ticker.clone(),
                    date: panel.date(),
                    horizon: horizon.label(),
                    quantile: tau,
                    value: q,
                });
            }
        }
    }

    Ok(rows)
}

/// Monotone rearrangement. Called on the seven raw values of one
/// (ticker, date, horizon) before anything is stored. Returns the sorted
/// values and whether the raw vector was already non-decreasing.
/// Non-finite input is an error; sorting must not hide a NaN.
pub fn rearrange(raw: [f64; 7]) -> Result<([f64; 7], bool), InferenceError> {
    if raw.iter().any(|v| !v.is_finite()) {
        return Err(InferenceError::NonFinitePrediction);
    }
    let crossed = raw.windows(2).any(|w| w[0] > w[1]);
    let mut sorted = raw;
    sorted.sort_by(f64::total_cmp); // inputs already checked finite
    Ok((sorted, crossed))
}
```

The seven per-quantile predictions of one `(ticker, date, horizon)` are
collected, passed through `rearrange`, and only then written (by Julia) to
`gspqr_predictions.parquet`: one row per (ticker, date, horizon), seven
quantile columns holding the **rearranged** values, plus the `crossed`
flag (true if the raw values were not non-decreasing). See
`13-parquet-lake/SCHEMAS.md` #7. The Julia path
(`models/src/inference/predict.jl`) uses `sort!` on the seven-vector and
`!issorted(raw)`; the Rust function above must give the same values and
flag (§7).

## 6. Feature Vector Construction in Rust

The 12 features come from the panel. The Rust loader (parity test or
on-demand use) reads the panel, filters to the requested date, and builds a
12-element vector per ticker.

```rust
const EXPECTED_FEATURE_ORDER: [&str; 12] = [
    "mom_20d", "mom_60d", "mom_12m_1m",
    "vol_60d_res", "sp_ratio", "turn_20d",
    "r_mom_20d", "r_mom_60d", "r_mom_12m_1m",
    "r_vol_60d", "r_sp", "r_turn",
];

fn feature_vector(row: &PanelRow) -> [f64; 12] {
    [
        row.mom_20d,
        row.mom_60d,
        row.mom_12m_1m,
        row.vol_60d_res,
        row.sp_ratio,
        row.turn_20d,
        row.r_mom_20d,
        row.r_mom_60d,
        row.r_mom_12m_1m,
        row.r_vol_60d,
        row.r_sp,
        row.r_turn,
    ]
}
```

NaN features are passed through as `f64::NAN`. `bosk` is expected to
handle them the same way LightGBM does (verify; the parity test checks it).

## 7. The Round-Trip Test

Before deploying a new generation, run a round-trip test:

1. Pick 100 rows from the validation set, plus at least 10 rows whose raw
   output is crossed (found in the validation set, or constructed by
   perturbing a quantile).
2. Compute Julia predictions for each row using the newly trained models,
   after rearrangement.
3. Compute Rust predictions for the same rows using the same models,
   after rearrangement.
4. Assert `max(abs(julia - rust)) < 1e-6` over all seven stored quantiles,
   and that every row is non-decreasing in quantile order.
5. Assert the `crossed` flags are equal. If the two languages differ on a
   row whose raw adjacent gap is below the 1e-6 tolerance, report the row;
   a flag mismatch on a gap larger than the tolerance fails the test.

This test catches:

- Feature order mismatches (predictions differ wildly)
- NaN handling differences (predictions differ on rows with NaN)
- Parser bugs in `bosk`
- A rearrangement implemented in only one language, or with different
  tie/NaN handling
- LightGBM version mismatches between training and inference

The test lives in `tests/export_roundtrip.rs` and runs in CI on every
commit that touches `models/` or `EXPORT.md`.

If the test fails, do not deploy. Fix the mismatch first.

## 8. What Not to Do

- Do not export to pickle, joblib, or any Python-specific format. Rust
  cannot read them.
- Do not export to ONNX. It adds a dependency and loses the tree
  structure that makes debugging possible.
- Do not strip feature names from the export.
- Do not rename features between training and inference.
- Do not assume `bosk` and `lightgbm-rust` produce identical output.
  Test both if you switch, and do not assume either exists as described
  (verify).
- Do not write `gspqr_predictions.parquet` from Rust. Julia is the sole
  writer; the Rust path is parity and on-demand only.
- Do not use a model file name or registry key other than `§2`
  (`63d_q05`).
- Do not write unsorted quantiles to `gspqr_predictions.parquet`.
- Do not skip the round-trip test. Silent prediction drift is the worst
  class of bug in a quant system.
- Do not deploy a model generation whose `.txt` files have been
  manually edited. Regenerate from training.