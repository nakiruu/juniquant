//! J2 and J7: the EDGAR bulk zips and `fundamentals` (`BACKFILL.md` §5.2, `SOURCES.md` "SEC
//! EDGAR" and "Building `fundamentals` from `companyfacts`"). The zips are landed as
//! downloaded; parsing reads only the CIKs of `security_master` from them, and is a pure
//! function of the two zips plus that CIK list.

use std::collections::BTreeMap;
use std::path::Path;

pub mod facts;

use chrono::{Datelike, NaiveDate};

use crate::infra::fetch::{Fatal, Fetcher, Outcome, Request};

/// The bulk submissions zip (all filers).
pub const SUBMISSIONS_ZIP: &str =
    "https://www.sec.gov/Archives/edgar/daily-index/bulkdata/submissions.zip";
/// The bulk XBRL facts zip (all filers).
pub const FACTS_ZIP: &str = "https://www.sec.gov/Archives/edgar/daily-index/xbrl/companyfacts.zip";

/// `(entity, url)` of the two zips, in the manifest under source `sec_bulk`.
pub const ZIPS: [(&str, &str); 2] = [
    ("submissions_zip", SUBMISSIONS_ZIP),
    ("companyfacts_zip", FACTS_ZIP),
];

/// Quarters `(year, q)` from `first` through the last completed quarter before `today`.
#[must_use]
pub fn quarters(first: (i32, u32), today: NaiveDate) -> Vec<(i32, u32)> {
    let current = (today.year(), (today.month() - 1) / 3 + 1);
    let mut out = Vec::new();
    let (mut y, mut q) = first;
    while (y, q) < current {
        out.push((y, q));
        (y, q) = if q == 4 { (y + 1, 1) } else { (y, q + 1) };
    }
    out
}

/// J2: lands the two zips (chunk = the fetch date; `Last-Modified` is in the sidecar) and the
/// `form.idx` of every completed quarter from `first` (cross-check input for gate G7).
///
/// shortcut: a zip is fetched in one GET held in memory (about 1.5 GB each), not resumed with
/// `Range`; upgrade if the download fails on this link.
///
/// # Errors
///
/// [`Fatal`] from the fetcher (manifest, landing, refused by SEC).
pub async fn fetch(
    sec: &Fetcher,
    today: NaiveDate,
    first: (i32, u32),
) -> Result<BTreeMap<String, Outcome>, Fatal> {
    let mut out = BTreeMap::new();
    let day = today.to_string();
    for (entity, url) in ZIPS {
        let r = Request {
            entity,
            chunk: &day,
            url,
            ext: "zip",
            cost: 1,
            dataset: "fundamentals",
            empty_is_gone: false,
        };
        out.insert(entity.to_owned(), sec.fetch(&r).await?);
    }
    for (y, q) in quarters(first, today) {
        let (chunk, url) = (
            format!("{y}q{q}"),
            format!("https://www.sec.gov/Archives/edgar/full-index/{y}/QTR{q}/form.idx"),
        );
        let r = Request {
            entity: "form_idx",
            chunk: &chunk,
            url: &url,
            ext: "txt",
            cost: 1,
            dataset: "filing_events",
            empty_is_gone: false,
        };
        out.insert(format!("form_idx/{chunk}"), sec.fetch(&r).await?);
    }
    Ok(out)
}

/// Reads one entry of a landed zip, `None` when the zip has no such entry.
///
/// # Errors
///
/// I/O and zip format errors.
pub fn entry(
    zip: &mut zip::ZipArchive<std::fs::File>,
    name: &str,
) -> Result<Option<Vec<u8>>, zip::result::ZipError> {
    use std::io::Read;
    let mut f = match zip.by_name(name) {
        Ok(f) => f,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut buf = Vec::with_capacity(usize::try_from(f.size()).unwrap_or_default());
    f.read_to_end(&mut buf)?;
    Ok(Some(buf))
}

/// Opens a landed zip.
///
/// # Errors
///
/// I/O and zip format errors.
pub fn open(path: &Path) -> Result<zip::ZipArchive<std::fs::File>, zip::result::ZipError> {
    zip::ZipArchive::new(std::fs::File::open(path)?)
}

/// One output row before batching: the CIK, the security, the filer row.
type Out<'a> = (&'a str, &'a Security, facts::Row);

/// A listed security of a filer (`security_master`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Security {
    /// Composite FIGI.
    pub figi: String,
    /// Canonical ticker.
    pub ticker: String,
}

/// What [`build`] produced.
#[derive(Debug)]
pub struct Built {
    /// The `fundamentals` rows, one per (security, accession).
    pub batch: arrow::array::RecordBatch,
    /// CIKs with at least one row.
    pub with_rows: usize,
    /// CIKs absent from the submissions zip.
    pub no_submissions: Vec<String>,
    /// CIKs with periodic filings since 2011-07-01 (XBRL phased in) but no row.
    pub no_rows: Vec<String>,
    /// Periodic filings without a row, summed over filers.
    pub dropped: facts::Dropped,
}

/// Errors of [`build`].
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    /// Zip I/O or format.
    #[error("zip: {0}")]
    Zip(#[from] zip::result::ZipError),
    /// A JSON entry that does not parse.
    #[error("{0}: {1}")]
    Json(String, serde_json::Error),
    /// Arrow (a bug).
    #[error("arrow: {0}")]
    Arrow(#[from] arrow::error::ArrowError),
}

fn json(
    zip: &mut zip::ZipArchive<std::fs::File>,
    name: &str,
) -> Result<Option<serde_json::Value>, BuildError> {
    entry(zip, name)?
        .map(|b| serde_json::from_slice(&b).map_err(|e| BuildError::Json(name.to_owned(), e)))
        .transpose()
}

/// J7: the `fundamentals` rows of every CIK in `by_cik` (10-digit CIK -> its securities), from
/// the two landed zips. Pure in the zips, the map and `today`.
///
/// # Errors
///
/// [`BuildError`]: a zip that cannot be read, an entry that is not JSON.
pub fn build(
    subs: &mut zip::ZipArchive<std::fs::File>,
    facts_zip: &mut zip::ZipArchive<std::fs::File>,
    by_cik: &BTreeMap<String, Vec<Security>>,
    today: NaiveDate,
) -> Result<Built, BuildError> {
    let xbrl = NaiveDate::from_ymd_opt(2011, 7, 1).unwrap_or_default();
    let mut out: Vec<Out<'_>> = Vec::new();
    let (mut no_submissions, mut no_rows, mut with_rows) = (Vec::new(), Vec::new(), 0);
    let mut dropped = facts::Dropped::default();
    for (cik, secs) in by_cik {
        let Some(main) = json(subs, &format!("CIK{cik}.json"))? else {
            no_submissions.push(cik.clone());
            continue;
        };
        let mut filings = main
            .pointer("/filings/recent")
            .map(facts::filings)
            .unwrap_or_default();
        for page in facts::page_names(&main) {
            if let Some(p) = json(subs, &page)? {
                filings.extend(facts::filings(&p));
            }
        }
        let parsed = json(facts_zip, &format!("CIK{cik}.json"))?
            .map(|v| facts::parse_facts(&v))
            .unwrap_or_default();
        let (rows, d) = facts::rows(&filings, &parsed, today);
        dropped.no_period += d.no_period;
        dropped.future += d.future;
        dropped.no_facts += d.no_facts;
        if rows.is_empty() {
            if filings
                .iter()
                .any(|f| f.filing_date >= xbrl && matches!(f.form.as_str(), "10-K" | "10-Q"))
            {
                no_rows.push(cik.clone());
            }
            continue;
        }
        with_rows += 1;
        for s in secs {
            out.extend(rows.iter().map(|r| (cik.as_str(), s, r.clone())));
        }
    }
    Ok(Built {
        batch: batch(&out)?,
        with_rows,
        no_submissions,
        no_rows,
        dropped,
    })
}

/// The 12 value columns, in `SCHEMAS.md` #3 order.
const VALUES: [&str; 12] = [
    "revenues",
    "net_income",
    "total_assets",
    "total_liabilities",
    "operating_cash_flow",
    "gross_profit",
    "assets_current",
    "liabilities_current",
    "long_term_debt",
    "retained_earnings",
    "ebit",
    "shares_outstanding",
];
const TTM: [(&str, &str); 5] = [
    ("revenue_ttm", "revenues"),
    ("net_income_ttm", "net_income"),
    ("operating_cash_flow_ttm", "operating_cash_flow"),
    ("gross_profit_ttm", "gross_profit"),
    ("ebit_ttm", "ebit"),
];
const PRIOR: [&str; 6] = [
    "prior_roa",
    "prior_debt_ratio",
    "prior_current_ratio",
    "prior_gross_margin",
    "prior_asset_turnover",
    "prior_shares_outstanding",
];

fn batch(rows: &[Out<'_>]) -> Result<arrow::array::RecordBatch, arrow::error::ArrowError> {
    use std::sync::Arc;

    use arrow::array::{
        ArrayRef, Date32Array, Float64Array, Int32Array, StringArray, TimestampNanosecondArray,
    };
    use arrow::datatypes::{DataType as T, Field, Schema, TimeUnit};
    let day =
        |d: NaiveDate| i32::try_from((d - NaiveDate::default()).num_days()).unwrap_or_default();
    let s = |f: &dyn Fn(&Out<'_>) -> Option<String>| -> ArrayRef {
        Arc::new(StringArray::from(rows.iter().map(f).collect::<Vec<_>>()))
    };
    let dt = |f: &dyn Fn(&facts::Row) -> Option<NaiveDate>| -> ArrayRef {
        Arc::new(Date32Array::from(
            rows.iter().map(|r| f(&r.2).map(day)).collect::<Vec<_>>(),
        ))
    };
    let num = |f: &dyn Fn(&facts::Row) -> Option<f64>| -> ArrayRef {
        Arc::new(Float64Array::from(
            rows.iter().map(|r| f(&r.2)).collect::<Vec<_>>(),
        ))
    };
    let int = |f: &dyn Fn(&facts::Row) -> Option<i32>| -> ArrayRef {
        Arc::new(Int32Array::from(
            rows.iter().map(|r| f(&r.2)).collect::<Vec<_>>(),
        ))
    };
    let ts = T::Timestamp(TimeUnit::Nanosecond, Some("UTC".into()));
    let mut fields = vec![
        ("ticker", T::Utf8),
        ("filing_date", T::Date32),
        ("period_end", T::Date32),
        ("form_type", T::Utf8),
    ];
    let mut cols: Vec<ArrayRef> = vec![
        s(&|r| Some(r.1.ticker.clone())),
        dt(&|r| Some(r.filing_date)),
        dt(&|r| Some(r.period_end)),
        s(&|r| Some(r.2.form_type.clone())),
    ];
    for v in VALUES {
        fields.push((v, T::Float64));
        cols.push(num(&|r| r.values.get(v).copied()));
    }
    fields.extend([
        ("accession", T::Utf8),
        ("cik", T::Utf8),
        ("fiscal_year", T::Int32),
        ("fiscal_period", T::Utf8),
        ("duration_days", T::Int32),
        ("acceptance_datetime", ts.clone()),
        ("amends_accession", T::Utf8),
        ("tag_used", T::Utf8),
        ("shares_asof_date", T::Date32),
    ]);
    cols.extend([
        s(&|r| Some(r.2.accession.clone())),
        s(&|r| Some(r.0.to_owned())),
        int(&|r| r.fiscal_year),
        s(&|r| r.2.fiscal_period.clone()),
        int(&|r| r.duration_days),
        Arc::new(
            TimestampNanosecondArray::from(
                rows.iter()
                    .map(|r| r.2.acceptance.and_then(|t| t.timestamp_nanos_opt()))
                    .collect::<Vec<_>>(),
            )
            .with_timezone("UTC"),
        ),
        s(&|r| r.2.amends_accession.clone()),
        s(&|r| {
            (!r.2.tag_used.is_empty())
                .then(|| serde_json::to_string(&r.2.tag_used).unwrap_or_default())
        }),
        dt(&|r| r.shares_asof_date),
    ]);
    for (name, field) in TTM {
        fields.push((name, T::Float64));
        cols.push(num(&|r| r.ttm.get(field).copied()));
    }
    for p in PRIOR {
        fields.push((p, T::Float64));
        cols.push(num(&|r| r.prior.get(p).copied()));
    }
    fields.push(("figi", T::Utf8));
    cols.push(s(&|r| Some(r.1.figi.clone())));
    let schema = Schema::new(
        fields
            .into_iter()
            .map(|(n, t)| Field::new(n, t, true))
            .collect::<Vec<_>>(),
    );
    arrow::array::RecordBatch::try_new(Arc::new(schema), cols)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test")]

    use super::*;

    #[test]
    fn quarters_stop_before_the_current_one() {
        let d = NaiveDate::from_ymd_opt(2026, 10, 11).unwrap();
        let q = quarters((2025, 3), d);
        assert_eq!(
            q,
            vec![(2025, 3), (2025, 4), (2026, 1), (2026, 2), (2026, 3)]
        );
        assert!(quarters((2026, 4), d).is_empty());
    }
}
