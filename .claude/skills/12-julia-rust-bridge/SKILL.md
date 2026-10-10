---
name: julia-rust-bridge
description: FFI patterns between Julia and Rust. Load when touching
  RustCall.jl, jlrs, Arrow IPC exchange, or any cross-language boundary.
---

# Julia–Rust Bridge

The bridge is where Julia's analytics and Rust's execution meet. This
skill documents which direction to bridge, when, and with what data
format.

## 0. The Current Boundary Is Files, Not FFI

In the current phase (data collection, then model training) **no FFI call
crosses the boundary at all**. Julia and Rust exchange data through files:

- Julia is the **sole writer** of `gspqr_predictions.parquet` (daily batch
  inference and the historical walk-forward backfill,
  `03-gspqr-model/PREDICTION-BACKFILL.md`, `EXPORT.md` §5). Rust reads the
  dataset.
- Rust loads the exported LightGBM `.txt` models only for the round-trip
  parity test and optional on-demand prediction that is **never persisted**
  and never feeds TCP or EVT (`03-gspqr-model/EXPORT.md` §4, §7). The `bosk`
  and `lightgbm-rust` crates are unverified (verify).
- Julia audit events go through the atomic-file spool (§4).
- Data tables are Parquet (and Arrow IPC for in-memory exchange, §4).

`jlrs` (Rust calls Julia) and `RustCall.jl` (Julia calls Rust) below are
**documented options for a later phase**, not part of the daily path. Nothing
in the system requires either today, and neither is installed or tested.

## 1. The Two Directions (later-phase options)

| Direction | Tool | Use case |
|---|---|---|
| Julia calls Rust | **RustCall.jl** | Julia needs a hot-path function written in Rust |
| Rust calls Julia | **jlrs** | Rust needs Julia's analytics for a decision |

Both are options, not requirements; see §3. The sole-writer rule of §0 holds
whichever is used. The details of each are in `RUSTCALL.md` (Julia calls Rust)
and `JLRS.md` (Rust calls Julia); both files describe **later-phase options**
only.

## 2. The Golden Rule: Do Not Bridge in the Hot Path

The system's hot path is the execution engine: order submission, lock
checks, risk gates. **This path must not cross the Julia boundary.**
Rust owns it end-to-end.

The Julia boundary is crossed (in a later phase, if ever) only for:
- **Model training** (Julia-only, no Rust)
- **Model inference** (Julia-only: batch inference writes
  `gspqr_predictions.parquet`; Rust reads the dataset. There is no
  daily Julia call from Rust)
- **Analytics** (Julia-only, no Rust)
- **One-off RPC** (a possible later option: Rust asks Julia for a specific
  prediction on demand; the answer is never persisted as a prediction row)

If you are writing code that calls Julia from within an order submission
path, stop. The design is wrong. Move the Julia call earlier, cache the
result, and let Rust read the cache.

## 3. Direction Decision

**Julia calls Rust** (later-phase option) when:
- A computation is on the critical path of a Julia pipeline
- The computation is not already a Julia function
- The Rust function has a simple scalar or array signature
- It is **not model training, calibration or fitting**: those are Julia's
  layer (constitution §4, "Rust never trains models"), so a GPD fit, a
  conformal quantile or an HMM does not move to Rust

There is no current example. A candidate would be a pure numeric primitive
(for instance a rolling-window kernel) after profiling shows Julia is too
slow; it would live in a Rust crate outside `models/` (the language-boundary
check allows `.rs` only under `execution/`, `ingest/`, `symbols/`). The earlier
example "GPD fitting in Rust called from `fit-tails.jl`" is withdrawn: it is
model fitting, which constitution §4 assigns to Julia, and `05-evt-tails`
specifies a native Julia MLE.

**Rust calls Julia** (later-phase option) when:
- Rust needs an analytic that only Julia computes and that is not a stored
  dataset
- The call is infrequent (once per forecast cycle, not once per order)
- The latency budget allows a round-trip

It is **not** the daily inference path: predictions are a Julia-written
dataset that Rust reads (§0). Julia never places orders (constitution §4),
so a call into Julia from the execution path is also excluded (§2).

## 4. Data Exchange Format

See `ARROW-IPC.md`. The short version:

**For arrays and tables: Arrow IPC.** Zero-copy where possible. No JSON.
No CSV. No serialization round-trips through strings.

**For scalars: direct FFI.** `Int32`, `Int64`, `Float64`, `Bool`.
RustCall.jl handles the conversion automatically.

**For structured records: Arrow IPC, not JSON.** JSON is for logs and
config, never for prediction outputs or feature matrices.

**For audit events from Julia: an atomic-file spool, not a shared
database.** Julia-originated audit events (`veto_triggered`,
`regime_flip`, `model_retrain_aborted`) are written as one JSON file per
event (log record, so JSON is allowed) to the spool directory
`$QUANT_STATE/audit-spool/` (`13-parquet-lake/IO-CONTRACT.md` §1): write `<event_id>.json.tmp`,
fsync, rename to `<event_id>.json`. The Rust audit writer ingests the
files and appends them to the audit journal; Julia never opens
`audit.db`. See `11-rust-event-bus` §5.

## 5. Memory Ownership

The most common source of FFI bugs is memory ownership confusion.

**Rule:** The language that allocates owns the deallocation, until it
explicitly transfers ownership. Transfers are explicit and documented.

RustCall.jl provides ownership types for this: `RustBox`, `RustRc`,
`RustArc`, `RustVec`, `RustSlice`. These map Rust's
ownership model to Julia's garbage collection.

**When passing arrays:**
- Rust allocates → Rust owns → Rust frees, or transfers ownership via
  `RustVec` to Julia
- Julia allocates → Julia owns → Julia frees, or Rust borrows for the
  duration of a call
- Never: Rust borrows a Julia array, then holds the pointer after the
  call returns

The `@rust` macro with explicit types ensures the borrow is scoped to
the call.

## 6. Arrow IPC

See `ARROW-IPC.md`. The key facts:

- Arrow's C Data Interface enables zero-copy exchange between runtimes
  that support it
- Julia's Arrow.jl and Rust's arrow crate both support the IPC format
 
- Zero-copy is not guaranteed across all data types or schemas.
  Document the copy semantics for every exchange

## 7. Error Handling

**Rust errors must be surfaced in Julia, not swallowed.**

RustCall.jl maps Rust `Result` to Julia's error mechanism. Do not
unwrap in Rust code that crosses the boundary. Do not `@assert` in
Julia code that calls Rust. Use `RustCall.is_err` and `unwrap`.

**Julia errors must be surfaced in Rust, not swallowed.**

jlrs propagates Julia exceptions as Rust errors. Do not use `try/catch`
in Rust code that calls Julia unless you are logging and re-raising.

## 8. What Not to Do

- Do not cross the boundary in the execution hot path.
- Do not add a second writer of `gspqr_predictions.parquet`. Julia writes it;
  the Rust loader is for parity and on-demand use only.
- Do not move model fitting (GPD, conformal, HMM, LightGBM) into Rust.
- Do not use JSON for arrays or prediction outputs.
- Do not hold a Rust pointer to a Julia array after the call returns.
- Do not use `unwrap()` in Rust code that crosses the boundary.
- Do not use `@assert` in Julia code that calls Rust.
- Do not assume Arrow is zero-copy without verifying per data type.
- Do not call Julia from Rust in a tight loop. Batch the calls.
- Do not call Rust from Julia for something Julia can do in <1ms.

## 9. Verification

- [ ] A round-trip test passes on 100 rows: Julia prediction, Rust
      prediction, max difference < 1e-6, after monotone rearrangement on
      both sides (including crossed rows and equal `crossed` flags,
      `03-gspqr-model/EXPORT.md` §7). It needs the Rust loader crate, which
      is unverified; until it exists the test cannot run
- [ ] A grep test asserts only Julia code writes `gspqr_predictions`
- [ ] A test asserts the Arrow schema matches on both sides
- [ ] A test asserts memory is freed (no leak) after 10,000 calls
- [ ] A test asserts errors propagate correctly in both directions
- [ ] No bridge call exists in the order submission path
- [ ] A test asserts a Julia-written spool file is ingested exactly once
      and a `*.json.tmp` file is ignored (`11-rust-event-bus` §9)

## 10. Related Skills

- `03-gspqr-model` — the model export/load path
- `10-execution-schwab` — the Rust execution engine
- `11-rust-event-bus` — where bridge calls are scheduled
- `13-parquet-lake` — the shared data format