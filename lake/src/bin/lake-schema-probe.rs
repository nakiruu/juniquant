//! Writes one empty leaf per registered dataset through the real writer, so the registry
//! generator can take each dataset's DuckDB `DESCRIBE` hash from it (13-parquet-lake SKILL.md §6).
//!
//! Usage: `lake-schema-probe <registry.json> <out-lake-dir> <locks-dir>`. The registry needs
//! only `fields`, keys, layout and merge; `columns`/`hash` may be absent.

use std::process::ExitCode;

use arrow::array::RecordBatch;
use lake::{Lake, Mode, Registry};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [registry, out, locks] = args.as_slice() else {
        eprintln!("usage: lake-schema-probe <registry.json> <out-lake-dir> <locks-dir>");
        return ExitCode::from(2);
    };
    match run(registry, out, locks) {
        Ok(n) => {
            println!("wrote {n} probe leaves to {out}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("lake-schema-probe: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(registry: &str, out: &str, locks: &str) -> Result<usize, Box<dyn std::error::Error>> {
    let lake = Lake::new(out, locks, Registry::load(registry.as_ref())?);
    for (name, spec) in &lake.registry().datasets {
        let empty = RecordBatch::new_empty(spec.schema()?);
        // Any valid partition: the leaf holds no rows.
        let partition: Vec<&str> = spec
            .partition_keys
            .iter()
            .map(|k| match k.as_str() {
                "year" => "1970",
                "month" => "01",
                _ => "1970-01-01",
            })
            .collect();
        lake.write_partition(name, &partition, &empty, Mode::Replace)?;
    }
    Ok(lake.registry().datasets.len())
}
