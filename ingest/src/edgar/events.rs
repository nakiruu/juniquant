//! J6: `filing_events` rows from a filer's `submissions` filings (`SOURCES.md` "EDGAR events
//! and delistings"), and the `delistings` rule table D1-D5 over them (`SURVIVORSHIP.md` §3).

use std::sync::Arc;

use arrow::array::{ArrayRef, Date32Array, RecordBatch, StringArray, TimestampNanosecondArray};
use arrow::datatypes::{DataType as T, Field, Schema, TimeUnit};
use chrono::{DateTime, Days, NaiveDate, Utc};

use super::Security;
use super::facts::Filing;

/// Bumped when the parser's output changes.
pub const PARSE_VERSION: &str = "1";

/// The event forms (`SCHEMAS.md` #33 `form_type`).
pub const FORMS: [&str; 9] = [
    "8-K", "8-K/A", "25", "25-NSE", "15-12B", "15-12G", "15-15D", "NT 10-K", "NT 10-Q",
];

/// One `filing_events` row of a filer, before it is copied to each of its securities.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// Accession.
    pub accession: String,
    /// 8-K item (`2.02`), `-` for the other forms and for an 8-K without items.
    pub item: String,
    /// Form type.
    pub form: String,
    /// The point-in-time stamp.
    pub filing_date: NaiveDate,
    /// `acceptanceDateTime`.
    pub acceptance: Option<DateTime<Utc>>,
    /// `SCHEMAS.md` #33 `event_type`.
    pub event_type: &'static str,
}

/// `event_type` of a form and 8-K item. Pre-2004 8-K item numbers (`5`, no dot) are `other`.
#[must_use]
pub fn event_type(form: &str, item: &str) -> &'static str {
    match (form, item) {
        ("25" | "25-NSE", _) => "delisting_filing",
        ("15-12B" | "15-12G" | "15-15D", _) => "deregistration",
        ("NT 10-K" | "NT 10-Q", _) => "late_filing_notice",
        (_, "2.02") => "earnings_release",
        (_, "3.01") => "delisting_notice",
        (_, "1.03") => "bankruptcy",
        (_, "2.01") => "acquisition_completed",
        (_, "5.01") => "change_in_control",
        (_, "4.01") => "auditor_change",
        (_, "4.02") => "non_reliance_restatement",
        (_, "2.04") => "obligation_acceleration",
        _ => "other",
    }
}

/// The event rows of a filer's filings (any order, duplicates by accession allowed), one per
/// (filing, item); filings after `today` are dropped and counted.
#[must_use]
pub fn events(filings: &[Filing], today: NaiveDate) -> (Vec<Event>, usize) {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    let mut future = 0;
    for f in filings.iter().filter(|f| FORMS.contains(&f.form.as_str())) {
        if f.filing_date > today {
            future += 1;
            continue;
        }
        let items: Vec<&str> = if f.form.starts_with("8-K") && !f.items.is_empty() {
            f.items.iter().map(String::as_str).collect()
        } else {
            vec!["-"]
        };
        for item in items {
            if seen.insert((f.accession.clone(), item.to_owned())) {
                out.push(Event {
                    accession: f.accession.clone(),
                    item: item.to_owned(),
                    form: f.form.clone(),
                    filing_date: f.filing_date,
                    acceptance: f.acceptance,
                    event_type: event_type(&f.form, item),
                });
            }
        }
    }
    out.sort_by(|a, b| (&a.accession, &a.item).cmp(&(&b.accession, &b.item)));
    (out, future)
}

fn day(d: NaiveDate) -> i32 {
    i32::try_from((d - NaiveDate::default()).num_days()).unwrap_or_default()
}

fn utf8(v: Vec<Option<String>>) -> ArrayRef {
    Arc::new(StringArray::from(v))
}

fn dates(v: Vec<Option<NaiveDate>>) -> ArrayRef {
    Arc::new(Date32Array::from(
        v.into_iter().map(|d| d.map(day)).collect::<Vec<_>>(),
    ))
}

fn ts() -> T {
    T::Timestamp(TimeUnit::Nanosecond, Some("UTC".into()))
}

fn record(
    fields: &[(&str, T)],
    cols: Vec<ArrayRef>,
) -> Result<RecordBatch, arrow::error::ArrowError> {
    let schema = Schema::new(
        fields
            .iter()
            .map(|(n, t)| Field::new(*n, t.clone(), true))
            .collect::<Vec<_>>(),
    );
    RecordBatch::try_new(Arc::new(schema), cols)
}

/// The `filing_events` rows of one filer, one copy per security (`SCHEMAS.md` #33 order).
///
/// # Errors
///
/// Arrow errors (a bug).
pub fn batch(rows: &[(&str, &Security, &Event)]) -> Result<RecordBatch, arrow::error::ArrowError> {
    let s = |f: &dyn Fn(&(&str, &Security, &Event)) -> String| {
        utf8(rows.iter().map(|r| Some(f(r))).collect())
    };
    record(
        &[
            ("accession", T::Utf8),
            ("item", T::Utf8),
            ("ticker", T::Utf8),
            ("cik", T::Utf8),
            ("form_type", T::Utf8),
            ("filing_date", T::Date32),
            ("acceptance_datetime", ts()),
            ("event_type", T::Utf8),
            ("figi", T::Utf8),
        ],
        vec![
            s(&|r| r.2.accession.clone()),
            s(&|r| r.2.item.clone()),
            s(&|r| r.1.ticker.clone()),
            s(&|r| r.0.to_owned()),
            s(&|r| r.2.form.clone()),
            dates(rows.iter().map(|r| Some(r.2.filing_date)).collect()),
            Arc::new(
                TimestampNanosecondArray::from(
                    rows.iter()
                        .map(|r| r.2.acceptance.and_then(|t| t.timestamp_nanos_opt()))
                        .collect::<Vec<_>>(),
                )
                .with_timezone("UTC"),
            ),
            s(&|r| r.2.event_type.to_owned()),
            s(&|r| r.1.figi.clone()),
        ],
    )
}

/// A `delistings` row before it is given its security.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delisting {
    /// `bankruptcy` / `acquired` / `delisted_other` / `deregistered`.
    pub class: &'static str,
    /// `high` / `medium` / `low`.
    pub confidence: &'static str,
    /// `D1`-`D5`.
    pub rule: &'static str,
    /// Latest Form 25 / 25-NSE.
    pub form25: Option<NaiveDate>,
    /// The Form 25 date; for a bankruptcy or without a Form 25, the evidence date (D22).
    pub est_last: Option<NaiveDate>,
    /// Latest Form 15.
    pub form15: Option<NaiveDate>,
    /// The Item 1.03 used.
    pub bankruptcy: Option<NaiveDate>,
    /// The Item 2.01 / 5.01 used.
    pub acquisition: Option<NaiveDate>,
    /// Accessions used.
    pub evidence: Vec<String>,
}

fn plus(d: NaiveDate, n: u64) -> NaiveDate {
    d.checked_add_days(Days::new(n)).unwrap_or(d)
}

fn minus(d: NaiveDate, n: u64) -> NaiveDate {
    d.checked_sub_days(Days::new(n)).unwrap_or(d)
}

/// `SURVIVORSHIP.md` §3: the first of D1-D5 that matches a filer's events, given the last
/// price date of the security (D1 without a Form 25 needs it). All dates are `filing_date`;
/// the result may use filings after any `t` (labels only, `POINT-IN-TIME.md` "EDGAR Events").
#[must_use]
pub fn classify(events: &[Event], last_price: Option<NaiveDate>) -> Option<Delisting> {
    // The earliest event of `types` in [from, to].
    let first = |types: &[&str], from: NaiveDate, to: NaiveDate| {
        events
            .iter()
            .filter(|e| types.contains(&e.event_type) && (from..=to).contains(&e.filing_date))
            .min_by_key(|e| e.filing_date)
    };
    let latest = |t: &str| {
        events
            .iter()
            .filter(|e| e.event_type == t)
            .max_by_key(|e| e.filing_date)
    };
    let (f25, f15) = (latest("delisting_filing"), latest("deregistration"));
    let acq: &[&str] = &["acquisition_completed", "change_in_control"];
    let row = |class, confidence, rule, est_last, bk: Option<&Event>, aq: Option<&Event>| {
        let mut evidence: Vec<String> = [f25, f15, bk, aq]
            .into_iter()
            .flatten()
            .map(|e| e.accession.clone())
            .collect();
        evidence.dedup();
        Delisting {
            class,
            confidence,
            rule,
            form25: f25.map(|e| e.filing_date),
            est_last,
            form15: f15.map(|e| e.filing_date),
            bankruptcy: bk.map(|e| e.filing_date),
            acquisition: aq.map(|e| e.filing_date),
            evidence,
        }
    };
    if let Some(e25) = f25.map(|e| e.filing_date) {
        // D22: trading ends on the Form 25 filing day (the 10 days to effectiveness are legal only).
        let est = Some(e25);
        if let Some(b) = first(&["bankruptcy"], minus(e25, 365), plus(e25, 30)) {
            // D22: a bankrupt stock stops trading near the 8-K, often long before the Form 25.
            return Some(row(
                "bankruptcy",
                "high",
                "D1",
                Some(b.filing_date),
                Some(b),
                None,
            ));
        }
        if let Some(a) = first(acq, minus(e25, 45), plus(e25, 10)) {
            return Some(row("acquired", "high", "D2", est, None, Some(a)));
        }
        if first(&["delisting_notice"], minus(e25, 45), e25).is_some()
            && let Some(a) = first(acq, minus(e25, 90), plus(e25, 90))
        {
            return Some(row("acquired", "medium", "D3", est, None, Some(a)));
        }
        return Some(row("delisted_other", "medium", "D4", est, None, None));
    }
    if let Some(b) = last_price.and_then(|lp| first(&["bankruptcy"], minus(lp, 365), lp)) {
        let est = Some(b.filing_date);
        return Some(row("bankruptcy", "medium", "D1", est, Some(b), None));
    }
    f15.map(|e| row("deregistered", "low", "D5", Some(e.filing_date), None, None))
}

/// D0 (`SURVIVORSHIP.md` §3): the security's prices outlive the Form 25 (or 15) `anchor` at
/// the same level of trade, so the form delisted another security of the filer (a note, a
/// preferred, a tracking stock) or moved exchange: more than 10 bars after the anchor, a
/// median dollar volume over bars 11-70 after it at least half that of the 60 bars to it, and
/// no ten-fold jump (`jumps`, later-bar indices) after it. `bars` are one security's, sorted.
#[must_use]
pub fn outlived(bars: &[crate::gates::Bar], anchor: NaiveDate, jumps: &[usize]) -> bool {
    let k = bars.partition_point(|b| b.date <= anchor);
    if k == 0 || k + 10 >= bars.len() || jumps.iter().any(|&j| j >= k) {
        return false;
    }
    let median = |w: Option<&[crate::gates::Bar]>| {
        let mut v: Vec<f64> = w
            .unwrap_or_default()
            .iter()
            .map(|b| b.close * b.volume.max(0) as f64)
            .filter(|x| x.is_finite())
            .collect();
        v.sort_by(f64::total_cmp);
        v.get(v.len() / 2).copied().unwrap_or(0.0)
    };
    let pre = median(bars.get(k.saturating_sub(60)..k));
    let post = median(bars.get(k + 10..(k + 70).min(bars.len())));
    pre > 0.0 && post >= 0.5 * pre
}

/// The `delistings` rows (`SCHEMAS.md` #32 order, `figi` last).
///
/// # Errors
///
/// Arrow errors (a bug).
pub fn delisting_batch(
    rows: &[(&str, &Security, Delisting)],
    built_at: DateTime<Utc>,
) -> Result<RecordBatch, arrow::error::ArrowError> {
    type R<'a> = (&'a str, &'a Security, Delisting);
    let s = |f: &dyn Fn(&R<'_>) -> String| utf8(rows.iter().map(|r| Some(f(r))).collect());
    let d =
        |f: &dyn Fn(&Delisting) -> Option<NaiveDate>| dates(rows.iter().map(|r| f(&r.2)).collect());
    record(
        &[
            ("ticker", T::Utf8),
            ("cik", T::Utf8),
            ("delist_class", T::Utf8),
            ("confidence", T::Utf8),
            ("rule_id", T::Utf8),
            ("form25_filing_date", T::Date32),
            ("est_last_trading_date", T::Date32),
            ("form15_filing_date", T::Date32),
            ("bankruptcy_filing_date", T::Date32),
            ("acquisition_filing_date", T::Date32),
            ("evidence_accessions", T::Utf8),
            ("built_at", ts()),
            ("figi", T::Utf8),
        ],
        vec![
            s(&|r| r.1.ticker.clone()),
            s(&|r| r.0.to_owned()),
            s(&|r| r.2.class.to_owned()),
            s(&|r| r.2.confidence.to_owned()),
            s(&|r| r.2.rule.to_owned()),
            d(&|x| x.form25),
            d(&|x| x.est_last),
            d(&|x| x.form15),
            d(&|x| x.bankruptcy),
            d(&|x| x.acquisition),
            s(&|r| serde_json::to_string(&r.2.evidence).unwrap_or_default()),
            Arc::new(
                TimestampNanosecondArray::from(vec![built_at.timestamp_nanos_opt(); rows.len()])
                    .with_timezone("UTC"),
            ),
            s(&|r| r.1.figi.clone()),
        ],
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing, reason = "test")]

    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn filing(acc: &str, form: &str, date: &str, items: &[&str]) -> Filing {
        Filing {
            accession: acc.into(),
            filing_date: d(date),
            report_date: None,
            form: form.into(),
            acceptance: None,
            items: items.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    #[test]
    fn one_row_per_item_and_event_forms_only() {
        let fs = [
            filing("a", "8-K", "2024-02-01", &["2.02", "9.01"]),
            filing("a", "8-K", "2024-02-01", &["2.02", "9.01"]),
            filing("b", "10-Q", "2024-05-01", &[]),
            filing("c", "25-NSE", "2024-06-01", &[]),
            filing("d", "8-K", "2003-01-01", &[]),
            filing("e", "8-K", "2030-01-01", &["1.03"]),
        ];
        let (ev, future) = events(&fs, d("2026-10-12"));
        let got: Vec<_> = ev
            .iter()
            .map(|e| (e.accession.as_str(), e.item.as_str(), e.event_type))
            .collect();
        assert_eq!(
            got,
            vec![
                ("a", "2.02", "earnings_release"),
                ("a", "9.01", "other"),
                ("c", "-", "delisting_filing"),
                ("d", "-", "other"),
            ]
        );
        assert_eq!(future, 1);
    }

    #[test]
    fn rules_fire_in_order() {
        let ev = |fs: &[Filing]| events(fs, d("2026-10-12")).0;
        let f25 = filing("f25", "25-NSE", "2023-06-01", &[]);
        let bk = filing("bk", "8-K", "2023-01-10", &["1.03"]);
        let acq = filing("acq", "8-K", "2023-05-25", &["2.01"]);
        let notice = filing("n", "8-K", "2023-05-01", &["3.01"]);
        let late_acq = filing("la", "8-K", "2023-08-01", &["5.01"]);
        let f15 = filing("f15", "15-12B", "2023-06-20", &[]);

        let r = classify(&ev(&[f25.clone(), bk.clone(), acq.clone()]), None).unwrap();
        assert_eq!(
            (r.class, r.rule, r.confidence),
            ("bankruptcy", "D1", "high")
        );
        assert_eq!(r.est_last, Some(d("2023-01-10")));
        assert_eq!(r.evidence, vec!["f25".to_owned(), "bk".to_owned()]);

        let r = classify(&ev(&[f25.clone(), acq, f15.clone()]), None).unwrap();
        assert_eq!((r.rule, r.form15), ("D2", Some(d("2023-06-20"))));

        let r = classify(&ev(&[f25.clone(), notice, late_acq.clone()]), None).unwrap();
        assert_eq!((r.class, r.rule), ("acquired", "D3"));
        assert_eq!(r.acquisition, Some(d("2023-08-01")));

        let r = classify(&ev(&[f25, late_acq]), None).unwrap();
        assert_eq!((r.class, r.rule), ("delisted_other", "D4"));
        assert_eq!(r.est_last, Some(d("2023-06-01")));

        // No Form 25: a bankruptcy in the year before the last price, else a Form 15.
        let r = classify(&ev(&[bk.clone(), f15.clone()]), Some(d("2023-09-01"))).unwrap();
        assert_eq!(
            (r.rule, r.confidence, r.est_last),
            ("D1", "medium", Some(d("2023-01-10")))
        );
        let r = classify(&ev(&[bk.clone(), f15]), Some(d("2025-01-01"))).unwrap();
        assert_eq!((r.class, r.rule), ("deregistered", "D5"));
        assert_eq!(classify(&ev(&[bk]), None), None);
    }

    #[test]
    fn d0_prices_that_outlive_the_form() {
        // 100 bars from 2024-01-01; the Form 25 on bar 49.
        let series = |vol_after: i64| -> Vec<crate::gates::Bar> {
            (0..100u64)
                .map(|k| crate::gates::Bar {
                    sec: 0,
                    date: d("2024-01-01") + Days::new(k),
                    open: 10.0,
                    high: 10.0,
                    low: 10.0,
                    close: 10.0,
                    volume: if k < 50 { 1000 } else { vol_after },
                    adj: 10.0,
                })
                .collect()
        };
        let anchor = d("2024-02-19");
        assert!(
            outlived(&series(900), anchor, &[]),
            "same trade: another security's form"
        );
        assert!(
            !outlived(&series(100), anchor, &[]),
            "trade collapses: delisted"
        );
        assert!(
            !outlived(&series(900), anchor, &[70]),
            "a ten-fold jump after: another holder"
        );
        assert!(
            !outlived(&series(900)[..55], anchor, &[]),
            "ends within 10 bars"
        );
        assert!(
            !outlived(&series(900), d("2023-12-01"), &[]),
            "no bar on or before the form"
        );
    }
}
