//! IO-CONTRACT.md §3-§5 against the committed registry: validation, merge rules, sort,
//! idempotency, atomicity (a killed write leaves only a dot-file), stale-temp cleanup, the
//! single-writer lock and read pruning.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    reason = "integration tests: any failure must fail the test"
)]

use std::fs::{self, File, FileTimes};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use arrow::array::{
    ArrayRef, AsArray, BooleanArray, Date32Array, Float64Array, Int16Array, Int32Array, Int64Array,
    RecordBatch, StringArray, TimestampNanosecondArray,
};
use arrow::datatypes::Date32Type;
use lake::{Lake, LakeError, Mode, Registry};
use tempfile::TempDir;

/// A cell value; `None` fields take a per-type default.
#[derive(Clone)]
enum V {
    S(&'static str),
    I(i64),
    F(f64),
    D(i32),
    Null,
}

fn registry() -> Registry {
    Registry::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../schemas/registry.json")).unwrap()
}

fn lake() -> (TempDir, Lake) {
    let dir = TempDir::new().unwrap();
    let lake = Lake::new(
        dir.path().join("lake"),
        dir.path().join("locks"),
        registry(),
    );
    (dir, lake)
}

/// A batch of `n` rows for `name`, cells from `cell(column, row)`.
fn batch(
    lake: &Lake,
    name: &str,
    n: usize,
    cell: impl Fn(&str, usize) -> Option<V>,
) -> RecordBatch {
    let spec = lake.registry().get(name).unwrap();
    let cols: Vec<ArrayRef> = spec
        .fields
        .iter()
        .map(|f| {
            let vals: Vec<V> = (0..n)
                .map(|i| {
                    cell(&f.name, i).unwrap_or(match f.ty.as_str() {
                        "utf8" => V::S("x"),
                        "float64" => V::F(0.0),
                        "date32" | "timestamp[ns, UTC]" => V::D(0),
                        _ => V::I(0),
                    })
                })
                .collect();
            let int = |v: &V| match v {
                V::I(x) => Some(*x),
                V::D(x) => Some(i64::from(*x)),
                _ => None,
            };
            let arr: ArrayRef = match f.ty.as_str() {
                "utf8" => Arc::new(StringArray::from_iter(vals.iter().map(|v| match v {
                    V::S(s) => Some(*s),
                    _ => None,
                }))),
                "float64" => Arc::new(Float64Array::from_iter(vals.iter().map(|v| match v {
                    V::F(x) => Some(*x),
                    _ => None,
                }))),
                "bool" => Arc::new(BooleanArray::from_iter(
                    vals.iter().map(|v| int(v).map(|x| x != 0)),
                )),
                "int16" => Arc::new(Int16Array::from_iter(
                    vals.iter().map(|v| int(v).map(|x| x as i16)),
                )),
                "int32" => Arc::new(Int32Array::from_iter(
                    vals.iter().map(|v| int(v).map(|x| x as i32)),
                )),
                "int64" => Arc::new(Int64Array::from_iter(vals.iter().map(int))),
                "date32" => Arc::new(Date32Array::from_iter(
                    vals.iter().map(|v| int(v).map(|x| x as i32)),
                )),
                _ => Arc::new(
                    TimestampNanosecondArray::from_iter(
                        vals.iter().map(|v| int(v).map(|d| d * 86_400_000_000_000)),
                    )
                    .with_timezone("UTC"),
                ),
            };
            arr
        })
        .collect();
    // Declared nullable, as Julia Arrow files often are: the writer checks the nulls itself.
    let loose = arrow::datatypes::Schema::new(
        spec.schema()
            .unwrap()
            .fields()
            .iter()
            .map(|f| f.as_ref().clone().with_nullable(true))
            .collect::<Vec<_>>(),
    );
    RecordBatch::try_new(Arc::new(loose), cols).unwrap()
}

// 2024-01-02 and 2024-01-03 as days since the epoch.
const JAN2: i32 = 19_724;
const JAN3: i32 = 19_725;

fn prices(lake: &Lake, rows: &[(&'static str, i32, f64)]) -> RecordBatch {
    batch(lake, "prices", rows.len(), |c, i| match c {
        "ticker" => Some(V::S(rows[i].0)),
        "date" => Some(V::D(rows[i].1)),
        "close" => Some(V::F(rows[i].2)),
        "fetched_at" => Some(V::D(rows[i].1)),
        _ => None,
    })
}

fn leaf(dir: &TempDir) -> PathBuf {
    dir.path()
        .join("lake/prices/year=2024/month=01/data.parquet")
}

fn closes(lake: &Lake) -> Vec<(String, f64)> {
    let b = lake
        .read_dataset("prices", Some(&["ticker", "close"]), &[])
        .unwrap();
    let t = b.column(0).as_string::<i32>();
    let c = b.column(1).as_primitive::<arrow::datatypes::Float64Type>();
    (0..b.num_rows())
        .map(|i| (t.value(i).to_owned(), c.value(i)))
        .collect()
}

#[test]
fn round_trip_keeps_registered_schema_and_sorts_by_primary_key() {
    let (dir, lake) = lake();
    let input = prices(
        &lake,
        &[
            ("MSFT", JAN2, 2.0),
            ("AAPL", JAN3, 1.0),
            ("AAPL", JAN2, 3.0),
        ],
    );
    lake.write_partition("prices", &["2024", "01"], &input, Mode::Append)
        .unwrap();
    assert!(leaf(&dir).is_file());
    let back = lake.read_dataset("prices", None, &[]).unwrap();
    assert_eq!(
        back.schema(),
        lake.registry().get("prices").unwrap().schema().unwrap(),
        "relabelled to the registered nullability"
    );
    assert_eq!(
        closes(&lake),
        [
            ("AAPL".into(), 3.0),
            ("AAPL".into(), 1.0),
            ("MSFT".into(), 2.0)
        ]
    );
}

#[test]
fn rewrite_of_same_input_is_byte_identical() {
    let (dir, lake) = lake();
    let input = prices(&lake, &[("AAPL", JAN2, 1.0), ("MSFT", JAN3, 2.0)]);
    lake.write_partition("prices", &["2024", "01"], &input, Mode::Append)
        .unwrap();
    let first = fs::read(leaf(&dir)).unwrap();
    lake.write_partition("prices", &["2024", "01"], &input, Mode::Append)
        .unwrap();
    assert_eq!(first, fs::read(leaf(&dir)).unwrap());
    lake.write_partition("prices", &["2024", "01"], &input, Mode::Replace)
        .unwrap();
    assert_eq!(first, fs::read(leaf(&dir)).unwrap());
    assert_eq!(closes(&lake).len(), 2);
}

#[test]
fn overwrite_merge_new_row_wins_and_others_stay() {
    let (_dir, lake) = lake();
    let a = prices(&lake, &[("AAPL", JAN2, 1.0), ("MSFT", JAN2, 2.0)]);
    lake.write_partition("prices", &["2024", "01"], &a, Mode::Append)
        .unwrap();
    let b = prices(&lake, &[("AAPL", JAN2, 9.0), ("AAPL", JAN3, 4.0)]);
    lake.write_partition("prices", &["2024", "01"], &b, Mode::Append)
        .unwrap();
    assert_eq!(
        closes(&lake),
        [
            ("AAPL".into(), 9.0),
            ("AAPL".into(), 4.0),
            ("MSFT".into(), 2.0)
        ]
    );
    lake.write_partition("prices", &["2024", "01"], &b, Mode::Replace)
        .unwrap();
    assert_eq!(closes(&lake).len(), 2, "replace drops stored rows");
}

fn costs(lake: &Lake, slip: f64) -> RecordBatch {
    batch(lake, "execution_costs", 1, |c, _| match c {
        "date" => Some(V::D(JAN2)),
        "cost_buy_bps" => Some(V::F(slip)),
        _ => None,
    })
}

#[test]
fn append_only_drops_identical_rows_and_rejects_different_ones() {
    let (dir, lake) = lake();
    lake.write_dataset("execution_costs", &costs(&lake, 1.0), Mode::Append)
        .unwrap();
    let path = dir
        .path()
        .join("lake/execution_costs/year=2024/data.parquet");
    let before = fs::read(&path).unwrap();
    lake.write_dataset("execution_costs", &costs(&lake, 1.0), Mode::Append)
        .unwrap();
    assert_eq!(before, fs::read(&path).unwrap());
    let err = lake
        .write_dataset("execution_costs", &costs(&lake, 2.0), Mode::Append)
        .unwrap_err();
    assert!(
        matches!(err, LakeError::AppendOnlyConflict { row: 0, .. }),
        "{err}"
    );
    assert_eq!(
        before,
        fs::read(&path).unwrap(),
        "a failed write leaves the partition untouched"
    );
}

#[test]
fn audit_log_without_primary_key_dedups_whole_rows_and_partitions_by_utc_day() {
    let (dir, lake) = lake();
    let ev = |id: &'static str, day: i32| {
        batch(&lake, "audit_log", 1, move |c, _| match c {
            "event_id" => Some(V::S(id)),
            "timestamp" => Some(V::D(day)),
            "ticker" => Some(V::Null),
            _ => None,
        })
    };
    lake.write_dataset("audit_log", &ev("e1", JAN2), Mode::Append)
        .unwrap();
    lake.write_dataset("audit_log", &ev("e1", JAN2), Mode::Append)
        .unwrap();
    lake.write_dataset("audit_log", &ev("e2", JAN2), Mode::Append)
        .unwrap();
    assert!(
        dir.path()
            .join("lake/audit_log/date=2024-01-02/data.parquet")
            .is_file()
    );
    assert_eq!(
        lake.read_dataset("audit_log", None, &[])
            .unwrap()
            .num_rows(),
        2
    );
}

#[test]
fn null_key_columns_compare_equal() {
    let (_dir, lake) = lake();
    let row = |crps: f64| {
        batch(&lake, "evaluation_metrics", 1, move |c, _| match c {
            "crps" => Some(V::F(crps)),
            "run_id" | "track" | "path_id" | "fold" | "spread_source" => Some(V::Null),
            _ if c.starts_with("q") || c.ends_with("frac") || c.ends_with("bps") => Some(V::Null),
            _ => None,
        })
    };
    lake.write_dataset("evaluation_metrics", &row(1.0), Mode::Append)
        .unwrap();
    lake.write_dataset("evaluation_metrics", &row(2.0), Mode::Append)
        .unwrap();
    let b = lake
        .read_dataset("evaluation_metrics", Some(&["crps"]), &[])
        .unwrap();
    assert_eq!(
        b.num_rows(),
        1,
        "legacy rows stay unique on (eval_date, horizon)"
    );
}

#[test]
fn validation_rejects_and_writes_nothing() {
    let (dir, lake) = lake();
    let dup = prices(&lake, &[("AAPL", JAN2, 1.0), ("AAPL", JAN2, 2.0)]);
    assert!(matches!(
        lake.write_partition("prices", &["2024", "01"], &dup, Mode::Append),
        Err(LakeError::DuplicateKey { row: 1, .. })
    ));
    let feb = prices(&lake, &[("AAPL", JAN2 + 31, 1.0)]);
    assert!(matches!(
        lake.write_partition("prices", &["2024", "01"], &feb, Mode::Append),
        Err(LakeError::Partition { .. })
    ));
    let ok = prices(&lake, &[("AAPL", JAN2, 1.0)]);
    for bad in [
        &["2024"][..],
        &["2024", "1"],
        &["2024", "13"],
        &["../x", "01"],
    ] {
        assert!(matches!(
            lake.write_partition("prices", bad, &ok, Mode::Append),
            Err(LakeError::Partition { .. })
        ));
    }
    let wrong_type = ok.project(&[1, 0, 2, 3, 4, 5, 6, 7, 8, 9, 10]).unwrap();
    assert!(matches!(
        lake.write_partition("prices", &["2024", "01"], &wrong_type, Mode::Append),
        Err(LakeError::Schema { .. })
    ));
    let null_ticker = batch(&lake, "prices", 1, |c, _| match c {
        "ticker" => Some(V::Null),
        "date" | "fetched_at" => Some(V::D(JAN2)),
        _ => None,
    });
    assert!(matches!(
        lake.write_partition("prices", &["2024", "01"], &null_ticker, Mode::Append),
        Err(LakeError::Schema { .. })
    ));
    assert!(!dir.path().join("lake/prices").exists());
    assert!(matches!(
        lake.write_partition("nope", &[], &ok, Mode::Append),
        Err(LakeError::UnknownDataset(_))
    ));
}

#[test]
fn killed_write_leaves_only_a_dot_file_that_readers_ignore_and_a_later_run_removes() {
    let (dir, lake) = lake();
    lake.write_partition(
        "prices",
        &["2024", "01"],
        &prices(&lake, &[("AAPL", JAN2, 1.0)]),
        Mode::Append,
    )
    .unwrap();
    let leaf_dir = leaf(&dir).parent().unwrap().to_path_buf();
    // A process killed between create and rename leaves this behind.
    let stale = leaf_dir.join(".data.parquet.tmp.1.1");
    let fresh = leaf_dir.join(".data.parquet.tmp.2.2");
    fs::write(&stale, b"torn").unwrap();
    fs::write(&fresh, b"in flight").unwrap();
    let old = SystemTime::now() - Duration::from_secs(7200);
    File::options()
        .write(true)
        .open(&stale)
        .unwrap()
        .set_times(FileTimes::new().set_modified(old))
        .unwrap();
    assert_eq!(closes(&lake), [("AAPL".into(), 1.0)]);
    lake.write_partition(
        "prices",
        &["2024", "01"],
        &prices(&lake, &[("MSFT", JAN2, 2.0)]),
        Mode::Append,
    )
    .unwrap();
    assert!(
        !stale.exists(),
        "temporaries older than an hour are removed"
    );
    assert!(
        fresh.exists(),
        "a recent temporary may belong to a live run"
    );
    assert_eq!(closes(&lake).len(), 2);
}

#[test]
fn second_concurrent_run_is_refused() {
    let (dir, lake) = lake();
    fs::create_dir_all(dir.path().join("locks")).unwrap();
    let held = File::create(dir.path().join("locks/prices.lock")).unwrap();
    held.lock().unwrap();
    let input = prices(&lake, &[("AAPL", JAN2, 1.0)]);
    assert!(matches!(
        lake.write_dataset("prices", &input, Mode::Append),
        Err(LakeError::LockHeld { .. })
    ));
    held.unlock().unwrap();
    lake.write_dataset("prices", &input, Mode::Append).unwrap();
}

#[test]
fn write_dataset_splits_by_partition_and_read_prunes() {
    let (dir, lake) = lake();
    let input = prices(
        &lake,
        &[
            ("AAPL", JAN2, 1.0),
            ("AAPL", JAN2 + 31, 2.0),
            ("AAPL", JAN2 + 366, 3.0),
        ],
    );
    lake.write_dataset("prices", &input, Mode::Append).unwrap();
    for p in [
        "year=2024/month=01",
        "year=2024/month=02",
        "year=2025/month=01",
    ] {
        assert!(
            dir.path()
                .join("lake/prices")
                .join(p)
                .join("data.parquet")
                .is_file(),
            "{p}"
        );
    }
    let jan = lake
        .read_dataset("prices", Some(&["date", "ticker"]), &[("month", "01")])
        .unwrap();
    assert_eq!(jan.num_rows(), 2);
    assert_eq!(
        jan.schema().field(0).name(),
        "date",
        "columns come back in the requested order"
    );
    let d = jan.column(0).as_primitive::<Date32Type>();
    assert_eq!((d.value(0), d.value(1)), (JAN2, JAN2 + 366));
    assert_eq!(
        lake.read_dataset("splits", None, &[]).unwrap().num_rows(),
        0,
        "unwritten reads as empty"
    );
}

#[test]
fn wrong_timestamp_unit_is_a_schema_error_not_a_panic() {
    let (_dir, lake) = lake();
    let ok = batch(&lake, "audit_log", 1, |c, _| {
        (c == "ticker").then_some(V::Null)
    });
    let mut cols = ok.columns().to_vec();
    cols[1] =
        Arc::new(arrow::array::TimestampMicrosecondArray::from(vec![0_i64]).with_timezone("UTC"));
    let fields: Vec<_> = ok
        .schema()
        .fields()
        .iter()
        .zip(&cols)
        .map(|(f, c)| f.as_ref().clone().with_data_type(c.data_type().clone()))
        .collect();
    let micros =
        RecordBatch::try_new(Arc::new(arrow::datatypes::Schema::new(fields)), cols).unwrap();
    assert!(matches!(
        lake.write_dataset("audit_log", &micros, Mode::Append),
        Err(LakeError::Schema { .. })
    ));
}

#[test]
fn duplicate_whole_rows_in_one_input_are_rejected_without_a_primary_key() {
    let (_dir, lake) = lake();
    let twice = batch(&lake, "audit_log", 2, |c, _| {
        (c == "ticker").then_some(V::Null)
    });
    assert!(matches!(
        lake.write_dataset("audit_log", &twice, Mode::Replace),
        Err(LakeError::DuplicateKey { row: 1, .. })
    ));
}
