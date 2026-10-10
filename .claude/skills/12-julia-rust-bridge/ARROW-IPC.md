# Arrow IPC

The data exchange format between Julia and Rust. Arrow's columnar format
is designed for zero-copy exchange, but zero-copy is not guaranteed
across all data types.

## 1. Why Arrow

- **Columnar**: matches how both Julia (`DataFrames.jl`, `Arrow.jl`)
  and Rust (`arrow`, `polars`) represent tabular data 
- **Zero-copy capable**: the C Data Interface allows sharing Arrow
  arrays in-process without serialization 
- **Both languages support it**: Julia's `Arrow.jl` and Rust's `arrow`
  crate implement the IPC format 
- **Schema-preserving**: types and nullability travel with the data

## 2. The IPC Format

Two variants:

| Format | Use |
|---|---|
| **Stream** | Sequential batches, no random access |
| **File** | Random access, footer with metadata |

For Julia ↔ Rust exchange in the same process, use the **stream format**.
It has less overhead and no footer.

## 3. The C Data Interface

The C Data Interface defines a struct-based ABI for exchanging Schema
and Array objects without serialization. Both Julia and
Rust implement it.

**Important caveat:** not every language implements the C Data Interface.
Julia and Swift do not currently implement the C Stream Interface
. For Julia ↔ Rust, use the **IPC format**, not the C
Data Interface directly. The IPC format is universally supported by
both.

## 4. Julia Side

```julia
using Arrow

# Write
Arrow.write("data.arrow", table)

# Read
table = Arrow.Table("data.arrow")
```

## 5. Rust Side

```rust
use arrow::ipc::reader::StreamReader;
use arrow::ipc::writer::StreamWriter;

// Write
let file = File::create("data.arrow")?;
let mut writer = StreamWriter::try_new(file, &schema)?;
writer.write(&batch)?;
writer.finish()?;

// Read
let file = File::open("data.arrow")?;
let reader = StreamReader::try_new(file, None)?;
for batch in reader {
    let batch = batch?;
    // process
}
```

## 6. Copy Semantics

**Zero-copy is not guaranteed** . The actual semantics
depend on:

- Data type (fixed-width vs. variable-width)
- Whether dictionaries are involved
- The IPC reader's configuration

**Rule:** measure. If an exchange is on a performance-critical path,
benchmark it. If it copies, either accept the cost or investigate
the C Data Interface directly.

For the daily GSPQR inference (Julia, once per day, a few thousand rows,
written to Parquet for Rust to read), copy cost is irrelevant. For per-order exchanges, the bridge should be
avoided entirely (see `SKILL.md` §2).

## 7. Schema Consistency

The Arrow schema must match on both sides. A test asserts:

- Column names match
- Column types match
- Nullability matches
- Row count matches

The schema for each exchange is documented in this file. Changes to
the schema are a skill amendment.

## 8. What Not to Do

- Do not use JSON for arrays.
- Do not use CSV.
- Do not assume zero-copy without measuring.
- Do not exchange data without a schema assertion.
- Do not use the C Data Interface directly across Julia ↔ Rust; use
  the IPC format.