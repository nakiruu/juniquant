//! P5: EDGAR bulk files (`BACKFILL.md` §2, `SOURCES.md` "SEC EDGAR"): sizes and `Last-Modified`
//! of the bulk files, whether the submissions zip holds the `filings.files[]` pages (read from
//! its central directory by range requests, so the 1.5 GB file is not downloaded), the dei shares
//! facts of two multi-class issuers, the `fy`/`fp` quirk, and `sic`. Human-readable result:
//! `docs/probes/p5-edgar.md`.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use serde_json::{Value, json};

use super::{Probe, ProbeError, Reply, Source};

const SUBMISSIONS_ZIP: &str =
    "https://www.sec.gov/Archives/edgar/daily-index/bulkdata/submissions.zip";
const FACTS_ZIP: &str = "https://www.sec.gov/Archives/edgar/daily-index/xbrl/companyfacts.zip";
const TICKERS: &str = "https://www.sec.gov/files/company_tickers.json";
const CIK_LOOKUP: &str = "https://www.sec.gov/Archives/edgar/cik-lookup-data.txt";

/// Multi-class issuers (Alphabet; Berkshire Hathaway, whose A and B share one CIK).
const COMPANIES: [(&str, &str); 2] = [("GOOGL", "0001652044"), ("BRK", "0001067983")];

/// Runs P5; findings are keyed `<what>` or `<company>.<what>`.
///
/// # Errors
///
/// [`ProbeError::EnvUnset`] without `SEC_EDGAR_EMAIL`, [`ProbeError::Parse`] for a zip or JSON
/// that is not shaped as expected, and transport errors.
pub fn run(p: &mut Probe) -> Result<(), ProbeError> {
    let mut zip_size = None;
    for (label, url) in [
        ("submissions_zip", SUBMISSIONS_ZIP),
        ("companyfacts_zip", FACTS_ZIP),
        ("company_tickers", TICKERS),
        ("cik_lookup", CIK_LOOKUP),
    ] {
        let r = p.head(Source::Edgar, url)?;
        if label == "submissions_zip" {
            zip_size = header(&r, "content-length").and_then(|s| s.parse::<u64>().ok());
        }
        p.note(
            &format!("{label}.head"),
            json!({
                "status": r.status,
                "content_length": header(&r, "content-length"),
                "last_modified": header(&r, "last-modified"),
                "accept_ranges": header(&r, "accept-ranges"),
            }),
        );
    }
    let size =
        zip_size.ok_or_else(|| ProbeError::Parse("submissions.zip: no content-length".into()))?;
    zip_pages(p, size)?;
    for (label, cik) in COMPANIES {
        company(p, label, cik)?;
    }
    Ok(())
}

/// Reads the zip64 central directory of `submissions.zip` and counts the page files.
fn zip_pages(p: &mut Probe, size: u64) -> Result<(), ProbeError> {
    let tail_from = size.saturating_sub(200);
    let tail = p
        .get_range(Source::Edgar, SUBMISSIONS_ZIP, tail_from, size - 1)?
        .body;
    // The zip64 end-of-central-directory locator ("PK\x06\x07") holds the offset of the record.
    let loc = tail
        .windows(4)
        .rposition(|w| w == b"PK\x06\x07")
        .ok_or_else(|| ProbeError::Parse("zip: no zip64 locator".into()))?;
    let z64_at = le_u64(&tail, loc + 8)?;
    let z64 = p
        .get_range(Source::Edgar, SUBMISSIONS_ZIP, z64_at, z64_at + 55)?
        .body;
    let entries = le_u64(&z64, 32)?;
    let cd_size = le_u64(&z64, 40)?;
    let cd_at = le_u64(&z64, 48)?;
    let cd = p
        .get_range(Source::Edgar, SUBMISSIONS_ZIP, cd_at, cd_at + cd_size - 1)?
        .body;

    let (mut main, mut pages, mut max_page) = (0u64, 0u64, 0u64);
    let mut with_pages = HashSet::new();
    let mut listed = BTreeSet::new();
    let mut at = 0usize;
    while cd.get(at..at + 4) == Some(b"PK\x01\x02") {
        let name_len = usize::from(le_u16(&cd, at + 28)?);
        let skip =
            name_len + usize::from(le_u16(&cd, at + 30)?) + usize::from(le_u16(&cd, at + 32)?);
        let raw = cd
            .get(at + 46..at + 46 + name_len)
            .ok_or_else(|| ProbeError::Parse("zip: truncated central directory".into()))?;
        let name = String::from_utf8_lossy(raw);
        if let Some((cik, page)) = name.split_once("-submissions-") {
            pages += 1;
            max_page = max_page.max(page.trim_end_matches(".json").parse().unwrap_or(0));
            with_pages.insert(cik.to_owned());
        } else if name.starts_with("CIK") && name.ends_with(".json") {
            main += 1;
        }
        if COMPANIES.iter().any(|(_, c)| name.contains(c)) {
            listed.insert(name.into_owned());
        }
        at += 46 + skip;
    }
    p.note(
        "submissions_zip.central_directory",
        json!({
            "entries_declared": entries,
            "main_files": main,
            "page_files": pages,
            "filers_with_pages": with_pages.len(),
            "max_page_number": max_page,
            "target_filers_listed": listed,
        }),
    );
    Ok(())
}

/// One company's submissions and companyfacts JSON.
fn company(p: &mut Probe, label: &str, cik: &str) -> Result<(), ProbeError> {
    let sub = p
        .get(
            Source::Edgar,
            &format!("https://data.sec.gov/submissions/CIK{cik}.json"),
        )?
        .json()?;
    let count = |v: Option<&Value>| v.and_then(Value::as_array).map_or(0, Vec::len);
    p.note(
        &format!("{label}.submissions"),
        json!({
            "name": sub.get("name"),
            "tickers": sub.get("tickers"),
            "exchanges": sub.get("exchanges"),
            "sic": sub.get("sic"),
            "sicDescription": sub.get("sicDescription"),
            "former_names": count(sub.get("formerNames")),
            "recent_filings": count(sub.pointer("/filings/recent/accessionNumber")),
            "files": sub.pointer("/filings/files"),
        }),
    );

    let facts = p
        .get(
            Source::Edgar,
            &format!("https://data.sec.gov/api/xbrl/companyfacts/CIK{cik}.json"),
        )?
        .json()?;
    let shares = |path: &str| -> &[Value] {
        facts
            .pointer(path)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
    };
    let dei_tags: Vec<&String> = facts
        .pointer("/facts/dei")
        .and_then(Value::as_object)
        .map(|o| o.keys().collect())
        .unwrap_or_default();
    let dei = shares("/facts/dei/EntityCommonStockSharesOutstanding/units/shares");
    let gaap = shares("/facts/us-gaap/CommonStockSharesOutstanding/units/shares");
    let latest = |f: &[Value]| {
        f.iter()
            .max_by_key(|x| x.get("filed").and_then(Value::as_str))
            .cloned()
    };
    p.note(
        &format!("{label}.shares"),
        json!({
            "dei_tags": dei_tags,
            "dei_entity_common_stock_shares": { "facts": dei.len(), "latest": latest(dei) },
            "us_gaap_common_stock_shares_outstanding": { "facts": gaap.len(), "latest": latest(gaap) },
        }),
    );

    // fy/fp describe the filing: one `end` reported by several filings carries several pairs.
    let mut by_end: BTreeMap<&str, BTreeSet<(i64, &str)>> = BTreeMap::new();
    for f in gaap {
        if let (Some(end), Some(fy), Some(fp)) = (
            f.get("end").and_then(Value::as_str),
            f.get("fy").and_then(Value::as_i64),
            f.get("fp").and_then(Value::as_str),
        ) {
            by_end.entry(end).or_default().insert((fy, fp));
        }
    }
    let example = by_end
        .iter()
        .rev()
        .find(|(_, pairs)| pairs.len() > 1)
        .map(|(end, pairs)| json!({ "end": end, "fy_fp_pairs": pairs }));
    p.note(&format!("{label}.fy_fp_quirk"), json!(example));
    Ok(())
}

fn header<'a>(r: &'a Reply, name: &str) -> Option<&'a str> {
    r.headers
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

fn short_read(at: usize) -> ProbeError {
    ProbeError::Parse(format!("zip: short read at byte {at}"))
}

fn le_u64(b: &[u8], at: usize) -> Result<u64, ProbeError> {
    b.get(at..at + 8)
        .and_then(|s| s.try_into().ok())
        .map(u64::from_le_bytes)
        .ok_or_else(|| short_read(at))
}

fn le_u16(b: &[u8], at: usize) -> Result<u16, ProbeError> {
    b.get(at..at + 2)
        .and_then(|s| s.try_into().ok())
        .map(u16::from_le_bytes)
        .ok_or_else(|| short_read(at))
}

#[cfg(test)]
mod tests {
    use super::{le_u16, le_u64};

    #[test]
    fn little_endian_reads_and_short_reads() {
        assert_eq!(le_u64(&[1, 0, 0, 0, 0, 0, 0, 0, 9], 0).unwrap(), 1);
        assert_eq!(le_u16(&[0, 0, 2, 1], 2).unwrap(), 0x0102);
        assert!(le_u64(&[0; 7], 0).is_err());
    }
}
