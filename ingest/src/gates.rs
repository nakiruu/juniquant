//! Gates G7 (filings) and G8 (events) of `QUALITY-GATES.md` §2, as pure functions over rows
//! the caller reads from the lake and the landing zone. Each sub-check is one [`Outcome`]: an
//! aggregate `data_quality` row (`entity = '*'`, `chunk` = the sub-check) plus one row per
//! offending entity.
//!
//! The `validate-filings` rules (§4) live here, not in the Julia sketch
//! `scripts/validate-filings.jl`, so the gate runner is one program. Ingestion may name
//! `period_end` (`POINT-IN-TIME.md` "period_end exception"); these rules only validate it.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::NaiveDate;

use crate::edgar::IndexLine;
use crate::infra::quality::Level;

/// One sub-check's result.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// Sub-check id, the `chunk` of its rows (`validate_filings.r4`, `index_misses`, ...).
    pub chunk: &'static str,
    /// `Fail` only for a HARD check that failed or could not run.
    pub level: Level,
    /// The value measured; `None` when the check could not run.
    pub measured: Option<f64>,
    /// The threshold it was compared with.
    pub threshold: Option<f64>,
    /// What was measured, for the aggregate row.
    pub note: String,
    /// `(entity, note)` of each offender.
    pub offenders: Vec<(String, String)>,
}

/// The `fundamentals` columns the G7 rules read.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FilingRow {
    /// Security.
    pub figi: String,
    /// Accession.
    pub accession: String,
    /// The point-in-time stamp.
    pub filing_date: Option<NaiveDate>,
    /// Period end (validated, never used to decide).
    pub period_end: Option<NaiveDate>,
    /// Form type.
    pub form: String,
    /// `Q1` / `Q2` / `Q3` / `FY`.
    pub fiscal_period: Option<String>,
    /// `acceptance_datetime` as an Eastern date.
    pub acceptance_et: Option<NaiveDate>,
    /// `amends_accession`.
    pub amends: Option<String>,
    /// `revenue_ttm`.
    pub revenue_ttm: Option<f64>,
    /// `tag_used.revenues`.
    pub revenue_tag: Option<String>,
}

/// XBRL phase-in: the start of the G7 checks that need XBRL facts.
#[must_use]
pub fn xbrl_start() -> NaiveDate {
    NaiveDate::from_ymd_opt(2011, 7, 1).unwrap_or_default()
}

fn count(chunk: &'static str, hard: bool, what: &str, offenders: Vec<(String, String)>) -> Outcome {
    let n = offenders.len();
    Outcome {
        chunk,
        level: match (n, hard) {
            (0, _) => Level::Ok,
            (_, true) => Level::Fail,
            (_, false) => Level::Warn,
        },
        measured: Some(n as f64),
        threshold: Some(0.0),
        note: format!("{n} {what}"),
        offenders,
    }
}

fn is_periodic(form: &str) -> bool {
    matches!(form, "10-K" | "10-Q" | "10-KT")
}

/// `QUALITY-GATES.md` §4 rules 1-8, in order; a row that fails rule 1 is skipped by the rest.
#[must_use]
pub fn validate_filings(
    rows: &[FilingRow],
    as_of: NaiveDate,
    cal: &calendar::Calendar,
) -> Vec<Outcome> {
    let off = |r: &FilingRow, why: String| (r.figi.clone(), format!("{} {why}", r.accession));
    let r1: Vec<_> = rows
        .iter()
        .filter(|r| r.filing_date.is_none() || r.period_end.is_none())
        .map(|r| off(r, "null filing_date or period_end".into()))
        .collect();
    // (row, filing_date, period_end) for rows with both.
    let dated: Vec<_> = rows
        .iter()
        .filter_map(|r| Some((r, r.filing_date?, r.period_end?)))
        .collect();
    let r2 = dated
        .iter()
        .filter(|(_, f, p)| f < p)
        .map(|(r, f, p)| off(r, format!("filed {f} before period end {p}")))
        .collect();
    let r3 = dated
        .iter()
        .filter(|(_, f, _)| *f > as_of)
        .map(|(r, f, _)| off(r, format!("filed {f} after {as_of}")))
        .collect();
    let mut seen = HashSet::new();
    let r4 = rows
        .iter()
        .filter(|r| !seen.insert((&r.figi, &r.accession)))
        .map(|r| off(r, "duplicate (figi, accession)".into()))
        .collect();
    let accessions: HashSet<&str> = rows.iter().map(|r| r.accession.as_str()).collect();
    let r5 = rows
        .iter()
        .filter_map(|r| {
            let a = r.amends.as_deref()?;
            (!accessions.contains(a)).then(|| off(r, format!("amends unknown {a}")))
        })
        .collect();
    let r6 = rows
        .iter()
        .filter_map(|r| {
            let p = r.fiscal_period.as_deref()?;
            (!matches!(p, "Q1" | "Q2" | "Q3" | "FY")).then(|| off(r, format!("fiscal_period {p}")))
        })
        .collect();
    let r7 = dated
        .iter()
        .filter_map(|(r, f, _)| {
            let a = r.acceptance_et?;
            let before = cal.prev_trading_day(*f).unwrap_or(*f);
            (a != *f && a != before).then(|| off(r, format!("accepted {a}, filed {f}")))
        })
        .collect();
    let mut by_figi: BTreeMap<&str, Vec<NaiveDate>> = BTreeMap::new();
    for (r, f, _) in dated.iter().filter(|(r, _, _)| is_periodic(&r.form)) {
        by_figi.entry(&r.figi).or_default().push(*f);
    }
    let mut r8 = Vec::new();
    for (figi, mut ds) in by_figi {
        ds.sort_unstable();
        for w in ds.windows(2) {
            if let [a, b] = w
                && (*b - *a).num_days() > 120
            {
                r8.push((figi.to_owned(), format!("no periodic filing {a} to {b}")));
            }
        }
    }
    vec![
        count(
            "validate_filings.r1",
            true,
            "rows with a null filing_date or period_end",
            r1,
        ),
        count(
            "validate_filings.r2",
            true,
            "rows filed before their period end",
            r2,
        ),
        count(
            "validate_filings.r3",
            true,
            "rows filed after the validation date",
            r3,
        ),
        count(
            "validate_filings.r4",
            true,
            "duplicate (figi, accession) rows",
            r4,
        ),
        count(
            "validate_filings.r5",
            true,
            "amendments of an unknown accession",
            r5,
        ),
        count(
            "validate_filings.r6",
            true,
            "rows with an unknown fiscal_period",
            r6,
        ),
        count(
            "validate_filings.r7",
            false,
            "acceptances off the filing day",
            r7,
        ),
        count(
            "validate_filings.r8",
            false,
            "gaps over 120 days between periodic filings",
            r8,
        ),
    ]
}

/// Staleness: for eligible `(date, figi)` from 2011-07-01, the latest `filing_date <= date` is
/// at most 135 days old, for at least 97% of them. FAIL when `universe` is empty (not built).
#[must_use]
pub fn staleness(eligible: &[(NaiveDate, String)], rows: &[FilingRow]) -> Outcome {
    let mut by_figi: HashMap<&str, Vec<NaiveDate>> = HashMap::new();
    for r in rows {
        if let Some(f) = r.filing_date {
            by_figi.entry(&r.figi).or_default().push(f);
        }
    }
    by_figi.values_mut().for_each(|v| v.sort_unstable());
    let mut stale: BTreeMap<&str, usize> = BTreeMap::new();
    let mut n = 0usize;
    for (d, figi) in eligible.iter().filter(|(d, _)| *d >= xbrl_start()) {
        n += 1;
        let fds = by_figi
            .get(figi.as_str())
            .map(Vec::as_slice)
            .unwrap_or_default();
        let fresh = fds
            .partition_point(|f| f <= d)
            .checked_sub(1)
            .and_then(|i| fds.get(i))
            .is_some_and(|f| (*d - *f).num_days() <= 135);
        if !fresh {
            *stale.entry(figi).or_default() += 1;
        }
    }
    let threshold = 0.97;
    if n == 0 {
        return Outcome {
            chunk: "staleness",
            level: Level::Fail,
            measured: None,
            threshold: Some(threshold),
            note: "no eligible universe rows since 2011-07-01 (universe not built, J9)".into(),
            offenders: Vec::new(),
        };
    }
    let fresh = 1.0 - stale.values().sum::<usize>() as f64 / n as f64;
    Outcome {
        chunk: "staleness",
        level: if fresh >= threshold {
            Level::Ok
        } else {
            Level::Fail
        },
        measured: Some(fresh),
        threshold: Some(threshold),
        note: format!("{:.2}% of {n} eligible ticker-days fresh", fresh * 100.0),
        offenders: stale
            .into_iter()
            .map(|(f, k)| (f.to_owned(), format!("{k} stale eligible days")))
            .collect(),
    }
}

/// Index misses: 10-K/10-Q lines of the quarterly `form.idx` from 2011-07-01 for `ciks` whose
/// accession has no `fundamentals` row; at most 1%.
#[must_use]
pub fn index_misses(lines: &[IndexLine], ciks: &HashSet<String>, rows: &[FilingRow]) -> Outcome {
    let have: HashSet<&str> = rows.iter().map(|r| r.accession.as_str()).collect();
    let wanted: Vec<_> = lines
        .iter()
        .filter(|l| matches!(l.form.as_str(), "10-K" | "10-Q"))
        .filter(|l| l.date >= xbrl_start() && ciks.contains(&l.cik))
        .collect();
    let missing: Vec<_> = wanted
        .iter()
        .filter(|l| !have.contains(l.accession.as_str()))
        .map(|l| {
            (
                l.cik.clone(),
                format!("{} {} {}", l.accession, l.form, l.date),
            )
        })
        .collect();
    let share = missing.len() as f64 / wanted.len().max(1) as f64;
    let threshold = 0.01;
    Outcome {
        chunk: "index_misses",
        level: if !wanted.is_empty() && share <= threshold {
            Level::Ok
        } else {
            Level::Fail
        },
        measured: Some(share),
        threshold: Some(threshold),
        note: format!(
            "{} of {} indexed 10-K/10-Q since 2011-07-01 without a row ({:.2}%)",
            missing.len(),
            wanted.len(),
            share * 100.0
        ),
        offenders: missing,
    }
}

/// Revenue-tag switches with a level jump above 30% (consecutive rows of a security with a
/// `revenue_ttm`), listed; WARN.
#[must_use]
pub fn tag_switches(rows: &[FilingRow]) -> Outcome {
    let mut by_figi: BTreeMap<&str, Vec<(NaiveDate, &str, f64, &str)>> = BTreeMap::new();
    for r in rows {
        if let (Some(f), Some(t), Some(v)) = (r.filing_date, &r.revenue_tag, r.revenue_ttm) {
            by_figi
                .entry(&r.figi)
                .or_default()
                .push((f, t, v, &r.accession));
        }
    }
    let mut out = Vec::new();
    for (figi, mut v) in by_figi {
        v.sort_by_key(|x| x.0);
        for w in v.windows(2) {
            if let [a, b] = w
                && a.1 != b.1
                && a.2 != 0.0
                && (b.2 / a.2 - 1.0).abs() > 0.3
            {
                out.push((
                    figi.to_owned(),
                    format!("{} {} -> {}: {:.0} -> {:.0}", b.3, a.1, b.1, a.2, b.2),
                ));
            }
        }
    }
    count(
        "tag_switches",
        false,
        "revenue-tag switches with a jump above 30%",
        out,
    )
}

/// Share of rows from 2011-07-01 with a null `revenue_ttm`; reported, no threshold.
#[must_use]
pub fn ttm_nulls(rows: &[FilingRow]) -> Outcome {
    let since: Vec<_> = rows
        .iter()
        .filter(|r| r.filing_date.is_some_and(|f| f >= xbrl_start()))
        .collect();
    let null = since.iter().filter(|r| r.revenue_ttm.is_none()).count();
    let share = null as f64 / since.len().max(1) as f64;
    Outcome {
        chunk: "ttm_nulls",
        level: Level::Ok,
        measured: Some(share),
        threshold: None,
        note: format!(
            "{null} of {} rows since 2011-07-01 without revenue_ttm",
            since.len()
        ),
        offenders: Vec::new(),
    }
}

/// G8: CIKs of `security_master` absent from the submissions zip.
#[must_use]
pub fn missing_submissions(ciks: &HashSet<String>, in_zip: &HashSet<String>) -> Outcome {
    let mut out: Vec<_> = ciks
        .difference(in_zip)
        .map(|c| (c.clone(), "no submissions entry".to_owned()))
        .collect();
    out.sort();
    count(
        "submissions",
        false,
        "CIKs without a submissions entry",
        out,
    )
}

/// G8: the share of 8-K filings (by accession) with at least one item, at least 99%.
/// `items` is `(accession, item)` of the 8-K and 8-K/A rows in scope.
#[must_use]
pub fn items_share(items: &[(String, String)]) -> Outcome {
    let mut has: HashMap<&str, bool> = HashMap::new();
    for (a, i) in items {
        *has.entry(a).or_default() |= i != "-";
    }
    let with = has.values().filter(|h| **h).count();
    let share = with as f64 / has.len().max(1) as f64;
    let threshold = 0.99;
    Outcome {
        chunk: "items",
        level: if !has.is_empty() && share >= threshold {
            Level::Ok
        } else {
            Level::Warn
        },
        measured: Some(share),
        threshold: Some(threshold),
        note: format!(
            "{with} of {} 8-Ks with items ({:.3}%)",
            has.len(),
            share * 100.0
        ),
        offenders: Vec::new(),
    }
}

/// G8: securities whose price series ends before `cutoff` (10 trading days before the data
/// end) with no `delistings` row and no `price_coverage` gap row.
#[must_use]
pub fn unexplained_ends(
    last: &HashMap<String, NaiveDate>,
    cutoff: NaiveDate,
    explained: &HashSet<String>,
) -> Outcome {
    let mut out: Vec<_> = last
        .iter()
        .filter(|(f, d)| **d < cutoff && !explained.contains(*f))
        .map(|(f, d)| (f.clone(), format!("prices end {d}")))
        .collect();
    out.sort();
    count(
        "ends_explained",
        false,
        "ended series with no delisting or gap row",
        out,
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing, reason = "test")]

    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn row(acc: &str, filed: &str, end: &str) -> FilingRow {
        FilingRow {
            figi: "F".into(),
            accession: acc.into(),
            filing_date: Some(d(filed)),
            period_end: Some(d(end)),
            form: "10-Q".into(),
            fiscal_period: Some("Q1".into()),
            acceptance_et: Some(d(filed)),
            ..FilingRow::default()
        }
    }

    #[test]
    fn each_rule_catches_its_defect_only() {
        let cal = calendar::Calendar::rules_only();
        let good = || {
            vec![
                row("a", "2024-02-05", "2023-12-31"),
                row("b", "2024-05-06", "2024-03-31"),
            ]
        };
        let fired = |rows: &[FilingRow]| -> Vec<&'static str> {
            validate_filings(rows, d("2026-10-12"), &cal)
                .into_iter()
                .filter(|o| !o.offenders.is_empty())
                .map(|o| o.chunk)
                .collect()
        };
        assert!(fired(&good()).is_empty());
        let plant = |f: &dyn Fn(&mut Vec<FilingRow>)| {
            let mut v = good();
            f(&mut v);
            fired(&v)
        };
        // The null row is skipped by the later rules; the gap rule sees one filing left.
        assert_eq!(plant(&|v| v[1].period_end = None), ["validate_filings.r1"]);
        assert_eq!(
            plant(&|v| v[1].period_end = Some(d("2024-06-30"))),
            ["validate_filings.r2"]
        );
        assert_eq!(
            plant(&|v| v.push(row("c", "2027-01-01", "2026-12-31"))),
            ["validate_filings.r3", "validate_filings.r8"]
        );
        assert_eq!(plant(&|v| v.push(v[1].clone())), ["validate_filings.r4"]);
        assert_eq!(
            plant(&|v| v[1].amends = Some("zz".into())),
            ["validate_filings.r5"]
        );
        assert_eq!(
            plant(&|v| v[1].fiscal_period = Some("Q4".into())),
            ["validate_filings.r6"]
        );
        // Accepted the Friday before a Monday filing date: allowed; two days earlier: not.
        assert!(plant(&|v| v[1].acceptance_et = Some(d("2024-05-03"))).is_empty());
        assert_eq!(
            plant(&|v| v[1].acceptance_et = Some(d("2024-05-02"))),
            ["validate_filings.r7"]
        );
        assert_eq!(
            plant(&|v| {
                v[1].filing_date = Some(d("2024-09-06"));
                v[1].acceptance_et = v[1].filing_date;
            }),
            ["validate_filings.r8"]
        );
    }

    #[test]
    fn staleness_and_index_misses() {
        let rows = vec![row("a", "2024-02-05", "2023-12-31")];
        let e = |s: &str| (d(s), "F".to_owned());
        let o = staleness(&[e("2024-02-05"), e("2024-06-19"), e("2024-06-20")], &rows);
        assert!((o.measured.unwrap() - 2.0 / 3.0).abs() < 1e-12);
        assert_eq!(o.level, Level::Fail);
        assert_eq!(staleness(&[], &rows).level, Level::Fail);

        let line = |acc: &str, form: &str| IndexLine {
            form: form.into(),
            cik: "0000000001".into(),
            date: d("2024-02-05"),
            accession: acc.into(),
        };
        let ciks = HashSet::from(["0000000001".to_owned()]);
        let o = index_misses(
            &[line("a", "10-Q"), line("x", "10-K"), line("y", "8-K")],
            &ciks,
            &rows,
        );
        assert_eq!((o.measured, o.level), (Some(0.5), Level::Fail));
        assert_eq!(o.offenders.len(), 1);
    }

    #[test]
    fn g8_checks() {
        let it = |a: &str, i: &str| (a.to_owned(), i.to_owned());
        let o = items_share(&[it("a", "2.02"), it("a", "9.01"), it("b", "-")]);
        assert_eq!((o.measured, o.level), (Some(0.5), Level::Warn));
        let last = HashMap::from([
            ("A".to_owned(), d("2020-01-02")),
            ("B".to_owned(), d("2020-01-02")),
            ("C".to_owned(), d("2026-10-09")),
        ]);
        let o = unexplained_ends(&last, d("2026-09-25"), &HashSet::from(["A".to_owned()]));
        assert_eq!(
            o.offenders,
            vec![("B".to_owned(), "prices end 2020-01-02".to_owned())]
        );
    }
}
