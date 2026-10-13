//! W5: EODHD prices, splits and dividends (DECISIONS D15; `BACKFILL.md` §5.1,
//! `SOURCES.md` EODHD). Parsing is pure (raw bytes in, rows out) so a parser fix is replayed
//! from the landing zone, never re-fetched.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use arrow::array::{
    ArrayRef, Date32Array, Float64Array, Int64Array, RecordBatch, StringArray,
    TimestampNanosecondArray,
};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, Utc, Weekday};
use serde_json::Value;

/// Bumped when a parser's output changes; recorded per stage file.
pub const PARSE_VERSION: &str = "2";

/// Exchanges in the D12 scope (as the symbol list names them).
const EXCHANGES: &[&str] = &["NYSE", "NASDAQ", "AMEX", "NYSE MKT"];

/// One code from `exchange-symbol-list/US`.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Code {
    /// EODHD code without `.US` (`BRK-B`, `MON_old`).
    pub code: String,
    /// The canonical ticker it is stored under.
    pub ticker: String,
    /// From the delisted list.
    pub delisted: bool,
    /// `Name` in the symbol list.
    pub name: String,
    /// `Isin` in the symbol list, when it has one.
    pub isin: Option<String>,
    /// `Exchange` in the symbol list (`NYSE`, `NASDAQ`, `AMEX`, `NYSE MKT`); empty when unknown.
    pub exchange: String,
}

/// The canonical ticker of an EODHD code: the `_old`, `_old1`, ... suffix of an earlier
/// holder of a reused ticker is dropped, then the symbol is normalized (`BRK-B` -> `BRK.B`).
#[must_use]
pub fn code_ticker(code: &str) -> Option<String> {
    let base = match code.find("_old") {
        Some(i)
            if code
                .get(i + 4..)
                .is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit())) =>
        {
            code.get(..i)?
        }
        _ => code,
    };
    symbols::normalize(base).ok().map(|c| c.as_str().to_owned())
}

/// In-scope codes of a symbol-list body; codes that do not normalize are returned separately.
#[must_use]
pub fn parse_symbols(body: &[u8], delisted: bool) -> (Vec<Code>, Vec<String>) {
    let rows: Vec<Value> = serde_json::from_slice(body).unwrap_or_default();
    let (mut ok, mut bad) = (Vec::new(), Vec::new());
    for r in &rows {
        let (Some(code), Some(ex)) = (
            r.get("Code").and_then(Value::as_str),
            r.get("Exchange").and_then(Value::as_str),
        ) else {
            continue;
        };
        if !EXCHANGES.contains(&ex) {
            continue;
        }
        match code_ticker(code) {
            Some(ticker) => ok.push(Code {
                code: code.to_owned(),
                ticker,
                delisted,
                name: r
                    .get("Name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                isin: r
                    .get("Isin")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned),
                exchange: ex.to_owned(),
            }),
            None => bad.push(code.to_owned()),
        }
    }
    (ok, bad)
}

/// US Eastern local time of `t` (DST from the second Sunday of March 02:00 to the first
/// Sunday of November 02:00, the rule since 2007; earlier years are not needed here).
#[must_use]
pub fn eastern(t: DateTime<Utc>) -> NaiveDateTime {
    let year = t.year();
    let nth_sunday = |month: u32, n: u32| {
        NaiveDate::from_weekday_of_month_opt(
            year,
            month,
            Weekday::Sun,
            u8::try_from(n).unwrap_or(1),
        )
    };
    // 02:00 local = 07:00 UTC in March (EST), 06:00 UTC in November (EDT).
    let start = nth_sunday(3, 2)
        .and_then(|d| d.and_hms_opt(7, 0, 0))
        .map(|d| d.and_utc());
    let end = nth_sunday(11, 1)
        .and_then(|d| d.and_hms_opt(6, 0, 0))
        .map(|d| d.and_utc());
    let dst = matches!((start, end), (Some(s), Some(e)) if t >= s && t < e);
    (t - chrono::Duration::hours(if dst { 4 } else { 5 })).naive_utc()
}

/// `split` strings `"4.000000/1.000000"` as `(ex_date, numerator, denominator)`.
#[must_use]
pub fn parse_splits(body: &[u8]) -> Vec<(NaiveDate, f64, f64)> {
    let rows: Vec<Value> = serde_json::from_slice(body).unwrap_or_default();
    let mut out: Vec<_> = rows
        .iter()
        .filter_map(|r| {
            let d = NaiveDate::parse_from_str(r.get("date")?.as_str()?, "%Y-%m-%d").ok()?;
            let (n, m) = r.get("split")?.as_str()?.split_once('/')?;
            let (n, m) = (n.trim().parse::<f64>().ok()?, m.trim().parse::<f64>().ok()?);
            (n > 0.0 && m > 0.0).then_some((d, n, m))
        })
        .collect();
    out.sort_by_key(|s| s.0);
    out
}

/// Provenance stamped on every row.
#[derive(Debug, Clone)]
pub struct Origin {
    /// US composite FIGI (DECISIONS D17).
    pub figi: String,
    /// Canonical ticker.
    pub ticker: String,
    /// When the body was fetched.
    pub fetched_at: DateTime<Utc>,
    /// SHA-256 of the body.
    pub raw_sha256: String,
}

/// Rows dropped while parsing, by reason (counted into `data_quality`).
pub type Dropped = BTreeMap<&'static str, usize>;

fn days(d: NaiveDate) -> i32 {
    i32::try_from(d.signed_duration_since(NaiveDate::default()).num_days()).unwrap_or_default()
}

fn nanos(t: DateTime<Utc>) -> i64 {
    t.timestamp_nanos_opt().unwrap_or_default()
}

fn round4(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}

fn schema(fields: &[(&str, DataType)]) -> Arc<Schema> {
    Arc::new(Schema::new(
        fields
            .iter()
            .map(|(n, t)| Field::new(*n, t.clone(), true))
            .collect::<Vec<_>>(),
    ))
}

fn ts() -> DataType {
    DataType::Timestamp(TimeUnit::Nanosecond, Some("UTC".into()))
}

/// A split this large either way is tested for the vendor's volume step ([`volume_factors`]);
/// below it, ordinary changes in trading around a split hide the step.
const VOLUME_TEST_RATIO: f64 = 9.0;
/// Bars each side of an ex-date in the volume step test.
const VOLUME_WINDOW: usize = 20;

/// Median vendor volume of the [`VOLUME_WINDOW`] bars from `ex_date` over that of the ones
/// before it (each side needs half of them with volume); `None` when untestable.
fn volume_step(bars: &[(NaiveDate, f64, f64)], ex_date: NaiveDate) -> Option<f64> {
    let i = bars.partition_point(|b| b.0 < ex_date);
    let median = |w: Option<&[(NaiveDate, f64, f64)]>| {
        let mut v: Vec<f64> = w
            .unwrap_or_default()
            .iter()
            .map(|b| b.2)
            .filter(|v| *v > 0.0)
            .collect();
        v.sort_by(f64::total_cmp);
        (v.len() * 2 >= VOLUME_WINDOW)
            .then(|| v.get(v.len() / 2).copied())
            .flatten()
    };
    let pre = median(bars.get(i.saturating_sub(VOLUME_WINDOW)..i))?;
    let post = median(bars.get(i..(i + VOLUME_WINDOW).min(bars.len())))?;
    Some(post / pre)
}

/// The raw closes move by the split at `ex_date`: the first close on or after it over the last
/// one before it, times `ratio`, in `[0.4, 2.5]` (G2's split test). A split they do not show
/// is not in the vendor's raw prices: a phantom (Noble's "1:500" of 2009, a 1:1 redomicile)
/// or prices the vendor adjusted, whose volume it adjusted with them.
fn split_shows(bars: &[(NaiveDate, f64, f64)], ex_date: NaiveDate, ratio: f64) -> bool {
    let i = bars.partition_point(|b| b.0 < ex_date);
    match (i.checked_sub(1).and_then(|p| bars.get(p)), bars.get(i)) {
        (Some(p), Some(b)) if p.1 > 0.0 => (0.4..=2.5).contains(&(b.1 / p.1 * ratio)),
        _ => false,
    }
}

/// `(ex_date, F)` from the latest split back: the vendor volume of a bar before `ex_date`
/// (and on or after the previous split's) is `F` times the volume as traded. EODHD documents
/// split-adjusted volume, but not every history is (CHK before its 2020 1:200 split was raw;
/// Key Energy before 2016 was raw for its 2020 split too): between two splits the vendor's
/// factor is constant, so at each split of at least [`VOLUME_TEST_RATIO`] either way the step
/// of its volume picks the nearest of: adjusted for this split (`F` of the later segment times
/// the ratio), not for this one (the later `F`), or for none (1). A smaller split counts as
/// adjusted, the documented behaviour; an untestable large one, and a split the raw closes do
/// not show ([`split_shows`]), count as not adjusted. `bars` are `(date, close, vendor
/// volume)` in date order; `splits` `(ex_date, ratio)` in date order, none after the last bar.
///
/// shortcut: a window can straddle a neighbouring split less than 20 bars away; bound the
/// windows by the neighbours if such pairs matter.
fn volume_factors(
    bars: &[(NaiveDate, f64, f64)],
    splits: &[(NaiveDate, f64)],
) -> Vec<(NaiveDate, f64)> {
    let mut out = Vec::new();
    let mut later = 1.0;
    for &(x, r) in splits.iter().rev() {
        if !split_shows(bars, x, r) {
            continue;
        }
        let f = if r.ln().abs() < VOLUME_TEST_RATIO.ln() {
            later * r
        } else {
            match volume_step(bars, x) {
                Some(s) => {
                    let seen = later * r / s;
                    [later * r, later, 1.0]
                        .into_iter()
                        .min_by(|a, b| (a / seen).ln().abs().total_cmp(&(b / seen).ln().abs()))
                        .unwrap_or(later)
                }
                None => later,
            }
        };
        out.push((x, f));
        later = f;
    }
    out.reverse();
    out
}

/// `prices` rows from an `/api/eod` body (`SOURCES.md` EODHD normalization): OHLC as returned
/// (raw), volume divided by `F(d)`, the product of the ratios of the splits with
/// `d < ex_date <=` the fetch date and the last bar, as far as the vendor applied them to its
/// volume (`volume_factors`). Dropped: a null field, `high < low`, a close outside `[low, high]`,
/// a negative volume, a bar after the fetch date (ET), today's bar fetched before 16:05 ET,
/// and a bar on a day that is not a session of `cal` (the vendor has a few holiday bars).
///
/// # Errors
///
/// Arrow errors (a bug).
pub fn prices(
    body: &[u8],
    splits: &[(NaiveDate, f64, f64)],
    cal: &calendar::Calendar,
    o: &Origin,
) -> Result<(RecordBatch, Dropped), arrow::error::ArrowError> {
    let rows: Vec<Value> = serde_json::from_slice(body).unwrap_or_default();
    let et = eastern(o.fetched_at);
    let today = et.date();
    let session_done = et.time() >= chrono::NaiveTime::from_hms_opt(16, 5, 0).unwrap_or_default();
    let mut dropped = Dropped::new();
    // (date, [open, high, low, close], vendor volume, adjusted close)
    let mut bars: Vec<(NaiveDate, [f64; 4], f64, Option<f64>)> = Vec::new();
    for r in &rows {
        let num = |k: &str| r.get(k).and_then(Value::as_f64);
        let Some(d) = r
            .get("date")
            .and_then(Value::as_str)
            .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
        else {
            *dropped.entry("bad_date").or_default() += 1;
            continue;
        };
        let (Some(op), Some(hi), Some(lo), Some(cl), Some(vol)) = (
            num("open"),
            num("high"),
            num("low"),
            num("close"),
            num("volume"),
        ) else {
            *dropped.entry("null_field").or_default() += 1;
            continue;
        };
        let reason = if d > today {
            Some("future_bar")
        } else if d == today && !session_done {
            Some("partial_bar")
        } else if !cal.is_trading_day(d) {
            Some("non_session")
        } else if hi < lo || cl < lo || cl > hi || vol < 0.0 {
            Some("sanity")
        } else {
            None
        };
        if let Some(why) = reason {
            *dropped.entry(why).or_default() += 1;
            continue;
        }
        bars.push((d, [op, hi, lo, cl], vol, num("adjusted_close")));
    }
    bars.sort_by_key(|b| b.0);
    let series: Vec<(NaiveDate, f64, f64)> = bars.iter().map(|b| (b.0, b.1[3], b.2)).collect();
    // A split after the last bar is a later holder's of the code (SunEdison's SUNE, 2022-2025).
    let last = bars.last().map_or(NaiveDate::MIN, |b| b.0);
    let mut own: Vec<(NaiveDate, f64)> = splits
        .iter()
        .filter(|(x, _, _)| *x <= today && *x <= last)
        .map(|(x, n, m)| (*x, n / m))
        .collect();
    own.sort_by_key(|s| s.0);
    let factors = volume_factors(&series, &own);
    let n = bars.len();
    let mut ohlc: [Vec<f64>; 4] = Default::default();
    for b in &bars {
        for (col, v) in ohlc.iter_mut().zip(b.1) {
            col.push(round4(v));
        }
    }
    let dates: Vec<i32> = bars.iter().map(|b| days(b.0)).collect();
    #[allow(
        clippy::cast_possible_truncation,
        reason = "share counts are far below 2^53"
    )]
    let vols: Vec<i64> = bars
        .iter()
        .map(|(d, _, vol, _)| {
            // The factor of the segment the bar is in: the first split after it, else 1.
            let f = factors.iter().find(|(x, _)| x > d).map_or(1.0, |s| s.1);
            (vol / f).round() as i64
        })
        .collect();
    let adj: Vec<Option<f64>> = bars.iter().map(|b| b.3).collect();
    let [op, hi, lo, cl] = ohlc;
    let cols: Vec<ArrayRef> = vec![
        Arc::new(StringArray::from(vec![o.figi.as_str(); n])),
        Arc::new(StringArray::from(vec![o.ticker.as_str(); n])),
        Arc::new(Date32Array::from(dates)),
        Arc::new(Float64Array::from(op)),
        Arc::new(Float64Array::from(hi)),
        Arc::new(Float64Array::from(lo)),
        Arc::new(Float64Array::from(cl)),
        Arc::new(Int64Array::from(vols)),
        Arc::new(Float64Array::from(adj)),
        Arc::new(StringArray::from(vec!["eodhd"; n])),
        Arc::new(TimestampNanosecondArray::from(vec![nanos(o.fetched_at); n]).with_timezone("UTC")),
        Arc::new(StringArray::from(vec![o.raw_sha256.as_str(); n])),
    ];
    let s = schema(&[
        ("figi", DataType::Utf8),
        ("ticker", DataType::Utf8),
        ("date", DataType::Date32),
        ("open", DataType::Float64),
        ("high", DataType::Float64),
        ("low", DataType::Float64),
        ("close", DataType::Float64),
        ("volume", DataType::Int64),
        ("adj_close_vendor", DataType::Float64),
        ("source", DataType::Utf8),
        ("fetched_at", ts()),
        ("raw_sha256", DataType::Utf8),
    ]);
    Ok((RecordBatch::try_new(s, cols)?, dropped))
}

/// `splits` rows; a split dated after the fetch date (ET) is dropped (announced, not yet
/// effective: it returns with a later fetch).
///
/// # Errors
///
/// Arrow errors (a bug).
pub fn splits_batch(
    splits: &[(NaiveDate, f64, f64)],
    o: &Origin,
) -> Result<(RecordBatch, Dropped), arrow::error::ArrowError> {
    let today = eastern(o.fetched_at).date();
    let (splits, future): (Vec<_>, Vec<_>) = splits.iter().copied().partition(|s| s.0 <= today);
    let mut dropped = Dropped::new();
    if !future.is_empty() {
        dropped.insert("future_event", future.len());
    }
    let n = splits.len();
    let col = |f: fn(&(NaiveDate, f64, f64)) -> f64| -> ArrayRef {
        Arc::new(Float64Array::from(splits.iter().map(f).collect::<Vec<_>>()))
    };
    let cols: Vec<ArrayRef> = vec![
        Arc::new(StringArray::from(vec![o.figi.as_str(); n])),
        Arc::new(StringArray::from(vec![o.ticker.as_str(); n])),
        Arc::new(Date32Array::from(
            splits.iter().map(|s| days(s.0)).collect::<Vec<_>>(),
        )),
        col(|s| s.1),
        col(|s| s.2),
        col(|s| s.1 / s.2),
        Arc::new(StringArray::from(vec!["eodhd"; n])),
        Arc::new(TimestampNanosecondArray::from(vec![nanos(o.fetched_at); n]).with_timezone("UTC")),
        Arc::new(StringArray::from(vec![o.raw_sha256.as_str(); n])),
    ];
    let s = schema(&[
        ("figi", DataType::Utf8),
        ("ticker", DataType::Utf8),
        ("ex_date", DataType::Date32),
        ("numerator", DataType::Float64),
        ("denominator", DataType::Float64),
        ("ratio", DataType::Float64),
        ("source", DataType::Utf8),
        ("fetched_at", ts()),
        ("raw_sha256", DataType::Utf8),
    ]);
    Ok((RecordBatch::try_new(s, cols)?, dropped))
}

/// `dividends` rows from an `/api/div` body: `amount` = `unadjustedValue` (as paid); a row
/// without it is dropped, and so is one dated after the fetch date (ET: declared, not yet
/// paid). Two rows on one ex-date are summed (a regular and a special dividend on the same
/// day keep the key unique).
///
/// # Errors
///
/// Arrow errors (a bug).
pub fn dividends(
    body: &[u8],
    o: &Origin,
) -> Result<(RecordBatch, Dropped), arrow::error::ArrowError> {
    let rows: Vec<Value> = serde_json::from_slice(body).unwrap_or_default();
    let today = eastern(o.fetched_at).date();
    let mut dropped = Dropped::new();
    let mut by_date: BTreeMap<NaiveDate, (f64, Option<String>)> = BTreeMap::new();
    for r in &rows {
        let d = r
            .get("date")
            .and_then(Value::as_str)
            .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());
        let amount = r.get("unadjustedValue").and_then(Value::as_f64);
        let (Some(d), Some(a)) = (d, amount) else {
            *dropped.entry("null_field").or_default() += 1;
            continue;
        };
        if d > today {
            *dropped.entry("future_event").or_default() += 1;
            continue;
        }
        let e = by_date.entry(d).or_insert((
            0.0,
            r.get("currency").and_then(Value::as_str).map(str::to_owned),
        ));
        e.0 += a;
    }
    let n = by_date.len();
    let cols: Vec<ArrayRef> = vec![
        Arc::new(StringArray::from(vec![o.figi.as_str(); n])),
        Arc::new(StringArray::from(vec![o.ticker.as_str(); n])),
        Arc::new(Date32Array::from(
            by_date.keys().map(|d| days(*d)).collect::<Vec<_>>(),
        )),
        Arc::new(Float64Array::from(
            // Rounded to 1e-6: a sum of two amounts must not carry float noise.
            by_date
                .values()
                .map(|v| (v.0 * 1e6).round() / 1e6)
                .collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            by_date.values().map(|v| v.1.clone()).collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(vec!["eodhd"; n])),
        Arc::new(TimestampNanosecondArray::from(vec![nanos(o.fetched_at); n]).with_timezone("UTC")),
        Arc::new(StringArray::from(vec![o.raw_sha256.as_str(); n])),
    ];
    let s = schema(&[
        ("figi", DataType::Utf8),
        ("ticker", DataType::Utf8),
        ("ex_date", DataType::Date32),
        ("amount", DataType::Float64),
        ("currency", DataType::Utf8),
        ("source", DataType::Utf8),
        ("fetched_at", ts()),
        ("raw_sha256", DataType::Utf8),
    ]);
    Ok((RecordBatch::try_new(s, cols)?, dropped))
}

/// The in-scope codes of both symbol lists, one per code (active wins over delisted).
#[must_use]
pub fn merge_lists(active: Vec<Code>, delisted: Vec<Code>) -> Vec<Code> {
    let mut seen = BTreeSet::new();
    let mut out: Vec<Code> = active
        .into_iter()
        .chain(delisted)
        .filter(|c| seen.insert(c.code.clone()))
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing, reason = "test")]

    use arrow::array::AsArray;
    use arrow::datatypes::{Float64Type, Int64Type};

    use super::*;

    fn origin(fetched: &str) -> Origin {
        Origin {
            figi: "BBG000B9XRY4".into(),
            ticker: "AAPL".into(),
            fetched_at: fetched.parse().unwrap(),
            raw_sha256: "ab".into(),
        }
    }

    fn cal() -> calendar::Calendar {
        calendar::Calendar::rules_only()
    }

    /// An `/api/eod` body of `n` sessions from `from` (holidays skipped) with the close
    /// `close(i)` and the volume `vol(i)` of the i-th bar.
    fn series(
        from: NaiveDate,
        n: usize,
        close: impl Fn(usize) -> f64,
        vol: impl Fn(usize) -> f64,
    ) -> (Vec<u8>, Vec<NaiveDate>) {
        let days: Vec<NaiveDate> = from
            .iter_days()
            .filter(|d| cal().is_trading_day(*d))
            .take(n)
            .collect();
        let rows: Vec<String> = days
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let c = close(i);
                format!(
                    r#"{{"date":"{d}","open":{c},"high":{c},"low":{c},"close":{c},"volume":{}}}"#,
                    vol(i)
                )
            })
            .collect();
        (format!("[{}]", rows.join(",")).into_bytes(), days)
    }

    #[test]
    fn volume_is_undone_only_as_far_as_the_vendor_adjusted_it() {
        let volumes = |body: &[u8], splits: &[(NaiveDate, f64, f64)]| {
            let (b, _) = prices(body, splits, &cal(), &origin("2021-01-04T22:00:00Z")).unwrap();
            b.column(7).as_primitive::<Int64Type>().values().to_vec()
        };
        let start = NaiveDate::from_ymd_opt(2020, 3, 2).unwrap();
        // A 1:200 reverse split on bar 20: the raw close goes from 1 to 200.
        let px = |i: usize| if i < 20 { 1.0 } else { 200.0 };
        // Raw vendor volume (CHK 2020-04-15): 40M shares a day before, 200k after.
        let (raw, days) = series(start, 40, px, |i| if i < 20 { 4e7 } else { 2e5 });
        let split = [(days[20], 1.0, 200.0)];
        let v = volumes(&raw, &split);
        assert_eq!((v[0], v[39]), (40_000_000, 200_000), "raw: left as traded");
        // Adjusted vendor volume (no step): undone to 40M before the split.
        let (adjusted, _) = series(start, 40, px, |_| 2e5);
        let v = volumes(&adjusted, &split);
        assert_eq!((v[0], v[39]), (40_000_000, 200_000), "adjusted: undone");
        // A 2:1 split is too small to test and is undone, the documented default.
        let (two, days2) = series(start, 40, |i| if i < 20 { 2.0 } else { 1.0 }, |_| 4e7);
        assert_eq!(volumes(&two, &[(days2[20], 2.0, 1.0)])[0], 20_000_000);
        // A split the raw closes do not show (a phantom, or prices the vendor adjusted).
        let (flat, _) = series(start, 40, |_| 1.0, |_| 2e5);
        assert_eq!(volumes(&flat, &split)[0], 200_000, "phantom: not undone");
        // Too few bars before the split to test it: left as the vendor gave it.
        let (short, days3) = series(start, 40, |i| if i < 3 { 1.0 } else { 200.0 }, |_| 2e5);
        assert_eq!(volumes(&short, &[(days3[3], 1.0, 200.0)])[0], 200_000);
        // A split after the last bar belongs to a later holder of the code: not undone.
        let after = days[39] + chrono::Days::new(30);
        assert_eq!(volumes(&raw, &[(after, 1.0, 200.0)])[0], 40_000_000);
        // Key Energy: raw before its 2016 1:200 split even for the later 1:50 one, adjusted
        // for the 1:50 between the two. Raw volume 10M, then 50k, then 1k shares a day.
        let (keg, d4) = series(
            start,
            60,
            |i| match i {
                0..20 => 1.0,
                20..40 => 200.0,
                _ => 10_000.0,
            },
            |i| match i {
                0..20 => 1e7,
                20..40 => 5e4 * 0.02,
                _ => 1e3,
            },
        );
        let v = volumes(&keg, &[(d4[20], 1.0, 200.0), (d4[40], 1.0, 50.0)]);
        assert_eq!((v[0], v[20], v[59]), (10_000_000, 50_000, 1_000));
    }

    #[test]
    fn holiday_bars_and_future_events_are_dropped() {
        let body = br#"[{"date":"2026-09-04","open":1,"high":1,"low":1,"close":1,"volume":5},
            {"date":"2026-09-07","open":1,"high":1,"low":1,"close":1,"volume":5}]"#;
        let (b, dropped) = prices(body, &[], &cal(), &origin("2026-10-12T22:00:00Z")).unwrap();
        assert_eq!(b.num_rows(), 1);
        assert_eq!(dropped, BTreeMap::from([("non_session", 1)]), "Labor Day");
        let at = origin("2026-10-12T22:00:00Z");
        let (d, dropped) = dividends(
            br#"[{"date":"2026-09-01","unadjustedValue":0.5},{"date":"2026-12-01","unadjustedValue":0.5}]"#,
            &at,
        )
        .unwrap();
        assert_eq!((d.num_rows(), dropped.get("future_event")), (1, Some(&1)));
        let day = |m, d| NaiveDate::from_ymd_opt(2026, m, d).unwrap();
        let (s, dropped) =
            splits_batch(&[(day(9, 1), 2.0, 1.0), (day(10, 28), 2.0, 1.0)], &at).unwrap();
        assert_eq!((s.num_rows(), dropped.get("future_event")), (1, Some(&1)));
    }

    #[test]
    fn codes_map_to_canonical_tickers() {
        assert_eq!(code_ticker("BRK-B").as_deref(), Some("BRK.B"));
        assert_eq!(code_ticker("MON_old").as_deref(), Some("MON"));
        assert_eq!(code_ticker("AAC_old2").as_deref(), Some("AAC"));
        assert_eq!(code_ticker("AAPL").as_deref(), Some("AAPL"));
        assert_eq!(code_ticker("X_oldish"), None);
    }

    #[test]
    fn symbol_lists_keep_the_d12_exchanges() {
        let body = br#"[{"Code":"AAPL","Exchange":"NASDAQ"},{"Code":"ABCD","Exchange":"PINK"},{"Code":"MON_old","Exchange":"NYSE"},{"Code":"?!","Exchange":"NYSE"}]"#;
        let (ok, bad) = parse_symbols(body, true);
        assert_eq!(
            ok.iter().map(|c| c.code.as_str()).collect::<Vec<_>>(),
            ["AAPL", "MON_old"]
        );
        assert_eq!(bad, ["?!"]);
        let merged = merge_lists(
            vec![Code {
                code: "AAPL".into(),
                ticker: "AAPL".into(),
                delisted: false,
                ..Code::default()
            }],
            ok,
        );
        assert_eq!(merged.len(), 2);
        assert!(!merged.iter().find(|c| c.code == "AAPL").unwrap().delisted);
    }

    #[test]
    fn eastern_time_follows_dst() {
        let at = |s: &str| eastern(s.parse::<DateTime<Utc>>().unwrap()).to_string();
        assert_eq!(at("2026-07-01T20:10:00Z"), "2026-07-01 16:10:00");
        assert_eq!(at("2026-01-02T21:10:00Z"), "2026-01-02 16:10:00");
        assert_eq!(
            at("2026-03-08T06:59:00Z"),
            "2026-03-08 01:59:00",
            "before the switch"
        );
        assert_eq!(
            at("2026-03-08T07:00:00Z"),
            "2026-03-08 03:00:00",
            "after the switch"
        );
        assert_eq!(at("2026-11-01T05:59:00Z"), "2026-11-01 01:59:00");
        assert_eq!(at("2026-11-01T06:00:00Z"), "2026-11-01 01:00:00");
    }

    #[test]
    fn prices_undo_the_volume_adjustment_and_drop_bad_bars() {
        let body = br#"[
            {"date":"2020-08-28","open":504.05,"high":505.77,"low":498.31,"close":499.23,"adjusted_close":122.1,"volume":187629916},
            {"date":"2020-08-31","open":127.58,"high":131.0,"low":126.0,"close":129.04,"adjusted_close":126.2,"volume":225702700},
            {"date":"2020-09-01","open":1,"high":1,"low":2,"close":1,"volume":1},
            {"date":"2020-09-02","open":null,"high":1,"low":1,"close":1,"volume":1},
            {"date":"2020-09-03","open":1,"high":1,"low":1,"close":1,"volume":1}
        ]"#;
        let splits = [(NaiveDate::from_ymd_opt(2020, 8, 31).unwrap(), 4.0, 1.0)];
        // Fetched 2020-09-03 at 15:00 ET: the 09-03 bar is partial.
        let (b, dropped) = prices(body, &splits, &cal(), &origin("2020-09-03T19:00:00Z")).unwrap();
        assert_eq!(b.num_rows(), 2);
        assert_eq!(
            b.column(6).as_primitive::<Float64Type>().values().to_vec(),
            [499.23, 129.04],
            "closes raw"
        );
        assert_eq!(
            b.column(7).as_primitive::<Int64Type>().values().to_vec(),
            [46_907_479, 225_702_700],
            "volume as traded"
        );
        assert_eq!(
            dropped,
            BTreeMap::from([("null_field", 1), ("partial_bar", 1), ("sanity", 1)])
        );
        // A split after the fetch date is not in that response's adjustment.
        let later = [(NaiveDate::from_ymd_opt(2030, 1, 2).unwrap(), 2.0, 1.0)];
        let (b, _) = prices(body, &later, &cal(), &origin("2020-09-04T00:00:00Z")).unwrap();
        assert_eq!(
            b.column(7).as_primitive::<Int64Type>().value(0),
            187_629_916
        );
    }

    #[test]
    fn figi_collisions_keep_the_preferred_code() {
        let d = |y| NaiveDate::from_ymd_opt(y, 1, 1).unwrap();
        let code = |c: &str, delisted| Code {
            code: c.into(),
            ticker: "X".into(),
            delisted,
            ..Code::default()
        };
        let figis = BTreeMap::from([
            ("X".to_owned(), "F1".to_owned()),
            ("X_old".to_owned(), "F1".to_owned()),
            ("Y".to_owned(), "F2".to_owned()),
        ]);
        let spans = [
            (code("X_old", true), (d(2005), d(2015))),
            (code("X", false), (d(2010), d(2026))),
            (code("Y", true), (d(2005), d(2026))),
            (code("Z", true), (d(2005), d(2026))),
        ];
        let out = run::collisions(&spans, &figis);
        assert_eq!(
            out.iter().map(|c| c.code.as_str()).collect::<Vec<_>>(),
            ["X_old"],
            "active X kept; unresolved Z ignored"
        );
        let apart = [
            (code("X_old", true), (d(2005), d(2009))),
            (code("X", false), (d(2010), d(2026))),
        ];
        assert!(
            run::collisions(&apart, &figis).is_empty(),
            "spans that do not overlap are one series"
        );
    }

    #[test]
    fn splits_and_dividends() {
        let s = parse_splits(
            br#"[{"date":"2021-08-02","split":"1.000000/8.000000"},{"date":"x","split":"2/1"}]"#,
        );
        assert_eq!(
            s,
            [(NaiveDate::from_ymd_opt(2021, 8, 2).unwrap(), 1.0, 8.0)]
        );
        assert_eq!(
            splits_batch(&s, &origin("2026-01-01T00:00:00Z"))
                .unwrap()
                .0
                .column(5)
                .as_primitive::<Float64Type>()
                .value(0),
            0.125
        );
        let (d, dropped) = dividends(
            br#"[{"date":"2020-08-07","value":0.205,"unadjustedValue":0.82,"currency":"USD"},
                 {"date":"2020-08-07","unadjustedValue":0.5},{"date":"2020-11-06","value":0.205}]"#,
            &origin("2026-01-01T00:00:00Z"),
        )
        .unwrap();
        assert_eq!(
            d.column(3).as_primitive::<Float64Type>().values().to_vec(),
            [1.32]
        );
        assert_eq!(dropped.get("null_field"), Some(&1));
    }
}

/// The orchestration of a W5 backfill run (`BACKFILL.md` §5.1).
pub mod run {
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::sync::Arc;

    use chrono::NaiveDate;

    use super::{Code, Origin, PARSE_VERSION};
    use crate::infra::fetch::{Fatal, Fetcher, Outcome, Request};
    use crate::infra::landing;
    use crate::infra::manifest::{Key, Manifest};
    use crate::infra::quality::{Level, QualityLog, Row};
    use crate::infra::stage::{self, Compacted, StageError};

    /// A landed body: bytes, SHA-256, fetch time.
    type Body = (Vec<u8>, String, chrono::DateTime<chrono::Utc>);
    /// First and last bar of a code.
    pub type Span = (NaiveDate, NaiveDate);

    /// The EODHD API base URL.
    pub const BASE: &str = "https://eodhd.com/api";

    /// Reference codes fetched on every run (calendar and gate checks; `BACKFILL.md` §5.1).
    /// `(code, ISIN)`: J4a resolves them by ISIN whatever their type (they are ETFs, outside
    /// the stock scope, but `prices` needs them).
    pub const REFERENCE: &[(&str, &str)] = &[
        ("SPY", "US78462F1030"),
        ("IWF", "US4642876142"),
        ("IWP", "US4642874816"),
    ];

    /// A run failure.
    #[derive(Debug, thiserror::Error)]
    pub enum RunError {
        /// The source must stop (cap, refusal, manifest, landing).
        #[error(transparent)]
        Fatal(#[from] Fatal),
        /// Parse, stage or compaction failure.
        #[error(transparent)]
        Stage(#[from] StageError),
        /// Manifest error outside a fetch.
        #[error("manifest: {0}")]
        Manifest(#[from] rusqlite::Error),
        /// Reading a landed body.
        #[error("{0}")]
        Io(#[from] std::io::Error),
        /// Arrow error building rows (a bug).
        #[error("arrow: {0}")]
        Arrow(#[from] arrow::error::ArrowError),
    }

    /// Everything a run needs.
    pub struct Ctx {
        /// The EODHD fetcher.
        pub fetcher: Fetcher,
        /// The manifest.
        pub manifest: Arc<Manifest>,
        /// `$QUANT_ROOT/stage`.
        pub stage: std::path::PathBuf,
        /// `EODHD_API_KEY`.
        pub key: String,
        /// `BACKFILL_START`.
        pub from: NaiveDate,
        /// The run's quality log.
        pub quality: QualityLog,
        /// API base URL (a local server in tests).
        pub base: String,
        /// EODHD code (without `.US`) -> US composite FIGI, from `security_spells` (J4a).
        pub figis: BTreeMap<String, String>,
        /// The NYSE calendar (holiday bars are dropped at parse).
        pub cal: calendar::Calendar,
    }

    impl Ctx {
        fn url(&self, path: &str, query: &str) -> String {
            format!(
                "{}/{path}?api_token={}&fmt=json{query}",
                self.base, self.key
            )
        }
    }

    /// Fetch today's symbol lists and return the in-scope codes.
    ///
    /// # Errors
    ///
    /// [`RunError`] if a list cannot be fetched.
    pub async fn symbols(ctx: &Ctx, today: NaiveDate) -> Result<Vec<Code>, RunError> {
        let mut lists = Vec::new();
        for (name, query, delisted) in [
            ("active", "&type=common_stock", false),
            ("delisted", "&type=common_stock&delisted=1", true),
        ] {
            let chunk = format!("symbols-{name}-{today}");
            let url = ctx.url("exchange-symbol-list/US", query);
            let r = Request {
                entity: "US",
                chunk: &chunk,
                url: &url,
                ext: "json",
                cost: 1,
                dataset: "listings",
                empty_is_gone: false,
            };
            ctx.fetcher.fetch(&r).await?;
            let body = read_landed(&ctx.manifest, (ctx.fetcher.spec.id, "US", &chunk))?
                .map(|l| l.0)
                .unwrap_or_default();
            let (codes, bad) = super::parse_symbols(&body, delisted);
            if !bad.is_empty() {
                ctx.quality.push(Row {
                    source: "EODHD",
                    dataset: "prices".into(),
                    entity: "US".into(),
                    chunk,
                    level: Level::Warn,
                    attempts: 1,
                    notes: Some(format!(
                        "{} codes do not normalize, skipped: {}",
                        bad.len(),
                        bad.iter().take(20).cloned().collect::<Vec<_>>().join(" ")
                    )),
                });
            }
            lists.push(codes);
        }
        let delisted = lists.pop().unwrap_or_default();
        let active = lists.pop().unwrap_or_default();
        Ok(super::merge_lists(active, delisted))
    }

    /// `(body, sha256, fetched_at)` of a landed chunk.
    fn read_landed(m: &Manifest, key: Key<'_>) -> Result<Option<Body>, RunError> {
        match m.landed(key)? {
            Some((path, sha, at)) => Ok(Some((landing::read(Path::new(&path))?, sha, at))),
            None => Ok(None),
        }
    }

    /// What one code produced.
    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    pub struct CodeResult {
        /// Price rows staged.
        pub bars: usize,
        /// First and last bar.
        pub span: Option<Span>,
        /// The eod chunk was gone (no data) or failed.
        pub no_prices: bool,
        /// The code has a FIGI (`security_spells`), so its rows are staged.
        pub resolved: bool,
    }

    /// Fetch, land and (for a resolved code) stage one code. Prices come first: a code with no
    /// bars since `BACKFILL_START` gets no splits or dividends request. An unresolved code (no
    /// FIGI in `figis`) is fetched and landed but not staged, so it never reaches the lake;
    /// its span is still reported (the gap that matters is unresolved codes with prices).
    ///
    /// # Errors
    ///
    /// [`RunError`]; an ordinary fetch failure is not an error (the chunk is retried next run).
    pub async fn code(ctx: &Ctx, c: &Code) -> Result<CodeResult, RunError> {
        let entity = format!("{}.US", c.code);
        let from = format!("&from={}", ctx.from);
        let id = ctx.fetcher.spec.id;
        let key = |chunk: &'static str| (id, entity.as_str(), chunk);
        let fetch = |chunk: &'static str, path: &'static str, dataset: &'static str| {
            let url = ctx.url(&format!("{path}/{entity}"), &from);
            let entity = entity.clone();
            async move {
                let r = Request {
                    entity: &entity,
                    chunk,
                    url: &url,
                    ext: "json",
                    cost: 1,
                    dataset,
                    empty_is_gone: chunk == "eod-full",
                };
                ctx.fetcher.fetch(&r).await
            }
        };
        let figi = ctx.figis.get(&c.code).cloned();
        if figi.is_none() {
            // A code that lost its FIGI (J4a re-run) must not stay in compaction.
            for chunk in ["eod-full", "splits-full", "div-full"] {
                ctx.manifest.unstage(key(chunk))?;
            }
        }
        let mut result = CodeResult {
            resolved: figi.is_some(),
            ..CodeResult::default()
        };
        if matches!(
            fetch("eod-full", "eod", "prices").await?,
            Outcome::Failed(_) | Outcome::Gone(_)
        ) {
            result.no_prices = true;
            return Ok(result);
        }
        fetch("splits-full", "splits", "splits").await?;
        fetch("div-full", "div", "dividends").await?;
        let origin = |sha: String, at| Origin {
            figi: figi.clone().unwrap_or_default(),
            ticker: c.ticker.clone(),
            fetched_at: at,
            raw_sha256: sha,
        };
        // Prices need the splits (volume normalization): parse only when both are settled.
        let splits = match read_landed(&ctx.manifest, key("splits-full"))? {
            Some((body, sha, at)) => Some((super::parse_splits(&body), Some((sha, at)))),
            None if ctx.manifest.status(key("splits-full"))?
                == Some(crate::infra::manifest::Status::Gone) =>
            {
                Some((Vec::new(), None))
            }
            None => None,
        };
        let Some((splits, splits_origin)) = splits else {
            return Ok(result);
        };
        let Some((body, sha, at)) = read_landed(&ctx.manifest, key("eod-full"))? else {
            return Ok(result);
        };
        let (batch, dropped) = super::prices(&body, &splits, &ctx.cal, &origin(sha, at))?;
        result.bars = batch.num_rows();
        result.span = span(&batch);
        if figi.is_none() {
            return Ok(result);
        }
        stage::write_stage(
            &ctx.stage,
            &ctx.manifest,
            "prices",
            key("eod-full"),
            PARSE_VERSION,
            &batch,
        )?;
        note_dropped(
            ctx,
            &entity,
            "eod-full",
            "prices",
            &dropped,
            batch.num_rows(),
        );
        if let Some((sha, at)) = splits_origin {
            let (batch, dropped) = super::splits_batch(&splits, &origin(sha, at))?;
            stage::write_stage(
                &ctx.stage,
                &ctx.manifest,
                "splits",
                key("splits-full"),
                PARSE_VERSION,
                &batch,
            )?;
            note_dropped(
                ctx,
                &entity,
                "splits-full",
                "splits",
                &dropped,
                batch.num_rows(),
            );
        }
        if let Some((body, sha, at)) = read_landed(&ctx.manifest, key("div-full"))? {
            let (batch, dropped) = super::dividends(&body, &origin(sha, at))?;
            stage::write_stage(
                &ctx.stage,
                &ctx.manifest,
                "dividends",
                key("div-full"),
                PARSE_VERSION,
                &batch,
            )?;
            note_dropped(
                ctx,
                &entity,
                "div-full",
                "dividends",
                &dropped,
                batch.num_rows(),
            );
        }
        Ok(result)
    }

    fn span(b: &arrow::array::RecordBatch) -> Option<Span> {
        use arrow::array::AsArray;
        let d = b
            .column_by_name("date")?
            .as_primitive::<arrow::datatypes::Date32Type>();
        let day =
            |x: i32| NaiveDate::default().checked_add_signed(chrono::Duration::days(i64::from(x)));
        Some((
            day(*d.values().iter().min()?)?,
            day(*d.values().iter().max()?)?,
        ))
    }

    fn note_dropped(
        ctx: &Ctx,
        entity: &str,
        chunk: &str,
        dataset: &str,
        dropped: &super::Dropped,
        kept: usize,
    ) {
        let n: usize = dropped.values().sum();
        if n == 0 {
            return;
        }
        // More than 1% of a series dropped is a WARN (SOURCES.md); fewer are an OK note.
        let level = if n * 100 > (n + kept) {
            Level::Warn
        } else {
            Level::Ok
        };
        ctx.quality.push(Row {
            source: "EODHD",
            dataset: dataset.into(),
            entity: entity.into(),
            chunk: format!("{chunk}/parse"),
            level,
            attempts: 1,
            notes: Some(format!("dropped {dropped:?} of {}", n + kept)),
        });
    }

    /// Codes to leave out because another code of the same FIGI covers overlapping dates (two
    /// vendor codes of one security, `SOURCES.md` EODHD): per FIGI the preferred code is kept
    /// (active before delisted, a plain code before `_old`, then by name) and every later code
    /// that overlaps a kept one is returned. Unresolved codes are not considered.
    #[must_use]
    pub fn collisions(spans: &[(Code, Span)], figis: &BTreeMap<String, String>) -> Vec<Code> {
        let mut by_figi: BTreeMap<&str, Vec<&(Code, Span)>> = BTreeMap::new();
        for s in spans {
            if let Some(f) = figis.get(&s.0.code) {
                by_figi.entry(f.as_str()).or_default().push(s);
            }
        }
        let mut out = Vec::new();
        for group in by_figi.values_mut() {
            group.sort_by_key(|(c, _)| (c.delisted, c.code.contains("_old"), c.code.clone()));
            let mut kept: Vec<Span> = Vec::new();
            for (c, s) in group.iter() {
                if kept.iter().any(|k| s.0 <= k.1 && k.0 <= s.1) {
                    out.push(c.clone());
                } else {
                    kept.push(*s);
                }
            }
        }
        out
    }

    /// Mark a colliding code's chunks gone so compaction leaves them out.
    ///
    /// # Errors
    ///
    /// Manifest errors.
    pub fn exclude(ctx: &Ctx, c: &Code) -> Result<(), RunError> {
        let entity = format!("{}.US", c.code);
        for chunk in ["eod-full", "splits-full", "div-full"] {
            ctx.manifest
                .gone((ctx.fetcher.spec.id, &entity, chunk), 200, "figi_collision")?;
        }
        ctx.quality.push(Row {
            source: "EODHD",
            dataset: "prices".into(),
            entity,
            chunk: "eod-full/collision".into(),
            level: Level::Fail,
            attempts: 1,
            notes: Some(format!(
                "shares its FIGI with a preferred code over overlapping dates; excluded (ticker {})",
                c.ticker
            )),
        });
        Ok(())
    }

    /// Compact the three datasets.
    ///
    /// # Errors
    ///
    /// [`StageError`] (validation included).
    pub fn compact(
        ctx: &Ctx,
        lake: &lake::Lake,
    ) -> Result<Vec<(&'static str, Compacted)>, RunError> {
        let mut out = Vec::new();
        for dataset in ["prices", "splits", "dividends"] {
            out.push((
                dataset,
                stage::compact(
                    lake,
                    &ctx.manifest,
                    &ctx.stage,
                    ctx.fetcher.spec.id,
                    dataset,
                )?,
            ));
        }
        ctx.manifest.compacted(ctx.fetcher.spec.id)?;
        Ok(out)
    }
}
