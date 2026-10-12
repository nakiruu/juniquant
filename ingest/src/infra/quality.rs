//! The `data_quality` rows of a run (`SKILL.md` §4 step 7): one per fetched, gone or failed
//! chunk, written through the lake writer at the end of the run.

use std::sync::{Arc, Mutex};

use arrow::array::{ArrayRef, Date32Array, Float64Array, Int32Array, RecordBatch, StringArray};
use chrono::{NaiveDate, Utc};

/// `data_quality.status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Fetched.
    Ok,
    /// A result to look at, not a failure (a `gone` chunk).
    Warn,
    /// Failed.
    Fail,
}

impl Level {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Warn => "WARN",
            Self::Fail => "FAIL",
        }
    }
}

/// One fetch outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// `data_quality.source` label (`EODHD`, `SEC`, ...).
    pub source: &'static str,
    /// Lake dataset the chunk feeds.
    pub dataset: String,
    /// Ticker, CIK, series id or ETF.
    pub entity: String,
    /// Manifest chunk id.
    pub chunk: String,
    /// Outcome.
    pub level: Level,
    /// Attempts made.
    pub attempts: u32,
    /// Reason or error.
    pub notes: Option<String>,
}

/// What a gate row adds to a fetch row (`QUALITY-GATES.md` §1).
#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    /// `G0` ... `G9`.
    pub id: &'static str,
    /// The value measured.
    pub measured: Option<f64>,
    /// The threshold it was compared with.
    pub threshold: Option<f64>,
}

/// Rows with their gate fields, if any.
type Entries = Vec<(Row, Option<Check>)>;

/// The rows of one run; cheap to clone and share between fetch tasks.
#[derive(Debug, Clone)]
pub struct QualityLog {
    /// `data_quality.run_id`: UTC start time and mode.
    pub run_id: String,
    /// `data_quality.date`: the run date (ET).
    pub date: NaiveDate,
    rows: Arc<Mutex<Entries>>,
}

impl QualityLog {
    /// A log for a run starting now in `mode` (`backfill`, `daily`, ...).
    #[must_use]
    pub fn new(mode: &str, date: NaiveDate) -> Self {
        Self {
            run_id: format!("{}-{mode}", Utc::now().format("%Y%m%dT%H%M%SZ")),
            date,
            rows: Arc::default(),
        }
    }

    /// Record an outcome.
    pub fn push(&self, row: Row) {
        self.lock().push((row, None));
    }

    /// Record a gate result.
    pub fn push_gate(&self, row: Row, check: Check) {
        self.lock().push((row, Some(check)));
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Entries> {
        self.rows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The rows so far.
    #[must_use]
    pub fn rows(&self) -> Vec<Row> {
        self.lock().iter().map(|(r, _)| r.clone()).collect()
    }

    /// The rows as a `data_quality` batch (registry column order).
    ///
    /// # Errors
    ///
    /// Arrow errors (a column-count mismatch is a bug).
    pub fn batch(&self) -> Result<RecordBatch, arrow::error::ArrowError> {
        let all = self.lock().clone();
        let rows: Vec<Row> = all.iter().map(|(r, _)| r.clone()).collect();
        let n = rows.len();
        let check = |f: &dyn Fn(&Check) -> Option<f64>| -> ArrayRef {
            std::sync::Arc::new(Float64Array::from(
                all.iter()
                    .map(|(_, c)| c.as_ref().and_then(f))
                    .collect::<Vec<_>>(),
            ))
        };
        let days = i32::try_from(
            self.date
                .signed_duration_since(NaiveDate::default())
                .num_days(),
        )
        .unwrap_or_default();
        let s = |f: &dyn Fn(&Row) -> Option<String>| -> ArrayRef {
            std::sync::Arc::new(StringArray::from(rows.iter().map(f).collect::<Vec<_>>()))
        };
        let ok = |r: &Row| i32::from(r.level != Level::Fail);
        let schema = lake_schema()?;
        let cols: Vec<ArrayRef> = vec![
            std::sync::Arc::new(Date32Array::from(vec![days; n])),
            s(&|r| Some(r.source.to_owned())),
            s(&|r| Some(r.dataset.clone())),
            std::sync::Arc::new(Int32Array::from(vec![1; n])),
            std::sync::Arc::new(Int32Array::from(rows.iter().map(ok).collect::<Vec<_>>())),
            std::sync::Arc::new(Float64Array::from(
                rows.iter()
                    .map(|r| if ok(r) == 1 { 0.0 } else { 100.0 })
                    .collect::<Vec<_>>(),
            )),
            std::sync::Arc::new(Int32Array::from(vec![0; n])),
            s(&|r| Some(r.level.as_str().to_owned())),
            s(&|r| r.notes.clone()),
            s(&|r| Some(r.entity.clone())),
            s(&|r| Some(r.chunk.clone())),
            s(&|_| Some(self.run_id.clone())),
            std::sync::Arc::new(Int32Array::from(
                rows.iter()
                    .map(|r| i32::try_from(r.attempts).ok())
                    .collect::<Vec<_>>(),
            )),
            std::sync::Arc::new(StringArray::from(
                all.iter()
                    .map(|(_, c)| Some(c.as_ref().map_or("*", |c| c.id)))
                    .collect::<Vec<_>>(),
            )),
            check(&|c| c.measured),
            check(&|c| c.threshold),
        ];
        RecordBatch::try_new(schema, cols)
    }

    /// Append the rows to `data_quality`.
    ///
    /// # Errors
    ///
    /// Lake errors (a re-run with the same `run_id` and the same rows is a no-op).
    pub fn write(&self, lake: &lake::Lake) -> Result<(), lake::LakeError> {
        if self.rows().is_empty() {
            return Ok(());
        }
        lake.write_dataset("data_quality", &self.batch()?, lake::Mode::Append)
    }
}

/// The batch schema: registry names and types, every column declared nullable (the lake
/// writer checks the required ones and relabels).
fn lake_schema() -> Result<arrow::datatypes::SchemaRef, arrow::error::ArrowError> {
    use arrow::datatypes::{DataType as T, Field, Schema};
    let f = |name: &str, ty: T| Field::new(name, ty, true);
    Ok(std::sync::Arc::new(Schema::new(vec![
        f("date", T::Date32),
        f("source", T::Utf8),
        f("dataset", T::Utf8),
        f("tickers_expected", T::Int32),
        f("tickers_received", T::Int32),
        f("missing_pct", T::Float64),
        f("latency_min", T::Int32),
        f("status", T::Utf8),
        f("notes", T::Utf8),
        f("entity", T::Utf8),
        f("chunk", T::Utf8),
        f("run_id", T::Utf8),
        f("attempts", T::Int32),
        f("check_id", T::Utf8),
        f("measured", T::Float64),
        f("threshold", T::Float64),
    ])))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing, reason = "test")]

    use super::*;

    #[test]
    fn rows_write_to_data_quality_and_rewrite_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let registry = lake::Registry::load(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../schemas/registry.json"),
        )
        .unwrap();
        let lake = lake::Lake::new(dir.path().join("lake"), dir.path().join("locks"), registry);
        let log = QualityLog::new("test", NaiveDate::from_ymd_opt(2026, 10, 11).unwrap());
        for (entity, level) in [
            ("AAPL.US", Level::Ok),
            ("XX.US", Level::Warn),
            ("YY.US", Level::Fail),
        ] {
            log.push(Row {
                source: "EODHD",
                dataset: "prices".into(),
                entity: entity.into(),
                chunk: "eod-full".into(),
                level,
                attempts: 1,
                notes: (level != Level::Ok).then(|| "x".into()),
            });
        }
        log.write(&lake).unwrap();
        log.write(&lake).unwrap();
        let back = lake
            .read_dataset("data_quality", Some(&["status"]), &[], None)
            .unwrap();
        assert_eq!(back.num_rows(), 3);
    }
}
