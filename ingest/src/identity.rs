//! J4a: the identity job (DECISIONS D17, `docs/adr/0001-figi-security-key.md`). Every EODHD
//! code of the D12 scope is resolved to its US composite FIGI: through its EODHD ISIN, or,
//! without one, through a CUSIP from the SEC 13(f) lists whose OpenFIGI name equals the
//! code's name. The result is `security_master` (#31) and `security_spells` (#36).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use arrow::array::{
    ArrayRef, BooleanArray, Date32Array, RecordBatch, StringArray, TimestampNanosecondArray,
};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{Value, json};

use crate::eodhd::Code;

/// A 9-character CUSIP with a valid check digit (the modulus-10 "double add double" rule).
#[must_use]
pub fn cusip_valid(c: &str) -> bool {
    let b = c.as_bytes();
    let (Some(body), Some(&check)) = (b.get(..8), b.get(8)) else {
        return false;
    };
    if b.len() != 9 || !check.is_ascii_digit() {
        return false;
    }
    let mut sum = 0u32;
    for (i, &ch) in body.iter().enumerate() {
        let v = match ch {
            b'0'..=b'9' => u32::from(ch - b'0'),
            b'A'..=b'Z' => u32::from(ch - b'A') + 10,
            b'*' => 36,
            b'@' => 37,
            b'#' => 38,
            _ => return false,
        };
        let v = if i % 2 == 1 { v * 2 } else { v };
        sum += v / 10 + v % 10;
    }
    (10 - sum % 10) % 10 == u32::from(check - b'0')
}

/// The CUSIPs of a 13(f) list, from the text file (`037833100 APPLE INC ...`) or the text a
/// PDF yields (`037833 10 0 APPLE INC ...`). A line counts only if its CUSIP checks.
#[must_use]
pub fn parse_13f(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in text.lines() {
        let t = line.trim_start();
        let pdf_form = t
            .get(..11)
            .filter(|s| s.as_bytes().get(6) == Some(&b' ') && s.as_bytes().get(9) == Some(&b' '));
        let cusip = match pdf_form {
            Some(s) => s.replace(' ', ""),
            None => t.get(..9).unwrap_or_default().to_owned(),
        };
        if cusip_valid(&cusip) {
            out.insert(cusip);
        }
    }
    out
}

/// The CUSIP inside a US ISIN (`US` + CUSIP + check).
#[must_use]
pub fn isin_cusip(isin: &str) -> Option<&str> {
    isin.strip_prefix("US")
        .and_then(|r| r.get(..9))
        .filter(|c| cusip_valid(c))
}

/// Calendar quarters from `from` to `to`, inclusive, as `(year, quarter)`.
#[must_use]
pub fn quarters(from: (i32, u32), to: (i32, u32)) -> Vec<(i32, u32)> {
    let mut out = Vec::new();
    let (mut y, mut q) = from;
    while (y, q) <= to {
        out.push((y, q));
        (y, q) = if q == 4 { (y + 1, 1) } else { (y, q + 1) };
    }
    out
}

/// Candidate URLs of one quarter's list, in the order to try (`(ext, url)`): text from 2020,
/// PDFs at the two paths the SEC has used.
#[must_use]
pub fn list_urls(y: i32, q: u32) -> Vec<(&'static str, String)> {
    let files = |ext: &str| format!("https://www.sec.gov/files/investment/13flist{y}q{q}.{ext}");
    let mut v = Vec::new();
    if y >= 2020 {
        v.push(("txt", files("txt")));
    }
    v.push(("pdf", files("pdf")));
    v.push((
        "pdf",
        format!("https://www.sec.gov/divisions/investment/13f/13flist{y}q{q}.pdf"),
    ));
    v
}

/// The text of a PDF, page by page.
///
/// # Errors
///
/// A message if the PDF cannot be read.
pub fn pdf_text(bytes: &[u8]) -> Result<String, String> {
    let doc = pdf_oxide::PdfDocument::from_bytes(bytes.to_vec()).map_err(|e| e.to_string())?;
    let n = doc.page_count().map_err(|e| e.to_string())?;
    let mut out = String::new();
    for p in 0..n {
        out.push_str(&doc.extract_text(p).map_err(|e| e.to_string())?);
        out.push('\n');
    }
    Ok(out)
}

/// Words dropped when comparing company names (`UNIVERSE.md` §4).
const NAME_NOISE: &[&str] = &[
    "INC",
    "INCORPORATED",
    "CORP",
    "CORPORATION",
    "CO",
    "COMPANY",
    "LTD",
    "LIMITED",
    "PLC",
    "LP",
    "LLC",
    "THE",
    "NEW",
    "CL",
    "CLASS",
    "COM",
    "COMMON",
    "STOCK",
    "SHS",
    "ORD",
    "A",
    "B",
    "C",
];

/// A company name reduced for exact comparison: upper case, `&` as `AND`, punctuation as
/// spaces, legal-form and share-class words dropped.
#[must_use]
pub fn normalize_name(s: &str) -> String {
    // Periods and apostrophes vanish (`P.F. CHANG'S` -> `PF CHANGS`); other punctuation splits.
    let up = s
        .to_ascii_uppercase()
        .replace('&', " AND ")
        .replace(['.', '\''], "");
    let cleaned: String = up
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' })
        .collect();
    cleaned
        .split_whitespace()
        .filter(|w| !NAME_NOISE.contains(w))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A pre-merger SPAC (blank-check company) by its name: `ACQUISITION` or `BLANK CHECK` in the
/// normalized name (DECISIONS D19). Shells are outside the training scope and not an identity
/// gap.
///
/// shortcut: a name rule; replace it with EDGAR SIC 6770 ("blank checks") once the EDGAR
/// worker fills `security_master.sic` (W6).
#[must_use]
pub fn is_spac_name(s: &str) -> bool {
    let n = normalize_name(s);
    n.split_whitespace().any(|w| w == "ACQUISITION") || n.contains("BLANK CHECK")
}

/// The comparison key of a name: [`normalize_name`] without spaces, so `P F CHANGS` and
/// `PF CHANGS` are one name.
#[must_use]
pub fn name_key(s: &str) -> String {
    normalize_name(s).replace(' ', "")
}

/// One OpenFIGI answer kept: the US composite listing of an equity (common stock or not;
/// [`resolve`] keeps only common stock in scope).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Figi {
    /// US composite FIGI.
    pub figi: String,
    /// Share-class FIGI.
    pub share_class: Option<String>,
    /// OpenFIGI `name`.
    pub name: String,
    /// OpenFIGI `securityType`.
    pub security_type: Option<String>,
    /// OpenFIGI `securityType2` (`Common Stock`, `Depositary Receipt`, `Mutual Fund`, ...).
    pub security_type2: String,
}

/// The OpenFIGI mapping request body for `values` of `id_type` (`ID_ISIN`, `ID_CUSIP`).
#[must_use]
pub fn jobs_json(id_type: &str, values: &[String]) -> String {
    let jobs: Vec<Value> = values
        .iter()
        .map(|v| json!({"idType": id_type, "idValue": v, "exchCode": "US", "includeUnlistedEquities": true}))
        .collect();
    Value::Array(jobs).to_string()
}

/// One answer per job: the US composite equity listing, or `None` (no match, an error,
/// or more than one US composite, which is ambiguous).
#[must_use]
pub fn parse_mapping(body: &[u8]) -> Vec<Option<Figi>> {
    let answers: Vec<Value> = serde_json::from_slice(body).unwrap_or_default();
    answers
        .iter()
        .map(|a| {
            let rows = a.get("data")?.as_array()?;
            let us: Vec<&Value> = rows
                .iter()
                .filter(|r| {
                    r.get("exchCode").and_then(Value::as_str) == Some("US")
                        && r.get("marketSector").and_then(Value::as_str) == Some("Equity")
                        // `SWC/WD`, `X/WI`: when-distributed and when-issued lines of the same stock
                        // (a class share is also written with a slash, `BRK/B`, and is kept).
                        && !r.get("ticker").and_then(Value::as_str).unwrap_or_default().rsplit_once('/').is_some_and(|(_, s)| matches!(s, "WI" | "WD"))
                })
                .collect();
            let composites: BTreeSet<&str> = us
                .iter()
                .filter_map(|r| r.get("compositeFIGI").and_then(Value::as_str))
                .collect();
            if composites.len() != 1 {
                return None;
            }
            let r = us.first()?;
            Some(Figi {
                figi: (*composites.iter().next()?).to_owned(),
                share_class: r
                    .get("shareClassFIGI")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                name: r
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                security_type: r
                    .get("securityType")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                security_type2: r
                    .get("securityType2")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            })
        })
        .collect()
}

/// How a code was resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Through its EODHD ISIN.
    Isin(String),
    /// Through a 13(f) CUSIP whose OpenFIGI name equals the code's name.
    Cusip13f(String),
}

/// A resolved code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The EODHD code.
    pub code: Code,
    /// Its security.
    pub figi: Figi,
    /// The evidence.
    pub how: Resolution,
}

/// Why a code is unresolved (`data_quality` notes, gate G5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unresolved {
    /// No ISIN route and no 13(f) FIGI with the same name.
    NoMatch,
    /// More than one 13(f) FIGI with the same name (never a guess).
    Ambiguous(Vec<String>),
    /// Found, but not a common stock (`securityType2`): outside the D12 scope (ADRs, funds,
    /// rights, units). Not a gap.
    OutOfScope(String),
}

/// OpenFIGI `securityType2` values in the D12 scope (a REIT is a common stock; ADRs, funds,
/// units, warrants, rights and partnership units are not).
const IN_SCOPE: &[&str] = &["Common Stock", "REIT"];

/// Resolve every code (ADR 0001 decision 3): ISIN first, then a unique name match against the
/// FIGIs reached from 13(f) CUSIPs.
#[must_use]
pub fn resolve(
    codes: &[Code],
    by_isin: &BTreeMap<String, Figi>,
    by_cusip: &BTreeMap<String, Figi>,
) -> (Vec<Resolved>, Vec<(Code, Unresolved)>) {
    let mut by_name: BTreeMap<String, BTreeMap<String, (&Figi, &str)>> = BTreeMap::new();
    for (cusip, f) in by_cusip
        .iter()
        .filter(|(_, f)| IN_SCOPE.contains(&f.security_type2.as_str()))
    {
        by_name
            .entry(name_key(&f.name))
            .or_default()
            .insert(f.figi.clone(), (f, cusip.as_str()));
    }
    let (mut ok, mut bad) = (Vec::new(), Vec::new());
    for c in codes {
        if let Some((isin, f)) = c.isin.as_ref().and_then(|i| by_isin.get(i).map(|f| (i, f))) {
            let reference = crate::eodhd::run::REFERENCE
                .iter()
                .any(|(r, _)| *r == c.code);
            if !reference && !IN_SCOPE.contains(&f.security_type2.as_str()) {
                bad.push((c.clone(), Unresolved::OutOfScope(f.security_type2.clone())));
                continue;
            }
            ok.push(Resolved {
                code: c.clone(),
                figi: f.clone(),
                how: Resolution::Isin(isin.clone()),
            });
            continue;
        }
        let name = name_key(&c.name);
        match by_name.get(&name).filter(|_| !name.is_empty()) {
            Some(m) if m.len() == 1 => {
                if let Some((f, cusip)) = m.values().next() {
                    ok.push(Resolved {
                        code: c.clone(),
                        figi: (*f).clone(),
                        how: Resolution::Cusip13f((*cusip).to_owned()),
                    });
                }
            }
            _ if is_spac_name(&c.name) => bad.push((
                c.clone(),
                Unresolved::OutOfScope("SPAC shell (name, D19)".into()),
            )),
            Some(m) => bad.push((
                c.clone(),
                Unresolved::Ambiguous(m.keys().cloned().collect()),
            )),
            None => bad.push((c.clone(), Unresolved::NoMatch)),
        }
    }
    (ok, bad)
}

fn nullable(fields: &[(&str, DataType)]) -> Arc<Schema> {
    Arc::new(Schema::new(
        fields
            .iter()
            .map(|(n, t)| Field::new(*n, t.clone(), true))
            .collect::<Vec<_>>(),
    ))
}

/// `security_master` rows, one per FIGI. When several codes share a FIGI (a vendor rename),
/// the active code wins, then the plain code over an `_old` one.
///
/// # Errors
///
/// Arrow errors (a bug).
pub fn master_batch(
    resolved: &[Resolved],
    cik_by_ticker: &BTreeMap<String, String>,
    now: DateTime<Utc>,
) -> Result<RecordBatch, arrow::error::ArrowError> {
    let mut by_figi: BTreeMap<&str, Vec<&Resolved>> = BTreeMap::new();
    for r in resolved {
        by_figi.entry(r.figi.figi.as_str()).or_default().push(r);
    }
    let mut rows = Vec::new();
    for group in by_figi.values() {
        let mut g = group.clone();
        g.sort_by_key(|r| {
            (
                r.code.delisted,
                r.code.code.contains("_old"),
                r.code.code.clone(),
            )
        });
        if let Some(top) = g.first() {
            let delisted = g.iter().all(|r| r.code.delisted);
            rows.push((*top, delisted));
        }
    }
    let s = |f: &dyn Fn(&(&Resolved, bool)) -> Option<String>| -> ArrayRef {
        Arc::new(StringArray::from(rows.iter().map(f).collect::<Vec<_>>()))
    };
    let n = rows.len();
    let note = |r: &Resolved| match &r.how {
        Resolution::Isin(i) => ("isin", format!("ISIN {i}")),
        Resolution::Cusip13f(c) => (
            "cusip_13f",
            format!(
                "CUSIP {c}; name {:?} = {:?}",
                normalize_name(&r.code.name),
                normalize_name(&r.figi.name)
            ),
        ),
    };
    let cols: Vec<ArrayRef> = vec![
        s(&|(r, _)| Some(r.figi.figi.clone())),
        s(&|(r, _)| r.figi.share_class.clone()),
        s(&|(r, d)| {
            if *d {
                None
            } else {
                cik_by_ticker.get(&r.code.ticker).cloned()
            }
        }),
        s(&|(r, _)| Some(r.code.ticker.clone())),
        s(&|(r, _)| Some(r.code.name.clone())),
        s(&|(r, _)| Some(r.code.code.clone())),
        s(&|_| None),
        Arc::new(BooleanArray::from(
            rows.iter().map(|(_, d)| *d).collect::<Vec<_>>(),
        )),
        s(&|(r, _)| r.figi.security_type.clone()),
        s(&|(r, _)| Some(r.figi.name.clone())),
        s(&|(r, _)| Some(note(r).0.to_owned())),
        s(&|(r, _)| Some(note(r).1)),
        Arc::new(
            TimestampNanosecondArray::from(vec![now.timestamp_nanos_opt().unwrap_or_default(); n])
                .with_timezone("UTC"),
        ),
        s(&|_| None),
        s(&|_| None),
        Arc::new(Date32Array::from(vec![None; n])),
    ];
    let ts = DataType::Timestamp(TimeUnit::Nanosecond, Some("UTC".into()));
    let schema = nullable(&[
        ("figi", DataType::Utf8),
        ("share_class_figi", DataType::Utf8),
        ("cik", DataType::Utf8),
        ("ticker", DataType::Utf8),
        ("name", DataType::Utf8),
        ("eodhd_code", DataType::Utf8),
        ("exchange", DataType::Utf8),
        ("delisted", DataType::Boolean),
        ("security_type", DataType::Utf8),
        ("figi_name", DataType::Utf8),
        ("resolution", DataType::Utf8),
        ("resolution_note", DataType::Utf8),
        ("updated_at", ts),
        ("sic", DataType::Utf8),
        ("sic_description", DataType::Utf8),
        ("sic_asof", DataType::Date32),
    ]);
    RecordBatch::try_new(schema, cols)
}

/// `(figi, id_type, value)`.
type SpellKey<'a> = (String, &'a str, String);
/// `(valid_from, valid_to, source)`.
type Span<'a> = (Option<NaiveDate>, Option<NaiveDate>, &'a str);

/// `security_spells` rows: each code's EODHD code, ticker, ISIN and CUSIP; the CUSIP of a
/// 13(f) resolution carries the first and last quarter it was listed.
///
/// # Errors
///
/// Arrow errors (a bug).
pub fn spells_batch(
    resolved: &[Resolved],
    cusip_span: &BTreeMap<String, (NaiveDate, NaiveDate)>,
) -> Result<RecordBatch, arrow::error::ArrowError> {
    let mut rows: BTreeMap<SpellKey<'_>, Span<'_>> = BTreeMap::new();
    for r in resolved {
        let f = r.figi.figi.clone();
        rows.insert(
            (f.clone(), "eodhd_code", r.code.code.clone()),
            (None, None, "eodhd_symbols"),
        );
        rows.insert(
            (f.clone(), "ticker", r.code.ticker.clone()),
            (None, None, "eodhd_symbols"),
        );
        if let Some(i) = &r.code.isin {
            rows.insert(
                (f.clone(), "isin", i.clone()),
                (None, None, "eodhd_symbols"),
            );
        }
        let cusip = match &r.how {
            Resolution::Isin(i) => isin_cusip(i).map(|c| (c.to_owned(), "eodhd_symbols")),
            Resolution::Cusip13f(c) => Some((c.clone(), "sec_13f")),
        };
        if let Some((c, src)) = cusip {
            let span = cusip_span.get(&c);
            rows.insert((f, "cusip", c), (span.map(|s| s.0), span.map(|s| s.1), src));
        }
    }
    let days = |d: Option<NaiveDate>| {
        d.and_then(|d| i32::try_from(d.signed_duration_since(NaiveDate::default()).num_days()).ok())
    };
    let cols: Vec<ArrayRef> = vec![
        Arc::new(StringArray::from(
            rows.keys().map(|k| k.0.clone()).collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            rows.keys().map(|k| k.1).collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            rows.keys().map(|k| k.2.clone()).collect::<Vec<_>>(),
        )),
        Arc::new(Date32Array::from(
            rows.values().map(|v| days(v.0)).collect::<Vec<_>>(),
        )),
        Arc::new(Date32Array::from(
            rows.values().map(|v| days(v.1)).collect::<Vec<_>>(),
        )),
        Arc::new(StringArray::from(
            rows.values().map(|v| v.2).collect::<Vec<_>>(),
        )),
    ];
    let schema = nullable(&[
        ("figi", DataType::Utf8),
        ("id_type", DataType::Utf8),
        ("value", DataType::Utf8),
        ("valid_from", DataType::Date32),
        ("valid_to", DataType::Date32),
        ("source", DataType::Utf8),
    ]);
    RecordBatch::try_new(schema, cols)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing, reason = "test")]

    use super::*;

    #[test]
    fn cusip_check_digits() {
        for ok in [
            "037833100",
            "524908100",
            "812350106",
            "464289503",
            "G0084W101",
        ] {
            assert!(cusip_valid(ok), "{ok}");
        }
        for bad in [
            "037833101",
            "03783310",
            "0378331000",
            "03783310X",
            "037833 10",
        ] {
            assert!(!cusip_valid(bad), "{bad}");
        }
        assert_eq!(isin_cusip("US61166W1018"), Some("61166W101"));
        assert_eq!(isin_cusip("BMG1234567"), None);
    }

    #[test]
    fn thirteen_f_lines_in_both_formats() {
        let txt = "B38564108*CMB.TECH NV                   SHS              N\nG0084W101*ADIENT PLC                    ORD SHS          N\nnot a line\n037833101 BAD CHECK DIGIT\n";
        assert_eq!(
            parse_13f(txt).into_iter().collect::<Vec<_>>(),
            ["B38564108", "G0084W101"]
        );
        let pdf = "Run Date: 3/29/2010 ** List of Section 13F Securities ** Page 227\nCUSIP NO ISSUER NAME ISSUER DESCRIPTION STATUS\n464289 50 3 ISHARES TR S&P TARGET 25\n524908 10 0 LEHMAN BROS HLDGS INC COM\n";
        assert_eq!(
            parse_13f(pdf).into_iter().collect::<Vec<_>>(),
            ["464289503", "524908100"]
        );
        assert_eq!(
            quarters((2004, 4), (2005, 2)),
            [(2004, 4), (2005, 1), (2005, 2)]
        );
        assert_eq!(list_urls(2010, 1).len(), 2);
        assert_eq!(list_urls(2024, 4)[0].0, "txt");
    }

    #[test]
    fn names_compare_without_legal_form_and_punctuation() {
        assert!(
            is_spac_name("Golden Path Acquisition Corp")
                && is_spac_name("Colombier Acquisition Corp. II")
        );
        assert!(
            !is_spac_name("Stillwater Mining Co") && !is_spac_name("Acquisitions Group Inc"),
            "the whole word only"
        );
        assert_eq!(
            name_key("P F Changs China Bistro Inc"),
            name_key("PF CHANG'S CHINA BISTRO INC")
        );
        assert_eq!(
            name_key("P.F. Chang's China Bistro, Inc."),
            "PFCHANGSCHINABISTRO"
        );
        assert_eq!(
            normalize_name("Lehman Brothers Holdings Inc"),
            normalize_name("LEHMAN BROTHERS HOLDINGS INC")
        );
        assert_eq!(
            normalize_name("Monsanto"),
            normalize_name("MONSANTO CO NEW")
        );
        assert_eq!(normalize_name("AT&T Inc."), "AT AND T");
        assert_ne!(
            normalize_name("General Motors Co"),
            normalize_name("General Mills Inc")
        );
    }

    fn figi(id: &str, name: &str) -> Value {
        json!({"figi": id, "compositeFIGI": id, "shareClassFIGI": "BBG001", "name": name, "exchCode": "US", "marketSector": "Equity", "securityType": "Common Stock", "securityType2": "Common Stock"})
    }

    #[test]
    fn mapping_keeps_one_us_composite_common_stock() {
        let wd = json!({"figi": "BBG_WD", "compositeFIGI": "BBG_WD", "ticker": "SWC/WD", "name": "STILLWATER MINING CO- W/D", "exchCode": "US", "marketSector": "Equity", "securityType2": "Common Stock"});
        let only = parse_mapping(
            json!([{"data": [figi("BBG000HYNVL9", "STILLWATER MINING CO"), wd]}])
                .to_string()
                .as_bytes(),
        );
        assert_eq!(
            only[0].as_ref().map(|f| f.figi.as_str()),
            Some("BBG000HYNVL9"),
            "a /WD line is not a second security"
        );
        let brk = json!({"figi": "BBG000DWG505", "compositeFIGI": "BBG000DWG505", "ticker": "BRK/B", "name": "BERKSHIRE HATHAWAY INC-CL B", "exchCode": "US", "marketSector": "Equity", "securityType2": "Common Stock"});
        let class = parse_mapping(json!([{"data": [brk]}]).to_string().as_bytes());
        assert_eq!(
            class[0].as_ref().map(|f| f.figi.as_str()),
            Some("BBG000DWG505"),
            "a class share keeps its slash ticker"
        );
        let foreign = json!({"figi": "BBGX", "compositeFIGI": "BBGX", "exchCode": "GR", "marketSector": "Equity", "securityType2": "Common Stock"});
        let body = json!([
            {"data": [foreign, figi("BBG000BKRK35", "LEHMAN BROTHERS HOLDINGS INC")]},
            {"warning": "No identifier found."},
            {"data": [figi("BBG1", "X"), figi("BBG2", "X")]},
        ])
        .to_string();
        let m = parse_mapping(body.as_bytes());
        assert_eq!(m[0].as_ref().map(|f| f.figi.as_str()), Some("BBG000BKRK35"));
        assert_eq!(m[1], None);
        assert_eq!(m[2], None, "two US composites are ambiguous");
        assert!(
            jobs_json("ID_CUSIP", &["524908100".into()])
                .contains("\"includeUnlistedEquities\":true")
        );
    }

    #[test]
    fn resolution_prefers_isin_then_a_unique_name() {
        let code = |c: &str, name: &str, isin: Option<&str>| Code {
            code: c.into(),
            ticker: c.into(),
            delisted: true,
            name: name.into(),
            isin: isin.map(Into::into),
        };
        let f = |id: &str, name: &str| Figi {
            figi: id.into(),
            share_class: None,
            name: name.into(),
            security_type: None,
            security_type2: "Common Stock".into(),
        };
        let adr = Figi {
            security_type2: "Depositary Receipt".into(),
            ..f("BBG_ADR", "SOME ADR")
        };
        let by_isin = BTreeMap::from([
            ("US61166W1018".to_owned(), f("BBG000BFWKC0", "MONSANTO CO")),
            ("US0000000001".to_owned(), adr),
        ]);
        let by_cusip = BTreeMap::from([
            (
                "524908100".to_owned(),
                f("BBG000BKRK35", "LEHMAN BROTHERS HOLDINGS INC"),
            ),
            (
                "370442105".to_owned(),
                f("BBG_OLD_GM", "GENERAL MOTORS CORP"),
            ),
            ("37045V100".to_owned(), f("BBG_NEW_GM", "GENERAL MOTORS CO")),
        ]);
        let codes = [
            code("MON_old", "Monsanto", Some("US61166W1018")),
            code("LEH", "Lehman Brothers Holdings Inc", None),
            code("GM_old", "General Motors Corp", None),
            code("ZZZ", "Nobody Inc", None),
            code("ADRX", "Some ADR", Some("US0000000001")),
        ];
        let (ok, bad) = resolve(&codes, &by_isin, &by_cusip);
        assert_eq!(ok.len(), 2);
        assert_eq!(ok[0].how, Resolution::Isin("US61166W1018".into()));
        assert_eq!(
            (ok[1].figi.figi.as_str(), &ok[1].how),
            ("BBG000BKRK35", &Resolution::Cusip13f("524908100".into()))
        );
        assert!(
            matches!(&bad[0].1, Unresolved::Ambiguous(v) if v.len() == 2),
            "old and new GM share a name"
        );
        assert_eq!(bad[1].1, Unresolved::NoMatch);
        assert_eq!(
            bad[2].1,
            Unresolved::OutOfScope("Depositary Receipt".into()),
            "found but not common stock"
        );
        let now = Utc::now();
        let m = master_batch(&ok, &BTreeMap::new(), now).unwrap();
        assert_eq!(m.num_rows(), 2);
        let spans = BTreeMap::from([(
            "524908100".to_owned(),
            (
                NaiveDate::from_ymd_opt(2004, 10, 1).unwrap(),
                NaiveDate::from_ymd_opt(2008, 9, 30).unwrap(),
            ),
        )]);
        let s = spells_batch(&ok, &spans).unwrap();
        assert_eq!(
            s.num_rows(),
            7,
            "MON_old: code, ticker, isin, cusip; LEH: code, ticker, cusip"
        );
    }
}

/// The orchestration of a J4a run.
pub mod run {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::Path;

    use chrono::{Datelike, NaiveDate, Utc};

    use super::{Figi, Unresolved};
    use crate::eodhd::Code;
    use crate::infra::fetch::{Fatal, Fetcher, Outcome, Request};
    use crate::infra::landing::{self, sha256_hex};
    use crate::infra::manifest::Manifest;
    use crate::infra::quality::{Level, QualityLog, Row};

    /// A run failure.
    #[derive(Debug, thiserror::Error)]
    pub enum RunError {
        /// The source must stop.
        #[error(transparent)]
        Fatal(#[from] Fatal),
        /// The EODHD symbol lists.
        #[error(transparent)]
        Symbols(#[from] crate::eodhd::run::RunError),
        /// Manifest error.
        #[error("manifest: {0}")]
        Manifest(#[from] rusqlite::Error),
        /// Reading a landed body.
        #[error("{0}")]
        Io(#[from] std::io::Error),
        /// Writing the lake.
        #[error("lake: {0}")]
        Lake(#[from] lake::LakeError),
        /// Building rows (a bug).
        #[error("arrow: {0}")]
        Arrow(#[from] arrow::error::ArrowError),
        /// A PDF that cannot be read, or a worker thread that died.
        #[error("{0}")]
        Other(String),
    }

    /// Everything a run needs.
    pub struct Job {
        /// EODHD context (symbol lists).
        pub eodhd: crate::eodhd::run::Ctx,
        /// SEC files (13(f) lists, ticker files).
        pub sec: Fetcher,
        /// OpenFIGI.
        pub figi: Fetcher,
        /// Jobs per OpenFIGI request (100 with a key, 10 without).
        pub batch: usize,
        /// The manifest.
        pub manifest: std::sync::Arc<Manifest>,
        /// The run's quality log.
        pub quality: QualityLog,
        /// First 13(f) quarter (`BACKFILL_START`'s quarter, less one).
        pub first_quarter: (i32, u32),
        /// Last 13(f) quarter; `None` = the current one.
        pub last_quarter: Option<(i32, u32)>,
        /// Without an OpenFIGI key, refuse a mapping larger than this many lookups.
        pub keyless_limit: Option<usize>,
    }

    /// What a run produced.
    #[derive(Debug, Clone, Default)]
    pub struct Summary {
        /// Codes in scope.
        pub codes: usize,
        /// Resolved through the ISIN.
        pub by_isin: usize,
        /// Resolved through a 13(f) CUSIP.
        pub by_cusip: usize,
        /// Unresolved: no match.
        pub no_match: usize,
        /// Unresolved: ambiguous.
        pub ambiguous: usize,
        /// Found, but not a common stock (outside D12).
        pub out_of_scope: usize,
        /// Distinct FIGIs written.
        pub securities: usize,
        /// 13(f) quarters read.
        pub quarters: usize,
        /// Distinct 13(f) CUSIPs.
        pub cusips: usize,
    }

    fn body(m: &Manifest, key: (&str, &str, &str)) -> Result<Option<Vec<u8>>, RunError> {
        match m.landed(key)? {
            Some((p, _, _)) => Ok(Some(landing::read(Path::new(&p))?)),
            None => Ok(None),
        }
    }

    /// Every 13(f) CUSIP with the first and last day of the quarters it was listed in.
    async fn thirteen_f(
        job: &Job,
        today: NaiveDate,
    ) -> Result<(BTreeMap<String, (NaiveDate, NaiveDate)>, usize), RunError> {
        let now = job
            .last_quarter
            .unwrap_or((today.year(), (today.month() - 1) / 3 + 1));
        let mut span: BTreeMap<String, (NaiveDate, NaiveDate)> = BTreeMap::new();
        let mut read = 0;
        for (y, q) in super::quarters(job.first_quarter, now) {
            let start = NaiveDate::from_ymd_opt(y, (q - 1) * 3 + 1, 1).unwrap_or_default();
            let end = start
                .checked_add_months(chrono::Months::new(3))
                .and_then(|d| d.pred_opt())
                .unwrap_or(start);
            for (i, (ext, url)) in super::list_urls(y, q).into_iter().enumerate() {
                let chunk = format!("{y}q{q}-{i}.{ext}");
                let r = Request {
                    entity: "13flist",
                    chunk: &chunk,
                    url: &url,
                    ext,
                    cost: 1,
                    dataset: "security_master",
                    empty_is_gone: false,
                };
                if matches!(
                    job.sec.fetch(&r).await?,
                    Outcome::Failed(_) | Outcome::Gone(_)
                ) {
                    continue;
                }
                let Some(bytes) = body(&job.manifest, (job.sec.spec.id, "13flist", &chunk))? else {
                    continue;
                };
                let text = if ext == "pdf" {
                    tokio::task::spawn_blocking(move || super::pdf_text(&bytes))
                        .await
                        .map_err(|e| RunError::Other(e.to_string()))?
                        .map_err(RunError::Other)?
                } else {
                    String::from_utf8_lossy(&bytes).into_owned()
                };
                for c in super::parse_13f(&text) {
                    let e = span.entry(c).or_insert((start, end));
                    e.0 = e.0.min(start);
                    e.1 = e.1.max(end);
                }
                read += 1;
                break;
            }
        }
        Ok((span, read))
    }

    /// Map `values` through OpenFIGI in batches; every answered batch is landed (chunk = the
    /// batch's hash, so a re-run skips it).
    async fn map(
        job: &Job,
        id_type: &str,
        values: &[String],
    ) -> Result<BTreeMap<String, Figi>, RunError> {
        if let Some(limit) = job.keyless_limit.filter(|l| values.len() > *l) {
            return Err(RunError::Other(format!(
                "{} {id_type} lookups without OPENFIGI_API_KEY would take about {} minutes (limit {limit}); set the free key",
                values.len(),
                values.len() / 250
            )));
        }
        let mut out = BTreeMap::new();
        for chunk_values in values.chunks(job.batch.max(1)) {
            let request = super::jobs_json(id_type, chunk_values);
            let chunk = sha256_hex(request.as_bytes())
                .get(..16)
                .unwrap_or_default()
                .to_owned();
            let entity = id_type.to_ascii_lowercase();
            let r = Request {
                entity: &entity,
                chunk: &chunk,
                url: "https://api.openfigi.com/v3/mapping",
                ext: "json",
                cost: 1,
                dataset: "security_master",
                empty_is_gone: false,
            };
            if matches!(
                job.figi.post(&r, &request).await?,
                Outcome::Failed(_) | Outcome::Gone(_)
            ) {
                continue;
            }
            if let Some(b) = body(&job.manifest, (job.figi.spec.id, &entity, &chunk))? {
                for (v, f) in chunk_values.iter().zip(super::parse_mapping(&b)) {
                    if let Some(f) = f {
                        out.insert(v.clone(), f);
                    }
                }
            }
        }
        Ok(out)
    }

    /// Current ticker -> 10-digit CIK from the SEC's `company_tickers_exchange.json`.
    async fn ciks(job: &Job, today: NaiveDate) -> Result<BTreeMap<String, String>, RunError> {
        let chunk = format!("company_tickers_exchange-{today}");
        let url = "https://www.sec.gov/files/company_tickers_exchange.json";
        let r = Request {
            entity: "company_tickers",
            chunk: &chunk,
            url,
            ext: "json",
            cost: 1,
            dataset: "security_master",
            empty_is_gone: false,
        };
        job.sec.fetch(&r).await?;
        let mut out = BTreeMap::new();
        if let Some(b) = body(&job.manifest, (job.sec.spec.id, "company_tickers", &chunk))? {
            let v: serde_json::Value = serde_json::from_slice(&b).unwrap_or_default();
            for row in v
                .get("data")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
            {
                let (Some(cik), Some(t)) = (
                    row.get(0).and_then(serde_json::Value::as_u64),
                    row.get(2).and_then(serde_json::Value::as_str),
                ) else {
                    continue;
                };
                if let Some(t) = crate::eodhd::code_ticker(t) {
                    out.entry(t).or_insert_with(|| format!("{cik:010}"));
                }
            }
        }
        Ok(out)
    }

    /// Run J4a: resolve `codes` (all in-scope codes when `None`) and write #31 and #36.
    ///
    /// # Errors
    ///
    /// [`RunError`]; the quality rows gathered so far are kept by the caller.
    pub async fn run(
        job: &Job,
        lake: &lake::Lake,
        today: NaiveDate,
        codes: Option<Vec<Code>>,
    ) -> Result<Summary, RunError> {
        let mut codes = match codes {
            Some(c) => c,
            None => crate::eodhd::run::symbols(&job.eodhd, today).await?,
        };
        for (code, isin) in crate::eodhd::run::REFERENCE {
            if !codes.iter().any(|c| c.code == *code) {
                codes.push(Code {
                    code: (*code).into(),
                    ticker: (*code).into(),
                    name: (*code).into(),
                    isin: Some((*isin).into()),
                    delisted: false,
                });
            }
        }
        let mut s = Summary {
            codes: codes.len(),
            ..Summary::default()
        };
        let (span, quarters) = thirteen_f(job, today).await?;
        s.quarters = quarters;
        s.cusips = span.len();
        println!(
            "identity: {} codes, {quarters} 13(f) quarters, {} distinct CUSIPs",
            s.codes, s.cusips
        );
        let isins: Vec<String> = codes
            .iter()
            .filter_map(|c| c.isin.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let by_isin = map(job, "ID_ISIN", &isins).await?;
        // The name pool needs only CUSIPs no code reaches through its own ISIN.
        let covered: BTreeSet<&str> = by_isin
            .keys()
            .filter_map(|i| super::isin_cusip(i))
            .collect();
        let pool: Vec<String> = span
            .keys()
            .filter(|c| !covered.contains(c.as_str()))
            .cloned()
            .collect();
        let by_cusip = map(job, "ID_CUSIP", &pool).await?;
        let (ok, bad) = super::resolve(&codes, &by_isin, &by_cusip);
        s.by_isin = ok
            .iter()
            .filter(|r| matches!(r.how, super::Resolution::Isin(_)))
            .count();
        s.by_cusip = ok.len() - s.by_isin;
        for (c, why) in &bad {
            let note = match why {
                Unresolved::NoMatch => {
                    s.no_match += 1;
                    format!(
                        "identity_unresolved: no FIGI (isin {:?}, name {:?})",
                        c.isin,
                        super::normalize_name(&c.name)
                    )
                }
                Unresolved::Ambiguous(f) => {
                    s.ambiguous += 1;
                    format!(
                        "identity_unresolved: ambiguous name {:?}: {}",
                        super::normalize_name(&c.name),
                        f.join(" ")
                    )
                }
                Unresolved::OutOfScope(t) => {
                    s.out_of_scope += 1;
                    format!("out_of_scope: {t}")
                }
            };
            let level = if matches!(why, Unresolved::OutOfScope(_)) {
                Level::Ok
            } else {
                Level::Warn
            };
            job.quality.push(Row {
                source: "OpenFIGI",
                dataset: "security_master".into(),
                entity: format!("{}.US", c.code),
                chunk: "identity".into(),
                level,
                attempts: 1,
                notes: Some(note),
            });
        }
        let cik = ciks(job, today).await?;
        let master = super::master_batch(&ok, &cik, Utc::now())?;
        s.securities = master.num_rows();
        lake.write_dataset("security_master", &master, lake::Mode::Replace)?;
        lake.write_dataset(
            "security_spells",
            &super::spells_batch(&ok, &span)?,
            lake::Mode::Replace,
        )?;
        Ok(s)
    }
}
