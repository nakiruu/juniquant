//! File helpers for the runtime overrides: optional read, writer lock, atomic replace.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::CalendarError;

/// Reads `path` as UTF-8; `Ok(None)` when it does not exist, an error for anything else.
pub(crate) fn read_optional(path: &Path) -> Result<Option<String>, CalendarError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(CalendarError::Read {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// The directory holding `path` (`.` for a bare file name).
fn parent_dir(path: &Path) -> PathBuf {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// An exclusive advisory lock on `<dir>/locks/<stem>.lock`, released on drop.
pub(crate) struct RuntimeLock {
    _file: File,
}

impl RuntimeLock {
    /// Takes the writer lock for the runtime file `path` without waiting.
    pub(crate) fn acquire(path: &Path) -> Result<Self, CalendarError> {
        let dir = parent_dir(path).join("locks");
        let stem = path
            .file_stem()
            .map_or_else(|| "nyse_calendar_overrides".into(), |s| s.to_string_lossy());
        let lock_path = dir.join(format!("{stem}.lock"));
        let write_err = |source| CalendarError::Write {
            path: lock_path.clone(),
            source,
        };
        fs::create_dir_all(&dir).map_err(write_err)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .map_err(write_err)?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(TryLockError::WouldBlock) => Err(CalendarError::Locked { path: lock_path }),
            Err(TryLockError::Error(source)) => Err(write_err(source)),
        }
    }
}

/// Replaces `path` with `content` atomically: temporary dot-file in the same directory,
/// fsync, rename, fsync of the directory. On failure the temporary file is removed and the
/// existing file is untouched.
pub(crate) fn write_atomic(path: &Path, content: &[u8]) -> Result<(), CalendarError> {
    let dir = parent_dir(path);
    let name = path.file_name().map_or_else(
        || "nyse_calendar_overrides.toml".into(),
        |s| s.to_string_lossy(),
    );
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let tmp = dir.join(format!(".{name}.tmp.{}.{nanos}", std::process::id()));

    let result = (|| -> io::Result<()> {
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(content)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)?;
        File::open(&dir)?.sync_all()
    })();
    result.map_err(|source| {
        // Best effort: the temporary may not exist (or was already renamed).
        let _ = fs::remove_file(&tmp);
        CalendarError::Write {
            path: path.to_path_buf(),
            source,
        }
    })
}
