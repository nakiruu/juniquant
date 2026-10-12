//! J8: FRED / ALFRED macro series with every vintage (`BACKFILL.md` §5.3, `SOURCES.md`
//! "FRED / ALFRED", `POINT-IN-TIME.md`). Per series: list the vintage dates, fetch the
//! observations in windows of at most 2,000 vintage dates (paged at 100,000 rows), stitch the
//! windows, compute `known_from`. Parsing is a pure function of the landed bodies and their
//! `fetched_at`.

use std::collections::BTreeSet;
use std::sync::Arc;

use arrow::array::{
    ArrayRef, Date32Array, Float64Array, RecordBatch, StringArray, TimestampNanosecondArray,
};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use serde_json::Value;

/// Bumped when the parser's output changes.
pub const PARSE_VERSION: &str = "1";

/// The FRED API root.
pub const BASE: &str = "https://api.stlouisfed.org/fred";

/// First observation date asked for (`BACKFILL.md` §5.3).
pub const OBSERVATION_START: &str = "2004-01-01";

/// Vintage dates the API accepts per request (P6).
pub const MAX_VINTAGES: usize = 2000;

/// Rows per page.
pub const PAGE: usize = 100_000;

/// Days between an observation and its first vintage beyond which the vintage is a late
/// capture, not a publication date (`POINT-IN-TIME.md`).
pub const MAX_CAPTURE_DELAY: i64 = 120;

/// A series is `deferred` when its first observation is this many days after
/// [`OBSERVATION_START`] (the ICE BofA series start in 2023, P6).
pub const DEFERRED_AFTER_DAYS: i64 = 31;

/// One row of the series table (`SOURCES.md`, floors and buffers of `POINT-IN-TIME.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Series {
    /// FRED series id.
    pub id: &'static str,
    /// Monthly (period ends at month-end) rather than daily.
    pub monthly: bool,
    /// Floor on `known_from - obs_date`.
    pub min_lag_days: i64,
    /// Days after the end of the observation period, for a late-captured first vintage.
    pub buffer_days: i64,
}

const fn daily(id: &'static str) -> Series {
    Series {
        id,
        monthly: false,
        min_lag_days: 1,
        buffer_days: 1,
    }
}

/// The 9 series.
///
/// shortcut: the table of `POINT-IN-TIME.md` lives here, not in `config/macro_series.toml`;
/// move it there when a buffer is tuned or a series added.
pub const SERIES: &[Series] = &[
    daily("DGS3MO"),
    daily("DGS2"),
    daily("DGS10"),
    daily("BAMLH0A0HYM2"),
    daily("BAMLC0A0CM"),
    Series {
        id: "CPIAUCSL",
        monthly: true,
        min_lag_days: 1,
        buffer_days: 45,
    },
    Series {
        id: "FEDFUNDS",
        monthly: true,
        min_lag_days: 1,
        buffer_days: 30,
    },
    Series {
        id: "UNRATE",
        monthly: true,
        min_lag_days: 1,
        buffer_days: 14,
    },
    daily("VIXCLS"),
];

/// Why a body could not be parsed (schema mismatch: keep the raw file, log FAIL).
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    /// Not JSON.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    /// JSON, but not the expected shape.
    #[error("shape: {0}")]
    Shape(String),
}

/// `series/vintagedates` for `id` (up to 10,000 dates).
#[must_use]
pub fn vintage_dates_url(id: &str, key: &str) -> String {
    format!(
        "{BASE}/series/vintagedates?series_id={id}&api_key={key}&file_type=json\
         &realtime_start=1776-07-04&realtime_end=9999-12-31&limit=10000"
    )
}

/// `series/observations` for one window and page.
#[must_use]
pub fn observations_url(id: &str, key: &str, w: &Window, offset: usize) -> String {
    format!(
        "{BASE}/series/observations?series_id={id}&api_key={key}&file_type=json\
         &observation_start={OBSERVATION_START}&realtime_start={}&realtime_end={}\
         &limit={PAGE}&offset={offset}",
        w.realtime_start, w.realtime_end
    )
}

fn date(s: &str) -> Result<NaiveDate, ParseError> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|e| ParseError::Shape(format!("date {s:?}: {e}")))
}

fn field<'a>(v: &'a Value, k: &str) -> Result<&'a str, ParseError> {
    v.get(k)
        .and_then(Value::as_str)
        .ok_or_else(|| ParseError::Shape(format!("no string field {k:?}")))
}

/// The vintage dates of a `series/vintagedates` body, ascending.
///
/// # Errors
///
/// [`ParseError`] when the body is not a vintage-date list, is empty, or is truncated.
pub fn parse_vintage_dates(body: &[u8]) -> Result<Vec<NaiveDate>, ParseError> {
    let v: Value = serde_json::from_slice(body)?;
    let list = v
        .get("vintage_dates")
        .and_then(Value::as_array)
        .ok_or_else(|| ParseError::Shape("no vintage_dates".into()))?;
    let mut dates = list
        .iter()
        .map(|d| {
            d.as_str()
                .ok_or_else(|| ParseError::Shape("vintage date not a string".into()))
                .and_then(date)
        })
        .collect::<Result<Vec<_>, _>>()?;
    // shortcut: one call holds 10,000 dates (a daily series until about 2045); page if exceeded.
    if v.get("count")
        .and_then(Value::as_u64)
        .is_some_and(|c| c as usize > dates.len())
    {
        return Err(ParseError::Shape(
            "more vintage dates than one call returns".into(),
        ));
    }
    if dates.is_empty() {
        return Err(ParseError::Shape("no vintage dates".into()));
    }
    dates.sort_unstable();
    Ok(dates)
}

/// One request's real-time range, and the vintage dates it covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    /// `realtime_start` sent (`1776-07-04` for the first window).
    pub realtime_start: NaiveDate,
    /// `realtime_end` sent (`9999-12-31` for the last window, so current rows end there).
    pub realtime_end: NaiveDate,
    /// First vintage date in the window.
    pub first: NaiveDate,
    /// Last vintage date in the window.
    pub last: NaiveDate,
}

impl Window {
    /// Manifest chunk id of one page of the window.
    #[must_use]
    pub fn chunk(&self, offset: usize) -> String {
        let base = format!("all_vintages-{}-{}", self.first, self.last);
        if offset == 0 {
            base
        } else {
            format!("{base}-o{offset}")
        }
    }
}

/// Cut ascending vintage dates into runs of at most [`MAX_VINTAGES`]. The outer windows are
/// opened to the full real-time range (same vintage count), so a series with at most 2,000
/// vintages is the single request of `SOURCES.md`.
#[must_use]
pub fn windows(dates: &[NaiveDate]) -> Vec<Window> {
    let n = dates.chunks(MAX_VINTAGES).count();
    dates
        .chunks(MAX_VINTAGES)
        .enumerate()
        .filter_map(|(i, c)| {
            let (first, last) = (*c.first()?, *c.last()?);
            Some(Window {
                realtime_start: if i == 0 {
                    NaiveDate::from_ymd_opt(1776, 7, 4)?
                } else {
                    first
                },
                realtime_end: if i + 1 == n {
                    NaiveDate::from_ymd_opt(9999, 12, 31)?
                } else {
                    last
                },
                first,
                last,
            })
        })
        .collect()
}

/// Where adjacent windows meet: `(last vintage of window k, first vintage of window k+1)`.
#[must_use]
pub fn boundaries(ws: &[Window]) -> BTreeSet<(NaiveDate, NaiveDate)> {
    ws.windows(2)
        .filter_map(|p| Some((p.first()?.last, p.get(1)?.first)))
        .collect()
}

/// One (observation, vintage interval) row as returned.
#[derive(Debug, Clone, PartialEq)]
pub struct Obs {
    /// `date`.
    pub obs_date: NaiveDate,
    /// `realtime_start`.
    pub realtime_start: NaiveDate,
    /// `realtime_end`.
    pub realtime_end: NaiveDate,
    /// `value`; `.` is null.
    pub value: Option<f64>,
    /// When the body holding the row was fetched.
    pub fetched_at: DateTime<Utc>,
}

/// One page of `series/observations`.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    /// `count`: rows in the whole window, all pages.
    pub count: usize,
    /// Rows kept.
    pub rows: Vec<Obs>,
    /// Rows dropped as future-dated (date or `realtime_start` after the fetch date, ET).
    pub future: usize,
}

/// Parse a `series/observations` body fetched at `fetched_at`.
///
/// # Errors
///
/// [`ParseError`] when the body is not an observations page (an API error message included).
pub fn parse_observations(body: &[u8], fetched_at: DateTime<Utc>) -> Result<Page, ParseError> {
    let v: Value = serde_json::from_slice(body)?;
    if let Some(m) = v.get("error_message").and_then(Value::as_str) {
        return Err(ParseError::Shape(format!("FRED error: {m}")));
    }
    let list = v
        .get("observations")
        .and_then(Value::as_array)
        .ok_or_else(|| ParseError::Shape("no observations".into()))?;
    let count = v
        .get("count")
        .and_then(Value::as_u64)
        .ok_or_else(|| ParseError::Shape("no count".into()))? as usize;
    let today = crate::eodhd::eastern(fetched_at).date();
    let (mut rows, mut future) = (Vec::with_capacity(list.len()), 0);
    for o in list {
        let raw = field(o, "value")?;
        let value = match raw {
            "." => None,
            t => Some(
                t.parse::<f64>()
                    .map_err(|e| ParseError::Shape(format!("value {t:?}: {e}")))?,
            ),
        };
        let r = Obs {
            obs_date: date(field(o, "date")?)?,
            realtime_start: date(field(o, "realtime_start")?)?,
            realtime_end: date(field(o, "realtime_end")?)?,
            value,
            fetched_at,
        };
        if r.obs_date > today || r.realtime_start > today {
            future += 1;
        } else {
            rows.push(r);
        }
    }
    Ok(Page {
        count,
        rows,
        future,
    })
}

/// Sort by `(obs_date, realtime_start)` and join intervals clipped at a window boundary: an
/// interval ending at window k's last vintage continues as the next window's interval starting
/// at its first vintage when the value is the same; when it differs (a revision at that
/// vintage) the earlier interval ends the day before. Returns the rows and the joins made.
#[must_use]
pub fn stitch(mut rows: Vec<Obs>, bounds: &BTreeSet<(NaiveDate, NaiveDate)>) -> (Vec<Obs>, usize) {
    rows.sort_by_key(|r| (r.obs_date, r.realtime_start));
    let mut out: Vec<Obs> = Vec::with_capacity(rows.len());
    let mut joined = 0;
    for r in rows {
        if let Some(p) = out.last_mut()
            && p.obs_date == r.obs_date
            && bounds.contains(&(p.realtime_end, r.realtime_start))
        {
            if p.value == r.value {
                p.realtime_end = r.realtime_end;
                joined += 1;
                continue;
            }
            p.realtime_end = r.realtime_start.pred_opt().unwrap_or(r.realtime_start);
        }
        out.push(r);
    }
    (out, joined)
}

/// `known_from_basis`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Basis {
    /// Vintage date trusted.
    Alfred,
    /// Late-captured first vintage: the buffer is used.
    LagRule,
}

impl Basis {
    /// The column value.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Alfred => "alfred",
            Self::LagRule => "lag_rule",
        }
    }
}

fn period_end(d: NaiveDate, monthly: bool) -> NaiveDate {
    if !monthly {
        return d;
    }
    let (y, m) = if d.month() == 12 {
        (d.year() + 1, 1)
    } else {
        (d.year(), d.month() + 1)
    };
    NaiveDate::from_ymd_opt(y, m, 1)
        .and_then(|n| n.pred_opt())
        .unwrap_or(d)
}

/// `known_from` of each row of stitched `rows` (sorted by `(obs_date, realtime_start)`), by the
/// rule of `POINT-IN-TIME.md`: the first vintage of an observation is known from
/// `max(realtime_start, obs_date + min_lag)`, or, captured more than [`MAX_CAPTURE_DELAY`] days
/// after the observation, from the end of its period plus the buffer; a later vintage from its
/// `realtime_start`.
#[must_use]
pub fn known_from(s: &Series, rows: &[Obs]) -> Vec<(NaiveDate, Basis)> {
    let mut prev: Option<NaiveDate> = None;
    rows.iter()
        .map(|r| {
            let first = prev != Some(r.obs_date);
            prev = Some(r.obs_date);
            if !first {
                (r.realtime_start, Basis::Alfred)
            } else if (r.realtime_start - r.obs_date).num_days() > MAX_CAPTURE_DELAY {
                (
                    period_end(r.obs_date, s.monthly) + Duration::days(s.buffer_days),
                    Basis::LagRule,
                )
            } else {
                (
                    r.realtime_start
                        .max(r.obs_date + Duration::days(s.min_lag_days)),
                    Basis::Alfred,
                )
            }
        })
        .collect()
}

/// `macro` rows (registry column order) for one series.
///
/// # Errors
///
/// Arrow errors (a bug).
pub fn batch(s: &Series, rows: &[Obs]) -> Result<RecordBatch, arrow::error::ArrowError> {
    let kf = known_from(s, rows);
    let days = |d: NaiveDate| {
        i32::try_from(d.signed_duration_since(NaiveDate::default()).num_days()).unwrap_or_default()
    };
    let d = |f: &dyn Fn(&Obs) -> NaiveDate| -> ArrayRef {
        Arc::new(Date32Array::from(
            rows.iter().map(|r| days(f(r))).collect::<Vec<_>>(),
        ))
    };
    let cols: Vec<ArrayRef> = vec![
        Arc::new(StringArray::from(vec![s.id; rows.len()])),
        d(&|r| r.obs_date),
        d(&|r| r.realtime_start),
        d(&|r| r.realtime_end),
        Arc::new(Float64Array::from(
            rows.iter().map(|r| r.value).collect::<Vec<_>>(),
        )),
        Arc::new(Date32Array::from(
            kf.iter().map(|(k, _)| days(*k)).collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            kf.iter().map(|(_, b)| b.as_str()).collect::<Vec<_>>(),
        )),
        Arc::new(
            TimestampNanosecondArray::from(
                rows.iter()
                    .map(|r| r.fetched_at.timestamp_nanos_opt().unwrap_or_default())
                    .collect::<Vec<_>>(),
            )
            .with_timezone("UTC"),
        ),
    ];
    let ts = DataType::Timestamp(TimeUnit::Nanosecond, Some("UTC".into()));
    let f = |name: &str, ty: DataType| Field::new(name, ty, true);
    let schema = Schema::new(vec![
        f("series_id", DataType::Utf8),
        f("obs_date", DataType::Date32),
        f("realtime_start", DataType::Date32),
        f("realtime_end", DataType::Date32),
        f("value", DataType::Float64),
        f("known_from", DataType::Date32),
        f("known_from_basis", DataType::Utf8),
        f("fetched_at", ts),
    ]);
    RecordBatch::try_new(Arc::new(schema), cols)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing, reason = "test")]

    use super::*;

    fn d(s: &str) -> NaiveDate {
        date(s).unwrap()
    }

    fn obs(o: &str, a: &str, b: &str, v: Option<f64>) -> Obs {
        Obs {
            obs_date: d(o),
            realtime_start: d(a),
            realtime_end: d(b),
            value: v,
            fetched_at: DateTime::default(),
        }
    }

    #[test]
    fn parses_observations_and_drops_future_rows() {
        let at = DateTime::parse_from_rfc3339("2026-10-11T14:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let body = br#"{"count":3,"offset":0,"limit":100000,"observations":[
            {"realtime_start":"2005-06-28","realtime_end":"9999-12-31","date":"2004-01-02","value":"0.91"},
            {"realtime_start":"2005-06-28","realtime_end":"9999-12-31","date":"2004-01-05","value":"."},
            {"realtime_start":"2026-10-12","realtime_end":"9999-12-31","date":"2026-10-09","value":"3.9"}]}"#;
        let p = parse_observations(body, at).unwrap();
        assert_eq!((p.count, p.rows.len(), p.future), (3, 2, 1));
        assert_eq!(p.rows[0].value, Some(0.91));
        assert_eq!(p.rows[1].value, None, "'.' is null");
        assert_eq!(p.rows[0].realtime_end, d("9999-12-31"));
        assert!(
            parse_observations(br#"{"error_code":400,"error_message":"exceeds"}"#, at).is_err()
        );
        assert!(
            parse_vintage_dates(br#"{"count":2,"vintage_dates":["2005-06-29","2005-06-28"]}"#)
                .is_ok_and(|v| v == [d("2005-06-28"), d("2005-06-29")])
        );
    }

    #[test]
    fn windows_hold_at_most_2000_vintages_and_stitch_back() {
        let dates: Vec<NaiveDate> = (0..4500)
            .map(|i| d("2005-01-01") + Duration::days(i))
            .collect();
        let ws = windows(&dates);
        assert_eq!(ws.len(), 3);
        assert_eq!(
            (ws[0].realtime_start, ws[0].realtime_end),
            (d("1776-07-04"), dates[1999])
        );
        assert_eq!(
            (ws[1].realtime_start, ws[1].realtime_end),
            (dates[2000], dates[3999])
        );
        assert_eq!(ws[2].realtime_end, d("9999-12-31"));
        assert_eq!(
            ws[0].chunk(0),
            format!("all_vintages-2005-01-01-{}", dates[1999])
        );
        let b = boundaries(&ws);
        assert_eq!(b.len(), 2);
        let (e0, s1) = (dates[1999], dates[2000]);
        let rows = vec![
            // Unchanged across the boundary: one interval.
            obs("2005-01-03", "2005-01-04", &e0.to_string(), Some(2.0)),
            obs("2005-01-03", &s1.to_string(), "9999-12-31", Some(2.0)),
            // Revised exactly at the next window's first vintage: two intervals, no gap.
            obs("2005-01-04", "2005-01-05", &e0.to_string(), Some(2.1)),
            obs("2005-01-04", &s1.to_string(), "9999-12-31", Some(2.2)),
        ];
        let (out, joined) = stitch(rows, &b);
        assert_eq!(joined, 1);
        assert_eq!(out.len(), 3);
        assert_eq!(
            (out[0].realtime_start, out[0].realtime_end),
            (d("2005-01-04"), d("9999-12-31"))
        );
        assert_eq!(out[1].realtime_end, e0, "ends the day before the revision");
    }

    #[test]
    fn known_from_follows_point_in_time() {
        let cpi = SERIES.iter().find(|s| s.id == "CPIAUCSL").unwrap();
        let rows = vec![
            // Contemporaneous first vintage: the vintage date.
            obs("2004-01-01", "2004-02-20", "2004-03-16", Some(1.0)),
            // Later vintage: its realtime_start.
            obs("2004-01-01", "2004-03-17", "9999-12-31", Some(1.1)),
            // Late capture (more than 120 days): month-end + 45 days.
            obs("2004-02-01", "2005-06-28", "9999-12-31", Some(1.2)),
        ];
        assert_eq!(
            known_from(cpi, &rows),
            [
                (d("2004-02-20"), Basis::Alfred),
                (d("2004-03-17"), Basis::Alfred),
                (d("2004-02-29") + Duration::days(45), Basis::LagRule),
            ]
        );
        // The floor wins when the vintage is the observation day itself.
        let dgs = SERIES[0];
        let r = [obs("2026-10-08", "2026-10-08", "9999-12-31", Some(3.9))];
        assert_eq!(known_from(&dgs, &r), [(d("2026-10-09"), Basis::Alfred)]);
        let b = batch(&dgs, &r).unwrap();
        assert_eq!((b.num_rows(), b.num_columns()), (1, 8));
    }
}
