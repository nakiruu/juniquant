// Audit writer: the sole subscriber to the `audit` channel and the only
// process that writes the audit journal (constitution §8, single-writer rule).
//
// System of record: a dedicated SQLite journal, `audit.db`
// (`$QUANT_STATE/audit.db`; host /mnt/cache/appdata/quant/audit.db; paths per
// 13-parquet-lake/IO-CONTRACT.md §1). Parquet cannot be appended row by row, so Parquet is an
// export format only: a nightly job copies closed days to
// `$QUANT_ROOT/lake/audit_log/date=YYYY-MM-DD/data.parquet` (see `export_closed_days`).
//
// Durability contract:
//   - `AuditHandle::append_durable` returns only after the row is COMMITTED
//     with `journal_mode=WAL` and `synchronous=FULL` (the WAL is fsynced
//     before COMMIT returns).
//   - Concurrent appends are group-committed: the writer drains whatever is
//     queued (up to MAX_GROUP) into ONE transaction, so N waiting appends
//     share one fsync. Each caller is acked through its own oneshot after
//     the commit, never before.
//   - Append-only is enforced by SQL triggers (AUDIT_SCHEMA_SQL) that
//     RAISE(ABORT) on UPDATE and DELETE. Nothing in this file truncates,
//     rewrites or deletes. Exported rows are NOT deleted from the journal
//     (~20K rows/year is a few MB); the `audit_export` table marks them.
//
// If the audit channel is full, the bus applies backpressure to all other
// channels. Trading halts. This is intentional.
//
// SKETCH, not compiled, not normative. Crate home: execution/src/audit/writer.rs (scaffold.md).
// API names follow rusqlite / arrow / parquet / sha2 as the author remembers them:
// verify against the pinned crate versions.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::array::{Array, StringArray, TimestampNanosecondArray};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;
use rusqlite::{params, Connection, TransactionBehavior};
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, oneshot};

use crate::events::AuditEvent;

/// Rows per commit at most. Bounds the latency of the first waiter.
const MAX_GROUP: usize = 256;

/// Journal schema. `audit_events` and `audit_export` are insert-only.
/// `seq` never repeats (AUTOINCREMENT). `day` is the UTC date at COMMIT time
/// (assigned by the writer, not taken from the event), so a day is closed
/// once the UTC date has moved past it and no later row can land in it.
/// `phase` is copied from `details.phase` (default 'single') so the startup
/// scan needs no JSON functions. The Parquet export contains only the seven
/// columns of SCHEMAS.md #21; `seq`, `dedup_key`, `phase`, `day` stay here.
pub const AUDIT_SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS audit_events (
    seq          INTEGER PRIMARY KEY AUTOINCREMENT,
    dedup_key    TEXT    NOT NULL UNIQUE,          -- event_id || ':' || phase
    event_id     TEXT    NOT NULL,
    timestamp_ns INTEGER NOT NULL,                 -- UTC, nanoseconds
    event_type   TEXT    NOT NULL,
    ticker       TEXT,
    actor        TEXT    NOT NULL,
    details      TEXT    NOT NULL,                 -- JSON text
    severity     TEXT    NOT NULL CHECK (severity IN ('INFO','WARN','CRITICAL')),
    phase        TEXT    NOT NULL CHECK (phase IN ('intent','outcome','single')),
    day          TEXT    NOT NULL                  -- 'YYYY-MM-DD', UTC, at commit
);
CREATE INDEX IF NOT EXISTS audit_by_event ON audit_events (event_id, phase);
CREATE INDEX IF NOT EXISTS audit_by_day   ON audit_events (day, timestamp_ns, seq);

CREATE TABLE IF NOT EXISTS audit_export (
    day            TEXT    PRIMARY KEY,
    row_count      INTEGER NOT NULL,
    content_sha256 TEXT    NOT NULL,
    exported_at_ns INTEGER NOT NULL
);

CREATE TRIGGER IF NOT EXISTS audit_events_no_update
BEFORE UPDATE ON audit_events
BEGIN SELECT RAISE(ABORT, 'audit_events is append-only'); END;

CREATE TRIGGER IF NOT EXISTS audit_events_no_delete
BEFORE DELETE ON audit_events
BEGIN SELECT RAISE(ABORT, 'audit_events is append-only'); END;

CREATE TRIGGER IF NOT EXISTS audit_export_no_update
BEFORE UPDATE ON audit_export
BEGIN SELECT RAISE(ABORT, 'audit_export is append-only'); END;

CREATE TRIGGER IF NOT EXISTS audit_export_no_delete
BEFORE DELETE ON audit_export
BEGIN SELECT RAISE(ABORT, 'audit_export is append-only'); END;
"#;

/// Triggers that must exist at startup. A journal missing one is refused:
/// triggers do not stop DROP TRIGGER / DROP TABLE, so the writer re-checks
/// them every start and every other process opens the file read-only.
const REQUIRED_TRIGGERS: [&str; 4] = [
    "audit_events_no_update",
    "audit_events_no_delete",
    "audit_export_no_update",
    "audit_export_no_delete",
];

#[derive(Debug, Clone)]
pub enum AuditError {
    Db(String),
    Closed,
    Invariant(String),
}

impl From<rusqlite::Error> for AuditError {
    fn from(e: rusqlite::Error) -> Self {
        AuditError::Db(e.to_string())
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Appended {
    Inserted(i64),
    /// Same `event_id` + phase already in the journal; nothing written.
    Duplicate,
}

/// What travels on the bounded `audit` channel.
pub struct AuditRequest {
    pub event: AuditEvent,
    pub ack: oneshot::Sender<Result<Appended, AuditError>>,
}

/// Cloneable handle given to every handler (and to the spool ingester).
#[derive(Clone)]
pub struct AuditHandle {
    tx: mpsc::Sender<AuditRequest>,
}

impl AuditHandle {
    /// Returns after the row is committed and fsynced. Awaits channel space
    /// first, so a full channel blocks the caller (backpressure).
    pub async fn append_durable(&self, event: AuditEvent) -> Result<Appended, AuditError> {
        let (ack, done) = oneshot::channel();
        self.tx
            .send(AuditRequest { event, ack })
            .await
            .map_err(|_| AuditError::Closed)?;
        done.await.map_err(|_| AuditError::Closed)?
    }
}

pub struct AuditWriter {
    path: PathBuf,
    rx: mpsc::Receiver<AuditRequest>,
}

impl AuditWriter {
    pub fn new(path: PathBuf, capacity: usize) -> (Self, AuditHandle) {
        let (tx, rx) = mpsc::channel(capacity); // 10000, CHANNELS.md
        (Self { path, rx }, AuditHandle { tx })
    }

    /// Runs on a dedicated OS thread (rusqlite is blocking). The caller
    /// must not start trading until `open_journal` has succeeded and
    /// `unresolved_intents` has been resolved (SKILL.md §6).
    pub fn run(mut self) -> Result<(), AuditError> {
        let mut conn = open_journal(&self.path)?;

        // recv() yields every queued request even after all senders drop,
        // so a clean shutdown loses nothing. No in-memory batch exists.
        while let Some(first) = self.rx.blocking_recv() {
            let mut group = vec![first];
            while group.len() < MAX_GROUP {
                match self.rx.try_recv() {
                    Ok(r) => group.push(r),
                    Err(_) => break,
                }
            }

            match commit_group(&mut conn, &group) {
                Ok(outcomes) => {
                    for (req, out) in group.into_iter().zip(outcomes) {
                        let _ = req.ack.send(Ok(out));
                    }
                }
                Err(e) => {
                    // Nothing in this group was committed (transaction
                    // dropped => rolled back). Tell every waiter, then stop:
                    // fail closed, the caller sets the global halt flag.
                    for req in group {
                        let _ = req.ack.send(Err(e.clone()));
                    }
                    return Err(e);
                }
            }
        }
        Ok(())
    }
}

/// Open (never truncate) and prepare the journal.
pub fn open_journal(path: &Path) -> Result<Connection, AuditError> {
    // The journal must be on the cache pool ($QUANT_STATE, host /mnt/cache/appdata/quant), never
    // on /mnt/user (FUSE) or the array: WAL needs real shared-memory and
    // fsync semantics.
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "FULL")?;
    conn.pragma_update(None, "recursive_triggers", "ON")?; // REPLACE would fire the DELETE trigger
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch(AUDIT_SCHEMA_SQL)?;

    for name in REQUIRED_TRIGGERS {
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger' AND name=?1",
            params![name],
            |r| r.get(0),
        )?;
        if n != 1 {
            return Err(AuditError::Invariant(format!("journal trigger missing: {name}")));
        }
    }
    // Read back, do not trust: the pragma may have been refused.
    let mode: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
    let sync: i64 = conn.query_row("PRAGMA synchronous", [], |r| r.get(0))?; // 2 = FULL
    if mode.to_lowercase() != "wal" || sync != 2 {
        return Err(AuditError::Invariant(format!("journal_mode={mode} synchronous={sync}")));
    }
    Ok(conn)
}

/// One transaction, one fsync, for the whole group.
fn commit_group(conn: &mut Connection, group: &[AuditRequest]) -> Result<Vec<Appended>, AuditError> {
    let day = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut out = Vec::with_capacity(group.len());
    {
        // ON CONFLICT DO NOTHING, never INSERT OR REPLACE (that deletes).
        let mut ins = tx.prepare_cached(
            "INSERT INTO audit_events
               (dedup_key, event_id, timestamp_ns, event_type, ticker, actor,
                details, severity, phase, day)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
             ON CONFLICT(dedup_key) DO NOTHING",
        )?;
        for req in group {
            let e = &req.event;
            let phase = e.details.get("phase").and_then(|p| p.as_str()).unwrap_or("single");
            let dedup_key = format!("{}:{}", e.event_id, phase);
            let n = ins.execute(params![
                dedup_key,
                e.event_id.to_string(),
                e.timestamp.timestamp_nanos_opt().ok_or_else(|| AuditError::Invariant("timestamp out of range".into()))?,
                e.event_type,
                e.ticker,
                e.actor,
                e.details.to_string(),
                e.severity.as_str(),
                phase,
                day,
            ])?;
            out.push(if n == 1 {
                Appended::Inserted(tx.last_insert_rowid())
            } else {
                Appended::Duplicate
            });
        }
    }
    tx.commit()?; // fsync happens here (synchronous=FULL, WAL)
    Ok(out)
}

/// Startup scan (SKILL.md §6): intents with no matching outcome. Trading
/// stays blocked until each one is resolved against the broker and an
/// outcome row with `details.phase = "outcome"` and `details.resolution`
/// is appended for it.
pub struct UnresolvedIntent {
    pub event_id: String,
    pub event_type: String,
    pub ticker: Option<String>,
    pub timestamp_ns: i64,
}

pub fn unresolved_intents(conn: &Connection) -> Result<Vec<UnresolvedIntent>, AuditError> {
    let mut st = conn.prepare(
        "SELECT i.event_id, i.event_type, i.ticker, i.timestamp_ns
           FROM audit_events i
          WHERE i.phase = 'intent'
            AND NOT EXISTS (SELECT 1 FROM audit_events o
                             WHERE o.event_id = i.event_id AND o.phase = 'outcome')
          ORDER BY i.seq",
    )?;
    let rows = st.query_map([], |r| {
        Ok(UnresolvedIntent {
            event_id: r.get(0)?,
            event_type: r.get(1)?,
            ticker: r.get(2)?,
            timestamp_ns: r.get(3)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

/// Julia-originated events (veto_triggered, regime_flip, model_retrain_aborted)
/// arrive as files in the spool directory ($QUANT_STATE/audit-spool/).
/// Julia writes `<event_id>.json.tmp`, fsyncs, renames to `<event_id>.json`
/// (atomic on one filesystem). This ingester is the only reader. It appends
/// each file through the same durable path, deletes the file only after the
/// ack, and treats a crash between the two as harmless: the re-read is a
/// `Duplicate` (UNIQUE dedup_key). Julia never opens audit.db.
pub async fn ingest_spool(dir: &Path, audit: &AuditHandle) -> Result<usize, AuditError> {
    let mut names: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| AuditError::Db(e.to_string()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map_or(false, |x| x == "json")) // skips *.json.tmp
        .collect();
    names.sort();

    let mut n = 0;
    for p in names {
        let bytes = fs::read(&p).map_err(|e| AuditError::Db(e.to_string()))?;
        match serde_json::from_slice::<AuditEvent>(&bytes) {
            Ok(ev) => {
                audit.append_durable(ev).await?;
                fs::remove_file(&p).map_err(|e| AuditError::Db(e.to_string()))?;
                n += 1;
            }
            Err(e) => {
                // Malformed file: quarantine and raise a CRITICAL row; do not loop on it.
                let rej = dir.join("rejected");
                fs::create_dir_all(&rej).ok();
                fs::rename(&p, rej.join(p.file_name().unwrap())).ok();
                audit
                    .append_durable(AuditEvent::spool_rejected(&p, &e.to_string()))
                    .await?;
            }
        }
    }
    Ok(n)
}

// ---------------------------------------------------------------------------
// Nightly export (outline: 18:00 slot, before the rsync backup).
// ---------------------------------------------------------------------------

type Row = (String, i64, String, Option<String>, String, String, String);

fn parquet_schema() -> Arc<Schema> {
    // Identical to SCHEMAS.md #21. Unchanged by the move to SQLite.
    Arc::new(Schema::new(vec![
        Field::new("event_id", DataType::Utf8, false),
        Field::new("timestamp", DataType::Timestamp(TimeUnit::Nanosecond, Some("UTC".into())), false),
        Field::new("event_type", DataType::Utf8, false),
        Field::new("ticker", DataType::Utf8, true),
        Field::new("actor", DataType::Utf8, false),
        Field::new("details", DataType::Utf8, false),
        Field::new("severity", DataType::Utf8, false),
    ]))
}

/// Canonical digest, computed identically from journal rows and from the
/// rows read back out of the Parquet file. Rows in (timestamp_ns, seq) order.
fn content_digest(rows: &[Row]) -> String {
    let mut h = Sha256::new();
    for (id, ts, ty, tk, actor, details, sev) in rows {
        h.update(format!(
            "{id}\x1f{ts}\x1f{ty}\x1f{}\x1f{actor}\x1f{details}\x1f{sev}\n",
            tk.as_deref().unwrap_or("\x00")
        ));
    }
    format!("{:x}", h.finalize())
}

fn read_back(path: &Path) -> Result<Vec<Row>, AuditError> {
    let f = File::open(path).map_err(|e| AuditError::Db(e.to_string()))?;
    let reader = ParquetRecordBatchReaderBuilder::try_new(f)
        .and_then(|b| b.build())
        .map_err(|e| AuditError::Db(e.to_string()))?;
    let mut rows = Vec::new();
    for batch in reader {
        let b = batch.map_err(|e| AuditError::Db(e.to_string()))?;
        let s = |i: usize| b.column(i).as_any().downcast_ref::<StringArray>().unwrap();
        let ts = b.column(1).as_any().downcast_ref::<TimestampNanosecondArray>().unwrap();
        for r in 0..b.num_rows() {
            let tk = if s(3).is_null(r) { None } else { Some(s(3).value(r).to_string()) };
            rows.push((
                s(0).value(r).to_string(), ts.value(r), s(2).value(r).to_string(), tk,
                s(4).value(r).to_string(), s(5).value(r).to_string(), s(6).value(r).to_string(),
            ));
        }
    }
    Ok(rows)
}

/// Export every closed (day < today UTC), not-yet-exported day. A day is
/// marked in `audit_export` only after the Parquet file has been written to
/// a temp path, fsynced, read back, and its row count and digest match the
/// journal; then renamed into place (atomic) and the directory fsynced.
/// Idempotent: a crash before the mark just redoes the day (the rename
/// replaces an identical file).
pub fn export_closed_days(conn: &Connection, lake_audit_dir: &Path) -> Result<Vec<String>, AuditError> {
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let days: Vec<String> = {
        let mut st = conn.prepare(
            "SELECT DISTINCT day FROM audit_events
              WHERE day < ?1 AND day NOT IN (SELECT day FROM audit_export)
              ORDER BY day",
        )?;
        let it = st.query_map(params![today], |r| r.get(0))?;
        it.collect::<Result<_, _>>()?
    };

    let mut done = Vec::new();
    for day in days {
        let rows: Vec<Row> = {
            let mut st = conn.prepare(
                "SELECT event_id, timestamp_ns, event_type, ticker, actor, details, severity
                   FROM audit_events WHERE day = ?1 ORDER BY timestamp_ns, seq",
            )?;
            let it = st.query_map(params![day], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))
            })?;
            it.collect::<Result<_, _>>()?
        };
        let want_n = rows.len();
        let want_hash = content_digest(&rows);

        let part_dir = lake_audit_dir.join(format!("date={day}"));
        fs::create_dir_all(&part_dir).map_err(|e| AuditError::Db(e.to_string()))?;
        // Temporary dot-file per IO-CONTRACT §2; readers glob the exact name date=*/data.parquet.
        let tmp = part_dir.join(format!(
            ".data.parquet.tmp.{}.{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let fin = part_dir.join("data.parquet");

        write_parquet(&tmp, &rows)?;
        let back = read_back(&tmp)?;
        if back.len() != want_n || content_digest(&back) != want_hash {
            fs::remove_file(&tmp).ok();
            return Err(AuditError::Invariant(format!("export verification failed for {day}")));
        }
        File::open(&tmp).and_then(|f| f.sync_all()).map_err(|e| AuditError::Db(e.to_string()))?;
        fs::rename(&tmp, &fin).map_err(|e| AuditError::Db(e.to_string()))?;
        File::open(&part_dir).and_then(|d| d.sync_all()).map_err(|e| AuditError::Db(e.to_string()))?;

        conn.execute(
            "INSERT INTO audit_export (day, row_count, content_sha256, exported_at_ns)
             VALUES (?1, ?2, ?3, ?4)",
            params![day, want_n as i64, want_hash, chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)],
        )?;
        done.push(day);
    }
    Ok(done)
}

fn write_parquet(path: &Path, rows: &[Row]) -> Result<(), AuditError> {
    let schema = parquet_schema();
    let col = |f: fn(&Row) -> Option<String>| -> StringArray { rows.iter().map(f).collect() };
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(col(|r| Some(r.0.clone()))),
            Arc::new(TimestampNanosecondArray::from(rows.iter().map(|r| r.1).collect::<Vec<_>>()).with_timezone("UTC")),
            Arc::new(col(|r| Some(r.2.clone()))),
            Arc::new(col(|r| r.3.clone())),
            Arc::new(col(|r| Some(r.4.clone()))),
            Arc::new(col(|r| Some(r.5.clone()))),
            Arc::new(col(|r| Some(r.6.clone()))),
        ],
    )
    .map_err(|e| AuditError::Db(e.to_string()))?;

    let props = WriterProperties::builder()
        .set_compression(parquet::basic::Compression::ZSTD(
            parquet::basic::ZstdLevel::try_new(3).map_err(|e| AuditError::Db(e.to_string()))?,
        ))
        .build();
    let file = File::create(path).map_err(|e| AuditError::Db(e.to_string()))?; // temp path only
    let mut w = ArrowWriter::try_new(file, schema, Some(props)).map_err(|e| AuditError::Db(e.to_string()))?;
    w.write(&batch).map_err(|e| AuditError::Db(e.to_string()))?;
    w.close().map_err(|e| AuditError::Db(e.to_string()))?;
    Ok(())
}
