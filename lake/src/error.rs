//! Typed errors of the lake crate. Every error leaves the existing partition untouched
//! (`IO-CONTRACT.md` §3 step 6).

use std::io;
use std::path::PathBuf;

/// Why a lake read or write failed.
#[derive(Debug, thiserror::Error)]
pub enum LakeError {
    /// A required environment variable is unset or empty.
    #[error("{0} is not set (13-parquet-lake/IO-CONTRACT.md §1)")]
    EnvUnset(&'static str),
    /// The registry file cannot be read or parsed.
    #[error("registry {path}: {message}")]
    Registry {
        /// Registry path.
        path: PathBuf,
        /// What is wrong.
        message: String,
    },
    /// The dataset is not in the registry.
    #[error("dataset `{0}` is not registered")]
    UnknownDataset(String),
    /// The input (or an existing leaf) does not have the registered columns and types.
    #[error("{dataset}: schema mismatch: {message}")]
    Schema {
        /// Dataset name.
        dataset: String,
        /// What differs.
        message: String,
    },
    /// The partition values are wrong for the dataset, or a row is outside the partition.
    #[error("{dataset}: partition: {message}")]
    Partition {
        /// Dataset name.
        dataset: String,
        /// What is wrong.
        message: String,
    },
    /// The primary key is not unique in the input.
    #[error("{dataset}: primary key not unique in the input (row {row})")]
    DuplicateKey {
        /// Dataset name.
        dataset: String,
        /// Index of the second occurrence in the input.
        row: usize,
    },
    /// An append-only dataset already holds a different row with the same key.
    #[error(
        "{dataset}: append-only conflict: input row {row} differs from the stored row with the same key"
    )]
    AppendOnlyConflict {
        /// Dataset name.
        dataset: String,
        /// Index of the conflicting input row.
        row: usize,
    },
    /// Another write run holds the dataset's single-writer lock.
    #[error("{dataset}: another write run holds {}", path.display())]
    LockHeld {
        /// Dataset name.
        dataset: String,
        /// Lock file.
        path: PathBuf,
    },
    /// A filesystem operation failed.
    #[error("{}: {source}", path.display())]
    Io {
        /// Path involved.
        path: PathBuf,
        /// Underlying error.
        source: io::Error,
    },
    /// Arrow failed (concatenation, row conversion, IPC).
    #[error(transparent)]
    Arrow(#[from] arrow::error::ArrowError),
    /// Parquet encoding or decoding failed.
    #[error(transparent)]
    Parquet(#[from] parquet::errors::ParquetError),
}

/// Attach a path to an I/O error.
pub(crate) fn io_err(path: impl Into<PathBuf>) -> impl FnOnce(io::Error) -> LakeError {
    let path = path.into();
    move |source| LakeError::Io { path, source }
}
