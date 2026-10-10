# jlrs

Rust calling Julia. The crate is `jlrs`, which provides access to the
Julia C API.

**Later-phase option, not used today.** The daily GSPQR inference is a Julia
job that writes `gspqr_predictions.parquet`; Rust reads the dataset
(`SKILL.md` §0, `03-gspqr-model/EXPORT.md` §5). Nothing in this file is
part of the current pipeline, and the crate version and feature claims below
are unverified (verify).

## 1. Installation

Add to `Cargo.toml`:

```toml
[dependencies]
jlrs = { version = "0.22", features = ["local-rt"] }
```

**Version features:** Exactly one Julia version feature must be enabled.
Supported: `julia-1-10`, `julia-1-11`, `julia-1-12`, `julia-1-13` .

**Runtime features:** `local-rt` (single-threaded blocking), `tokio-rt`
(async, Tokio-backed), `multi-rt` (multithreaded) .

**Requirements:** Julia 1.10–1.13, Rust 1.97+ .

**Environment:** If Julia is not on PATH, set `JULIA_DIR` to the Julia
installation directory. On Linux, add `/path/to/julia/lib` to
`LD_LIBRARY_PATH` .

## 2. Initialization

Julia must be initialized before use. The `Builder` configures the runtime
:

```rust
use jlrs::prelude::*;

fn main() {
    let handle = Builder::new()
        .start_local()
        .expect("Julia initialization failed");

    // ... use handle to call into Julia
}
```

The runtime shuts down when all handles are dropped. `Julia::init` is
deprecated in favor of the local runtime's `LocalHandle` .

## 3. Calling Julia Functions

Use a scope (`frame`) to protect values from Julia's garbage collector
:

```rust
handle.with_local_scope(|frame| {
    let func = Module::base(global).function("sqrt")?;
    let result = func.call(frame, (4.0,))?;
    let x = result.unbox::<f64>()?;
    assert_eq!(x, 2.0);
    Ok(())
})?;
```

Values created inside a frame are valid only within that frame. Do not
hold `Value` across scope boundaries .

## 4. From `ccall`

When Julia calls Rust via `ccall` and Rust needs to call back into Julia,
use `CCall` instead of initializing the runtime :

```rust
#[no_mangle]
pub extern "C" fn my_function() -> f64 {
    let ccall = unsafe { CCall::new() };
    ccall.frame(2, |global, frame| {
        // call Julia functions here
        Ok(())
    }).unwrap();
    0.0
}
```

`CCall` must only be used inside a function called through `ccall` from
Julia, and only once per call.

## 5. The `julia_module` Macro

Export Rust functions to Julia as a module:

```rust
julia_module! {
    become my_module_init;
    fn my_rust_function(x: f64) -> f64;
}
```

On the Julia side, use `JlrsCore.Wrap` to load the module .

## 6. When to Use jlrs

- **Embedding Julia** in a Rust application (a possible later option for an
  on-demand analytic; not the daily inference job, which is Julia-only)
- **Writing interop libraries** that are called from Julia but need
  Julia state
- **Long-running Rust services** that need Julia analytics

Do not use it for:
- Simple scalar calls (use `ccall` directly)
- Code that runs in the execution hot path

## 7. Performance Notes

jlrs is designed for correctness and safety, not raw speed. Each call
has overhead from frame management and GC protection. For batch
inference, call once with an array of feature vectors, not once per
ticker.

The async runtime runs Julia on a separate thread and uses a task-based
system. This is the recommended pattern for a Rust service that calls
Julia periodically.

**LTO:** Enable the `lto` feature and set `RUSTFLAGS` for cross-language
LTO to recover performance lost from the C API redesign in v0.20
.

## 8. What Not to Do

- Do not call `Builder::start_local` in a `ccall` context. Use `CCall`.
- Do not hold `Value` across frame boundaries.
- Do not call Julia from multiple threads without the async runtime.
- Do not use jlrs in the execution hot path. Cache predictions instead.
- Do not enable a runtime feature in a library crate. Only applications
  that embed Julia should enable runtimes.
