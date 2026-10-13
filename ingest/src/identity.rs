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

/// A when-issued or when-distributed line of a stock, which OpenFIGI lists under the same
/// ISIN with its own composite: the ticker `SWC/WD`, `X/WI` or `RBC-W` (a class share is also
/// written with a slash, `BRK/B`, and is kept), or the name's last word `WI`, `W/I`, `WD` or
/// `W/D` (`NEOGEN CORP-WI` for `NEOGV`).
fn when_issued(r: &Value) -> bool {
    let text = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or_default();
    let ticker = text("ticker");
    ticker
        .rsplit_once('/')
        .is_some_and(|(_, s)| matches!(s, "WI" | "WD"))
        || ticker.ends_with("-W")
        || text("name")
            .rsplit([' ', '-'])
            .next()
            .is_some_and(|w| matches!(w, "WI" | "W/I" | "WD" | "W/D"))
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
                        && !when_issued(r)
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
    /// Through a 13(f) CUSIP whose OpenFIGI name, cut at 28 characters (`FIGI_NAME_MAX`), starts
    /// the code's name.
    Cusip13fCut(String),
    /// A rename or vendor twin of the resolved code given (same name, continuous or identical
    /// price series); the string is the evidence.
    Rename(String),
    /// No FIGI exists: the training-only key `EOD:<code>` of a delisted code whose name names one
    /// domestic EDGAR filer (D17, amended 2026-10-12): that filer's CIK and the evidence.
    EodCode(String, String),
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

/// OpenFIGI cuts `name` at this many characters (`PREFERRED APARTMENT COMMUNIT`).
const FIGI_NAME_MAX: usize = 28;

/// A cut name matches by its start only with a key this long (shorter starts are generic).
const MIN_CUT_KEY: usize = 12;

/// Words of an EODHD name that mark an instrument beside a stock: such a code never matches a
/// cut OpenFIGI name by its start (the stock's name starts the warrant's).
const INSTRUMENT_WORDS: &[&str] = &[
    "WARRANT",
    "WARRANTS",
    "UNIT",
    "UNITS",
    "RIGHT",
    "RIGHTS",
    "PREFERRED",
    "PFD",
    "NOTE",
    "NOTES",
    "DEBENTURE",
    "DEBENTURES",
    "DEPOSITARY",
    "SERIES",
    "SUBORDINATED",
];

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
    let cut: Vec<(String, &Figi, &str)> = by_cusip
        .iter()
        .filter(|(_, f)| {
            IN_SCOPE.contains(&f.security_type2.as_str()) && f.name.chars().count() >= FIGI_NAME_MAX
        })
        .map(|(cusip, f)| (name_key(&f.name), f, cusip.as_str()))
        .filter(|(k, _, _)| k.len() >= MIN_CUT_KEY)
        .collect();
    let by_code: BTreeMap<&str, &Code> = codes.iter().map(|c| (c.code.as_str(), c)).collect();
    let (mut ok, mut bad) = (Vec::new(), Vec::new());
    for c in codes {
        let reference = crate::eodhd::run::REFERENCE
            .iter()
            .any(|(r, _)| *r == c.code);
        // Before every route: a SPAC is out of scope even when its ISIN or name resolves (D19).
        if !reference && is_spac_name(&c.name) {
            bad.push((
                c.clone(),
                Unresolved::OutOfScope("SPAC shell (name, D19)".into()),
            ));
            continue;
        }
        if let Some((isin, f)) = c.isin.as_ref().and_then(|i| by_isin.get(i).map(|f| (i, f))) {
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
        if let Some(kind) = instrument_suffix(c, &by_code) {
            bad.push((
                c.clone(),
                Unresolved::OutOfScope(format!("{kind} (code suffix)")),
            ));
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
            Some(m) => bad.push((
                c.clone(),
                Unresolved::Ambiguous(m.keys().cloned().collect()),
            )),
            None => {
                // A name OpenFIGI cut short: the code's name must start with it.
                let instrument = normalize_name(&c.name)
                    .split_whitespace()
                    .any(|w| INSTRUMENT_WORDS.contains(&w));
                let hits: BTreeMap<&str, (&Figi, &str)> = cut
                    .iter()
                    .filter(|(k, _, _)| !instrument && name.starts_with(k.as_str()))
                    .map(|(_, f, cusip)| (f.figi.as_str(), (*f, *cusip)))
                    .collect();
                match (hits.len(), hits.values().next()) {
                    (1, Some((f, cusip))) => ok.push(Resolved {
                        code: c.clone(),
                        figi: (*f).clone(),
                        how: Resolution::Cusip13fCut((*cusip).to_owned()),
                    }),
                    (0, _) => bad.push((c.clone(), Unresolved::NoMatch)),
                    _ => bad.push((
                        c.clone(),
                        Unresolved::Ambiguous(hits.keys().map(|f| (*f).to_owned()).collect()),
                    )),
                }
            }
        }
    }
    (ok, bad)
}

/// A warrant, unit or right listed beside a stock: the code is another in-scope code plus a
/// suffix (`-WT`, `-WS`, `-U`, `-UN`, `-RT`, `-R`, or a fifth letter `W` / `U` / `R` / `Z` after a
/// four-letter base), and that base has the same name. Requiring the base keeps an ordinary
/// five-letter ticker out.
#[must_use]
pub fn instrument_suffix(c: &Code, by_code: &BTreeMap<&str, &Code>) -> Option<&'static str> {
    let code = c.code.as_str();
    let (base, kind) = if let Some((base, s)) = code.rsplit_once('-') {
        let kind = match s {
            "WT" | "WS" => "warrant",
            "U" | "UN" => "unit",
            "RT" | "R" => "right",
            _ => return None,
        };
        (base, kind)
    } else if code.len() == 5 {
        let kind = match code.as_bytes().get(4) {
            Some(b'W') => "warrant",
            Some(b'U') => "unit",
            Some(b'R') => "right",
            Some(b'Z') => "other instrument",
            _ => return None,
        };
        (code.get(..4)?, kind)
    } else {
        return None;
    };
    let b = by_code.get(base)?;
    (name_key(&b.name) == name_key(&c.name) && !name_key(&c.name).is_empty()).then_some(kind)
}

/// Daily closes of an `/api/eod` body.
#[must_use]
pub fn closes(body: &[u8]) -> BTreeMap<NaiveDate, f64> {
    let rows: Vec<Value> = serde_json::from_slice(body).unwrap_or_default();
    rows.iter()
        .filter_map(|r| {
            let d = NaiveDate::parse_from_str(r.get("date")?.as_str()?, "%Y-%m-%d").ok()?;
            Some((d, r.get("close")?.as_f64()?))
        })
        .collect()
}

/// How an unresolved code's prices relate to a resolved code's: a ticker change (its last bar
/// at most 10 days before the other's first) or the same series under two codes (at least 20
/// common dates, closes within 0.5% on 95% of them).
#[must_use]
pub fn same_security(
    a: &BTreeMap<NaiveDate, f64>,
    b: &BTreeMap<NaiveDate, f64>,
) -> Option<&'static str> {
    let (a_last, b_first) = (*a.keys().next_back()?, *b.keys().next()?);
    if a_last < b_first && (b_first - a_last).num_days() <= 10 {
        return Some("continuation");
    }
    let common: Vec<(f64, f64)> = a
        .iter()
        .filter_map(|(d, x)| b.get(d).map(|y| (*x, *y)))
        .collect();
    let agree = common
        .iter()
        .filter(|(x, y)| *y != 0.0 && ((x / y) - 1.0).abs() <= 0.005)
        .count();
    (common.len() >= 20 && agree * 100 >= common.len() * 95).then_some("same series")
}

/// Renames and vendor twins (ADR 0001 rule 4): an unresolved code whose name equals the name of
/// exactly one resolved security and whose prices continue or repeat that security's series
/// takes its FIGI. `bars` gives a code's closes (from the landing zone).
pub fn rename_links(
    unresolved: &[Code],
    resolved: &[Resolved],
    bars: &mut dyn FnMut(&str) -> Option<BTreeMap<NaiveDate, f64>>,
) -> Vec<Resolved> {
    let mut by_name: BTreeMap<String, Vec<&Resolved>> = BTreeMap::new();
    for r in resolved {
        by_name.entry(name_key(&r.code.name)).or_default().push(r);
    }
    let mut out = Vec::new();
    for u in unresolved {
        let key = name_key(&u.name);
        let Some(cands) = by_name.get(&key).filter(|_| key.len() > 3) else {
            continue;
        };
        let figis: BTreeSet<&str> = cands.iter().map(|r| r.figi.figi.as_str()).collect();
        let Some(r) = cands.first().filter(|_| figis.len() == 1) else {
            continue;
        };
        let (Some(ub), Some(rb)) = (bars(&u.code), bars(&r.code.code)) else {
            continue;
        };
        if let Some(how) = same_security(&ub, &rb) {
            out.push(Resolved {
                code: u.clone(),
                figi: r.figi.clone(),
                how: Resolution::Rename(format!("{how} of {}", r.code.code)),
            });
        }
    }
    out
}

/// The periodic report forms of a domestic filer (a foreign private issuer files 20-F / 40-F).
const DOMESTIC_FORMS: &[&str] = &["10-K", "10-K405", "10-KSB", "10-KT", "10-Q", "10-QSB"];

/// A filer's `(form, filing_date)` list.
pub type Forms = Vec<(String, NaiveDate)>;

/// A filer's 10-K/10-Q filing dates and its first and last filing of any form.
pub type Filer = (Vec<NaiveDate>, (NaiveDate, NaiveDate));

/// The [`Filer`] of a `(form, filing_date)` list; `None` without a filing.
#[must_use]
pub fn filer(forms: &Forms) -> Option<Filer> {
    let periodic = forms
        .iter()
        .filter(|(form, _)| DOMESTIC_FORMS.contains(&form.as_str()))
        .map(|x| x.1)
        .collect();
    let first = forms.iter().map(|x| x.1).min()?;
    let last = forms.iter().map(|x| x.1).max()?;
    Some((periodic, (first, last)))
}

/// Of the EDGAR filers `ciks` with a security's name, the one that filed a 10-K or 10-Q
/// (`DOMESTIC_FORMS`) while the security traded (`priced`, a year of slack each side,
/// [`cik_confirmed`]); of several, the one that filed one between its first and last bar
/// (Bear Stearns Companies Inc, not the LLC that filed after the merger); `None` for none or
/// a tie. Returns the CIK and the filer's filing span.
pub fn pick_filer<'a>(
    ciks: impl IntoIterator<Item = &'a String>,
    priced: (NaiveDate, NaiveDate),
    filers: &mut dyn FnMut(&str) -> Option<Filer>,
) -> Option<(String, (NaiveDate, NaiveDate))> {
    let mut ok: Vec<(String, Filer)> = Vec::new();
    for cik in ciks {
        let Some((periodic, span)) = filers(cik) else {
            continue;
        };
        let range = periodic
            .iter()
            .min()
            .copied()
            .zip(periodic.iter().max().copied());
        if cik_confirmed(Some(priced), range) {
            ok.push((cik.clone(), (periodic, span)));
        }
    }
    if ok.len() > 1 {
        ok.retain(|(_, (p, _))| p.iter().any(|d| priced.0 <= *d && *d <= priced.1));
    }
    match ok.as_slice() {
        [(cik, (_, span))] => Some((cik.clone(), *span)),
        _ => None,
    }
}

/// `(CIK, filing span, price span)` of a security whose name several filers share.
pub type SharedCik = (String, (NaiveDate, NaiveDate), (NaiveDate, NaiveDate));

/// The CIK of each delisted resolved code whose name several EDGAR filers share
/// ([`pick_filer`]; a name of one filer goes through `cik_by_name` in [`master_batch`]):
/// EODHD code -> [`SharedCik`]. Lehman Brothers Holdings and Fannie Mae had no CIK, so no
/// filings, no fundamentals and no delisting record.
pub fn shared_name_ciks(
    resolved: &[Resolved],
    candidates: &BTreeMap<String, BTreeSet<String>>,
    spans: &mut dyn FnMut(&str) -> Option<(NaiveDate, NaiveDate)>,
    filers: &mut dyn FnMut(&str) -> Option<Filer>,
) -> BTreeMap<String, SharedCik> {
    resolved
        .iter()
        .filter(|r| r.code.delisted)
        .filter_map(|r| {
            let ciks = candidates
                .get(&name_key(&r.code.name))
                .filter(|c| c.len() > 1)?;
            let priced = spans(&r.code.code)?;
            let (cik, filed) = pick_filer(ciks, priced, filers)?;
            Some((r.code.code.clone(), (cik, filed, priced)))
        })
        .collect()
}

/// A code given a training-only key by [`eod_keys`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EodKeyed {
    /// The code, its key `EOD:<code>` and the evidence.
    pub resolved: Resolved,
    /// The filer found by name.
    pub cik: String,
    /// First and last bar of the code.
    pub priced: (NaiveDate, NaiveDate),
    /// First and last filing of the filer.
    pub filed: (NaiveDate, NaiveDate),
}

/// Training-only keys (DECISIONS D17, amended 2026-10-12) for delisted codes no FIGI route
/// reached: the key is `EOD:<code>` when [`pick_filer`] finds the code's EDGAR filer among
/// those with its name (`candidates`, [`cik_candidates`]). Left out, never guessed: a code
/// without bars, a filer a resolved security already has (`taken`: a twin or another class),
/// and every code of a filer whose candidate codes trade over overlapping dates. `spans`
/// gives a code's first and last bar, `filers` a filer's [`Filer`].
pub fn eod_keys(
    unresolved: &[Code],
    candidates: &BTreeMap<String, BTreeSet<String>>,
    taken: &BTreeSet<String>,
    spans: &mut dyn FnMut(&str) -> Option<(NaiveDate, NaiveDate)>,
    filers: &mut dyn FnMut(&str) -> Option<Filer>,
) -> Vec<EodKeyed> {
    let mut by_cik: BTreeMap<String, Vec<EodKeyed>> = BTreeMap::new();
    for c in unresolved.iter().filter(|c| c.delisted) {
        let Some(ciks) = candidates.get(&name_key(&c.name)) else {
            continue;
        };
        let Some(priced) = spans(&c.code) else {
            continue;
        };
        let free = ciks.iter().filter(|k| !taken.contains(*k));
        let Some((cik, filed)) = pick_filer(free, priced, filers) else {
            continue;
        };
        by_cik.entry(cik.clone()).or_default().push(EodKeyed {
            resolved: Resolved {
                code: c.clone(),
                figi: Figi {
                    figi: format!("EOD:{}", c.code),
                    share_class: None,
                    name: c.name.clone(),
                    security_type: Some("Common Stock".into()),
                    security_type2: "Common Stock".into(),
                },
                how: Resolution::EodCode(
                    cik.clone(),
                    format!(
                        "no FIGI; CIK {cik} by name {:?} ({} filers of that name), 10-K/10-Q filer, \
                         filed {}..{}, priced {}..{}",
                        normalize_name(&c.name),
                        ciks.len(),
                        filed.0,
                        filed.1,
                        priced.0,
                        priced.1
                    ),
                ),
            },
            cik,
            priced,
            filed,
        });
    }
    by_cik
        .into_values()
        .filter(|g| {
            g.iter().enumerate().all(|(i, a)| {
                g.iter()
                    .skip(i + 1)
                    .all(|b| a.priced.1 < b.priced.0 || b.priced.1 < a.priced.0)
            })
        })
        .flatten()
        .collect()
}

fn nullable(fields: &[(&str, DataType)]) -> Arc<Schema> {
    Arc::new(Schema::new(
        fields
            .iter()
            .map(|(n, t)| Field::new(*n, t.clone(), true))
            .collect::<Vec<_>>(),
    ))
}

/// EDGAR's `cik-lookup-data.txt` (`NAME:CIK:` per line, current and former names of every
/// filer) as [`name_key`] -> every CIK with that key.
#[must_use]
pub fn cik_candidates(text: &str) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for line in text.lines() {
        let Some((name, cik)) = line
            .trim_end()
            .strip_suffix(':')
            .and_then(|l| l.rsplit_once(':'))
        else {
            continue;
        };
        let key = name_key(name);
        if key.is_empty() || cik.len() != 10 || !cik.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        out.entry(key).or_default().insert(cik.to_owned());
    }
    out
}

/// [`cik_candidates`] with one CIK per key; `None` when the key belongs to more than one.
#[must_use]
pub fn unique_ciks(c: &BTreeMap<String, BTreeSet<String>>) -> BTreeMap<String, Option<String>> {
    c.iter()
        .map(|(k, v)| (k.clone(), v.first().filter(|_| v.len() == 1).cloned()))
        .collect()
}

/// EDGAR's `cik-lookup-data.txt` as [`name_key`] -> CIK; `None` when the key belongs to more
/// than one CIK.
#[must_use]
pub fn parse_cik_lookup(text: &str) -> BTreeMap<String, Option<String>> {
    unique_ciks(&cik_candidates(text))
}

/// First and last filing date of an EDGAR submissions JSON (`filings.recent` plus the older
/// `filings.files` pages); `None` without a date.
#[must_use]
pub fn submissions_span(body: &[u8]) -> Option<(NaiveDate, NaiveDate)> {
    let v: Value = serde_json::from_slice(body).ok()?;
    let f = v.get("filings")?;
    let date = |s: &Value| NaiveDate::parse_from_str(s.as_str()?, "%Y-%m-%d").ok();
    let recent = f
        .pointer("/recent/filingDate")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(date);
    let pages = f
        .get("files")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|p| [p.get("filingFrom"), p.get("filingTo")])
        .flatten()
        .filter_map(date);
    let all: Vec<NaiveDate> = recent.chain(pages).collect();
    Some((*all.iter().min()?, *all.iter().max()?))
}

/// A CIK found by name is kept only when the filer's filings overlap the price history, with
/// a year of slack each side: an unrelated filer that shares the name filed in another era.
/// Either span unknown: not confirmed.
#[must_use]
pub fn cik_confirmed(
    prices: Option<(NaiveDate, NaiveDate)>,
    filings: Option<(NaiveDate, NaiveDate)>,
) -> bool {
    let (Some((p0, p1)), Some((f0, f1))) = (prices, filings) else {
        return false;
    };
    let slack = chrono::Days::new(365);
    p0 <= f1.checked_add_days(slack).unwrap_or(NaiveDate::MAX)
        && f0.checked_sub_days(slack).unwrap_or(NaiveDate::MIN) <= p1
}

/// `security_master` rows, one per FIGI. When several codes share a FIGI (a vendor rename),
/// the active code wins, then the plain code over an `_old` one. The CIK is the current
/// ticker's for an active security, else the one CIK whose EDGAR name or former name has the
/// security's [`name_key`] (`cik_by_name`, from [`parse_cik_lookup`]) that `confirm` accepts
/// (see [`cik_confirmed`]), or for a name several filers share the one [`shared_name_ciks`]
/// chose (`chosen`, by EODHD code); otherwise null. A training-only key carries its CIK.
///
/// # Errors
///
/// Arrow errors (a bug).
pub fn master_batch(
    resolved: &[Resolved],
    cik_by_ticker: &BTreeMap<String, String>,
    cik_by_name: &BTreeMap<String, Option<String>>,
    chosen: &BTreeMap<String, String>,
    confirm: &dyn Fn(&Resolved, &str) -> bool,
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
    // `(cik, by name)`; a delisted ticker may now belong to another issuer, so only its name counts.
    let cik = |(r, d): &(&Resolved, bool)| {
        if let Resolution::EodCode(c, _) = &r.how {
            return Some((c.clone(), true));
        }
        let by_ticker = if *d {
            None
        } else {
            cik_by_ticker.get(&r.code.ticker)
        };
        by_ticker
            .map(|c| (c.clone(), false))
            .or_else(|| chosen.get(&r.code.code).map(|c| (c.clone(), true)))
            .or_else(|| {
                cik_by_name
                    .get(&name_key(&r.code.name))
                    .cloned()
                    .flatten()
                    .filter(|c| confirm(r, c))
                    .map(|c| (c, true))
            })
    };
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
        Resolution::Cusip13fCut(c) => (
            "cusip_13f",
            format!(
                "CUSIP {c}; name {:?} starts with the cut OpenFIGI name {:?}",
                normalize_name(&r.code.name),
                normalize_name(&r.figi.name)
            ),
        ),
        Resolution::Rename(e) => ("rename", e.clone()),
        Resolution::EodCode(_, e) => ("eod_code", e.clone()),
    };
    let cols: Vec<ArrayRef> = vec![
        s(&|(r, _)| Some(r.figi.figi.clone())),
        s(&|(r, _)| r.figi.share_class.clone()),
        s(&|row| cik(row).map(|c| c.0)),
        s(&|(r, _)| Some(r.code.ticker.clone())),
        s(&|(r, _)| Some(r.code.name.clone())),
        s(&|(r, _)| Some(r.code.code.clone())),
        s(&|(r, _)| Some(r.code.exchange.clone()).filter(|e| !e.is_empty())),
        Arc::new(BooleanArray::from(
            rows.iter().map(|(_, d)| *d).collect::<Vec<_>>(),
        )),
        s(&|(r, _)| r.figi.security_type.clone()),
        s(&|(r, _)| Some(r.figi.name.clone())),
        s(&|(r, _)| Some(note(r).0.to_owned())),
        s(&|row| {
            let by_name = cik(row).is_some_and(|c| c.1);
            Some(format!(
                "{}{}",
                note(row.0).1,
                if by_name { "; cik:name" } else { "" }
            ))
        }),
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

/// An issuer's `(sic, sic_description, sic_asof)` (J4).
pub type Sic = (Option<String>, Option<String>, Option<NaiveDate>);

/// The SIC columns of a `security_master` batch, by CIK (rows without a CIK or SIC skipped).
#[must_use]
pub fn sic_by_cik(master: &RecordBatch) -> BTreeMap<String, Sic> {
    use arrow::array::{Array, AsArray};
    let col = |n: &str| {
        master
            .column_by_name(n)
            .map(|c| c.as_string::<i32>().clone())
    };
    let (Some(cik), Some(sic), Some(desc)) = (col("cik"), col("sic"), col("sic_description"))
    else {
        return BTreeMap::new();
    };
    let asof = master
        .column_by_name("sic_asof")
        .map(|c| c.as_primitive::<arrow::datatypes::Date32Type>().clone());
    let get = |a: &StringArray, i| a.is_valid(i).then(|| a.value(i).to_owned());
    (0..master.num_rows())
        .filter(|&i| cik.is_valid(i) && sic.is_valid(i))
        .map(|i| {
            let d = asof
                .as_ref()
                .and_then(|a| a.is_valid(i).then(|| a.value_as_date(i)).flatten());
            (cik.value(i).to_owned(), (get(&sic, i), get(&desc, i), d))
        })
        .collect()
}

/// `master` with its SIC columns set from `sic` by CIK (null for a CIK not in the map).
///
/// # Errors
///
/// Arrow errors (a batch without the `security_master` columns).
pub fn with_sic(
    master: &RecordBatch,
    sic: &BTreeMap<String, Sic>,
) -> Result<RecordBatch, arrow::error::ArrowError> {
    use arrow::array::{Array, AsArray};
    let schema = master.schema();
    let cik = master
        .column(schema.index_of("cik")?)
        .as_string::<i32>()
        .clone();
    let row = |i: usize| cik.is_valid(i).then(|| sic.get(cik.value(i))).flatten();
    let n = master.num_rows();
    let epoch = NaiveDate::default();
    let text = |f: fn(&Sic) -> Option<String>| -> ArrayRef {
        Arc::new(StringArray::from(
            (0..n).map(|i| row(i).and_then(f)).collect::<Vec<_>>(),
        ))
    };
    let cols = schema
        .fields()
        .iter()
        .zip(master.columns())
        .map(|(f, c)| match f.name().as_str() {
            "sic" => text(|s| s.0.clone()),
            "sic_description" => text(|s| s.1.clone()),
            "sic_asof" => Arc::new(Date32Array::from(
                (0..n)
                    .map(|i| {
                        row(i)
                            .and_then(|s| s.2)
                            .and_then(|d| i32::try_from((d - epoch).num_days()).ok())
                    })
                    .collect::<Vec<_>>(),
            )),
            _ => c.clone(),
        })
        .collect();
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
            Resolution::Cusip13f(c) | Resolution::Cusip13fCut(c) => Some((c.clone(), "sec_13f")),
            Resolution::Rename(_) | Resolution::EodCode(..) => None,
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
    use arrow::array::Array;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn instruments_need_a_base_code_with_the_same_name() {
        let c = |code: &str, name: &str| Code {
            code: code.into(),
            name: name.into(),
            ..Code::default()
        };
        let all = [
            c("DKNG", "DraftKings Inc"),
            c("DKNGW", "DraftKings Inc"),
            c("MPLN", "MultiPlan Corp"),
            c("MPLN-WT", "MultiPlan Corporation"),
            c("GOOGL", "Alphabet Inc"),
            c("ABCDW", "Other Co"),
        ];
        let by: BTreeMap<&str, &Code> = all.iter().map(|x| (x.code.as_str(), x)).collect();
        assert_eq!(instrument_suffix(&all[1], &by), Some("warrant"));
        assert_eq!(instrument_suffix(&all[3], &by), Some("warrant"));
        assert_eq!(
            instrument_suffix(&all[4], &by),
            None,
            "five letters, no base code"
        );
        assert_eq!(instrument_suffix(&all[5], &by), None, "no ABCD code");
        assert_eq!(instrument_suffix(&all[0], &by), None);
    }

    #[test]
    fn renames_link_by_name_and_price_series() {
        let series = |from: NaiveDate, n: i64, px: f64| {
            (0..n)
                .map(|i| (from + chrono::Days::new(u64::try_from(i).unwrap()), px))
                .collect::<BTreeMap<_, _>>()
        };
        let old = series(day(2010, 1, 1), 100, 5.0);
        let new = series(day(2010, 4, 15), 100, 6.0);
        assert_eq!(same_security(&old, &new), Some("continuation"));
        assert_eq!(
            same_security(&old, &series(day(2010, 1, 1), 300, 5.0)),
            Some("same series")
        );
        assert_eq!(
            same_security(&old, &series(day(2010, 1, 1), 300, 9.0)),
            None,
            "same dates, different prices"
        );
        assert_eq!(
            same_security(&old, &series(day(2012, 1, 1), 100, 5.0)),
            None,
            "a two-year gap is not a rename"
        );
        let f = Figi {
            figi: "BBG_PGEN".into(),
            share_class: None,
            name: "PRECIGEN INC".into(),
            security_type: None,
            security_type2: "Common Stock".into(),
        };
        let pgen = Resolved {
            code: Code {
                code: "PGEN".into(),
                name: "Precigen Inc".into(),
                ..Code::default()
            },
            figi: f,
            how: Resolution::Isin("US74017N1054".into()),
        };
        let xon = Code {
            code: "XON".into(),
            name: "Precigen Inc".into(),
            ..Code::default()
        };
        let mut bars = |code: &str| {
            Some(if code == "XON" {
                old.clone()
            } else {
                new.clone()
            })
        };
        let links = rename_links(
            std::slice::from_ref(&xon),
            std::slice::from_ref(&pgen),
            &mut bars,
        );
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].figi.figi, "BBG_PGEN");
        assert_eq!(
            links[0].how,
            Resolution::Rename("continuation of PGEN".into())
        );
    }

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
        // Regal Rexnord's ISIN also answers Regal Beloit's when-issued line; NEOGV is named -WI.
        let line = |id: &str, ticker: &str, name: &str| json!({"figi": id, "compositeFIGI": id, "ticker": ticker, "name": name, "exchCode": "US", "marketSector": "Equity", "securityType2": "Common Stock"});
        let m = parse_mapping(
            json!([
                {"data": [line("BBG000BRRG02", "RRX", "REGAL REXNORD CORP"), line("BBG_RBCW", "RBC-W", "REGAL BELOIT CORP - W/I")]},
                {"data": [line("BBG000BZPV39", "NEOG", "NEOGEN CORP"), line("BBG_NEOGV", "NEOGV", "NEOGEN CORP-WI")]},
            ])
            .to_string()
            .as_bytes(),
        );
        assert_eq!(
            m.iter()
                .map(|f| f.as_ref().map(|f| f.figi.as_str()))
                .collect::<Vec<_>>(),
            [Some("BBG000BRRG02"), Some("BBG000BZPV39")],
            "a when-issued line is not a second security"
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
    fn cut_names_match_by_their_start_and_never_an_instrument() {
        let code = |c: &str, name: &str| Code {
            code: c.into(),
            ticker: c.into(),
            delisted: true,
            name: name.into(),
            ..Code::default()
        };
        let f = |id: &str, name: &str| Figi {
            figi: id.into(),
            share_class: None,
            name: name.into(),
            security_type: None,
            security_type2: "Common Stock".into(),
        };
        let by_cusip = BTreeMap::from([
            (
                "693718108".to_owned(),
                f("BBG_PSUN", "PACIFIC SUNWEAR OF CALIFORNI"),
            ),
            (
                "629337106".to_owned(),
                f("BBG_NETD", "NABORS ENERGY TRANSITION C-A"),
            ),
            (
                "320000001".to_owned(),
                f("BBG_FNB1", "FIRST NATIONAL BANCORP OF IN"),
            ),
        ]);
        let codes = [
            code("PSUNQ", "Pacific Sunwear Of California Inc"),
            code("NETDW", "Nabors Energy Transition Corp. II Warrant"),
            code("FNBO", "First National Bancorp of Ohio"),
        ];
        let (ok, bad) = resolve(&codes, &BTreeMap::new(), &by_cusip);
        assert_eq!(
            ok.iter()
                .map(|r| (r.code.code.as_str(), r.figi.figi.as_str()))
                .collect::<Vec<_>>(),
            [("PSUNQ", "BBG_PSUN")]
        );
        assert_eq!(ok[0].how, Resolution::Cusip13fCut("693718108".into()));
        assert_eq!(bad.len(), 2, "a warrant and another bank stay unresolved");
    }

    #[test]
    fn training_only_keys_need_one_domestic_filer_that_filed_while_priced() {
        let d = |y, m| NaiveDate::from_ymd_opt(y, m, 1).unwrap();
        let code = |c: &str, name: &str| Code {
            code: c.into(),
            ticker: c.into(),
            delisted: true,
            name: name.into(),
            ..Code::default()
        };
        // Bear Stearns shares its name with the LLC that filed after the 2008 merger.
        let names = cik_candidates(
            "BEAR STEARNS COMPANIES INC:0000777001:\nBEAR STEARNS COMPANIES LLC:0001442180:\n\
             NATIONAL CITY CORP:0000069970:\nSOME FOREIGN CO:0000000003:\nTAKEN CORP:0000000004:\n\
             TWIN CORP:0000000005:\nTWO FILERS INC:0000000006:\nTWO FILERS CORP:0000000007:\n",
        );
        let codes = [
            code("BSC_old", "Bear Stearns Companies Inc"),
            code("NCC_old", "National City Corp"),
            code("SFC", "Some Foreign Co"),
            code("TKN", "Taken Corp"),
            code("TWN", "Twin Corp"),
            code("TWN_old", "Twin Corp"),
            code("TWO", "Two Filers Inc"),
            Code {
                delisted: false,
                ..code("LIVE", "Bear Stearns Companies Inc")
            },
        ];
        let taken = BTreeSet::from(["0000000004".to_owned()]);
        let mut spans = |c: &str| match c {
            "NCC_old" => Some((d(2005, 1), d(2008, 12))),
            _ => Some((d(2005, 1), d(2008, 3))),
        };
        let mut filers = |cik: &str| {
            filer(&match cik {
                "0000000003" => vec![("20-F".to_owned(), d(2006, 6))],
                "0000069970" => vec![("10-K".to_owned(), d(2001, 3))],
                "0001442180" => vec![("10-K".to_owned(), d(2009, 2))],
                _ => vec![
                    ("10-Q".to_owned(), d(2007, 10)),
                    ("8-K".to_owned(), d(2008, 3)),
                ],
            })
        };
        let keyed = eod_keys(&codes, &names, &taken, &mut spans, &mut filers);
        // A FIGI security whose name two filers share takes the one that filed while priced.
        let lehman = Resolved {
            code: code("LEH", "Bear Stearns Companies Inc"),
            figi: keyed[0].resolved.figi.clone(),
            how: Resolution::Cusip13f("524908100".into()),
        };
        let shared = shared_name_ciks(&[lehman], &names, &mut spans, &mut filers);
        assert_eq!(shared.get("LEH").map(|s| s.0.as_str()), Some("0000777001"));
        assert_eq!(
            keyed
                .iter()
                .map(|k| k.resolved.figi.figi.as_str())
                .collect::<Vec<_>>(),
            ["EOD:BSC_old"],
            "National City stopped filing years before it was priced; a 20-F filer, a taken \
             filer, overlapping twins, two filers that both filed while priced and an active \
             code get no key"
        );
        assert_eq!(
            keyed[0].cik, "0000777001",
            "the filer that filed while priced"
        );
        assert!(matches!(&keyed[0].resolved.how, Resolution::EodCode(c, _) if c == "0000777001"));
        let m = master_batch(
            &[keyed[0].resolved.clone()],
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
            &|_, _| false,
            Utc::now(),
        )
        .unwrap();
        let cik = m.column_by_name("cik").unwrap();
        assert_eq!(
            cik.as_any().downcast_ref::<StringArray>().unwrap().value(0),
            "0000777001",
            "the master row takes the filer even when its name is shared"
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
            exchange: "NYSE".into(),
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
            (
                "US0000000002".to_owned(),
                f("BBG_SPAC", "GOLDEN PATH ACQUISITION CORP"),
            ),
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
            code("GPCO", "Golden Path Acquisition Corp", Some("US0000000002")),
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
        assert_eq!(
            bad[3].1,
            Unresolved::OutOfScope("SPAC shell (name, D19)".into()),
            "a SPAC is out of scope even when its ISIN resolves"
        );
        let now = Utc::now();
        let m = master_batch(
            &ok,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
            &|_, _| true,
            now,
        )
        .unwrap();
        assert_eq!(m.num_rows(), 2);
        let names = parse_cik_lookup(
            "MONSANTO CO:0001110783:\nMONSANTO CO /NEW/:0001110783:\n\
             LEHMAN BROTHERS HOLDINGS INC:0000806085:\nLEHMAN BROTHERS HOLDINGS:0000999999:\n:0001003197:\n",
        );
        assert_eq!(
            names.get("MONSANTO"),
            Some(&Some("0001110783".into())),
            "a former name of the same CIK"
        );
        assert_eq!(
            names.get("LEHMANBROTHERSHOLDINGS"),
            Some(&None),
            "two CIKs: ambiguous"
        );
        let col = |m: &RecordBatch, c: &str| {
            let a = m
                .column_by_name(c)
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap()
                .clone();
            (0..a.len())
                .map(|i| a.is_valid(i).then(|| a.value(i).to_owned()))
                .collect::<Vec<_>>()
        };
        let m = master_batch(
            &ok,
            &BTreeMap::new(),
            &names,
            &BTreeMap::new(),
            &|_, _| true,
            now,
        )
        .unwrap();
        assert_eq!(
            col(&m, "cik"),
            [Some("0001110783".into()), None],
            "MON by name; LEH ambiguous"
        );
        assert!(
            col(&m, "resolution_note")[0]
                .as_deref()
                .unwrap()
                .ends_with("; cik:name")
        );
        let mut active = ok[0].clone();
        active.code.delisted = false;
        let tickers = BTreeMap::from([("MON_old".to_owned(), "0000000001".to_owned())]);
        let m = master_batch(
            &[active],
            &tickers,
            &names,
            &BTreeMap::new(),
            &|_, _| true,
            now,
        )
        .unwrap();
        assert_eq!(
            col(&m, "cik"),
            [Some("0000000001".into())],
            "the current ticker's CIK wins"
        );
        let asof = NaiveDate::from_ymd_opt(2026, 10, 11);
        let sic = BTreeMap::from([(
            "0000000001".to_owned(),
            (Some("2800".to_owned()), Some("Chemicals".to_owned()), asof),
        )]);
        let with = with_sic(&m, &sic).unwrap();
        assert_eq!(col(&with, "sic"), [Some("2800".into())]);
        assert_eq!(sic_by_cik(&with), sic, "SIC survives a rebuild by CIK");
        assert_eq!(col(&with_sic(&m, &BTreeMap::new()).unwrap(), "sic"), [None]);
        let m = master_batch(
            &ok,
            &BTreeMap::new(),
            &names,
            &BTreeMap::new(),
            &|_, _| false,
            now,
        )
        .unwrap();
        assert_eq!(
            col(&m, "cik"),
            [None, None],
            "an unconfirmed name match stays null"
        );
        let d = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        let sub = br#"{"filings":{"recent":{"filingDate":["2018-03-01","2009-05-04"]},"files":[{"filingFrom":"1996-01-02","filingTo":"2009-05-01"}]}}"#;
        assert_eq!(
            submissions_span(sub),
            Some((d("1996-01-02"), d("2018-03-01")))
        );
        assert_eq!(
            submissions_span(br#"{"filings":{"recent":{"filingDate":[]},"files":[]}}"#),
            None
        );
        let filed = Some((d("2019-01-01"), d("2021-12-31")));
        assert!(
            cik_confirmed(Some((d("2005-01-03"), d("2018-06-01"))), filed),
            "within the slack year"
        );
        assert!(
            !cik_confirmed(Some((d("2005-01-03"), d("2017-06-01"))), filed),
            "another era"
        );
        assert!(
            !cik_confirmed(None, filed)
                && !cik_confirmed(Some((d("2020-01-02"), d("2020-02-03"))), None)
        );
        assert!(
            !col(&m, "resolution_note")[0]
                .as_deref()
                .unwrap()
                .contains("cik:name")
        );
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

    use arrow::array::AsArray;

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
        /// The landed EDGAR submissions zip (the filers of the training-only keys, D17); `None`
        /// gives no such key.
        pub submissions: Option<std::path::PathBuf>,
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
        /// Resolved as a rename or vendor twin of a resolved code.
        pub by_rename: usize,
        /// Given a training-only key `EOD:<code>` (no FIGI, D17).
        pub by_eod_code: usize,
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

    /// EDGAR name -> its CIKs from `cik-lookup-data.txt`, landed once a month (it changes slowly).
    async fn cik_names(
        job: &Job,
        today: NaiveDate,
    ) -> Result<BTreeMap<String, BTreeSet<String>>, RunError> {
        let chunk = format!("cik-lookup-data-{}", today.format("%Y-%m"));
        let r = Request {
            entity: "cik_lookup",
            chunk: &chunk,
            url: "https://www.sec.gov/Archives/edgar/cik-lookup-data.txt",
            ext: "txt",
            cost: 1,
            dataset: "security_master",
            empty_is_gone: false,
        };
        job.sec.fetch(&r).await?;
        Ok(
            body(&job.manifest, (job.sec.spec.id, "cik_lookup", &chunk))?
                .map(|b| super::cik_candidates(&String::from_utf8_lossy(&b)))
                .unwrap_or_default(),
        )
    }

    /// First and last filing date of `cik` from its EDGAR submissions JSON, landed once a month.
    async fn filing_span(
        job: &Job,
        cik: &str,
        today: NaiveDate,
    ) -> Result<Option<(NaiveDate, NaiveDate)>, RunError> {
        let (entity, chunk) = (
            format!("CIK{cik}"),
            format!("submissions-{}", today.format("%Y-%m")),
        );
        let url = format!("https://data.sec.gov/submissions/CIK{cik}.json");
        let r = Request {
            entity: &entity,
            chunk: &chunk,
            url: &url,
            ext: "json",
            cost: 1,
            dataset: "security_master",
            empty_is_gone: false,
        };
        if matches!(
            job.sec.fetch(&r).await?,
            Outcome::Failed(_) | Outcome::Gone(_)
        ) {
            return Ok(None);
        }
        Ok(body(&job.manifest, (job.sec.spec.id, &entity, &chunk))?
            .and_then(|b| super::submissions_span(&b)))
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
                    exchange: String::new(),
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
        let (mut ok, mut bad) = super::resolve(&codes, &by_isin, &by_cusip);
        // Renames and vendor twins, from the prices already in the landing zone (no requests).
        let unresolved: Vec<Code> = bad
            .iter()
            .filter(|(_, w)| !matches!(w, Unresolved::OutOfScope(_)))
            .map(|(c, _)| c.clone())
            .collect();
        let manifest = job.manifest.clone();
        let mut bars = |code: &str| {
            let entity = format!("{code}.US");
            let path = manifest
                .raw_path(("eodhd", &entity, "eod-full"))
                .ok()
                .flatten()?;
            landing::read(Path::new(&path))
                .ok()
                .map(|b| super::closes(&b))
        };
        let links = super::rename_links(&unresolved, &ok, &mut bars);
        let linked: BTreeSet<String> = links.iter().map(|r| r.code.code.clone()).collect();
        bad.retain(|(c, _)| !linked.contains(&c.code));
        ok.extend(links);
        let cik = ciks(job, today).await?;
        let candidates = cik_names(job, today).await?;
        let names = super::unique_ciks(&candidates);
        // Each CIK found by name must have filed while the security traded (`cik_confirmed`).
        // shortcut: one request at a time (about 25 minutes the first run of a month, then
        // landed); run them concurrently if this ever runs daily.
        let (mut priced, mut filed) = (BTreeMap::new(), BTreeMap::new());
        for r in &ok {
            if !r.code.delisted && cik.contains_key(&r.code.ticker) {
                continue;
            }
            let Some(Some(c)) = names.get(&super::name_key(&r.code.name)) else {
                continue;
            };
            if !filed.contains_key(c) {
                filed.insert(c.clone(), filing_span(job, c, today).await?);
            }
            let span =
                bars(&r.code.code).and_then(|b| Some((*b.keys().next()?, *b.keys().next_back()?)));
            priced.insert(r.code.code.clone(), span);
        }
        // Names several EDGAR filers share, and the D17 training-only keys: both checked against
        // the landed submissions zip (no requests).
        let mut subs = match &job.submissions {
            Some(p) => Some(crate::edgar::open(p).map_err(|e| RunError::Other(e.to_string()))?),
            None => None,
        };
        if subs.is_none() {
            println!(
                "identity: no submissions zip landed, no shared-name CIKs or training-only keys"
            );
        }
        let mut zip_error = None;
        let mut cache: BTreeMap<String, Option<super::Filer>> = BTreeMap::new();
        let mut filers = |c: &str| {
            cache
                .entry(c.to_owned())
                .or_insert_with(
                    || match subs.as_mut().map(|z| crate::edgar::filer_filings(z, c)) {
                        Some(Ok(f)) => f.and_then(|f| {
                            super::filer(&f.into_iter().map(|x| (x.form, x.filing_date)).collect())
                        }),
                        Some(Err(e)) => {
                            zip_error.get_or_insert(e.to_string());
                            None
                        }
                        None => None,
                    },
                )
                .clone()
        };
        let mut spans =
            |code: &str| bars(code).and_then(|b| Some((*b.keys().next()?, *b.keys().next_back()?)));
        let shared = super::shared_name_ciks(&ok, &candidates, &mut spans, &mut filers);
        let mut chosen: BTreeMap<String, String> = BTreeMap::new();
        for (code, (c, f, p)) in shared {
            priced.insert(code.clone(), Some(p));
            filed.insert(c.clone(), Some(f));
            chosen.insert(code, c);
        }
        println!(
            "identity: {} CIKs of names several filers share",
            chosen.len()
        );
        let taken: BTreeSet<String> = {
            let confirm = |r: &super::Resolved, c: &str| {
                super::cik_confirmed(
                    priced.get(&r.code.code).copied().flatten(),
                    filed.get(c).copied().flatten(),
                )
            };
            let m = super::master_batch(&ok, &cik, &names, &chosen, &confirm, Utc::now())?;
            let col = m.column_by_name("cik").map(|c| {
                c.as_string::<i32>()
                    .iter()
                    .flatten()
                    .map(str::to_owned)
                    .collect()
            });
            col.unwrap_or_default()
        };
        let unresolved_now: Vec<Code> = bad
            .iter()
            .filter(|(_, w)| matches!(w, Unresolved::NoMatch))
            .map(|(c, _)| c.clone())
            .collect();
        let keyed = super::eod_keys(
            &unresolved_now,
            &candidates,
            &taken,
            &mut spans,
            &mut filers,
        );
        if let Some(e) = zip_error {
            return Err(RunError::Other(format!("submissions zip: {e}")));
        }
        for k in &keyed {
            priced.insert(k.resolved.code.code.clone(), Some(k.priced));
            filed.insert(k.cik.clone(), Some(k.filed));
        }
        let keyed_codes: BTreeSet<&str> = keyed
            .iter()
            .map(|k| k.resolved.code.code.as_str())
            .collect();
        bad.retain(|(c, _)| !keyed_codes.contains(c.code.as_str()));
        ok.extend(keyed.into_iter().map(|k| k.resolved));
        let count = |f: fn(&super::Resolution) -> bool| ok.iter().filter(|r| f(&r.how)).count();
        s.by_isin = count(|h| matches!(h, super::Resolution::Isin(_)));
        s.by_cusip = count(|h| {
            matches!(
                h,
                super::Resolution::Cusip13f(_) | super::Resolution::Cusip13fCut(_)
            )
        });
        s.by_rename = count(|h| matches!(h, super::Resolution::Rename(_)));
        s.by_eod_code = count(|h| matches!(h, super::Resolution::EodCode(..)));
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
        let confirm = |r: &super::Resolved, c: &str| {
            super::cik_confirmed(
                priced.get(&r.code.code).copied().flatten(),
                filed.get(c).copied().flatten(),
            )
        };
        let rejected = ok
            .iter()
            .filter(|r| priced.contains_key(&r.code.code))
            .filter(|r| {
                names
                    .get(&super::name_key(&r.code.name))
                    .cloned()
                    .flatten()
                    .is_some_and(|c| !confirm(r, &c))
            })
            .count();
        println!(
            "identity: {} name-matched codes, {rejected} CIKs rejected (no filings while priced)",
            priced.len()
        );
        let master = super::master_batch(&ok, &cik, &names, &chosen, &confirm, Utc::now())?;
        // J4 owns the SIC columns (`quant-ingest edgar sic`): keep them across a rebuild.
        let old = lake.read_dataset("security_master", None, &[], None)?;
        let master = super::with_sic(&master, &super::sic_by_cik(&old))?;
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
