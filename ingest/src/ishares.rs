//! W8: iShares IWF / IWP holdings, archived from today on (`SOURCES.md` iShares, probe P7).
//! The site serves only the latest file, so the history is built forward, one snapshot per
//! run; earlier membership needs another source (`BACKFILL.md` §7). Parsing is pure.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use arrow::array::{
    ArrayRef, Date32Array, Float64Array, RecordBatch, StringArray, TimestampNanosecondArray,
};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use chrono::{DateTime, NaiveDate, Utc};

/// Bumped when the parser's output changes.
pub const PARSE_VERSION: &str = "1";

/// `(etf, product id, slug)` (probe P7).
pub const FUNDS: &[(&str, u32, &str)] = &[
    ("IWF", 239_706, "ishares-russell-1000-growth-etf"),
    ("IWP", 239_717, "ishares-russell-midcap-growth-etf"),
];

/// The latest-holdings URL of a fund.
#[must_use]
pub fn url(id: u32, slug: &str) -> String {
    format!("https://www.ishares.com/us/products/{id}/{slug}/latest-holdings.csv")
}

/// One equity line of a holdings file.
#[derive(Debug, Clone, PartialEq)]
pub struct Holding {
    /// Canonical ticker.
    pub ticker: String,
    /// The ticker as in the file.
    pub vendor_ticker: String,
    /// `Name`.
    pub name: String,
    /// `Sector`.
    pub sector: Option<String>,
    /// `Asset Class`.
    pub asset_class: Option<String>,
    /// `Weight (%)`.
    pub weight: Option<f64>,
    /// `Quantity`.
    pub shares: Option<f64>,
    /// `Price`.
    pub price: Option<f64>,
    /// `Market Value`.
    pub market_value: Option<f64>,
    /// `Exchange`.
    pub exchange: Option<String>,
    /// `Location`.
    pub location: Option<String>,
}

/// A parsed file: the snapshot date from its header, its equity lines, and what was dropped.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    /// `Fund Holdings as of`.
    pub snapshot_date: NaiveDate,
    /// Equity lines, one per canonical ticker.
    pub holdings: Vec<Holding>,
    /// Lines dropped, by reason.
    pub dropped: BTreeMap<&'static str, usize>,
}

fn number(s: &str) -> Option<f64> {
    let t = s.trim().replace(',', "");
    if t.is_empty() || t == "-" {
        None
    } else {
        t.parse().ok()
    }
}

fn text(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty() && t != "-").then(|| t.to_owned())
}

/// Parse a `latest-holdings.csv` body (P7 layout): a header block holding
/// `Fund Holdings as of,"Oct 09, 2026"`, then the table from the `Ticker,` row. Only
/// `Asset Class = Equity` lines with a ticker that normalizes are kept.
///
/// # Errors
///
/// A message when the body is not a holdings file (an HTML page, no date, no table).
pub fn parse(body: &[u8]) -> Result<Snapshot, String> {
    let text_body = String::from_utf8_lossy(body);
    let text_body = text_body.trim_start_matches('\u{feff}');
    let date_line = text_body
        .lines()
        .find(|l| l.starts_with("Fund Holdings as of"))
        .ok_or("no 'Fund Holdings as of' line (not a holdings file)")?;
    let date_text = date_line
        .split_once(',')
        .map(|(_, d)| d.trim().trim_matches('"'))
        .unwrap_or_default();
    let snapshot_date = NaiveDate::parse_from_str(date_text, "%b %d, %Y")
        .map_err(|e| format!("snapshot date {date_text:?}: {e}"))?;
    let start = text_body
        .find("\nTicker,")
        .ok_or("no 'Ticker,' table header")?
        + 1;
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .from_reader(text_body.get(start..).unwrap_or_default().as_bytes());
    let headers = reader.headers().map_err(|e| e.to_string())?.clone();
    let col = |name: &str| headers.iter().position(|h| h.trim() == name);
    let idx = |name: &str| col(name).ok_or_else(|| format!("no column {name:?}"));
    let (t, n, s, a, w, q, p, mv, ex, loc) = (
        idx("Ticker")?,
        idx("Name")?,
        idx("Sector")?,
        idx("Asset Class")?,
        idx("Weight (%)")?,
        idx("Quantity")?,
        idx("Price")?,
        idx("Market Value")?,
        idx("Exchange")?,
        idx("Location")?,
    );
    let mut holdings = Vec::new();
    let mut seen = BTreeSet::new();
    let mut dropped: BTreeMap<&'static str, usize> = BTreeMap::new();
    for rec in reader.records() {
        let Ok(rec) = rec else {
            *dropped.entry("unreadable_line").or_default() += 1;
            continue;
        };
        let get = |i: usize| rec.get(i).unwrap_or_default();
        // A disclaimer block after the table has no ticker column.
        if rec.len() < headers.len() {
            continue;
        }
        if get(a).trim() != "Equity" {
            *dropped.entry("not_equity").or_default() += 1;
            continue;
        }
        let vendor = get(t).trim().to_owned();
        let Ok(canon) = symbols::normalize(&vendor) else {
            // iShares files a distributed right or warrant under Equity: out of scope (D12).
            let derivative = get(n).split_whitespace().any(|w| {
                matches!(
                    w.to_ascii_uppercase().as_str(),
                    "RIGHT" | "RIGHTS" | "WARRANT" | "WARRANTS" | "CVR" | "UNIT" | "UNITS"
                )
            });
            let why = if derivative {
                "not_common"
            } else {
                "ticker_does_not_normalize"
            };
            *dropped.entry(why).or_default() += 1;
            continue;
        };
        if !seen.insert(canon.as_str().to_owned()) {
            *dropped.entry("duplicate_ticker").or_default() += 1;
            continue;
        }
        holdings.push(Holding {
            ticker: canon.as_str().to_owned(),
            vendor_ticker: vendor,
            name: get(n).trim().to_owned(),
            sector: text(get(s)),
            asset_class: text(get(a)),
            weight: number(get(w)),
            shares: number(get(q)),
            price: number(get(p)),
            market_value: number(get(mv)),
            exchange: text(get(ex)),
            location: text(get(loc)),
        });
    }
    if holdings.is_empty() {
        return Err("no equity lines".into());
    }
    Ok(Snapshot {
        snapshot_date,
        holdings,
        dropped,
    })
}

/// Provenance and joins for one fund's rows.
pub struct Context<'a> {
    /// `IWF` / `IWP`.
    pub etf: &'a str,
    /// First trading day the snapshot may be used.
    pub available_date: NaiveDate,
    /// When the file was fetched.
    pub fetched_at: DateTime<Utc>,
    /// SHA-256 of the raw file.
    pub raw_sha256: &'a str,
    /// Canonical ticker -> FIGI of the active security holding it today.
    pub figis: &'a BTreeMap<String, String>,
}

/// `etf_holdings` rows (registry column order).
///
/// # Errors
///
/// Arrow errors (a bug).
pub fn batch(snap: &Snapshot, cx: &Context<'_>) -> Result<RecordBatch, arrow::error::ArrowError> {
    let h = &snap.holdings;
    let n = h.len();
    let days = |d: NaiveDate| {
        i32::try_from(d.signed_duration_since(NaiveDate::default()).num_days()).unwrap_or_default()
    };
    let s = |f: &dyn Fn(&Holding) -> Option<String>| -> ArrayRef {
        Arc::new(StringArray::from(h.iter().map(f).collect::<Vec<_>>()))
    };
    let x = |f: &dyn Fn(&Holding) -> Option<f64>| -> ArrayRef {
        Arc::new(Float64Array::from(h.iter().map(f).collect::<Vec<_>>()))
    };
    let cols: Vec<ArrayRef> = vec![
        Arc::new(Date32Array::from(vec![days(snap.snapshot_date); n])),
        s(&|_| Some(cx.etf.to_owned())),
        s(&|r| Some(r.ticker.clone())),
        s(&|r| Some(r.vendor_ticker.clone())),
        s(&|r| Some(r.name.clone())),
        s(&|r| r.sector.clone()),
        s(&|r| r.asset_class.clone()),
        x(&|r| r.weight),
        x(&|r| r.shares),
        x(&|r| r.price),
        x(&|r| r.market_value),
        s(&|r| r.exchange.clone()),
        s(&|r| r.location.clone()),
        Arc::new(Date32Array::from(vec![days(cx.available_date); n])),
        Arc::new(
            TimestampNanosecondArray::from(vec![
                cx.fetched_at
                    .timestamp_nanos_opt()
                    .unwrap_or_default();
                n
            ])
            .with_timezone("UTC"),
        ),
        s(&|_| Some(cx.raw_sha256.to_owned())),
        s(&|r| cx.figis.get(&r.ticker).cloned()),
    ];
    let ts = DataType::Timestamp(TimeUnit::Nanosecond, Some("UTC".into()));
    let f = |name: &str, ty: DataType| Field::new(name, ty, true);
    let schema = Schema::new(vec![
        f("snapshot_date", DataType::Date32),
        f("etf", DataType::Utf8),
        f("ticker", DataType::Utf8),
        f("vendor_ticker", DataType::Utf8),
        f("name", DataType::Utf8),
        f("sector", DataType::Utf8),
        f("asset_class", DataType::Utf8),
        f("weight", DataType::Float64),
        f("shares", DataType::Float64),
        f("price", DataType::Float64),
        f("market_value", DataType::Float64),
        f("exchange", DataType::Utf8),
        f("location", DataType::Utf8),
        f("available_date", DataType::Date32),
        f("fetched_at", ts),
        f("raw_sha256", DataType::Utf8),
        f("figi", DataType::Utf8),
    ]);
    RecordBatch::try_new(Arc::new(schema), cols)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing, reason = "test")]

    use super::*;

    const FILE: &str = "\u{feff}iShares Russell 1000 Growth ETF\nFund Holdings as of,\"Oct 09, 2026\"\nInception Date,\"May 22, 2000\"\n\nTicker,Name,Sector,Asset Class,Market Value,Weight (%),Notional Value,Quantity,Price,Location,Exchange,Currency,FX Rate,Market Currency,Accrual Date\n\"NVDA\",\"NVIDIA\",\"Information Technology\",\"Equity\",\"20,081,938,647.36\",\"15.39\",\"20,081,938,647.36\",\"87,586,962.00\",\"229.28\",\"United States\",\"NASDAQ\",\"USD\",\"1.00\",\"USD\",\"-\"\n\"BRK.B\",\"BERKSHIRE HATHAWAY INC CLASS B\",\"Financials\",\"Equity\",\"1.00\",\"0.10\",\"1.00\",\"2.00\",\"500.00\",\"United States\",\"New York Stock Exchange Inc.\",\"USD\",\"1.00\",\"USD\",\"-\"\n\"UWMC-R-W\",\"UWM HOLDINGS RIGHTS WHEN ISSUED EX\",\"Financials\",\"Equity\",\"280.93\",\"0.00\",\"280.93\",\"133,777.00\",\"0.00\",\"United States\",\"NYSE\",\"USD\",\"1.00\",\"USD\",\"-\"\n\"X-Y-Z\",\"ODD TICKER CORP\",\"Financials\",\"Equity\",\"1.00\",\"0.00\",\"1.00\",\"1.00\",\"1.00\",\"United States\",\"NYSE\",\"USD\",\"1.00\",\"USD\",\"-\"\n\"RGYZ6\",\"RUSSELL1000 GRWTH MINI CME DEC 26\",\"Cash and/or Derivatives\",\"Futures\",\"0.00\",\"0.00\",\"112,625,170.00\",\"434.00\",\"5,190.10\",\"-\",\"Chicago Mercantile Exchange\",\"USD\",\"1.00\",\"USD\",\"-\"\n \nThe content contained herein is owned or licensed by BlackRock\n";

    #[test]
    fn parses_the_p7_layout() {
        let s = parse(FILE.as_bytes()).unwrap();
        assert_eq!(
            s.snapshot_date,
            NaiveDate::from_ymd_opt(2026, 10, 9).unwrap()
        );
        assert_eq!(
            s.holdings
                .iter()
                .map(|h| h.ticker.as_str())
                .collect::<Vec<_>>(),
            ["NVDA", "BRK.B"]
        );
        assert_eq!(s.holdings[0].market_value, Some(20_081_938_647.36));
        assert_eq!(s.holdings[0].shares, Some(87_586_962.0));
        assert_eq!(
            s.dropped,
            BTreeMap::from([
                ("not_common", 1),
                ("not_equity", 1),
                ("ticker_does_not_normalize", 1)
            ]),
            "the UWMC rights line is out of scope, X-Y-Z is unreadable"
        );
    }

    #[test]
    fn an_html_page_is_not_a_holdings_file() {
        assert!(parse(b"<!DOCTYPE html><html>...</html>").is_err());
    }

    #[test]
    fn rows_carry_figi_when_known() {
        let s = parse(FILE.as_bytes()).unwrap();
        let figis = BTreeMap::from([("NVDA".to_owned(), "BBG000BBJQV0".to_owned())]);
        let cx = Context {
            etf: "IWF",
            available_date: NaiveDate::from_ymd_opt(2026, 10, 12).unwrap(),
            fetched_at: Utc::now(),
            raw_sha256: "ab",
            figis: &figis,
        };
        let b = batch(&s, &cx).unwrap();
        assert_eq!(b.num_rows(), 2);
        assert_eq!(b.num_columns(), 17);
        assert_eq!(
            b.column(16).null_count(),
            1,
            "BRK.B has no FIGI in this map"
        );
    }
}
