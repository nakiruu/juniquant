//! `write_partition` / `write_dataset` (`IO-CONTRACT.md` §3): validate, merge on the primary
//! key, sort, write a ZSTD-3 temporary file, fsync, rename, fsync the directory.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use arrow::array::{Array, ArrayRef, AsArray, RecordBatch, UInt32Array};
use arrow::compute::{concat_batches, take_record_batch};
use arrow::datatypes::{DataType, Date32Type, SchemaRef, TimestampNanosecondType};
use arrow::row::{RowConverter, Rows, SortField};
use chrono::NaiveDate;
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::properties::WriterProperties;

use crate::error::io_err;
use crate::read::read_leaf;
use crate::{DatasetSpec, Lake, LakeError, Layout, Merge};

/// Days from 0001-01-01 (day 1 of the Common Era) to 1970-01-01.
const UNIX_EPOCH_DAYS_FROM_CE: i32 = 719_163;

const NANOS_PER_DAY: i64 = 86_400_000_000_000;

/// Rows per row group, fixed so a re-run writes the same file layout (`IO-CONTRACT.md` §3).
pub const ROW_GROUP_ROWS: usize = 1 << 20;

/// Temporary files older than this are deleted at the start of a run (`IO-CONTRACT.md` §3).
const STALE_TEMP_AGE: Duration = Duration::from_secs(3600);

/// How the input combines with the stored partition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Read the stored leaf, merge on the primary key by the dataset's rule, rewrite.
    Append,
    /// Write the input alone (rebuilds).
    Replace,
}

impl Lake {
    /// Write one leaf partition of `name`. `partition` holds the values of the dataset's
    /// partition keys in order (`["2024", "01"]` for `year`/`month`; empty for a single file).
    ///
    /// # Errors
    ///
    /// Validation errors ([`LakeError::Schema`], [`LakeError::Partition`],
    /// [`LakeError::DuplicateKey`]), [`LakeError::AppendOnlyConflict`], [`LakeError::LockHeld`]
    /// and I/O or Parquet errors. On any error the stored partition is unchanged.
    pub fn write_partition(
        &self,
        name: &str,
        partition: &[&str],
        batch: &RecordBatch,
        mode: Mode,
    ) -> Result<(), LakeError> {
        let spec = self.registry.get(name)?;
        let _lock = self.begin_run(name, spec)?;
        self.write_leaf(name, spec, partition, batch, mode)
    }

    /// Write `batch` to every partition it touches, one [`Lake::write_partition`] per group,
    /// under one lock. Each partition is atomic; the dataset as a whole is not, and a failed
    /// run is re-run (the merge makes that safe).
    ///
    /// # Errors
    ///
    /// As [`Lake::write_partition`]; the leaves are written in parallel, so leaves written
    /// before the failure was seen stay written (the error is that of the first failing leaf in
    /// partition-value order).
    pub fn write_dataset(
        &self,
        name: &str,
        batch: &RecordBatch,
        mode: Mode,
    ) -> Result<(), LakeError> {
        let spec = self.registry.get(name)?;
        let _lock = self.begin_run(name, spec)?;
        if spec.layout == Layout::Single {
            return self.write_leaf(name, spec, &[], batch, mode);
        }
        // Conform first: partition_values reads the partition column by its registered type.
        let schema = spec.schema().map_err(|m| schema_err(name, m))?;
        let batch = &conform(name, &schema, batch)?;
        let (keys, ids) = partition_ids(name, spec, batch)?;
        let mut rows: Vec<Vec<u32>> = vec![Vec::new(); keys.len()];
        for (row, &id) in ids.iter().enumerate() {
            let row = u32::try_from(row).map_err(|_| too_many(name))?;
            rows.get_mut(id as usize)
                .into_iter()
                .for_each(|r| r.push(row));
        }
        // Leaves in partition-value order; each is an independent file, so they are written by
        // as many threads as the machine has cores. The first failing leaf (in that order) is
        // the error; leaves written by then stay written, as `write_dataset` says.
        let mut order: Vec<usize> = (0..keys.len()).collect();
        order.sort_by(|&a, &b| keys.get(a).cmp(&keys.get(b)));
        let next = AtomicUsize::new(0);
        let failed = AtomicBool::new(false);
        let first_error: Mutex<Option<(usize, LakeError)>> = Mutex::new(None);
        let workers = std::thread::available_parallelism()
            .map_or(1, std::num::NonZero::get)
            .min(order.len());
        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| {
                    loop {
                        let job = next.fetch_add(1, Ordering::Relaxed);
                        let Some(&leaf) = order.get(job) else { break };
                        if failed.load(Ordering::Relaxed) {
                            break;
                        }
                        let result = (|| -> Result<(), LakeError> {
                            let picked =
                                UInt32Array::from(rows.get(leaf).cloned().unwrap_or_default());
                            let part = take_record_batch(batch, &picked)?;
                            let values: Vec<&str> = keys
                                .get(leaf)
                                .map(|k| k.iter().map(String::as_str).collect())
                                .unwrap_or_default();
                            self.write_leaf(name, spec, &values, &part, mode)
                        })();
                        if let Err(e) = result {
                            failed.store(true, Ordering::Relaxed);
                            if let Ok(mut slot) = first_error.lock()
                                && slot.as_ref().is_none_or(|(j, _)| job < *j)
                            {
                                *slot = Some((job, e));
                            }
                        }
                    }
                });
            }
        });
        match first_error.into_inner().ok().flatten() {
            Some((_, e)) => Err(e),
            None => Ok(()),
        }
    }

    /// Path of the leaf file for `partition` (validated values).
    pub(crate) fn leaf_path(&self, name: &str, spec: &DatasetSpec, partition: &[&str]) -> PathBuf {
        match spec.layout {
            Layout::Single => self.root.join(format!("{name}.parquet")),
            Layout::Partitioned => {
                let mut p = self.root.join(name);
                for (k, v) in spec.partition_keys.iter().zip(partition) {
                    p.push(format!("{k}={v}"));
                }
                p.join("data.parquet")
            }
        }
    }

    /// Take the single-writer lock and delete stale temporaries.
    fn begin_run(&self, name: &str, spec: &DatasetSpec) -> Result<File, LakeError> {
        fs::create_dir_all(&self.locks).map_err(io_err(&self.locks))?;
        let path = self.locks.join(format!("{name}.lock"));
        let file = File::create(&path).map_err(io_err(&path))?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(LakeError::LockHeld {
                    dataset: name.to_owned(),
                    path,
                });
            }
            Err(TryLockError::Error(e)) => return Err(io_err(&path)(e)),
        }
        let (dir, prefix) = match spec.layout {
            Layout::Single => (self.root.clone(), format!(".{name}.parquet.tmp.")),
            Layout::Partitioned => (self.root.join(name), ".data.parquet.tmp.".to_owned()),
        };
        remove_stale_temps(&dir, &prefix, spec.layout == Layout::Partitioned)?;
        Ok(file)
    }

    fn write_leaf(
        &self,
        name: &str,
        spec: &DatasetSpec,
        partition: &[&str],
        batch: &RecordBatch,
        mode: Mode,
    ) -> Result<(), LakeError> {
        let schema = spec.schema().map_err(|m| schema_err(name, m))?;
        let batch = validate(name, spec, &schema, partition, batch)?;
        let path = self.leaf_path(name, spec, partition);
        let existing = match mode {
            Mode::Append if path.exists() => {
                Some(conform(name, &schema, &read_leaf(&path, None, None)?)?)
            }
            _ => None,
        };
        let merged = merge(name, spec, existing.as_ref(), &batch)?;
        let sorted = sort(spec, &merged)?;
        write_atomic(&path, &sorted)
    }
}

/// Step 1: exact columns and types, no nulls in a required column, unique primary key, every
/// row in `partition`. Returns the batch carrying the registered schema.
fn validate(
    name: &str,
    spec: &DatasetSpec,
    schema: &SchemaRef,
    partition: &[&str],
    batch: &RecordBatch,
) -> Result<RecordBatch, LakeError> {
    let batch = conform(name, schema, batch)?;
    let part_err = |message: String| LakeError::Partition {
        dataset: name.to_owned(),
        message,
    };
    if partition.len() != spec.partition_keys.len() {
        return Err(part_err(format!(
            "{} values given for keys {:?}",
            partition.len(),
            spec.partition_keys
        )));
    }
    for (k, v) in spec.partition_keys.iter().zip(partition) {
        let ok = match k.as_str() {
            "year" => v.len() == 4 && v.bytes().all(|b| b.is_ascii_digit()),
            "month" => matches!(v.parse::<u8>(), Ok(1..=12)) && v.len() == 2,
            _ => NaiveDate::parse_from_str(v, "%Y-%m-%d").is_ok_and(|d| d.to_string() == *v),
        };
        if !ok {
            return Err(part_err(format!("bad value `{v}` for key `{k}`")));
        }
    }
    if spec.layout == Layout::Partitioned {
        let (keys, ids) = partition_ids(name, spec, &batch)?;
        let matches: Vec<bool> = keys
            .iter()
            .map(|k| k.iter().map(String::as_str).eq(partition.iter().copied()))
            .collect();
        let wrong = ids
            .iter()
            .position(|&id| !matches.get(id as usize).copied().unwrap_or(false));
        if let Some(row) = wrong {
            let key = ids.get(row).and_then(|&id| keys.get(id as usize));
            return Err(part_err(format!(
                "row {row} belongs to {key:?}, not {partition:?}"
            )));
        }
    }
    // Without a primary key (audit_log) the whole row is the key.
    let all: Vec<String> = spec.fields.iter().map(|f| f.name.clone()).collect();
    let key = if spec.primary_key.is_empty() {
        &all
    } else {
        &spec.primary_key
    };
    {
        let rows = key_rows(spec, key, &batch)?;
        let mut seen = HashSet::with_capacity(rows.num_rows());
        for (row, r) in rows.iter().enumerate() {
            if !seen.insert(r) {
                return Err(LakeError::DuplicateKey {
                    dataset: name.to_owned(),
                    row,
                });
            }
        }
    }
    Ok(batch)
}

/// Re-label `batch` with the registered schema after checking names, types and nulls
/// (an input column may be declared nullable as long as it holds no null).
fn conform(name: &str, schema: &SchemaRef, batch: &RecordBatch) -> Result<RecordBatch, LakeError> {
    let have = batch.schema();
    if have.fields().len() != schema.fields().len() {
        return Err(schema_err(
            name,
            format!(
                "{} columns, expected {}",
                have.fields().len(),
                schema.fields().len()
            ),
        ));
    }
    for ((want, got), col) in schema
        .fields()
        .iter()
        .zip(have.fields())
        .zip(batch.columns())
    {
        if want.name() != got.name() || want.data_type() != got.data_type() {
            return Err(schema_err(
                name,
                format!(
                    "column `{}: {}` where `{}: {}` is registered",
                    got.name(),
                    got.data_type(),
                    want.name(),
                    want.data_type()
                ),
            ));
        }
        if !want.is_nullable() && col.null_count() > 0 {
            return Err(schema_err(
                name,
                format!("nulls in required column `{}`", want.name()),
            ));
        }
    }
    Ok(RecordBatch::try_new(
        schema.clone(),
        batch.columns().to_vec(),
    )?)
}

/// Step 2. The key is the primary key, or the whole row for a dataset without one; key
/// comparison treats null as equal to null (row encoding).
fn merge(
    name: &str,
    spec: &DatasetSpec,
    existing: Option<&RecordBatch>,
    new: &RecordBatch,
) -> Result<RecordBatch, LakeError> {
    let Some(existing) = existing else {
        return Ok(new.clone());
    };
    let all: Vec<String> = spec.fields.iter().map(|f| f.name.clone()).collect();
    let key = if spec.primary_key.is_empty() {
        &all
    } else {
        &spec.primary_key
    };
    let (new_keys, old_keys) = key_rows_pair(spec, key, new, existing)?;
    let index: HashMap<_, _> = new_keys.iter().enumerate().map(|(i, r)| (r, i)).collect();
    let full = if spec.merge == Merge::AppendOnly && key != &all {
        Some(key_rows_pair(spec, &all, new, existing)?)
    } else {
        None
    };
    let mut keep = Vec::new();
    for (i, r) in old_keys.iter().enumerate() {
        let Some(&j) = index.get(&r) else {
            keep.push(u32::try_from(i).map_err(|_| too_many(name))?);
            continue;
        };
        if let Some((new_full, old_full)) = &full
            && new_full.row(j) != old_full.row(i)
        {
            return Err(LakeError::AppendOnlyConflict {
                dataset: name.to_owned(),
                row: j,
            });
        }
    }
    let kept = take_record_batch(existing, &UInt32Array::from(keep))?;
    Ok(concat_batches(&new.schema(), [&kept, new])?)
}

/// Step 3: order by the sort key (the primary key, or the registry's key for `audit_log`).
fn sort(spec: &DatasetSpec, batch: &RecordBatch) -> Result<RecordBatch, LakeError> {
    let rows = key_rows(spec, &spec.sort_key, batch)?;
    let mut order: Vec<u32> =
        (0..u32::try_from(rows.num_rows()).map_err(|_| too_many("sort"))?).collect();
    order.sort_by(|&a, &b| rows.row(a as usize).cmp(&rows.row(b as usize)));
    Ok(take_record_batch(batch, &UInt32Array::from(order))?)
}

/// Steps 4-6: temporary dot-file in the same directory, fsync, rename, fsync the directory;
/// the temporary file is removed on failure.
pub(crate) fn write_atomic(path: &Path, batch: &RecordBatch) -> Result<(), LakeError> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let created: Vec<PathBuf> = dir
        .ancestors()
        .take_while(|a| !a.exists())
        .map(Path::to_path_buf)
        .collect();
    fs::create_dir_all(dir).map_err(io_err(dir))?;
    // A new directory entry is durable only once its parent directory is fsynced.
    for parent in created.iter().filter_map(|c| c.parent()) {
        File::open(parent)
            .and_then(|d| d.sync_all())
            .map_err(io_err(parent))?;
    }
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("data.parquet");
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let tmp = dir.join(format!(".{file_name}.tmp.{}.{nanos}", std::process::id()));
    let result = (|| {
        let file = File::create(&tmp).map_err(io_err(&tmp))?;
        let props = WriterProperties::builder()
            .set_compression(Compression::ZSTD(ZstdLevel::try_new(3)?))
            .set_max_row_group_row_count(Some(ROW_GROUP_ROWS))
            .build();
        let mut writer = ArrowWriter::try_new(file, batch.schema(), Some(props))?;
        writer.write(batch)?;
        let file = writer.into_inner()?;
        file.sync_all().map_err(io_err(&tmp))?;
        fs::rename(&tmp, path).map_err(io_err(path))?;
        File::open(dir)
            .and_then(|d| d.sync_all())
            .map_err(io_err(dir))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Delete temporary files older than [`STALE_TEMP_AGE`] under `dir`.
fn remove_stale_temps(dir: &Path, prefix: &str, recurse: bool) -> Result<(), LakeError> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(io_err(dir)(e)),
    };
    for entry in entries {
        let entry = entry.map_err(io_err(dir))?;
        let path = entry.path();
        let meta = entry.metadata().map_err(io_err(&path))?;
        if meta.is_dir() {
            if recurse {
                remove_stale_temps(&path, prefix, true)?;
            }
        } else if entry
            .file_name()
            .to_str()
            .is_some_and(|n| n.starts_with(prefix))
            && meta
                .modified()
                .ok()
                .and_then(|m| m.elapsed().ok())
                .is_some_and(|a| a > STALE_TEMP_AGE)
        {
            fs::remove_file(&path).map_err(io_err(&path))?;
        }
    }
    Ok(())
}

/// The partition of every row, from the partition column (`SKILL.md` §6): the distinct
/// partition values (`["2024", "01"]`, in first-seen order) and, per row, an index into them.
/// The strings are built once per distinct day, not once per row.
fn partition_ids(
    name: &str,
    spec: &DatasetSpec,
    batch: &RecordBatch,
) -> Result<(Vec<Vec<String>>, Vec<u32>), LakeError> {
    let part_err = |message: String| LakeError::Partition {
        dataset: name.to_owned(),
        message,
    };
    let col_name = spec.partition_column.as_deref().unwrap_or_default();
    let col = spec
        .column_index(col_name)
        .and_then(|i| batch.columns().get(i))
        .ok_or_else(|| part_err(format!("no partition column `{col_name}`")))?;
    // Days since 1970-01-01 per row (a timestamp is cut at its UTC date).
    let days: Vec<Option<i32>> = match col.data_type() {
        DataType::Date32 => col.as_primitive::<Date32Type>().iter().collect(),
        DataType::Timestamp(..) => col
            .as_primitive::<TimestampNanosecondType>()
            .iter()
            .map(|t| t.and_then(|t| i32::try_from(t.div_euclid(NANOS_PER_DAY)).ok()))
            .collect(),
        other => {
            return Err(part_err(format!(
                "partition column `{col_name}` has type {other}"
            )));
        }
    };
    let mut keys: Vec<Vec<String>> = Vec::new();
    let mut ids = Vec::with_capacity(days.len());
    let mut by_day: HashMap<i32, u32> = HashMap::new();
    let mut by_key: HashMap<Vec<String>, u32> = HashMap::new();
    for (row, day) in days.into_iter().enumerate() {
        let bad = || part_err(format!("row {row}: null or out-of-range `{col_name}`"));
        let day = day.ok_or_else(bad)?;
        if let Some(&id) = by_day.get(&day) {
            ids.push(id);
            continue;
        }
        let d = day
            .checked_add(UNIX_EPOCH_DAYS_FROM_CE)
            .and_then(NaiveDate::from_num_days_from_ce_opt)
            .ok_or_else(bad)?;
        let key: Vec<String> = spec
            .partition_keys
            .iter()
            .map(|k| match k.as_str() {
                "year" => d.format("%Y").to_string(),
                "month" => d.format("%m").to_string(),
                _ => d.format("%Y-%m-%d").to_string(),
            })
            .collect();
        let id = *by_key.entry(key.clone()).or_insert_with(|| {
            keys.push(key);
            u32::try_from(keys.len() - 1).unwrap_or(u32::MAX)
        });
        by_day.insert(day, id);
        ids.push(id);
    }
    Ok((keys, ids))
}

/// Row-encoded key columns of `batch`.
fn key_rows(spec: &DatasetSpec, key: &[String], batch: &RecordBatch) -> Result<Rows, LakeError> {
    let (conv, cols) = converter(spec, key, batch)?;
    Ok(conv.convert_columns(&cols)?)
}

/// The same key encoded for two batches with one converter (so rows compare).
fn key_rows_pair(
    spec: &DatasetSpec,
    key: &[String],
    a: &RecordBatch,
    b: &RecordBatch,
) -> Result<(Rows, Rows), LakeError> {
    let (conv, cols_a) = converter(spec, key, a)?;
    let (_, cols_b) = converter(spec, key, b)?;
    Ok((
        conv.convert_columns(&cols_a)?,
        conv.convert_columns(&cols_b)?,
    ))
}

fn converter(
    spec: &DatasetSpec,
    key: &[String],
    batch: &RecordBatch,
) -> Result<(RowConverter, Vec<ArrayRef>), LakeError> {
    let cols = key
        .iter()
        .map(|k| {
            spec.column_index(k)
                .and_then(|i| batch.columns().get(i).cloned())
                .ok_or_else(|| schema_err("key", format!("no column `{k}`")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let conv = RowConverter::new(
        cols.iter()
            .map(|c| SortField::new(c.data_type().clone()))
            .collect(),
    )?;
    Ok((conv, cols))
}

fn schema_err(name: &str, message: String) -> LakeError {
    LakeError::Schema {
        dataset: name.to_owned(),
        message,
    }
}

fn too_many(name: &str) -> LakeError {
    schema_err(name, "more than u32::MAX rows in one write".into())
}
