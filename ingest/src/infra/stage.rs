//! Parse and compact (`BACKFILL.md` §4 steps 2-3). A parser turns one landed body into one
//! stage file `stage/{dataset}/{source}/{entity}/{chunk}.parquet`; compaction rebuilds the
//! dataset's lake partitions from all of the source's stage files for it, and skips the
//! rebuild when the inputs are unchanged.
//!
//! A key present in two stage files keeps its first row in stage-path order (two vendor codes of
//! one security return the same split); the count is reported.
//!
//! shortcut: compaction rereads every stage file of the dataset and writes every partition they
//! touch (`Mode::Replace`); add a stage-to-partition index if daily compaction gets slow.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use arrow::array::RecordBatch;
use arrow::compute::concat_batches;

use super::landing::sha256_hex;
use super::manifest::{Key, Manifest};

/// Why a stage or compaction step failed.
#[derive(Debug, thiserror::Error)]
pub enum StageError {
    /// Manifest error.
    #[error("manifest: {0}")]
    Manifest(#[from] rusqlite::Error),
    /// Lake read or write error.
    #[error("lake: {0}")]
    Lake(#[from] lake::LakeError),
    /// Reading a stage file back.
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// Arrow error while combining stage files.
    #[error("arrow: {0}")]
    Arrow(#[from] arrow::error::ArrowError),
}

/// Where a chunk's stage file for `dataset` goes.
#[must_use]
pub fn stage_path(stage: &Path, dataset: &str, (source, entity, chunk): Key<'_>) -> PathBuf {
    stage
        .join(dataset)
        .join(source)
        .join(entity)
        .join(format!("{chunk}.parquet"))
}

/// Write the parsed rows of one chunk and mark it parsed.
///
/// # Errors
///
/// Lake (Parquet), I/O or manifest errors; the chunk stays `fetched` on error.
pub fn write_stage(
    stage: &Path,
    manifest: &Manifest,
    dataset: &str,
    key: Key<'_>,
    parse_version: &str,
    batch: &RecordBatch,
) -> Result<PathBuf, StageError> {
    let path = stage_path(stage, dataset, key);
    lake::write_file(&path, batch)?;
    let sha = sha256_hex(&std::fs::read(&path)?);
    manifest.parsed(key, parse_version, &path, &sha)?;
    Ok(path)
}

/// What a compaction did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Compacted {
    /// Inputs unchanged since the last compaction: nothing written.
    Unchanged,
    /// The dataset's partitions were rebuilt.
    Written {
        /// Rows written.
        rows: usize,
        /// Rows dropped because an earlier stage file had the same key.
        duplicates: usize,
    },
}

/// Rebuild `dataset` from `source`'s stage files for it.
///
/// # Errors
///
/// Lake (validation included), I/O or manifest errors; partitions already renamed stay
/// written (`IO-CONTRACT.md` §3), and a re-run converges.
pub fn compact(
    lake: &lake::Lake,
    manifest: &Manifest,
    stage: &Path,
    source: &str,
    dataset: &str,
) -> Result<Compacted, StageError> {
    let prefix = stage.join(dataset).join(source);
    let files: Vec<(String, String, String)> = manifest
        .stage_files(source)?
        .into_iter()
        .filter(|(p, _, _)| Path::new(p).starts_with(&prefix))
        .collect();
    let inputs: Vec<String> = files
        .iter()
        .map(|(_, sha, ver)| format!("{sha}:{ver}"))
        .collect();
    let inputs_hash = sha256_hex(inputs.join("\n").as_bytes());
    let partition = format!("{source}/*");
    if manifest.partition_inputs(dataset, &partition)?.as_deref() == Some(inputs_hash.as_str()) {
        return Ok(Compacted::Unchanged);
    }
    let paths: Vec<&Path> = files
        .iter()
        .map(|(p, _, _)| Path::new(p.as_str()))
        .collect();
    let spec = lake.registry().get(dataset)?;
    let date_col = spec.partition_column.as_deref().and_then(|c| {
        spec.fields
            .iter()
            .position(|f| f.name == c && f.ty == "date32")
    });
    let (rows, duplicates) = match date_col {
        Some(col) => compact_by_year(lake, dataset, &paths, col)?,
        None => {
            let batches = paths
                .iter()
                .map(|p| lake::read_file(p))
                .collect::<Result<Vec<_>, _>>()?;
            let Some(first) = batches.first() else {
                return Ok(Compacted::Unchanged);
            };
            let all = concat_batches(&first.schema(), &batches)?;
            let (all, duplicates) = first_per_key(lake, dataset, &all)?;
            lake.write_dataset(dataset, &all, lake::Mode::Replace)?;
            (all.num_rows(), duplicates)
        }
    };
    // shortcut: content_hash is recorded as the inputs hash; hash the written leaves when the
    // kill-and-resume test (BACKFILL.md §12) compares lake content.
    manifest.record_partition(dataset, &partition, &inputs_hash, &inputs_hash)?;
    Ok(Compacted::Written { rows, duplicates })
}

/// A partitioned dataset in bounded memory: one pass reads only the date column of each stage
/// file to learn the years it spans, then each year is rebuilt from the rows of that year
/// alone (filtered while decoding), so about one year of rows is in memory at a time.
///
/// shortcut: a year that no stage file reaches any more keeps its old leaves; delete them by
/// hand if a whole year of inputs is ever withdrawn.
fn compact_by_year(
    lake: &lake::Lake,
    dataset: &str,
    paths: &[&Path],
    col: usize,
) -> Result<(usize, usize), StageError> {
    use arrow::array::AsArray;
    use chrono::Datelike;
    let mut by_year: BTreeMap<i32, Vec<&Path>> = BTreeMap::new();
    for p in paths {
        let b = lake::read_file_with(p, Some(&[col]), None)?;
        let d = b.column(0).as_primitive::<arrow::datatypes::Date32Type>();
        let year = |v: Option<i32>| {
            v.and_then(|x| {
                chrono::NaiveDate::default()
                    .checked_add_signed(chrono::Duration::days(i64::from(x)))
            })
            .map(|d| d.year())
        };
        if let (Some(a), Some(z)) = (year(arrow::compute::min(d)), year(arrow::compute::max(d))) {
            for y in a..=z {
                by_year.entry(y).or_default().push(p);
            }
        }
    }
    let (mut rows, mut duplicates) = (0, 0);
    for (y, files) in by_year {
        let (from, to) = match (
            chrono::NaiveDate::from_ymd_opt(y, 1, 1),
            chrono::NaiveDate::from_ymd_opt(y, 12, 31),
        ) {
            (Some(f), Some(t)) => (f, t),
            _ => continue,
        };
        let batches = files
            .iter()
            .map(|p| lake::read_file_with(p, None, Some((col, from, to))))
            .collect::<Result<Vec<_>, _>>()?;
        let Some(first) = batches.first() else {
            continue;
        };
        let year_rows = concat_batches(&first.schema(), &batches)?;
        let (kept, dropped) = first_per_key(lake, dataset, &year_rows)?;
        lake.write_dataset(dataset, &kept, lake::Mode::Replace)?;
        rows += kept.num_rows();
        duplicates += dropped;
    }
    Ok((rows, duplicates))
}

/// Keep the first row of each primary key, in stage-file order (a plain EODHD code sorts
/// before its `_old` twin, so the preferred code wins): two codes of one security can return
/// the same split or dividend. Returns the rows kept and the number dropped.
fn first_per_key(
    lake: &lake::Lake,
    dataset: &str,
    batch: &RecordBatch,
) -> Result<(RecordBatch, usize), StageError> {
    use arrow::row::{RowConverter, SortField};
    let spec = lake.registry().get(dataset)?;
    let cols = spec
        .primary_key
        .iter()
        .filter_map(|k| batch.column_by_name(k).cloned())
        .collect::<Vec<_>>();
    if cols.len() != spec.primary_key.len() {
        return Ok((batch.clone(), 0));
    }
    let conv = RowConverter::new(
        cols.iter()
            .map(|c| SortField::new(c.data_type().clone()))
            .collect(),
    )?;
    let rows = conv.convert_columns(&cols)?;
    let mut seen = std::collections::HashSet::with_capacity(rows.num_rows());
    let keep: Vec<u32> = (0..rows.num_rows())
        .filter(|&i| seen.insert(rows.row(i)))
        .filter_map(|i| u32::try_from(i).ok())
        .collect();
    let dropped = batch.num_rows() - keep.len();
    let kept = arrow::compute::take_record_batch(batch, &arrow::array::UInt32Array::from(keep))?;
    Ok((kept, dropped))
}
