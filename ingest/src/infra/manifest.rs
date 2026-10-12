//! The ingestion manifest (`BACKFILL.md` §4): SQLite at `$QUANT_STATE/ingest/manifest.db`,
//! WAL, `synchronous=FULL`, written only by this crate. Key `(source, entity, chunk)`; states
//! `pending -> fetching -> fetched -> parsed -> compacted`, plus `failed` (retryable) and
//! `gone` (a permanent "no data" answer, which is a result).
//!
//! shortcut: one connection behind a mutex, used from async code without `spawn_blocking`
//! (every statement is a primary-key read or write); move to a writer thread if profiling
//! shows the lock contended.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use chrono::{DateTime, Duration, Utc};
use rusqlite::{Connection, OptionalExtension, params};

/// A chunk's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Not fetched yet.
    Pending,
    /// A fetch is (or was, if the process died) in flight.
    Fetching,
    /// Raw body landed.
    Fetched,
    /// Stage file written.
    Parsed,
    /// Folded into the lake.
    Compacted,
    /// Last attempt failed; retried on the next run.
    Failed,
    /// The source has no data for it (`reason`); never re-fetched.
    Gone,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Fetching => "fetching",
            Self::Fetched => "fetched",
            Self::Parsed => "parsed",
            Self::Compacted => "compacted",
            Self::Failed => "failed",
            Self::Gone => "gone",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        [
            Self::Pending,
            Self::Fetching,
            Self::Fetched,
            Self::Parsed,
            Self::Compacted,
            Self::Failed,
            Self::Gone,
        ]
        .into_iter()
        .find(|v| v.as_str() == s)
    }

    /// A chunk in this state is never fetched again (`BACKFILL.md` §4 resume rule 4).
    #[must_use]
    pub fn is_done(self) -> bool {
        matches!(
            self,
            Self::Fetched | Self::Parsed | Self::Compacted | Self::Gone
        )
    }
}

/// `(source, entity, chunk)`.
pub type Key<'a> = (&'a str, &'a str, &'a str);

/// The manifest.
pub struct Manifest {
    conn: Mutex<Connection>,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS chunks (
  source TEXT NOT NULL, entity TEXT NOT NULL, chunk TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN
    ('pending','fetching','fetched','parsed','compacted','failed','gone')),
  attempts INTEGER NOT NULL DEFAULT 0,
  http_status INTEGER, fetched_at TEXT, last_checked_at TEXT,
  bytes INTEGER, sha256 TEXT, raw_path TEXT,
  parse_version TEXT, parsed_at TEXT, stage_path TEXT, stage_sha256 TEXT,
  compacted_at TEXT, reason TEXT, last_error TEXT,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (source, entity, chunk));
CREATE TABLE IF NOT EXISTS partitions (
  dataset TEXT NOT NULL, partition TEXT NOT NULL,
  inputs_hash TEXT NOT NULL, content_hash TEXT NOT NULL, written_at TEXT NOT NULL,
  PRIMARY KEY (dataset, partition));
CREATE TABLE IF NOT EXISTS probes (probe TEXT PRIMARY KEY, ran_at TEXT, result_json TEXT);
CREATE TABLE IF NOT EXISTS calls (
  source TEXT NOT NULL, day TEXT NOT NULL, n INTEGER NOT NULL, PRIMARY KEY (source, day));
";

fn now() -> String {
    Utc::now().to_rfc3339()
}

impl Manifest {
    /// Open (creating it and its directory if needed).
    ///
    /// # Errors
    ///
    /// I/O or SQLite errors.
    pub fn open(path: &Path) -> Result<Self, rusqlite::Error> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn db(&self) -> MutexGuard<'_, Connection> {
        // A panic while holding the lock cannot leave SQLite inconsistent: keep using it.
        self.conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The chunk's state, `None` if it has no row.
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn status(&self, (s, e, c): Key<'_>) -> Result<Option<Status>, rusqlite::Error> {
        let v: Option<String> = self
            .db()
            .query_row(
                "SELECT status FROM chunks WHERE source=?1 AND entity=?2 AND chunk=?3",
                params![s, e, c],
                |r| r.get(0),
            )
            .optional()?;
        Ok(v.as_deref().and_then(Status::parse))
    }

    /// The landed body of the chunk in any status, `gone` included (a `figi_collision` code
    /// keeps its raw prices, which J4a's rename rule still compares).
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn raw_path(&self, (s, e, c): Key<'_>) -> Result<Option<String>, rusqlite::Error> {
        self.db()
            .query_row(
                "SELECT raw_path FROM chunks WHERE source=?1 AND entity=?2 AND chunk=?3 AND raw_path IS NOT NULL",
                params![s, e, c],
                |r| r.get(0),
            )
            .optional()
    }

    /// Add the chunk as `pending` unless it already has a row.
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn ensure(&self, (s, e, c): Key<'_>) -> Result<(), rusqlite::Error> {
        self.db().execute(
            "INSERT OR IGNORE INTO chunks (source, entity, chunk, status, updated_at) VALUES (?1, ?2, ?3, 'pending', ?4)",
            params![s, e, c, now()],
        )?;
        Ok(())
    }

    /// Mark `fetching` and count the attempt.
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn begin_fetch(&self, key: Key<'_>) -> Result<(), rusqlite::Error> {
        self.ensure(key)?;
        let (s, e, c) = key;
        self.db().execute(
            "UPDATE chunks SET status='fetching', attempts=attempts+1, updated_at=?4 WHERE source=?1 AND entity=?2 AND chunk=?3",
            params![s, e, c, now()],
        )?;
        Ok(())
    }

    /// The body is landed.
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn fetched(
        &self,
        (s, e, c): Key<'_>,
        http: u16,
        fetched_at: DateTime<Utc>,
        landed: &super::landing::Landed,
    ) -> Result<(), rusqlite::Error> {
        let at = fetched_at.to_rfc3339();
        self.db().execute(
            "UPDATE chunks SET status='fetched', http_status=?4, fetched_at=?5, last_checked_at=?5, bytes=?6,
             sha256=?7, raw_path=?8, last_error=NULL, updated_at=?9 WHERE source=?1 AND entity=?2 AND chunk=?3",
            params![s, e, c, http, at, i64::try_from(landed.bytes).unwrap_or(i64::MAX), landed.sha256, landed.path.to_string_lossy(), now()],
        )?;
        Ok(())
    }

    /// Resume rule 2: record a landed body the manifest missed (a crash between the rename
    /// and the commit). Only a chunk that is not already done is changed.
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn adopt(
        &self,
        key: Key<'_>,
        http: u16,
        landed: &super::landing::Landed,
    ) -> Result<bool, rusqlite::Error> {
        if self.status(key)?.is_some_and(Status::is_done) {
            return Ok(false);
        }
        self.ensure(key)?;
        self.fetched(key, http, Utc::now(), landed)?;
        Ok(true)
    }

    /// The landed body of a fetched (or later) chunk: `(raw_path, sha256, fetched_at)`.
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn landed(
        &self,
        (s, e, c): Key<'_>,
    ) -> Result<Option<(String, String, DateTime<Utc>)>, rusqlite::Error> {
        let row: Option<(Option<String>, Option<String>, Option<String>)> = self
            .db()
            .query_row(
                "SELECT raw_path, sha256, fetched_at FROM chunks WHERE source=?1 AND entity=?2 AND chunk=?3
                 AND status IN ('fetched','parsed','compacted')",
                params![s, e, c],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        Ok(row.and_then(|(p, h, t)| {
            Some((
                p?,
                h?,
                DateTime::parse_from_rfc3339(&t?).ok()?.with_timezone(&Utc),
            ))
        }))
    }

    /// The source has no data for the chunk; it is never fetched again.
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn gone(&self, (s, e, c): Key<'_>, http: u16, reason: &str) -> Result<(), rusqlite::Error> {
        self.db().execute(
            "UPDATE chunks SET status='gone', http_status=?4, reason=?5, updated_at=?6 WHERE source=?1 AND entity=?2 AND chunk=?3",
            params![s, e, c, http, reason, now()],
        )?;
        Ok(())
    }

    /// The fetch failed; the next run retries it.
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn failed(
        &self,
        (s, e, c): Key<'_>,
        http: Option<u16>,
        error: &str,
    ) -> Result<(), rusqlite::Error> {
        self.db().execute(
            "UPDATE chunks SET status='failed', http_status=?4, last_error=?5, updated_at=?6 WHERE source=?1 AND entity=?2 AND chunk=?3",
            params![s, e, c, http, error, now()],
        )?;
        Ok(())
    }

    /// A stage file was written from the chunk's raw body.
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn parsed(
        &self,
        (s, e, c): Key<'_>,
        parse_version: &str,
        stage_path: &Path,
        stage_sha256: &str,
    ) -> Result<(), rusqlite::Error> {
        self.db().execute(
            "UPDATE chunks SET status='parsed', parse_version=?4, parsed_at=?5, stage_path=?6, stage_sha256=?7, updated_at=?5
             WHERE source=?1 AND entity=?2 AND chunk=?3",
            params![s, e, c, parse_version, now(), stage_path.to_string_lossy(), stage_sha256],
        )?;
        Ok(())
    }

    /// Withdraw a chunk's stage file from compaction (its code lost its FIGI): back to
    /// `fetched`, the raw body kept. Returns whether anything changed.
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn unstage(&self, (s, e, c): Key<'_>) -> Result<bool, rusqlite::Error> {
        let n = self.db().execute(
            "UPDATE chunks SET status='fetched', stage_path=NULL, stage_sha256=NULL, parse_version=NULL, parsed_at=NULL, compacted_at=NULL, updated_at=?4
             WHERE source=?1 AND entity=?2 AND chunk=?3 AND status IN ('parsed','compacted')",
            params![s, e, c, now()],
        )?;
        Ok(n > 0)
    }

    /// Chunks with a stage file for `source`: `(stage_path, stage_sha256, parse_version)`.
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn stage_files(
        &self,
        source: &str,
    ) -> Result<Vec<(String, String, String)>, rusqlite::Error> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT stage_path, stage_sha256, parse_version FROM chunks
             WHERE source=?1 AND status IN ('parsed','compacted') ORDER BY stage_path",
        )?;
        st.query_map(params![source], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect()
    }

    /// Mark every parsed chunk of `source` compacted.
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn compacted(&self, source: &str) -> Result<usize, rusqlite::Error> {
        let t = now();
        self.db().execute(
            "UPDATE chunks SET status='compacted', compacted_at=?2, updated_at=?2 WHERE source=?1 AND status='parsed'",
            params![source, t],
        )
    }

    /// Resume rule 1: `fetching` rows older than `stale` (a dead run) go back to `pending`.
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn reset_stale(&self, stale: Duration) -> Result<usize, rusqlite::Error> {
        let cutoff = (Utc::now() - stale).to_rfc3339();
        self.db().execute(
            "UPDATE chunks SET status='pending', updated_at=?2 WHERE status='fetching' AND updated_at < ?1",
            params![cutoff, now()],
        )
    }

    /// Resume rule 3: the work list of `source`, failed rows (under 5 attempts) first.
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn work(&self, source: &str) -> Result<Vec<(String, String)>, rusqlite::Error> {
        let db = self.db();
        let mut st = db.prepare(
            "SELECT entity, chunk FROM chunks WHERE source=?1 AND (status='pending' OR (status='failed' AND attempts < 5))
             ORDER BY status='pending', entity, chunk",
        )?;
        st.query_map(params![source], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect()
    }

    /// Add `n` to today's (UTC) call count of `source` and return the new total.
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn add_calls(&self, source: &str, n: u64) -> Result<u64, rusqlite::Error> {
        let day = Utc::now().format("%Y-%m-%d").to_string();
        let db = self.db();
        db.execute(
            "INSERT INTO calls (source, day, n) VALUES (?1, ?2, ?3) ON CONFLICT (source, day) DO UPDATE SET n = n + ?3",
            params![source, day, i64::try_from(n).unwrap_or(i64::MAX)],
        )?;
        let total: i64 = db.query_row(
            "SELECT n FROM calls WHERE source=?1 AND day=?2",
            params![source, day],
            |r| r.get(0),
        )?;
        Ok(u64::try_from(total).unwrap_or_default())
    }

    /// The recorded inputs hash of a lake partition.
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn partition_inputs(
        &self,
        dataset: &str,
        partition: &str,
    ) -> Result<Option<String>, rusqlite::Error> {
        self.db()
            .query_row(
                "SELECT inputs_hash FROM partitions WHERE dataset=?1 AND partition=?2",
                params![dataset, partition],
                |r| r.get(0),
            )
            .optional()
    }

    /// Record a written lake partition.
    ///
    /// # Errors
    ///
    /// SQLite errors.
    pub fn record_partition(
        &self,
        dataset: &str,
        partition: &str,
        inputs_hash: &str,
        content_hash: &str,
    ) -> Result<(), rusqlite::Error> {
        self.db().execute(
            "INSERT INTO partitions (dataset, partition, inputs_hash, content_hash, written_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (dataset, partition) DO UPDATE SET inputs_hash=?3, content_hash=?4, written_at=?5",
            params![dataset, partition, inputs_hash, content_hash, now()],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing, reason = "test")]

    use super::*;
    use crate::infra::landing::Landed;

    #[test]
    fn state_machine_and_resume_rules() {
        let dir = tempfile::tempdir().unwrap();
        let m = Manifest::open(&dir.path().join("ingest/manifest.db")).unwrap();
        let k = ("eodhd", "AAPL.US", "eod-full");
        assert_eq!(m.status(k).unwrap(), None);
        m.begin_fetch(k).unwrap();
        assert_eq!(m.status(k).unwrap(), Some(Status::Fetching));
        // A dead run's fetching row is reset; a live one (younger than the cutoff) is not.
        assert_eq!(m.reset_stale(Duration::minutes(15)).unwrap(), 0);
        assert_eq!(m.reset_stale(Duration::seconds(-1)).unwrap(), 1);
        assert_eq!(
            m.work("eodhd").unwrap(),
            [("AAPL.US".into(), "eod-full".into())]
        );
        let landed = Landed {
            path: "/x".into(),
            sha256: "ab".into(),
            bytes: 2,
            new: true,
        };
        m.fetched(k, 200, Utc::now(), &landed).unwrap();
        assert!(m.status(k).unwrap().unwrap().is_done());
        assert!(m.work("eodhd").unwrap().is_empty());
        // A collision exclusion stops compaction but keeps the body readable for J4a.
        m.gone(k, 200, "figi_collision").unwrap();
        assert_eq!(m.landed(k).unwrap(), None);
        assert_eq!(m.raw_path(k).unwrap().as_deref(), Some("/x"));
        let g = ("eodhd", "XX.US", "eod-full");
        m.begin_fetch(g).unwrap();
        m.gone(g, 200, "vendor_no_data").unwrap();
        assert_eq!(m.status(g).unwrap(), Some(Status::Gone));
        assert_eq!(m.add_calls("eodhd", 1).unwrap(), 1);
        assert_eq!(m.add_calls("eodhd", 100).unwrap(), 101);
    }
}
