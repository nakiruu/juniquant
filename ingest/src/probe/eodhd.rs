//! P1-P4 against EODHD, the primary price source (DECISIONS D15; `BACKFILL.md` §2,
//! `SOURCES.md` EODHD). The verdicts are pure functions over parsed responses, unit-tested
//! without the network.

#![allow(
    clippy::indexing_slicing,
    reason = "serde_json::Value indexing returns Null for a missing key or index; it never panics"
)]

use chrono::NaiveDate;
use serde_json::{Value, json};

use super::{Probe, ProbeError, Source};

const BASE: &str = "https://eodhd.com/api";

/// One `/api/eod` row.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Bar {
    pub date: NaiveDate,
    pub close: Option<f64>,
    pub adjusted_close: Option<f64>,
    pub volume: Option<f64>,
}

pub(crate) fn parse_eod(v: &Value) -> Vec<Bar> {
    v.as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|r| {
                    Some(Bar {
                        date: NaiveDate::parse_from_str(r["date"].as_str()?, "%Y-%m-%d").ok()?,
                        close: r["close"].as_f64(),
                        adjusted_close: r["adjusted_close"].as_f64(),
                        volume: r["volume"].as_f64(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `/api/splits` rows as `(ex_date, new shares per old share)`; `split` is `"4.000000/1.000000"`.
pub(crate) fn parse_splits(v: &Value) -> Vec<(NaiveDate, f64)> {
    v.as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|r| {
                    let date = NaiveDate::parse_from_str(r["date"].as_str()?, "%Y-%m-%d").ok()?;
                    let (num, den) = r["split"].as_str()?.split_once('/')?;
                    Some((
                        date,
                        num.trim().parse::<f64>().ok()? / den.trim().parse::<f64>().ok()?,
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Is a pre/post-split quantity ratio closer (on a log scale) to `traded` than to `adjusted`?
fn closer_to(observed: f64, traded: f64, adjusted: f64) -> bool {
    (observed / traded).ln().abs() < (observed / adjusted).ln().abs()
}

/// Verdict on one split: last bar before vs first bar on or after the ex-date. As traded, the
/// close ratio is near the split ratio and the volume ratio near its inverse; adjusted, both
/// are near 1.
pub(crate) fn split_verdict(bars: &[Bar], ex_date: NaiveDate, ratio: f64) -> Value {
    let before = bars
        .iter()
        .rev()
        .find(|b| b.date < ex_date && b.close.is_some());
    let after = bars.iter().find(|b| b.date >= ex_date && b.close.is_some());
    let (Some(b), Some(a)) = (before, after) else {
        return json!({"split": ex_date.to_string(), "verdict": "no bars around the split"});
    };
    let pr = b.close.unwrap_or(f64::NAN) / a.close.unwrap_or(f64::NAN);
    let vr = match (b.volume, a.volume) {
        (Some(vb), Some(va)) if va > 0.0 => vb / va,
        _ => f64::NAN,
    };
    json!({
        "split": ex_date.to_string(),
        "ratio": ratio,
        "before": {"date": b.date.to_string(), "close": b.close, "adjusted_close": b.adjusted_close, "volume": b.volume},
        "after": {"date": a.date.to_string(), "close": a.close, "adjusted_close": a.adjusted_close, "volume": a.volume},
        "close_ratio": pr,
        "volume_ratio": vr,
        "ohlc_raw": closer_to(pr, ratio, 1.0),
        "volume_split_adjusted": vr.is_finite() && !closer_to(vr, 1.0 / ratio, 1.0),
    })
}

fn url(p: &Probe, path: &str, query: &str) -> Result<String, ProbeError> {
    let key = p
        .eodhd_key
        .as_deref()
        .ok_or(ProbeError::EnvUnset("EODHD_API_KEY"))?;
    Ok(format!("{BASE}/{path}?api_token={key}&fmt=json{query}"))
}

fn date(y: i32, m: u32, d: u32) -> Result<NaiveDate, ProbeError> {
    NaiveDate::from_ymd_opt(y, m, d)
        .ok_or_else(|| ProbeError::Parse(format!("bad date {y}-{m}-{d}")))
}

/// P1: adjustment of OHLC, volume and dividends around a 4:1, a 10:1 and a 1:8 split.
pub(super) fn p1(p: &mut Probe) -> Result<(), ProbeError> {
    let cases = [
        (
            "AAPL",
            "2020-08-01",
            "2020-09-30",
            date(2020, 8, 31)?,
            Some(499.23),
        ),
        ("NVDA", "2024-05-15", "2024-06-30", date(2024, 6, 10)?, None),
        ("GE", "2021-07-15", "2021-08-31", date(2021, 8, 2)?, None),
    ];
    let mut out = Vec::new();
    for (sym, from, to, ex, known) in cases {
        let eod = p.get(
            Source::Eodhd,
            &url(
                p,
                &format!("eod/{sym}.US"),
                &format!("&from={from}&to={to}"),
            )?,
        )?;
        let splits = p.get(
            Source::Eodhd,
            &url(p, &format!("splits/{sym}.US"), "&from=2019-01-01")?,
        )?;
        let mut entry =
            json!({"symbol": sym, "eod_status": eod.status, "splits_status": splits.status});
        if eod.status == 200 && splits.status == 200 {
            let s = parse_splits(&splits.json()?);
            entry["splits"] = json!(
                s.iter()
                    .map(|(d, r)| (d.to_string(), r))
                    .collect::<Vec<_>>()
            );
            entry["verdict"] = match s.iter().find(|(d, _)| *d == ex) {
                Some((_, r)) => split_verdict(&parse_eod(&eod.json()?), ex, *r),
                None => json!("expected split not in the splits response"),
            };
            entry["known_as_traded_close_before"] = json!(known);
        }
        out.push(entry);
    }
    let div = p.get(
        Source::Eodhd,
        &url(p, "div/AAPL.US", "&from=2020-08-01&to=2020-08-31")?,
    )?;
    let first = div
        .json()
        .ok()
        .and_then(|v| v.as_array().and_then(|a| a.first().cloned()));
    p.note("aapl_dividend_2020_08", json!({"status": div.status, "row": first, "expected_unadjusted": 0.82, "expected_value": 0.205}));
    let all = |k: &str| {
        let flags: Vec<Option<bool>> = out.iter().map(|v| v["verdict"][k].as_bool()).collect();
        if !flags.is_empty() && flags.iter().all(|f| *f == Some(true)) {
            json!(true)
        } else if !flags.is_empty() && flags.iter().all(|f| *f == Some(false)) {
            json!(false)
        } else {
            json!("undecided")
        }
    };
    p.note("ohlc_raw", all("ohlc_raw"));
    p.note("volume_split_adjusted", all("volume_split_adjusted"));
    p.note("cases", json!(out));
    Ok(())
}

/// One code's whole-history outcome, for P2 and P3.
fn outcome(p: &mut Probe, code: &str) -> Result<Value, ProbeError> {
    let reply = p.get(
        Source::Eodhd,
        &url(p, &format!("eod/{code}"), "&from=1960-01-01")?,
    )?;
    let mut v = json!({"code": code, "status": reply.status});
    if let Ok(body) = reply.json() {
        let bars = parse_eod(&body);
        v["bars"] = json!(bars.len());
        v["first_bar"] = json!(bars.first().map(|b| (b.date.to_string(), b.close)));
        v["last_bar"] = json!(bars.last().map(|b| (b.date.to_string(), b.close)));
        if bars.is_empty() {
            v["body_head"] = json!(String::from_utf8_lossy(
                reply.body.get(..200).unwrap_or(&reply.body)
            ));
        }
    }
    Ok(v)
}

/// P2: delisted, acquired and renamed codes, and the size of the delisted symbol list.
pub(super) fn p2(p: &mut Probe) -> Result<(), ProbeError> {
    let mut out = Vec::new();
    for code in [
        "TWX", "MON", "CELG", "RHT", "LLTC", "SHLD", "LEH", "TWTR", "FB", "META",
    ] {
        out.push(outcome(p, &format!("{code}.US"))?);
    }
    p.note("codes", json!(out));
    for (key, query) in [
        ("active", "&type=common_stock"),
        ("delisted", "&type=common_stock&delisted=1"),
    ] {
        let reply = p.get(Source::Eodhd, &url(p, "exchange-symbol-list/US", query)?)?;
        let rows = reply
            .json()
            .ok()
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default();
        let exchanges: std::collections::BTreeMap<String, usize> =
            rows.iter().fold(Default::default(), |mut m, r| {
                *m.entry(r["Exchange"].as_str().unwrap_or("?").to_owned())
                    .or_default() += 1;
                m
            });
        p.note(
            &format!("symbol_list_{key}"),
            json!({"status": reply.status, "rows": rows.len(), "by_exchange": exchanges, "sample": rows.iter().take(3).collect::<Vec<_>>()}),
        );
    }
    Ok(())
}

/// P3: history depth, a reused ticker (GM) and the share-class code form.
pub(super) fn p3(p: &mut Probe) -> Result<(), ProbeError> {
    let mut out = Vec::new();
    for (code, why) in [
        ("SPY.US", "history depth (listed 1993-01-29)"),
        ("DLB.US", "2005 listing (Dolby IPO 2005-02-17)"),
        (
            "GM.US",
            "reused ticker: new GM listed 2010-11-18, old GM traded until 2009",
        ),
        ("BRK-B.US", "share class, hyphen form"),
        ("BRK.B.US", "share class, dot form"),
    ] {
        let mut v = outcome(p, code)?;
        v["why"] = json!(why);
        out.push(v);
    }
    p.note("codes", json!(out));
    Ok(())
}

/// P4: plan and usage counters, the cost of each request kind, and the latest bulk day.
pub(super) fn p4(p: &mut Probe) -> Result<(), ProbeError> {
    let user = |p: &mut Probe| -> Result<Value, ProbeError> {
        let r = p.get_private(Source::Eodhd, &url(p, "user", "")?)?;
        // Not landed, and only plan and counters are kept: the body carries name and email.
        let v = r.json().unwrap_or(Value::Null);
        Ok(
            json!({"status": r.status, "subscriptionType": v["subscriptionType"],
                  "apiRequests": v["apiRequests"], "dailyRateLimit": v["dailyRateLimit"], "apiRequestsDate": v["apiRequestsDate"]}),
        )
    };
    let mut steps = vec![json!({"step": "start", "user": user(p)?})];
    for (step, path, query) in [
        ("eod", "eod/AAPL.US", "&from=2026-01-01"),
        ("splits", "splits/AAPL.US", "&from=2019-01-01"),
        ("bulk_eod_last_day", "eod-bulk-last-day/US", ""),
    ] {
        let r = p.get(Source::Eodhd, &url(p, path, query)?)?;
        let latest = r.json().ok().and_then(|v| {
            v.as_array().map(|rows| {
                rows.iter()
                    .filter_map(|x| x["date"].as_str())
                    .max()
                    .map(str::to_owned)
            })
        });
        steps.push(json!({"step": step, "status": r.status, "bytes": r.body.len(), "latest_date": latest, "user_after": user(p)?}));
    }
    p.note("steps", json!(steps));
    p.note("fetched_at_local", json!(chrono::Local::now().to_rfc3339()));
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test")]

    use super::*;

    fn bars(close_fri: f64, vol_fri: f64) -> Vec<Bar> {
        parse_eod(&json!([
            {"date": "2020-08-28", "open": 1.0, "high": 1.0, "low": 1.0, "close": close_fri, "adjusted_close": 122.0, "volume": vol_fri},
            {"date": "2020-08-31", "open": 1.0, "high": 1.0, "low": 1.0, "close": 129.04, "adjusted_close": 126.0, "volume": 225_702_700.0}
        ]))
    }

    #[test]
    fn parses_splits_string_new_shares_first() {
        let s = parse_splits(
            &json!([{"date": "2020-08-31", "split": "4.000000/1.000000"}, {"date": "2021-08-02", "split": "1.000000/8.000000"}]),
        );
        assert_eq!(
            s,
            [
                (date(2020, 8, 31).unwrap(), 4.0),
                (date(2021, 8, 2).unwrap(), 0.125)
            ]
        );
    }

    #[test]
    fn raw_close_and_adjusted_volume_are_told_apart() {
        let ex = date(2020, 8, 31).unwrap();
        // EODHD as documented: raw close, split-adjusted volume (187.6M = 46.9M x 4).
        let v = split_verdict(&bars(499.23, 187_630_000.0), ex, 4.0);
        assert_eq!(
            (v["ohlc_raw"].clone(), v["volume_split_adjusted"].clone()),
            (json!(true), json!(true))
        );
        // Both as traded.
        let v = split_verdict(&bars(499.23, 46_907_500.0), ex, 4.0);
        assert_eq!(
            (v["ohlc_raw"].clone(), v["volume_split_adjusted"].clone()),
            (json!(true), json!(false))
        );
        // Both adjusted.
        let v = split_verdict(&bars(124.81, 187_630_000.0), ex, 4.0);
        assert_eq!(v["ohlc_raw"], json!(false));
    }

    #[test]
    fn empty_array_parses_to_no_bars() {
        assert!(parse_eod(&json!([])).is_empty());
        assert!(parse_eod(&json!({"error": "x"})).is_empty());
    }
}
