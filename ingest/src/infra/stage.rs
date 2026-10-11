//! Parse and compact (`BACKFILL.md` §4 steps 2-3). A parser turns one landed body into one
//! stage file `stage/{dataset}/{source}/{entity}/{chunk}.parquet`; compaction rebuilds the
//! dataset's lake partitions from all of the source's stage files for it, and skips the
//! rebuild when the inputs are unchanged.
//!
//! shortcut: compaction rereads every stage file of the dataset and writes every partition
//! they touch (`Mode::Replace`), so a duplicate key across two stage files (for example a
//! bulk day overlapping a full history) fails it; add "the later stage file wins" when the
//! daily bulk chunks arrive.

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
    /// The dataset's partitions were rebuilt from this many rows.
    Written(usize),
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
    let batches = files
        .iter()
        .map(|(p, _, _)| lake::read_file(Path::new(p)))
        .collect::<Result<Vec<_>, _>>()?;
    let Some(first) = batches.first() else {
        return Ok(Compacted::Unchanged);
    };
    let all = concat_batches(&first.schema(), &batches)?;
    lake.write_dataset(dataset, &all, lake::Mode::Replace)?;
    // shortcut: content_hash is recorded as the inputs hash; hash the written leaves when the
    // kill-and-resume test (BACKFILL.md §12) compares lake content.
    manifest.record_partition(dataset, &partition, &inputs_hash, &inputs_hash)?;
    Ok(Compacted::Written(all.num_rows()))
}
