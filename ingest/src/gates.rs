//! Gates G0 (calendar), G2 (price sanity), G7 (filings) and G8 (events) of `QUALITY-GATES.md` §2, as pure functions over rows
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

/// The 10-K/10-Q lines of the quarterly `form.idx` from 2011-07-01 for `ciks` whose accession
/// has no `fundamentals` row, and the number of lines checked.
#[must_use]
pub fn index_unmatched<'a>(
    lines: &'a [IndexLine],
    ciks: &HashSet<String>,
    rows: &[FilingRow],
) -> (Vec<&'a IndexLine>, usize) {
    let have: HashSet<&str> = rows.iter().map(|r| r.accession.as_str()).collect();
    let wanted: Vec<_> = lines
        .iter()
        .filter(|l| matches!(l.form.as_str(), "10-K" | "10-Q"))
        .filter(|l| l.date >= xbrl_start() && ciks.contains(&l.cik))
        .collect();
    let n = wanted.len();
    (
        wanted
            .into_iter()
            .filter(|l| !have.contains(l.accession.as_str()))
            .collect(),
        n,
    )
}

/// Every accession that reported a fact in a `companyfacts` JSON
/// (`facts.{taxonomy}.{tag}.units.{unit}[].accn`).
#[must_use]
pub fn fact_accessions(v: &serde_json::Value) -> HashSet<String> {
    fn obj(v: Option<&serde_json::Value>) -> Vec<&serde_json::Value> {
        v.and_then(serde_json::Value::as_object)
            .into_iter()
            .flat_map(serde_json::Map::values)
            .collect()
    }
    obj(v.get("facts"))
        .into_iter()
        .flat_map(|tax| obj(Some(tax)))
        .flat_map(|tag| obj(tag.get("units")))
        .filter_map(serde_json::Value::as_array)
        .flatten()
        .filter_map(|f| f.get("accn")?.as_str().map(str::to_owned))
        .collect()
}

/// Index misses (D23), from [`index_unmatched`]: a filing whose accession has XBRL facts
/// (`with_facts`) but no row is an ingestion loss, HARD at most 1% of `checked`; one with no
/// facts at the source is listed as WARN (its cost shows in `staleness`).
#[must_use]
pub fn index_misses(
    unmatched: &[&IndexLine],
    checked: usize,
    with_facts: &HashSet<String>,
) -> [Outcome; 2] {
    let off = |l: &&IndexLine| {
        (
            l.cik.clone(),
            format!("{} {} {}", l.accession, l.form, l.date),
        )
    };
    let (lost, absent): (Vec<_>, Vec<_>) = unmatched
        .iter()
        .partition(|l| with_facts.contains(&l.accession));
    let share = lost.len() as f64 / checked.max(1) as f64;
    let threshold = 0.01;
    let mut no_xbrl = count(
        "index_no_xbrl",
        false,
        "indexed 10-K/10-Q since 2011-07-01 with no XBRL facts at the source",
        absent.iter().map(off).collect(),
    );
    no_xbrl.measured = Some(absent.len() as f64 / checked.max(1) as f64);
    no_xbrl.threshold = None;
    [
        Outcome {
            chunk: "index_misses",
            level: if checked > 0 && share <= threshold {
                Level::Ok
            } else {
                Level::Fail
            },
            measured: Some(share),
            threshold: Some(threshold),
            note: format!(
                "{} of {checked} indexed 10-K/10-Q since 2011-07-01 have XBRL facts but no row ({:.3}%)",
                lost.len(),
                share * 100.0
            ),
            offenders: lost.iter().map(off).collect(),
        },
        no_xbrl,
    ]
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

/// G0: the calendar's trading days in `[from, to]` against the dates of the SPY bars. Either
/// side alone is a differing date (HARD, 0 allowed); the calendar is never rebuilt from bars.
#[must_use]
pub fn calendar_match(
    cal: &calendar::Calendar,
    spy: &std::collections::BTreeSet<NaiveDate>,
    from: NaiveDate,
    to: NaiveDate,
) -> Outcome {
    let mut out = Vec::new();
    let mut d = from;
    while d <= to {
        match (cal.is_trading_day(d), spy.contains(&d)) {
            (true, false) => {
                out.push(("SPY".to_owned(), format!("{d} calendar day without a bar")))
            }
            (false, true) => out.push(("SPY".to_owned(), format!("{d} bar on a closed day"))),
            _ => {}
        }
        d = d.succ_opt().unwrap_or(NaiveDate::MAX);
    }
    let mut o = count(
        "calendar",
        true,
        "dates differ between the calendar and SPY",
        out,
    );
    if spy.is_empty() {
        o.level = Level::Fail;
        o.measured = None;
        o.note = "no SPY bars".into();
    }
    o
}

/// One `prices` row for G2; a null price is NaN and a null volume is -1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bar {
    /// Index of the security's FIGI in the caller's name list.
    pub sec: u32,
    /// Trading day.
    pub date: NaiveDate,
    /// As traded.
    pub open: f64,
    /// As traded.
    pub high: f64,
    /// As traded.
    pub low: f64,
    /// As traded.
    pub close: f64,
    /// As traded; -1 when null.
    pub volume: i64,
    /// The vendor's adjusted close.
    pub adj: f64,
}

/// `(ex_date, value)` per security index: split ratios or dividends as paid, sorted.
pub type Events = HashMap<u32, Vec<(NaiveDate, f64)>>;

/// A day traded above this many dollars is a vendor unit error: over three times the largest real
/// one in the data (Tesla's S&P inclusion, 2020-12-18, $154bn).
///
/// shortcut: a fixed dollar level; raise it if real days approach it.
const MAX_DOLLAR_VOLUME: f64 = 5e11;

fn bad_row(b: &Bar) -> Option<&'static str> {
    let pos = [b.open, b.high, b.low, b.close]
        .iter()
        .all(|p| p.is_finite() && *p > 0.0);
    if !pos {
        Some("price not positive")
    } else if b.high < b.low {
        Some("high < low")
    } else if b.close < b.low || b.close > b.high {
        Some("close outside [low, high]")
    } else if b.volume < 0 {
        Some("volume negative or null")
    } else if b.close * b.volume as f64 > MAX_DOLLAR_VOLUME {
        Some("dollar volume over $500bn")
    } else {
        None
    }
}

/// Product of the ratios with `a < ex <= b`.
fn rho(splits: &[(NaiveDate, f64)], a: NaiveDate, b: NaiveDate) -> f64 {
    splits
        .iter()
        .filter(|(ex, _)| a < *ex && *ex <= b)
        .map(|(_, r)| r)
        .product()
}

fn priced(b: &Bar) -> bool {
    b.close.is_finite() && b.close > 0.0
}

/// A split with `ex_date` from the previous bar `a` to one day after the bar `b`.
fn explained(splits: &[(NaiveDate, f64)], a: NaiveDate, b: NaiveDate) -> bool {
    splits
        .iter()
        .any(|(ex, _)| a <= *ex && *ex <= b + chrono::Days::new(1))
}

/// The ten-fold close jumps of one security's sorted bars that no split explains: the index
/// of the later bar and the close ratio.
#[must_use]
pub fn ten_fold_jumps(s: &[Bar], splits: &[(NaiveDate, f64)]) -> Vec<(usize, f64)> {
    s.windows(2)
        .enumerate()
        .filter_map(|(k, w)| {
            let [p, b] = w else { return None };
            let r = b.close / p.close;
            (priced(p)
                && priced(b)
                && (r >= 10.0 || r <= 0.1)
                && !explained(splits, p.date, b.date))
            .then_some((k + 1, r))
        })
        .collect()
}

/// What G2 classifies ten-fold jumps with (`QUALITY-GATES.md` G2), by security index.
#[derive(Debug, Clone, Default)]
pub struct JumpContext {
    /// Jump bars the owner accepted as real moves (`config/price_reviews.toml`).
    pub accepted: HashSet<(u32, NaiveDate)>,
    /// Bar ranges the owner rejected.
    pub excluded: HashMap<u32, Vec<(NaiveDate, NaiveDate)>>,
    /// Sorted dates of eligible `universe` rows; `None` without a universe (all in scope).
    pub eligible: Option<HashMap<u32, Vec<NaiveDate>>>,
    /// Sorted 8-K Item 5.03 filing dates.
    pub item503: HashMap<u32, Vec<NaiveDate>>,
    /// Sorted `filing_events` dates of the security's filer.
    pub filed: HashMap<u32, Vec<NaiveDate>>,
    /// `(filing_date, shares_outstanding)` sorted by filing date.
    pub shares: HashMap<u32, Vec<(NaiveDate, f64)>>,
}

/// Days on which two securities close at the same price that make them twins.
const TWIN_DAYS: usize = 120;

/// Share of either twin's bars in the shared range that must close at the same price.
const TWIN_SHARE: f64 = 0.9;

/// Twins (owner, 2026-10-13): two securities that close at the same price (to $0.0001, over
/// $0.50) on `TWIN_DAYS` days or more, `TWIN_SHARE` of the bars of each between the first
/// and last such day. The vendor kept an old code and copied its history to the new one
/// (HollyFrontier HFC and HF Sinclair DINO, 2011-2022), so training would see those days twice.
/// Per security index, the ranges G2 rejects (`twin`): the copy held by the code that trades on
/// past the shared range (the successor's), else by the one whose filer filed less there
/// (`filed`), else by the later name.
#[must_use]
pub fn twins(
    bars: &[Bar],
    names: &[String],
    filed: &HashMap<u32, Vec<NaiveDate>>,
) -> HashMap<u32, Vec<(NaiveDate, NaiveDate)>> {
    let mut keys: Vec<(NaiveDate, i64, u32)> = bars
        .iter()
        .filter(|b| b.close.is_finite() && b.close > 0.5)
        .map(|b| (b.date, (b.close * 1e4).round() as i64, b.sec))
        .collect();
    keys.sort_unstable();
    let mut pairs: HashMap<(u32, u32), (usize, NaiveDate, NaiveDate)> = HashMap::new();
    // shortcut: a price eight securities share that day is a round number, not a twin.
    for g in keys
        .chunk_by(|a, b| a.0 == b.0 && a.1 == b.1)
        .filter(|g| (2..=8).contains(&g.len()))
    {
        for (i, a) in g.iter().enumerate() {
            for b in g.iter().skip(i + 1) {
                let e = pairs
                    .entry((a.2.min(b.2), a.2.max(b.2)))
                    .or_insert((0, a.0, a.0));
                e.0 += 1;
                e.1 = e.1.min(a.0);
                e.2 = e.2.max(a.0);
            }
        }
    }
    let series: HashMap<u32, &[Bar]> = bars
        .chunk_by(|a, b| a.sec == b.sec)
        .filter_map(|c| Some((c.first()?.sec, c)))
        .collect();
    let within = |s: &[Bar], a: NaiveDate, z: NaiveDate| {
        s.partition_point(|x| x.date <= z) - s.partition_point(|x| x.date < a)
    };
    let mut out: HashMap<u32, Vec<(NaiveDate, NaiveDate)>> = HashMap::new();
    for (&(a, b), &(n, d0, d1)) in &pairs {
        let (Some(sa), Some(sb)) = (series.get(&a), series.get(&b)) else {
            continue;
        };
        let most = within(sa, d0, d1).max(within(sb, d0, d1));
        if n < TWIN_DAYS || (n as f64) < TWIN_SHARE * most as f64 {
            continue;
        }
        let last = |s: &[Bar]| s.last().map_or(NaiveDate::MIN, |x| x.date);
        let filings = |sec: u32| {
            filed.get(&sec).map_or(0, |v| {
                v.partition_point(|x| *x <= d1) - v.partition_point(|x| *x < d0)
            })
        };
        let name = |sec: u32| names.get(sec as usize).map_or("", String::as_str);
        let on = (last(sa) - last(sb)).num_days();
        let copy = if on.abs() > 5 {
            if on > 0 { a } else { b }
        } else {
            match filings(a).cmp(&filings(b)) {
                std::cmp::Ordering::Less => a,
                std::cmp::Ordering::Greater => b,
                std::cmp::Ordering::Equal => {
                    if name(a) > name(b) {
                        a
                    } else {
                        b
                    }
                }
            }
        };
        out.entry(copy).or_default().push((d0, d1));
    }
    for v in out.values_mut() {
        v.sort_unstable();
    }
    out
}

/// An inclusive range of bar indices G2 rejects, with its reason.
type Span = (usize, usize, &'static str);

/// A jump: later-bar index, close ratio, and the reason of the span that explains it.
type Jump = (usize, f64, Option<&'static str>);

/// The closes around `ex` move by the split (the G2 `splits` test passes).
fn split_shows(s: &[Bar], ex: NaiveDate, ratio: f64) -> bool {
    let b = s.partition_point(|x| x.date < ex);
    match (b.checked_sub(1).and_then(|p| s.get(p)), s.get(b)) {
        (Some(p), Some(b)) => (0.4..=2.5).contains(&(b.close / p.close * ratio)),
        _ => false,
    }
}

fn median_close(s: &[Bar]) -> f64 {
    let mut v: Vec<f64> = s.iter().filter(|b| priced(b)).map(|b| b.close).collect();
    v.sort_by(f64::total_cmp);
    v.get(v.len() / 2).copied().unwrap_or(f64::NAN)
}

fn covering(spans: &[Span], k: usize) -> Option<&'static str> {
    spans
        .iter()
        .find(|(a, b, _)| (*a..=*b).contains(&k) || (*a..=*b).contains(&(k - 1)))
        .map(|x| x.2)
}

/// The rejected spans of one security and its jumps (`ten_fold_jumps` less the accepted ones),
/// each with the reason of the span that explains it, if any. Rules, in order: owner
/// exclusions (`reviewed`); the copy of a twin's history (`twin`, the ranges [`twins`] gives);
/// blocks the vendor scaled for later splits (the size of the jump
/// equals their product within 8%, `vendor_adjusted`: up to the move back by the same factor,
/// else before the jump when the split shows in the closes, else from the jump to the split); a jump undone by an opposite one within 20 bars
/// marks the bars between (`foreign_quotes`), and with two such flips the series is mixed:
/// the segments between jumps priced more than 3.16x off the segment with the most filings
/// of the filer (then the longest) are another
/// holder's (`vendor_wrong_holder`); a jump within 20 bars of an end drops that stub, and one
/// after a gap of more than 30 days drops the side without the filer's filings
/// (`symbol_reused`), only the jump bar when it files on both (`relisted`), else the shorter.
fn classify(
    s: &[Bar],
    sp: &[(NaiveDate, f64)],
    sec: u32,
    cx: &JumpContext,
    twin: &[(NaiveDate, NaiveDate)],
) -> (Vec<Span>, Vec<Jump>) {
    let n = s.len();
    let at = |k: usize| s.get(k).map_or_else(NaiveDate::default, |b| b.date);
    let mut spans: Vec<Span> = Vec::new();
    let reviewed = cx.excluded.get(&sec).into_iter().flatten();
    for (&(a, b), why) in reviewed
        .map(|r| (r, "reviewed"))
        .chain(twin.iter().map(|r| (r, "twin")))
    {
        let (i, j) = (
            s.partition_point(|x| x.date < a),
            s.partition_point(|x| x.date <= b),
        );
        if i < j {
            spans.push((i, j - 1, why));
        }
    }
    let jumps: Vec<(usize, f64)> = ten_fold_jumps(s, sp)
        .into_iter()
        .filter(|(k, _)| !cx.accepted.contains(&(sec, at(*k))))
        .collect();
    for (i, &(k, r)) in jumps.iter().enumerate() {
        if covering(&spans, k).is_some() {
            continue;
        }
        let mut f = 1.0;
        for (ex, ratio) in sp.iter().filter(|(ex, _)| *ex > at(k)) {
            f *= ratio;
            // The vendor scales a block by `f` or by `1 / f`, so the size of the jump is the test.
            if (r.ln().abs() - f.ln().abs()).abs() >= 0.08 {
                continue;
            }
            let split_at = s.partition_point(|x| x.date < *ex);
            // The move back by the same factor (with a day's noise, so not always ten-fold).
            let back = (k + 1..split_at).find(|&j| match (s.get(j - 1), s.get(j)) {
                (Some(p), Some(b)) => {
                    priced(p) && priced(b) && ((b.close / p.close).ln() + r.ln()).abs() < 0.16
                }
                _ => false,
            });
            let (a, b) = match back {
                Some(j) => (k, j - 1),
                // Alone: when the split shows in the closes, the bars up to it are as traded
                // and the block is the one before the jump; else the block runs to the split.
                None if split_shows(s, *ex, *ratio) => (
                    i.checked_sub(1)
                        .and_then(|j| jumps.get(j))
                        .map_or(0, |x| x.0),
                    k - 1,
                ),
                None => (k, jumps.get(i + 1).map_or(n, |x| x.0).min(split_at) - 1),
            };
            spans.push((a, b, "vendor_adjusted"));
            break;
        }
    }
    let open: Vec<(usize, f64)> = jumps
        .iter()
        .filter(|(k, _)| covering(&spans, *k).is_none())
        .copied()
        .collect();
    let filed = cx.filed.get(&sec).map(Vec::as_slice).unwrap_or_default();
    let filings = |a: usize, b: usize| {
        filed.partition_point(|d| *d <= at(b)) - filed.partition_point(|d| *d < at(a))
    };
    let has_filing = |a: usize, b: usize| filings(a, b) > 0;
    let flips: Vec<(usize, usize)> = open
        .iter()
        .enumerate()
        .filter_map(|(i, &(k, r))| {
            let later = open.get(i + 1..)?;
            let back = later
                .iter()
                .take_while(|x| x.0 <= k + 20)
                .find(|x| (x.1 > 1.0) != (r > 1.0))?;
            Some((k, back.0 - 1))
        })
        .collect();
    if flips.len() >= 2 {
        // A mixed series: the segments between open jumps priced off the holder's level.
        let cuts: Vec<usize> = std::iter::once(0)
            .chain(open.iter().map(|j| j.0))
            .chain(std::iter::once(n))
            .collect();
        let segs: Vec<(usize, usize, f64)> = cuts
            .windows(2)
            .filter_map(|w| match w {
                [a, b] => Some((*a, *b, median_close(s.get(*a..*b)?))),
                _ => None,
            })
            .collect();
        // The holder's level: the segment with the most of the filer's filings, then the longest.
        let home = segs
            .iter()
            .max_by_key(|x| (filings(x.0, x.1 - 1), x.1 - x.0))
            .map_or(f64::NAN, |x| x.2);
        spans.extend(
            segs.iter()
                .filter(|x| {
                    let off = (x.2 / home).log10().abs();
                    off.is_nan() || off >= 0.5
                })
                .map(|x| (x.0, x.1 - 1, "vendor_wrong_holder")),
        );
    } else {
        spans.extend(flips.iter().map(|&(a, b)| (a, b, "foreign_quotes")));
    }
    for &(k, _) in &open {
        if covering(&spans, k).is_some() {
            continue;
        }
        let stub = k <= 20 || k + 20 >= n;
        if !stub && (at(k) - at(k - 1)).num_days() <= 30 {
            continue;
        }
        let shorter = if k < n - k { (0, k - 1) } else { (k, n - 1) };
        let ((a, b), why) = match (stub, has_filing(0, k - 1), has_filing(k, n - 1)) {
            // After a gap, the side the filer files on is its own.
            (false, true, false) => ((k, n - 1), "symbol_reused"),
            (false, false, true) => ((0, k - 1), "symbol_reused"),
            // The filer on both sides: a relisting (new equity), so only the return across breaks.
            (false, true, true) => ((k, k), "relisted"),
            _ => (shorter, "symbol_reused"),
        };
        spans.push((a, b, why));
    }
    let out = jumps
        .iter()
        .map(|&(k, r)| (k, r, covering(&spans, k)))
        .collect();
    (spans, out)
}

/// Split evidence for a jump the rules leave open: an 8-K Item 5.03 from 45 days before to 10
/// after, or shares outstanding moving against the price (their ratio times `r` within 2x).
fn split_hint(cx: &JumpContext, sec: u32, d: NaiveDate, r: f64) -> String {
    let mut hint = String::new();
    let (lo, hi) = (d - chrono::Days::new(45), d + chrono::Days::new(10));
    if let Some(x) = cx
        .item503
        .get(&sec)
        .and_then(|v| v.iter().find(|x| (lo..=hi).contains(*x)))
    {
        hint.push_str(&format!("; split candidate: 8-K 5.03 {x}"));
    }
    let sh = cx.shares.get(&sec).map(Vec::as_slice).unwrap_or_default();
    let pre = sh.iter().rev().find(|(f, _)| *f < d);
    let post = sh
        .iter()
        .find(|(f, _)| *f >= d && *f < d + chrono::Days::new(200));
    if let (Some((_, a)), Some((_, b))) = (pre, post)
        && *a > 0.0
        && (0.5..=2.0).contains(&(b / a * r))
    {
        hint.push_str(&format!("; split candidate: shares {a:.0} -> {b:.0}"));
    }
    hint
}

/// G2 (`QUALITY-GATES.md` §2) over `bars` sorted by `(sec, date)`; `names[sec]` is the FIGI.
/// Returns `rows` (HARD, at most 0.1% of rows; one offender per failed row, chunk
/// `rows {date} {why}`), `ten_fold` (HARD, 0: jumps no rule of [`JumpContext`] explains,
/// within 365 days of an eligible `universe` row), `ten_fold_unscoped` (WARN: the others),
/// `spans` (WARN: the rejected bar ranges, chunk `spans {from} {to} {reason}`), `jumps` (WARN,
/// at most 0.05% of ticker-days outside the spans), `splits` (WARN) and `reconciliation`
/// (WARN, at least 99.5% of 20-bar windows within 0.5% of the vendor's adjusted ratio; one
/// offender per security). A jump is explained by a split with `ex_date` from the previous
/// bar to one day after the bar.
#[must_use]
pub fn price_sanity(
    bars: &[Bar],
    names: &[String],
    splits: &Events,
    divs: &Events,
    cx: &JumpContext,
) -> [Outcome; 7] {
    let name = |b: &Bar| names.get(b.sec as usize).cloned().unwrap_or_default();
    let none: Vec<(NaiveDate, f64)> = Vec::new();
    let (mut rows, mut ten, mut unscoped, mut jumps, mut split_bad) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut pairs, mut windows, mut within) = (0usize, 0usize, 0usize);
    let (mut recon, mut rejected) = (Vec::new(), Vec::new());
    let mut by_reason: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    let twin = twins(bars, names, &cx.filed);
    for s in bars.chunk_by(|a, b| a.sec == b.sec) {
        let sec = s.first().map_or(0, |b| b.sec);
        let sp = splits.get(&sec).unwrap_or(&none);
        let dv = divs.get(&sec).unwrap_or(&none);
        let tw = twin.get(&sec).map(Vec::as_slice).unwrap_or_default();
        let (spans, ten_fold) = classify(s, sp, sec, cx, tw);
        let mut masked = vec![false; s.len()];
        for &(a, b, why) in &spans {
            masked
                .iter_mut()
                .take(b + 1)
                .skip(a)
                .for_each(|m| *m = true);
            let (Some(x), Some(y)) = (s.get(a), s.get(b)) else {
                continue;
            };
            rejected.push((name(x), format!("{} {} {why}", x.date, y.date)));
            by_reason.entry(why).or_default().0 += b + 1 - a;
        }
        let near = |d: NaiveDate| {
            cx.eligible.as_ref().is_none_or(|e| {
                e.get(&sec).is_some_and(|v| {
                    let i = v.partition_point(|x| *x < d - chrono::Days::new(365));
                    v.get(i).is_some_and(|x| *x <= d + chrono::Days::new(365))
                })
            })
        };
        for (k, r, why) in ten_fold {
            let Some(b) = s.get(k) else { continue };
            match why {
                Some(w) => by_reason.entry(w).or_default().1 += 1,
                None => {
                    let off = (
                        name(b),
                        format!(
                            "{} close ratio {r:.3}{}",
                            b.date,
                            split_hint(cx, sec, b.date, r)
                        ),
                    );
                    if near(b.date) {
                        ten.push(off);
                    } else {
                        unscoped.push(off);
                    }
                }
            }
        }
        // One-bar total-return factor G (`FEATURES.md` §0); NaN across a bad or rejected close.
        let mut g = vec![f64::NAN; s.len()];
        for (k, b) in s.iter().enumerate() {
            if let Some(why) = bad_row(b) {
                rows.push((name(b), format!("{} {why}", b.date)));
            }
            let Some(p) = k.checked_sub(1).and_then(|j| s.get(j)) else {
                continue;
            };
            let rejected = masked.get(k).copied().unwrap_or(true)
                || masked.get(k - 1).copied().unwrap_or(true);
            if rejected || !(priced(p) && priced(b)) {
                continue;
            }
            pairs += 1;
            let r = b.close / p.close;
            if !explained(sp, p.date, b.date) && (r >= 1.8 || r <= 0.55) {
                jumps.push((name(b), format!("{} close ratio {r:.3}", b.date)));
            }
            let paid: f64 = dv
                .iter()
                .filter(|(ex, _)| p.date < *ex && *ex <= b.date)
                .map(|(ex, a)| a * rho(sp, p.date, *ex))
                .sum();
            if let Some(x) = g.get_mut(k) {
                *x = (rho(sp, p.date, b.date) * b.close + paid) / p.close;
            }
        }
        for (ex, ratio) in sp {
            // The first bar on or after the ex-date and the bar before it.
            let Some([p, b]) = s
                .windows(2)
                .find(|w| w.get(1).is_some_and(|b| b.date >= *ex))
            else {
                continue;
            };
            let v = b.close / p.close * ratio;
            if !(0.4..=2.5).contains(&v) {
                split_bad.push((
                    names.get(sec as usize).cloned().unwrap_or_default(),
                    format!("{ex} ratio {ratio} gives {v:.3}"),
                ));
            }
        }
        let (mut n, mut off) = (0usize, 0usize);
        // 21 bars span 20 factors G: those of bars 1..=20 of the window.
        for (w, gw) in s.windows(21).zip(g.windows(20).skip(1)) {
            let ours: f64 = gw.iter().product();
            let (a0, a1) = (
                w.first().map_or(0.0, |b| b.adj),
                w.last().map_or(0.0, |b| b.adj),
            );
            if !(ours.is_finite() && a0 > 0.0 && a1 > 0.0) {
                continue;
            }
            n += 1;
            if ((ours / (a1 / a0)) - 1.0).abs() > 0.005 {
                off += 1;
            }
        }
        windows += n;
        within += n - off;
        if off > 0 {
            recon.push((
                names.get(sec as usize).cloned().unwrap_or_default(),
                format!("{off} of {n} 20-bar windows off by more than 0.5%"),
            ));
        }
    }
    let share = |n: usize, d: usize| n as f64 / d.max(1) as f64;
    let rate =
        |chunk, hard: bool, what: &str, offenders: Vec<(String, String)>, d: usize, t: f64| {
            let m = share(offenders.len(), d);
            Outcome {
                chunk,
                level: if m <= t {
                    Level::Ok
                } else if hard {
                    Level::Fail
                } else {
                    Level::Warn
                },
                measured: Some(m),
                threshold: Some(t),
                note: format!("{} of {d} {what} ({:.4}%)", offenders.len(), m * 100.0),
                offenders,
            }
        };
    let rec = share(within, windows);
    let classified: Vec<String> = by_reason
        .iter()
        .map(|(w, (b, j))| format!("{w} {b} bars, {j} jumps"))
        .collect();
    let mut spans = count("spans", false, "rejected bar ranges", rejected);
    spans.note = format!("{}: {}", spans.note, classified.join("; "));
    [
        rate(
            "rows",
            true,
            "rows fail a per-row check",
            rows,
            bars.len(),
            0.001,
        ),
        count(
            "ten_fold",
            true,
            "unexplained ten-fold jumps within 365 days of an eligible row",
            ten,
        ),
        count(
            "ten_fold_unscoped",
            false,
            "unexplained ten-fold jumps away from eligible rows",
            unscoped,
        ),
        spans,
        rate(
            "jumps",
            false,
            "ticker-days outside the spans jump without a split",
            jumps,
            pairs,
            0.0005,
        ),
        count(
            "splits",
            false,
            "splits inconsistent with the closes",
            split_bad,
        ),
        Outcome {
            chunk: "reconciliation",
            level: if windows > 0 && rec >= 0.995 {
                Level::Ok
            } else {
                Level::Warn
            },
            measured: Some(rec),
            threshold: Some(0.995),
            note: format!(
                "{within} of {windows} 20-bar windows within 0.5% of the vendor ({:.3}%)",
                rec * 100.0
            ),
            offenders: recon,
        },
    ]
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
        let lines = [
            line("a", "10-Q"),
            line("x", "10-K"),
            line("z", "10-Q"),
            line("y", "8-K"),
        ];
        let (unmatched, checked) = index_unmatched(&lines, &ciks, &rows);
        assert_eq!((unmatched.len(), checked), (2, 3));
        // `x` has facts but no row (a loss); `z` has nothing at the source.
        let facts = serde_json::json!({"facts": {"us-gaap": {"Assets": {"units": {"USD": [
            {"accn": "x", "val": 1}, {"accn": "q", "val": 2}
        ]}}}}});
        let [lost, absent] = index_misses(&unmatched, checked, &fact_accessions(&facts));
        assert_eq!((lost.measured, lost.level), (Some(1.0 / 3.0), Level::Fail));
        assert_eq!(lost.offenders[0].1, "x 10-K 2024-02-05");
        assert_eq!((absent.level, absent.offenders.len()), (Level::Warn, 1));
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

    #[test]
    fn g0_calendar_against_spy() {
        let cal = calendar::Calendar::rules_only();
        let (from, to) = (d("2024-01-01"), d("2024-01-31"));
        let mut spy: std::collections::BTreeSet<_> = from
            .iter_days()
            .take_while(|x| *x <= to)
            .filter(|x| cal.is_trading_day(*x))
            .collect();
        assert_eq!(calendar_match(&cal, &spy, from, to).level, Level::Ok);
        spy.remove(&d("2024-01-10"));
        spy.insert(d("2024-01-13"));
        let o = calendar_match(&cal, &spy, from, to);
        assert_eq!(o.level, Level::Fail);
        assert_eq!(o.offenders.len(), 2);
        let none = calendar_match(&cal, &Default::default(), from, to);
        assert_eq!((none.level, none.measured), (Level::Fail, None));
    }

    #[test]
    fn a_day_over_500bn_is_a_bad_row() {
        let b = |volume| Bar {
            sec: 0,
            date: d("2024-01-02"),
            open: 100.0,
            high: 100.0,
            low: 100.0,
            close: 100.0,
            volume,
            adj: 100.0,
        };
        assert_eq!(
            bad_row(&b(1_000_000_000)),
            None,
            "$100bn: Tesla traded $154bn one day"
        );
        assert_eq!(
            bad_row(&b(10_000_000_000)),
            Some("dollar volume over $500bn")
        );
    }

    #[test]
    fn g2_checks() {
        let day = |k: u64| d("2024-01-01") + chrono::Days::new(k);
        let bar = |sec, k, close: f64, adj| Bar {
            sec,
            date: day(k),
            open: close,
            high: close,
            low: close,
            close,
            volume: 100,
            adj,
        };
        let mut bars = Vec::new();
        for k in 0..30 {
            // sec 0: flat, one dividend the vendor ignores, one broken row.
            let mut b = bar(0, k, 10.0, 10.0);
            if k == 3 {
                b.high = 9.0;
            }
            bars.push(b);
            // sec 1: a 4-for-1 split on day 10, the vendor's adjusted series is flat.
            bars.push(bar(1, k, if k < 10 { 40.0 } else { 10.0 }, 10.0));
            // sec 2: a twelve-fold jump on day 5 with no split.
            bars.push(bar(2, k, if k < 5 { 1.0 } else { 12.0 }, 1.0));
        }
        bars.sort_by_key(|b| (b.sec, b.date));
        let names = vec!["A".to_owned(), "B".to_owned(), "C".to_owned()];
        let splits = Events::from([(1, vec![(day(10), 4.0)])]);
        let divs = Events::from([(0, vec![(day(15), 0.1)])]);
        let [rows, ten, unscoped, spans, jumps, sp, rec] =
            price_sanity(&bars, &names, &splits, &divs, &JumpContext::default());
        assert_eq!(
            rows.offenders,
            vec![("A".to_owned(), format!("{} high < low", day(3)))]
        );
        assert_eq!(rows.level, Level::Fail, "1 of 90 rows is above 0.1%");
        assert!(ten.offenders.is_empty() && unscoped.offenders.is_empty());
        assert_eq!(
            spans.offenders,
            vec![(
                "C".to_owned(),
                format!("{} {} symbol_reused", day(0), day(4))
            )],
            "C's jump 5 bars in drops the 5 bars before it"
        );
        assert!(jumps.offenders.is_empty(), "the split explains B's drop");
        assert_eq!(sp.level, Level::Ok);
        assert_eq!(rec.level, Level::Warn);
        assert_eq!(
            rec.offenders
                .iter()
                .map(|o| o.0.as_str())
                .collect::<Vec<_>>(),
            ["A"],
            "A's dividend differs from the vendor; C's rejected bars are skipped"
        );
    }

    #[test]
    fn twins_keep_the_old_code_and_reject_the_copy() {
        let day = |k: u64| d("2011-07-01") + chrono::Days::new(k);
        let bar = |sec, k, close: f64| Bar {
            sec,
            date: day(k),
            open: close,
            high: close,
            low: close,
            close,
            volume: 100,
            adj: close,
        };
        let px = |k: u64| 20.0 + (k % 37) as f64 * 0.37 + (k / 37) as f64 * 0.01;
        let mut bars = Vec::new();
        for k in 0..300 {
            if k < 200 {
                bars.push(bar(0, k, px(k)));
            }
            // DINO: HFC's 200 days, then its own prices.
            bars.push(bar(1, k, if k < 200 { px(k) } else { px(k) * 1.1 }));
            bars.push(bar(2, k, px(k) + 3.0));
            // Two codes with one series to the end.
            bars.push(bar(3, k, px(k) + 7.0));
            bars.push(bar(4, k, px(k) + 7.0));
        }
        bars.sort_by_key(|b| (b.sec, b.date));
        let names: Vec<String> = ["HFC", "DINO", "X", "AD", "AD2"].map(String::from).to_vec();
        let filed = HashMap::from([(4, vec![day(50), day(150)])]);
        let t = twins(&bars, &names, &filed);
        assert_eq!(
            t.get(&1),
            Some(&vec![(day(0), day(199))]),
            "DINO trades on, so its copy goes"
        );
        assert_eq!(
            t.get(&3),
            Some(&vec![(day(0), day(299))]),
            "same end: the one whose filer filed less"
        );
        assert_eq!(
            t.len(),
            2,
            "HFC, the unrelated X and the filing AD2 keep their bars"
        );
    }

    #[test]
    fn g2_classifies_ten_fold_jumps() {
        let day = |k: u64| d("2024-01-01") + chrono::Days::new(k);
        // 100 daily bars of `sec`, closes from `c`; `gap` adds 60 days before bar 40.
        let series = |sec, c: &dyn Fn(u64) -> f64, gap: bool| -> Vec<Bar> {
            (0..100)
                .map(|k| Bar {
                    sec,
                    date: day(k + if gap && k >= 40 { 60 } else { 0 }),
                    open: c(k),
                    high: c(k),
                    low: c(k),
                    close: c(k),
                    volume: 100,
                    adj: c(k),
                })
                .collect()
        };
        let step = |k: u64| if k < 50 { 1.0 } else { 20.0 };
        let mut bars: Vec<Bar> = [
            series(0, &|k| if k == 50 { 100.0 } else { 10.0 }, false),
            series(1, &|k| if k == 40 || k == 60 { 100.0 } else { 10.0 }, false),
            series(2, &|k| if k < 70 { 10.0 } else { 100.0 }, false),
            // Bars 30-49 scaled by 10 for the 1:10 split of day 80; the move back is only 9x.
            series(
                3,
                &|k| match k {
                    30..50 | 80.. => 100.0,
                    50..80 => 11.1,
                    _ => 10.0,
                },
                false,
            ),
            series(4, &|k| if k < 40 { 1.0 } else { 20.0 }, true),
            series(5, &step, false),
            series(6, &step, false),
            series(7, &step, false),
            series(8, &|_| 10.0, false),
            series(9, &|k| if k < 40 { 1.0 } else { 20.0 }, true),
            series(10, &|k| if k < 40 { 1.0 } else { 20.0 }, true),
            // The filer's 20 bars at 10, then another holder at 100 with two dips back.
            series(
                11,
                &|k| {
                    if k < 20 || k == 50 || k == 70 {
                        10.0
                    } else {
                        100.0
                    }
                },
                false,
            ),
            // Bars 0-39 scaled by 1/10 for the split of day 80, which shows in the closes.
            series(
                12,
                &|k| match k {
                    0..40 => 1.0,
                    40..80 => 10.0,
                    _ => 100.0,
                },
                false,
            ),
        ]
        .concat();
        bars.sort_by_key(|b| (b.sec, b.date));
        let names: Vec<String> = (0..13).map(|i| format!("S{i}")).collect();
        let splits = Events::from([(3, vec![(day(80), 0.1)]), (12, vec![(day(80), 0.1)])]);
        let cx = JumpContext {
            accepted: HashSet::from([(7, day(50))]),
            excluded: HashMap::from([(8, vec![(day(10), day(20))])]),
            eligible: Some(HashMap::from([(5, vec![day(50)])])),
            item503: HashMap::from([(5, vec![day(45)])]),
            shares: HashMap::new(),
            filed: HashMap::from([
                (9, vec![day(10)]),
                (10, vec![day(10), day(120)]),
                (11, vec![day(5)]),
            ]),
        };
        let [_, ten, unscoped, spans, ..] =
            price_sanity(&bars, &names, &splits, &Events::new(), &cx);
        let span =
            |s: &str, a, b, why: &str| (s.to_owned(), format!("{} {} {why}", day(a), day(b)));
        for want in [
            span("S0", 50, 50, "foreign_quotes"),
            span("S1", 40, 40, "vendor_wrong_holder"),
            span("S1", 60, 60, "vendor_wrong_holder"),
            span("S3", 30, 49, "vendor_adjusted"),
            (
                "S4".to_owned(),
                format!("{} {} symbol_reused", day(0), day(39)),
            ),
            span("S8", 10, 20, "reviewed"),
            // After the 60-day gap: S9 files before it only, S10 on both sides.
            span("S9", 100, 159, "symbol_reused"),
            span("S10", 100, 100, "relisted"),
            span("S11", 20, 49, "vendor_wrong_holder"),
            span("S11", 71, 99, "vendor_wrong_holder"),
            span("S12", 0, 39, "vendor_adjusted"),
        ] {
            assert!(
                spans.offenders.contains(&want),
                "{want:?} in {:?}",
                spans.offenders
            );
        }
        assert_eq!(spans.offenders.len(), 12, "one span per planted defect");
        assert_eq!(
            ten.offenders,
            vec![(
                "S5".to_owned(),
                format!(
                    "{} close ratio 20.000; split candidate: 8-K 5.03 {}",
                    day(50),
                    day(45)
                )
            )],
            "S7's jump is accepted"
        );
        assert_eq!((ten.level, unscoped.level), (Level::Fail, Level::Warn));
        assert_eq!(unscoped.offenders.len(), 2, "S2 and S6 are never eligible");
        assert!(
            spans.note.contains("vendor_wrong_holder 80 bars, 9 jumps"),
            "{}",
            spans.note
        );
    }
}
