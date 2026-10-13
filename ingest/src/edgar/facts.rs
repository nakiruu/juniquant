//! J7: `fundamentals` rows of one filer from its `submissions` JSON (with the page files) and
//! its `companyfacts` JSON (`SOURCES.md` "Building `fundamentals` from `companyfacts`").
//! `period_end` arithmetic happens here and only here (`POINT-IN-TIME.md` "period_end
//! exception"): pairing fiscal periods, deriving discrete quarters, TTM and prior-period values.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::Value;

/// Bumped when the parser's output changes.
pub const PARSE_VERSION: &str = "2";

/// One filing from `submissions`.
#[derive(Debug, Clone, PartialEq)]
pub struct Filing {
    /// `accessionNumber`.
    pub accession: String,
    /// `filingDate`: the point-in-time stamp.
    pub filing_date: NaiveDate,
    /// `reportDate`: the period end (metadata).
    pub report_date: Option<NaiveDate>,
    /// `form`.
    pub form: String,
    /// `acceptanceDateTime`.
    pub acceptance: Option<DateTime<Utc>>,
    /// `items`: the 8-K item numbers (`2.02`), empty for other forms.
    pub items: Vec<String>,
}

fn date(v: Option<&Value>) -> Option<NaiveDate> {
    v.and_then(Value::as_str)
        .and_then(|s| NaiveDate::parse_from_str(s.get(..10)?, "%Y-%m-%d").ok())
}

/// The filings of one columnar block (`filings.recent` or a page file).
#[must_use]
pub fn filings(cols: &Value) -> Vec<Filing> {
    let col = |k: &str| cols.get(k).and_then(Value::as_array);
    let (Some(acc), Some(fd), Some(form)) =
        (col("accessionNumber"), col("filingDate"), col("form"))
    else {
        return Vec::new();
    };
    let (rd, at, items) = (col("reportDate"), col("acceptanceDateTime"), col("items"));
    acc.iter()
        .enumerate()
        .filter_map(|(i, a)| {
            Some(Filing {
                accession: a.as_str()?.to_owned(),
                filing_date: date(fd.get(i))?,
                report_date: date(rd.and_then(|r| r.get(i))),
                form: form.get(i)?.as_str()?.to_owned(),
                acceptance: at
                    .and_then(|r| r.get(i))
                    .and_then(Value::as_str)
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|t| t.with_timezone(&Utc)),
                items: items
                    .and_then(|r| r.get(i))
                    .and_then(Value::as_str)
                    .map(|s| {
                        s.split(',')
                            .map(str::trim)
                            .filter(|x| !x.is_empty())
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
            })
        })
        .collect()
}

/// Names of the older-filings page files of a submissions JSON (`filings.files[].name`).
#[must_use]
pub fn page_names(main: &Value) -> Vec<String> {
    main.pointer("/filings/files")
        .and_then(Value::as_array)
        .map(|fs| {
            fs.iter()
                .filter_map(|f| f.get("name").and_then(Value::as_str).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// One XBRL fact.
#[derive(Debug, Clone, PartialEq)]
pub struct Fact {
    /// Period start (flows only).
    pub start: Option<NaiveDate>,
    /// Period end, or the instant.
    pub end: NaiveDate,
    /// Value.
    pub val: f64,
    /// Accession that reported it.
    pub accn: String,
    /// Filing date of that accession.
    pub filed: NaiveDate,
    /// `fy` of the filing (not of the fact).
    pub fy: Option<i32>,
    /// `fp` of the filing.
    pub fp: Option<String>,
}

/// Flow fields and their candidate tags, first non-null wins (`SOURCES.md` tag table).
const FLOWS: &[(&str, &[&str])] = &[
    (
        "revenues",
        &[
            "Revenues",
            "RevenueFromContractWithCustomerExcludingAssessedTax",
            "SalesRevenueNet",
            "RevenueFromContractWithCustomerIncludingAssessedTax",
            "SalesRevenueGoodsNet",
            "SalesRevenueServicesNet",
        ],
    ),
    ("net_income", &["NetIncomeLoss", "ProfitLoss"]),
    (
        "operating_cash_flow",
        &["NetCashProvidedByUsedInOperatingActivities"],
    ),
    ("gross_profit", &["GrossProfit"]),
    ("ebit", &["OperatingIncomeLoss"]),
];

/// Instant fields and their candidate tags.
const INSTANTS: &[(&str, &[&str])] = &[
    ("total_assets", &["Assets"]),
    ("total_liabilities", &["Liabilities"]),
    ("assets_current", &["AssetsCurrent"]),
    ("liabilities_current", &["LiabilitiesCurrent"]),
    (
        "long_term_debt",
        &["LongTermDebtNoncurrent", "LongTermDebt"],
    ),
    ("retained_earnings", &["RetainedEarningsAccumulatedDeficit"]),
];

const COVER_SHARES: &str = "dei:EntityCommonStockSharesOutstanding";
const BALANCE_SHARES: &str = "CommonStockSharesOutstanding";
/// All classes together, the fallback for a multi-class filer (D4).
const WEIGHTED_SHARES: &str = "WeightedAverageNumberOfSharesOutstandingBasic";
/// Inputs of the two recorded fallbacks.
const EXTRA: &[&str] = &[
    "CostOfRevenue",
    "LiabilitiesAndStockholdersEquity",
    "StockholdersEquity",
];

/// The facts of one filer for the tags above, keyed `us-gaap` tag name or `dei:<tag>`.
pub type Facts = HashMap<String, Vec<Fact>>;

/// Parses a `companyfacts` JSON, keeping only the tags this builder reads.
#[must_use]
pub fn parse_facts(body: &Value) -> Facts {
    let wanted = |tag: &str| {
        FLOWS
            .iter()
            .chain(INSTANTS)
            .any(|(_, ts)| ts.contains(&tag))
            || tag == BALANCE_SHARES
            || tag == WEIGHTED_SHARES
            || EXTRA.contains(&tag)
    };
    let mut out = Facts::new();
    for (tax, prefix) in [("us-gaap", ""), ("dei", "dei:")] {
        let Some(tags) = body
            .pointer(&format!("/facts/{tax}"))
            .and_then(Value::as_object)
        else {
            continue;
        };
        for (tag, t) in tags {
            let key = format!("{prefix}{tag}");
            if !((wanted(tag) && prefix.is_empty()) || key == COVER_SHARES) {
                continue;
            }
            let unit = if key == COVER_SHARES || tag == BALANCE_SHARES || tag == WEIGHTED_SHARES {
                "shares"
            } else {
                "USD"
            };
            let Some(list) = t
                .pointer(&format!("/units/{unit}"))
                .and_then(Value::as_array)
            else {
                continue;
            };
            let mut v: Vec<Fact> = list
                .iter()
                .filter_map(|f| {
                    Some(Fact {
                        start: date(f.get("start")),
                        end: date(f.get("end"))?,
                        val: f.get("val")?.as_f64()?,
                        accn: f.get("accn")?.as_str()?.to_owned(),
                        filed: date(f.get("filed"))?,
                        fy: f
                            .get("fy")
                            .and_then(Value::as_i64)
                            .and_then(|y| i32::try_from(y).ok()),
                        fp: f.get("fp").and_then(Value::as_str).map(str::to_owned),
                    })
                })
                .collect();
            v.sort_by_key(|f| f.filed);
            out.insert(key, v);
        }
    }
    out
}

/// One `fundamentals` row of a filer, before the security columns are attached.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Row {
    /// Accession.
    pub accession: String,
    /// Point-in-time stamp.
    pub filing_date: NaiveDate,
    /// Period end (metadata).
    pub period_end: NaiveDate,
    /// `10-K`, `10-Q`, `10-K/A`, `10-Q/A`.
    pub form_type: String,
    /// Field name -> value (the 12 value columns).
    pub values: BTreeMap<&'static str, f64>,
    /// Field name -> trailing-twelve-month value (the 5 flow fields).
    pub ttm: BTreeMap<&'static str, f64>,
    /// `fy`, checked against the period.
    pub fiscal_year: Option<i32>,
    /// `fp`, checked against the form.
    pub fiscal_period: Option<String>,
    /// Length of the primary flow period.
    pub duration_days: Option<i32>,
    /// Acceptance timestamp.
    pub acceptance: Option<DateTime<Utc>>,
    /// The original filing an `/A` amends.
    pub amends_accession: Option<String>,
    /// Field -> winning tag.
    pub tag_used: BTreeMap<&'static str, String>,
    /// The date the share count is stated for.
    pub shares_asof_date: Option<NaiveDate>,
    /// The prior-period columns, by name.
    pub prior: BTreeMap<&'static str, f64>,
}

/// Why a periodic filing produced no row.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Dropped {
    /// No `reportDate`.
    pub no_period: usize,
    /// Filed after the run date.
    pub future: usize,
    /// No fact of any field for the accession (pre-XBRL, or an untagged filing).
    pub no_facts: usize,
}

/// `10-KT` is treated as `10-K` (`SCHEMAS.md` #3).
fn form_type(form: &str) -> Option<&'static str> {
    match form {
        "10-K" | "10-KT" => Some("10-K"),
        "10-Q" => Some("10-Q"),
        "10-K/A" | "10-KT/A" => Some("10-K/A"),
        "10-Q/A" => Some("10-Q/A"),
        _ => None,
    }
}

fn days(a: NaiveDate, b: NaiveDate) -> i64 {
    (a - b).num_days()
}

/// Cumulative values of one tag as known at `asof`: `(start, end) -> val` from the latest
/// fact with `filed <= asof` (facts are sorted by `filed`).
fn view(facts: &[Fact], asof: NaiveDate) -> BTreeMap<(NaiveDate, NaiveDate), f64> {
    facts
        .iter()
        .take_while(|f| f.filed <= asof)
        .filter_map(|f| Some(((f.start?, f.end), f.val)))
        .collect()
}

/// The discrete quarter ending at `e`: reported directly (80-100 days), or a cumulative value
/// minus the previous cumulative with the same start (YTD2 - Q1, YTD3 - YTD2, FY - YTD3).
fn quarter(v: &BTreeMap<(NaiveDate, NaiveDate), f64>, e: NaiveDate) -> Option<(f64, i64)> {
    let ending = || v.iter().filter(move |((_, en), _)| *en == e);
    if let Some(((s, _), x)) = ending().find(|((s, _), _)| (80..=100).contains(&days(e, *s))) {
        return Some((*x, days(e, *s)));
    }
    ending()
        .filter(|((s, _), _)| days(e, *s) > 100)
        .find_map(|((s, _), x)| {
            v.iter()
                .find(|((s2, e2), _)| s2 == s && (70..=110).contains(&days(e, *e2)))
                .map(|((_, e2), y)| (x - y, days(e, *e2)))
        })
}

/// Sum of the four consecutive discrete quarters ending at `pe` (ends three months apart, give
/// or take 20 days), from the values known at `asof`.
fn ttm(facts: &[Fact], pe: NaiveDate, asof: NaiveDate) -> Option<f64> {
    let v = view(facts, asof);
    let mut ends: Vec<NaiveDate> = v.keys().map(|(_, e)| *e).collect();
    ends.dedup();
    let (mut e, mut sum) = (pe, 0.0);
    for i in 0..4 {
        if i > 0 {
            e = *ends
                .iter()
                .filter(|x| (71..=111).contains(&days(e, **x)))
                .min_by_key(|x| (days(e, **x) - 91).abs())?;
        }
        sum += quarter(&v, e)?.0;
    }
    Some(sum)
}

/// The primary flow value of a filing: the fiscal year for a 10-K (350-380 days), the discrete
/// quarter for a 10-Q, from the accession's own facts (a derived quarter subtracts the prior
/// cumulative value known at the filing date). Returns `(value, duration_days)`.
fn primary_flow(facts: &[Fact], f: &Filing, pe: NaiveDate, annual: bool) -> Option<(f64, i64)> {
    let own: BTreeMap<(NaiveDate, NaiveDate), f64> = facts
        .iter()
        .filter(|x| x.accn == f.accession && x.end == pe)
        .filter_map(|x| Some(((x.start?, x.end), x.val)))
        .collect();
    if annual {
        return own
            .iter()
            .find(|((s, _), _)| (350..=380).contains(&days(pe, *s)))
            .map(|((s, _), x)| (*x, days(pe, *s)));
    }
    if let Some(q) = own
        .iter()
        .find(|((s, _), _)| (80..=100).contains(&days(pe, *s)))
    {
        return Some((*q.1, days(pe, q.0.0)));
    }
    let known = view(facts, f.filing_date);
    own.iter()
        .filter(|((s, _), _)| days(pe, *s) > 100)
        .find_map(|((s, _), x)| {
            known
                .iter()
                .find(|((s2, e2), _)| s2 == s && (70..=110).contains(&days(pe, *e2)))
                .map(|((_, e2), y)| (x - y, days(pe, *e2)))
        })
}

fn instant(facts: &Facts, tag: &str, f: &Filing, pe: NaiveDate) -> Option<f64> {
    facts
        .get(tag)?
        .iter()
        .find(|x| x.accn == f.accession && x.end == pe && x.start.is_none())
        .map(|x| x.val)
}

fn ratio(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(a), Some(b)) if b != 0.0 => Some(a / b),
        _ => None,
    }
}

/// The `fundamentals` rows of one filer, one per periodic filing with at least one fact,
/// filed on or before `today`, ordered by filing date.
#[must_use]
pub fn rows(filings: &[Filing], facts: &Facts, today: NaiveDate) -> (Vec<Row>, Dropped) {
    let mut dropped = Dropped::default();
    let mut fs: Vec<&Filing> = filings
        .iter()
        .filter(|f| form_type(&f.form).is_some())
        .collect();
    fs.sort_by(|a, b| (a.filing_date, &a.accession).cmp(&(b.filing_date, &b.accession)));
    let mut out: Vec<Row> = Vec::new();
    for f in fs {
        let Some(form) = form_type(&f.form) else {
            continue;
        };
        if f.filing_date > today {
            dropped.future += 1;
            continue;
        }
        let Some(pe) = f.report_date else {
            dropped.no_period += 1;
            continue;
        };
        let annual = form.starts_with("10-K");
        let mut r = Row {
            accession: f.accession.clone(),
            filing_date: f.filing_date,
            period_end: pe,
            form_type: form.to_owned(),
            acceptance: f.acceptance,
            ..Row::default()
        };
        for (field, tags) in INSTANTS {
            if let Some((t, v)) = tags
                .iter()
                .find_map(|t| instant(facts, t, f, pe).map(|v| (*t, v)))
            {
                r.values.insert(field, v);
                r.tag_used.insert(field, t.to_owned());
            }
        }
        if !r.values.contains_key("total_liabilities")
            && let (Some(a), Some(b)) = (
                instant(facts, "LiabilitiesAndStockholdersEquity", f, pe),
                instant(facts, "StockholdersEquity", f, pe),
            )
        {
            r.values.insert("total_liabilities", a - b);
            r.tag_used.insert(
                "total_liabilities",
                "LiabilitiesAndStockholdersEquity-StockholdersEquity".into(),
            );
        }
        let flow = |tag: &str| {
            let v = facts.get(tag)?;
            let (x, d) = primary_flow(v, f, pe, annual)?;
            let t = if annual {
                Some(x)
            } else {
                ttm(v, pe, f.filing_date)
            };
            Some((x, d, t))
        };
        for (field, tags) in FLOWS {
            if let Some((t, (x, d, tt))) = tags.iter().find_map(|t| flow(t).map(|v| (*t, v))) {
                r.values.insert(field, x);
                r.tag_used.insert(field, t.to_owned());
                if let Some(tt) = tt {
                    r.ttm.insert(field, tt);
                }
                // The net income period, else the first flow found.
                if *field == "net_income" || r.duration_days.is_none() {
                    r.duration_days = i32::try_from(d).ok();
                }
            }
        }
        if !r.values.contains_key("gross_profit")
            && let (Some(rev), Some((c, _, ct))) =
                (r.values.get("revenues").copied(), flow("CostOfRevenue"))
        {
            r.values.insert("gross_profit", rev - c);
            r.tag_used.insert(
                "gross_profit",
                format!(
                    "{}-CostOfRevenue",
                    r.tag_used.get("revenues").map_or("", String::as_str)
                ),
            );
            if let (Some(rt), Some(ct)) = (r.ttm.get("revenues").copied(), ct) {
                r.ttm.insert("gross_profit", rt - ct);
            }
        }
        shares(facts, f, pe, &mut r);
        if r.values.is_empty() {
            dropped.no_facts += 1;
            continue;
        }
        // fy/fp describe the filing; any fact of this accession carries them.
        let tagged = facts.values().flatten().find(|x| x.accn == f.accession);
        r.fiscal_year = tagged
            .and_then(|x| x.fy)
            .filter(|y| (i64::from(*y) - i64::from(chrono::Datelike::year(&pe))).abs() <= 1);
        r.fiscal_period = tagged.and_then(|x| x.fp.clone()).filter(|p| {
            if annual {
                p == "FY"
            } else {
                matches!(p.as_str(), "Q1" | "Q2" | "Q3")
            }
        });
        if form.ends_with("/A") {
            let base = form.trim_end_matches("/A");
            r.amends_accession = out
                .iter()
                .rev()
                .find(|o| o.form_type == base && o.period_end == pe)
                .map(|o| o.accession.clone());
        }
        prior(&out, &mut r);
        out.push(r);
    }
    (out, dropped)
}

/// The filing's share count (`SOURCES.md` step 7): the cover-page shares of this accession
/// (several values on the latest cover date are summed: one per class), else the balance-sheet
/// count at the period end, else the basic weighted-average shares of the filing's shortest
/// period ending at the period end. A count of zero or less is no count. A cover or balance
/// count under half the weighted average is one class of several (`companyfacts` leaves out the
/// classes a filer tags by class: DDOG, SPG, META), so the weighted average replaces it. The
/// cover count is stated as of its cover date; the other two as of the filing date, because
/// statements give a split before their issue retroactive effect.
fn shares(facts: &Facts, f: &Filing, pe: NaiveDate, r: &mut Row) {
    let cover: Vec<&Fact> = facts
        .get(COVER_SHARES)
        .map(|v| {
            v.iter()
                .filter(|x| x.accn == f.accession && x.end <= f.filing_date && x.val > 0.0)
                .collect()
        })
        .unwrap_or_default();
    let cover = cover.iter().map(|x| x.end).max().map(|asof| {
        let sum: f64 = cover.iter().filter(|x| x.end == asof).map(|x| x.val).sum();
        (sum, asof, COVER_SHARES)
    });
    let balance = instant(facts, BALANCE_SHARES, f, pe)
        .filter(|v| *v > 0.0)
        .map(|v| (v, f.filing_date, BALANCE_SHARES));
    let weighted = facts
        .get(WEIGHTED_SHARES)
        .into_iter()
        .flatten()
        .filter(|x| x.accn == f.accession && x.end == pe && x.val > 0.0)
        .filter_map(|x| Some((days(pe, x.start?), x.val)))
        .filter(|(d, _)| *d >= 80)
        .min_by_key(|(d, _)| *d)
        .map(|(_, v)| (v, f.filing_date, WEIGHTED_SHARES));
    let best = match (cover.or(balance), weighted) {
        (Some(p), Some(w)) if p.0 < 0.5 * w.0 => Some(w),
        (p, w) => p.or(w),
    };
    if let Some((v, asof, tag)) = best {
        r.values.insert("shares_outstanding", v);
        r.tag_used.insert("shares_outstanding", tag.into());
        r.shares_asof_date = Some(asof);
    }
}

/// The prior-period columns from the same fiscal period one year earlier: the latest earlier
/// row (by filing date) of the same annual/quarterly kind whose period ends 345-385 days
/// before this one, with the same `fiscal_period` when both are known.
fn prior(earlier: &[Row], r: &mut Row) {
    let annual = r.form_type.starts_with("10-K");
    let Some(p) = earlier.iter().rev().find(|o| {
        o.form_type.starts_with("10-K") == annual
            && (345..=385).contains(&days(r.period_end, o.period_end))
            && (o.fiscal_period.is_none()
                || r.fiscal_period.is_none()
                || o.fiscal_period == r.fiscal_period)
    }) else {
        return;
    };
    let v = |k: &str| p.values.get(k).copied();
    let t = |k: &str| p.ttm.get(k).copied();
    for (name, x) in [
        ("prior_roa", ratio(t("net_income"), v("total_assets"))),
        (
            "prior_debt_ratio",
            ratio(v("long_term_debt"), v("total_assets")),
        ),
        (
            "prior_current_ratio",
            ratio(v("assets_current"), v("liabilities_current")),
        ),
        (
            "prior_gross_margin",
            ratio(t("gross_profit"), t("revenues")),
        ),
        (
            "prior_asset_turnover",
            ratio(t("revenues"), v("total_assets")),
        ),
        ("prior_shares_outstanding", v("shares_outstanding")),
    ] {
        if let Some(x) = x {
            r.prior.insert(name, x);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing, reason = "test")]

    use super::*;
    use serde_json::json;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    /// A calendar-year filer: 10-K FY2023, then 10-Qs for 2024 Q1-Q3 with revenue tagged only
    /// as YTD in Q2/Q3, a 10-K FY2024, and a 10-K/A for FY2024.
    fn sample() -> (Vec<Filing>, Facts) {
        let subs = json!({
            "accessionNumber": ["a-23k", "a-24q1", "a-24q2", "a-24q3", "a-24k", "a-24ka", "a-8k"],
            "filingDate": ["2024-02-20", "2024-05-01", "2024-08-01", "2024-11-01", "2025-02-20", "2025-03-10", "2025-03-11"],
            "reportDate": ["2023-12-31", "2024-03-31", "2024-06-30", "2024-09-30", "2024-12-31", "2024-12-31", ""],
            "acceptanceDateTime": ["2024-02-20T16:05:00.000Z", "", "", "", "", "", ""],
            "form": ["10-K", "10-Q", "10-Q", "10-Q", "10-K", "10-K/A", "8-K"],
        });
        let fact = |s: Option<&str>, e: &str, v: f64, a: &str, filed: &str, fy: i32, fp: &str| {
            json!({
                "start": s, "end": e, "val": v, "accn": a, "filed": filed, "fy": fy, "fp": fp,
            })
        };
        let mut rev = vec![
            fact(
                Some("2023-01-01"),
                "2023-12-31",
                400.0,
                "a-23k",
                "2024-02-20",
                2023,
                "FY",
            ),
            fact(
                Some("2023-01-01"),
                "2023-09-30",
                290.0,
                "a-23k",
                "2024-02-20",
                2023,
                "FY",
            ),
            fact(
                Some("2024-01-01"),
                "2024-03-31",
                110.0,
                "a-24q1",
                "2024-05-01",
                2024,
                "Q1",
            ),
            fact(
                Some("2024-01-01"),
                "2024-06-30",
                230.0,
                "a-24q2",
                "2024-08-01",
                2024,
                "Q2",
            ),
            fact(
                Some("2024-01-01"),
                "2024-09-30",
                360.0,
                "a-24q3",
                "2024-11-01",
                2024,
                "Q3",
            ),
            fact(
                Some("2024-01-01"),
                "2024-12-31",
                500.0,
                "a-24k",
                "2025-02-20",
                2024,
                "FY",
            ),
            fact(
                Some("2024-01-01"),
                "2024-12-31",
                510.0,
                "a-24ka",
                "2025-03-10",
                2024,
                "FY",
            ),
        ];
        // Q2 and Q3 2023 comparatives (YTD), as reported in the 2024 10-Qs.
        rev.push(fact(
            Some("2023-01-01"),
            "2023-06-30",
            190.0,
            "a-24q2",
            "2024-08-01",
            2024,
            "Q2",
        ));
        rev.push(fact(
            Some("2023-01-01"),
            "2023-03-31",
            95.0,
            "a-24q1",
            "2024-05-01",
            2024,
            "Q1",
        ));
        let body = json!({"facts": {
            "us-gaap": {
                "Revenues": {"units": {"USD": rev}},
                "Assets": {"units": {"USD": [
                    fact(None, "2023-12-31", 1000.0, "a-23k", "2024-02-20", 2023, "FY"),
                    fact(None, "2024-09-30", 1200.0, "a-24q3", "2024-11-01", 2024, "Q3"),
                    fact(None, "2024-12-31", 1250.0, "a-24k", "2025-02-20", 2024, "FY"),
                ]}},
                "NetIncomeLoss": {"units": {"USD": [
                    fact(Some("2023-01-01"), "2023-12-31", 40.0, "a-23k", "2024-02-20", 2023, "FY"),
                    fact(Some("2024-01-01"), "2024-12-31", 60.0, "a-24k", "2025-02-20", 2024, "FY"),
                ]}},
            },
            "dei": {"EntityCommonStockSharesOutstanding": {"units": {"shares": [
                fact(None, "2025-02-10", 7.0, "a-24k", "2025-02-20", 2024, "FY"),
            ]}}},
        }});
        (filings(&subs), parse_facts(&body))
    }

    #[test]
    fn filings_skip_non_periodic_and_parse_dates() {
        let (fs, _) = sample();
        assert_eq!(fs.len(), 7);
        assert_eq!(fs[6].report_date, None);
        assert!(fs[0].acceptance.is_some() && fs[1].acceptance.is_none());
    }

    #[test]
    fn quarters_ttm_prior_and_amendments() {
        let (fs, facts) = sample();
        let (rows, dropped) = rows(&fs, &facts, d("2026-01-01"));
        let by = |a: &str| rows.iter().find(|r| r.accession == a).unwrap();
        assert_eq!(rows.len(), 6, "the 8-K is not periodic");
        assert_eq!(dropped, Dropped::default());
        // Q2 = YTD2 - Q1 (known at the Q2 filing), Q3 = YTD3 - YTD2.
        assert_eq!(by("a-24q2").values["revenues"], 120.0);
        assert_eq!(by("a-24q2").duration_days, Some(91));
        assert_eq!(by("a-24q3").values["revenues"], 130.0);
        // TTM at Q3 2024: Q4 2023 (400 - 290) + 110 + 120 + 130.
        assert_eq!(by("a-24q3").ttm["revenues"], 470.0);
        // Q1 2024 TTM needs Q2 2023, which no filing known on 2024-05-01 reports.
        assert!(!by("a-24q1").ttm.contains_key("revenues"));
        let k = by("a-24k");
        assert_eq!((k.values["revenues"], k.ttm["revenues"]), (500.0, 500.0));
        assert_eq!(k.fiscal_period.as_deref(), Some("FY"));
        assert_eq!(k.fiscal_year, Some(2024));
        assert_eq!(k.values["shares_outstanding"], 7.0);
        assert_eq!(k.shares_asof_date, Some(d("2025-02-10")));
        // Prior year: 40 / 1000.
        assert_eq!(k.prior["prior_roa"], 0.04);
        let a = by("a-24ka");
        assert_eq!(a.form_type, "10-K/A");
        assert_eq!(a.amends_accession.as_deref(), Some("a-24k"));
        assert_eq!(
            a.values["revenues"], 510.0,
            "the amendment row holds its own figure"
        );
        assert!(!a.values.contains_key("total_assets"));
        assert!(k.tag_used["revenues"] == "Revenues");
    }

    #[test]
    fn multi_class_filers_fall_back_to_the_weighted_average() {
        let subs = json!({
            "accessionNumber": ["zero", "one-class", "plain"],
            "filingDate": ["2020-02-20", "2020-05-07", "2020-08-06"],
            "reportDate": ["2019-12-31", "2020-03-31", "2020-06-30"],
            "acceptanceDateTime": ["", "", ""],
            "form": ["10-K", "10-Q", "10-Q"],
        });
        let fact = |s: Option<&str>, e: &str, v: f64, a: &str, filed: &str| json!({"start": s, "end": e, "val": v, "accn": a, "filed": filed});
        let body = json!({"facts": {
            "us-gaap": {
                "Assets": {"units": {"USD": [
                    fact(None, "2019-12-31", 1.0, "zero", "2020-02-20"),
                    fact(None, "2020-03-31", 1.0, "one-class", "2020-05-07"),
                    fact(None, "2020-06-30", 1.0, "plain", "2020-08-06"),
                ]}},
                "WeightedAverageNumberOfSharesOutstandingBasic": {"units": {"shares": [
                    fact(Some("2019-01-01"), "2019-12-31", 300e6, "zero", "2020-02-20"),
                    fact(Some("2020-01-01"), "2020-03-31", 324e6, "one-class", "2020-05-07"),
                    fact(Some("2019-01-01"), "2020-03-31", 999e6, "one-class", "2020-05-07"),
                ]}},
            },
            "dei": {"EntityCommonStockSharesOutstanding": {"units": {"shares": [
                fact(None, "2020-02-14", 0.0, "zero", "2020-02-20"),
                fact(None, "2020-05-01", 8000.0, "one-class", "2020-05-07"),
                fact(None, "2020-08-01", 7.0, "plain", "2020-08-06"),
            ]}}},
        }});
        let (rows, _) = rows(&filings(&subs), &parse_facts(&body), d("2026-01-01"));
        let got: Vec<(f64, &str, Option<NaiveDate>)> = rows
            .iter()
            .map(|r| {
                (
                    r.values["shares_outstanding"],
                    r.tag_used["shares_outstanding"].as_str(),
                    r.shares_asof_date,
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                (300e6, WEIGHTED_SHARES, Some(d("2020-02-20"))),
                (324e6, WEIGHTED_SHARES, Some(d("2020-05-07"))),
                (7.0, COVER_SHARES, Some(d("2020-08-01"))),
            ],
            "a zero cover is no count; one class of 8,000 shares gives way to the quarter's \
             weighted average; a plain cover count stays"
        );
    }

    #[test]
    fn future_and_untagged_filings_are_dropped() {
        let (fs, facts) = sample();
        let (rows, dropped) = rows(&fs, &facts, d("2024-12-01"));
        assert_eq!(rows.len(), 4);
        assert_eq!(dropped.future, 2);
        let mut fs = fs;
        fs[0].accession = "untagged".into();
        let (_, dropped) = super::rows(&fs, &facts, d("2026-01-01"));
        assert_eq!(dropped.no_facts, 1);
    }
}
