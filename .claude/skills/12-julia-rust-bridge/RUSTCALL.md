# RustCall.jl

Julia calling Rust. The package is `RustCall.jl`, registered in Julia's
General registry (verify).

**Later-phase option, not used today.** No current spec requires a Rust
function inside a Julia pipeline, and model fitting stays in Julia
(`SKILL.md` §3).

## 1. Installation

    using Pkg
    Pkg.add("RustCall")

Requirements: Julia 1.12+, Rust toolchain on PATH.

## 2. The `#[julia]` Attribute

The simplest path. Annotate a Rust function and call it directly from
Julia:

```julia
rust"""
#[julia]
fn add(a: i32, b: i32) -> i32 {
    a + b
}
"""

result = add(10, 20)  # 30
```

The `#[julia]` attribute converts the function to
`#[no_mangle] pub extern "C"` and generates a Julia wrapper with proper
type conversions.

## 3. The `@rust` Macro

For explicit control, export a `#[no_mangle] pub extern "C"` symbol and
call it with `@rust` :

```julia
rust"""
#[no_mangle]
pub extern "C" fn multiply(a: i32, b: i32) -> i32 {
    a * b
}
"""

result = @rust multiply(Int32(5), Int32(7))::Int32  # 35
```

`@rust` finds the exported symbol, converts arguments, and performs the
call. The `::Int32` annotation specifies the return type explicitly,
which is recommended.

## 4. Type Mapping

RustCall.jl maps Rust types to Julia types:

| Rust | Julia |
|---|---|
| `i8`..`i64` | `Int8`..`Int64` |
| `u8`..`u64` | `UInt8`..`UInt64` |
| `f32` | `Float32` |
| `f64` | `Float64` |
| `bool` | `Bool` |
| `*const u8` | `String` (C string) |

## 5. Ownership Types

For non-scalar data, RustCall.jl provides ownership wrappers:

| Type | Rust equivalent | Purpose |
|---|---|---|
| `RustBox<T>` | `Box<T>` | Single ownership |
| `RustRc<T>` | `Rc<T>` | Reference counting, single-threaded |
| `RustArc<T>` | `Arc<T>` | Atomic reference counting, thread-safe |
| `RustVec<T>` | `Vec<T>` | Owned vector |
| `RustSlice<T>` | `&[T]` | Borrowed slice |

Always call `drop!` on ownership types when done :

```julia
box = RustCall.RustBox{Int32}(ptr)
try
    # use box
finally
    RustCall.drop!(box)
end
```

## 6. When to Use RustCall.jl

- **Fast numerical kernels** that Julia's JIT cannot vectorize
- **Existing Rust crates** that have no Julia equivalent
- **Deterministic hot loops** where Julia's allocation overhead matters

Do not use it for:
- Anything already fast in Julia (most linear algebra, most statistics)
- Simple scalar arithmetic (Julia is fine)
- Code that crosses the boundary in a tight loop (batch instead)

## 7. The `@irust` Macro

For small scalar expressions inside Julia functions, `@irust` captures
Julia variables with `$var` :

```julia
function affine(a, x, b)
    @irust("\$a * \$x + \$b")
end
```

**Limitations:** scalars only, `$name` substitution is textual, not
type-stable. For anything beyond a small scalar expression, use
`rust"""..."""` with `@rust` .

## 8. What Not to Do

- Do not use RustCall.jl for the GSPQR inference path. GSPQR runs in
  Julia. The bridge is for code that *must* be in Rust.
- Do not skip the `::Type` annotation on `@rust` calls.
- Do not forget `drop!` on ownership types.
- Do not use `@irust` for anything involving strings, arrays, or
  structs.