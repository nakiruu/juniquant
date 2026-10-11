//! P1-P4: the Yahoo chart API (`BACKFILL.md` §2, `SOURCES.md` Yahoo). The analysis is in
//! pure functions over the parsed response, so it is unit-tested without the network.

#![allow(
    clippy::indexing_slicing,
    reason = "serde_json::Value indexing returns Null for a missing key or index; it never panics"
)]

use std::time::{Duration, Instant};

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{Value, json};

use super::{Probe, ProbeError, Source};

/// One daily bar as the vendor returned it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Bar {
    pub date: NaiveDate,
    pub close: Option<f64>,
    pub volume: Option<f64>,
}

/// A split event: `numerator` new shares per `denominator` old.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Split {
    pub date: NaiveDate,
    pub numerator: f64,
    pub denominator: f64,
}

/// The fields of a chart response the probes use.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Chart {
    pub symbol: Option<String>,
    pub first_trade_date: Option<NaiveDate>,
    pub instrument_type: Option<String>,
    pub bars: Vec<Bar>,
    pub splits: Vec<Split>,
    pub dividends: Vec<(NaiveDate, f64)>,
    /// `chart.error` as `code: description`.
    pub error: Option<String>,
}

/// Calendar date of a bar stamped `ts` (Unix seconds) in the exchange zone. `gmtoffset` is
/// the zone's current offset; bars are stamped at the 09:30 open, so a DST hour cannot move
/// the date.
fn exchange_date(ts: i64, gmtoffset: i64) -> Option<NaiveDate> {
    DateTime::<Utc>::from_timestamp(ts.checked_add(gmtoffset)?, 0).map(|d| d.date_naive())
}

/// Parse a chart response body.
pub(crate) fn parse_chart(v: &Value) -> Chart {
    let chart = &v["chart"];
    let error = chart["error"].as_object().map(|e| {
        format!(
            "{}: {}",
            e["code"].as_str().unwrap_or("?"),
            e["description"].as_str().unwrap_or("")
        )
    });
    let r = &chart["result"][0];
    let meta = &r["meta"];
    let off = meta["gmtoffset"].as_i64().unwrap_or(0);
    let q = &r["indicators"]["quote"][0];
    let bars = r["timestamp"]
        .as_array()
        .map(|ts| {
            ts.iter()
                .enumerate()
                .filter_map(|(i, t)| {
                    Some(Bar {
                        date: exchange_date(t.as_i64()?, off)?,
                        close: q["close"][i].as_f64(),
                        volume: q["volume"][i].as_f64(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let events = |kind: &str| -> Vec<&Value> {
        r["events"][kind]
            .as_object()
            .map(|m| m.values().collect())
            .unwrap_or_default()
    };
    let mut splits: Vec<Split> = events("splits")
        .into_iter()
        .filter_map(|s| {
            Some(Split {
                date: exchange_date(s["date"].as_i64()?, off)?,
                numerator: s["numerator"].as_f64()?,
                denominator: s["denominator"].as_f64()?,
            })
        })
        .collect();
    splits.sort_by_key(|s| s.date);
    let mut dividends: Vec<(NaiveDate, f64)> = events("dividends")
        .into_iter()
        .filter_map(|d| {
            Some((
                exchange_date(d["date"].as_i64()?, off)?,
                d["amount"].as_f64()?,
            ))
        })
        .collect();
    dividends.sort_by_key(|d| d.0);
    Chart {
        symbol: meta["symbol"].as_str().map(str::to_owned),
        first_trade_date: meta["firstTradeDate"]
            .as_i64()
            .and_then(|t| exchange_date(t, off)),
        instrument_type: meta["instrumentType"].as_str().map(str::to_owned),
        bars,
        splits,
        dividends,
        error,
    }
}

fn chart_url(symbol: &str, from: NaiveDate) -> String {
    let p1 = from
        .and_hms_opt(0, 0, 0)
        .map_or(0, |d| d.and_utc().timestamp());
    // The window ends at the fetch time (SOURCES.md window rule).
    let p2 = Utc::now().timestamp();
    format!(
        "https://query1.finance.yahoo.com/v8/finance/chart/{symbol}?period1={p1}&period2={p2}&interval=1d&events=div%2Csplit&includeAdjustedClose=true"
    )
}

fn date(y: i32, m: u32, d: u32) -> Result<NaiveDate, ProbeError> {
    NaiveDate::from_ymd_opt(y, m, d)
        .ok_or_else(|| ProbeError::Parse(format!("bad date {y}-{m}-{d}")))
}

/// Verdict on one split: are the vendor's pre-split closes and volumes adjusted?
///
/// `price_ratio` = last close before the split / first close on or after it; as traded it is
/// near the split ratio, adjusted it is near 1. Decided by which is closer on a log scale.
pub(crate) fn split_verdict(chart: &Chart, split: &Split) -> Value {
    let before = chart
        .bars
        .iter()
        .rev()
        .find(|b| b.date < split.date && b.close.is_some());
    let after = chart
        .bars
        .iter()
        .find(|b| b.date >= split.date && b.close.is_some());
    let ratio = split.numerator / split.denominator;
    let (Some(b), Some(a)) = (before, after) else {
        return json!({"split": split.date.to_string(), "verdict": "no bars around the split"});
    };
    let pr = b.close.unwrap_or(f64::NAN) / a.close.unwrap_or(f64::NAN);
    let adjusted = pr.ln().abs() < (pr / ratio).ln().abs();
    let vr = match (b.volume, a.volume) {
        (Some(vb), Some(va)) if va > 0.0 => Some(vb / va),
        _ => None,
    };
    json!({
        "split": split.date.to_string(),
        "ratio": ratio,
        "close_before": [b.date.to_string(), b.close],
        "close_after": [a.date.to_string(), a.close],
        "price_ratio": pr,
        "volume_ratio": vr,
        "adjusted": adjusted,
    })
}

/// P1: split adjustment of OHLCV and dividends, on a forward split, a large forward split and
/// a reverse split.
pub(super) fn p1(p: &mut Probe) -> Result<(), ProbeError> {
    // (symbol, window start, split date, known as-traded close the day before, if any)
    let cases = [
        ("AAPL", date(2020, 6, 1)?, date(2020, 8, 31)?, Some(499.23)),
        ("NVDA", date(2024, 4, 1)?, date(2024, 6, 10)?, None),
        ("GE", date(2021, 5, 1)?, date(2021, 8, 2)?, None),
    ];
    let mut verdicts = Vec::new();
    for (sym, from, split_date, known) in cases {
        let reply = p.get(Source::Yahoo, &chart_url(sym, from))?;
        let mut entry = json!({"symbol": sym, "status": reply.status});
        if reply.status == 200 {
            let chart = parse_chart(&reply.json()?);
            entry["error"] = json!(chart.error);
            entry["splits_in_response"] = json!(
                chart
                    .splits
                    .iter()
                    .map(|s| (s.date.to_string(), s.numerator, s.denominator))
                    .collect::<Vec<_>>()
            );
            entry["dividends_in_window"] = json!(
                chart
                    .dividends
                    .iter()
                    .map(|(d, a)| (d.to_string(), a))
                    .collect::<Vec<_>>()
            );
            match chart.splits.iter().find(|s| s.date == split_date) {
                Some(s) => entry["verdict"] = split_verdict(&chart, s),
                None => entry["verdict"] = json!("expected split not in the response"),
            }
            entry["known_as_traded_close_before"] = json!(known);
        }
        verdicts.push(entry);
    }
    let flags: Vec<Option<bool>> = verdicts
        .iter()
        .map(|v| v["verdict"]["adjusted"].as_bool())
        .collect();
    let flag = match flags.as_slice() {
        f if !f.is_empty() && f.iter().all(|x| *x == Some(true)) => json!(true),
        f if !f.is_empty() && f.iter().all(|x| *x == Some(false)) => json!(false),
        _ => json!("undecided"),
    };
    p.note("cases", json!(verdicts));
    p.note("yahoo_ohlcv_split_adjusted", flag);
    Ok(())
}

/// One symbol's outcome, for P2 and P3.
fn outcome(p: &mut Probe, sym: &str, from: NaiveDate) -> Result<Value, ProbeError> {
    let reply = p.get(Source::Yahoo, &chart_url(sym, from))?;
    let mut v = json!({"symbol": sym, "status": reply.status});
    if let Ok(body) = reply.json() {
        let c = parse_chart(&body);
        v["error"] = json!(c.error);
        v["meta_symbol"] = json!(c.symbol);
        v["instrument_type"] = json!(c.instrument_type);
        v["first_trade_date"] = json!(c.first_trade_date.map(|d| d.to_string()));
        v["bars"] = json!(c.bars.len());
        v["first_bar"] = json!(c.bars.first().map(|b| b.date.to_string()));
        v["last_bar"] = json!(c.bars.last().map(|b| b.date.to_string()));
        v["splits"] = json!(c.splits.len());
        v["dividends"] = json!(c.dividends.len());
    }
    Ok(v)
}

/// P2: delisted, acquired and renamed symbols.
pub(super) fn p2(p: &mut Probe) -> Result<(), ProbeError> {
    let from = date(1990, 1, 1)?;
    let mut out = Vec::new();
    for sym in ["TWX", "MON", "CELG", "RHT", "LLTC", "SHLD", "LEH", "TWTR"] {
        out.push(outcome(p, sym, from)?);
    }
    p.note("symbols", json!(out));
    Ok(())
}

/// P3: history depth (SPY, a 2005 listing) and whether `firstTradeDate` exposes a reused
/// ticker (GM: the old GM was delisted in 2009, the new GM listed on 2010-11-18).
pub(super) fn p3(p: &mut Probe) -> Result<(), ProbeError> {
    let from = date(1970, 1, 2)?;
    let mut out = Vec::new();
    for (sym, why) in [
        ("SPY", "history depth (listed 1993-01-29)"),
        ("DLB", "2005 listing (Dolby IPO 2005-02-17)"),
        (
            "GM",
            "reused ticker: new GM listed 2010-11-18, old GM traded until 2009",
        ),
    ] {
        let mut v = outcome(p, sym, from)?;
        v["why"] = json!(why);
        out.push(v);
    }
    p.note("symbols", json!(out));
    Ok(())
}

/// P4: ramp the request rate from 0.5/s until the first non-200, then time the recovery.
pub(super) fn p4(p: &mut Probe) -> Result<(), ProbeError> {
    const PER_STEP: usize = 20;
    const RATES: [f64; 5] = [0.5, 1.0, 2.0, 4.0, 8.0];
    let url = chart_url("AAPL", Utc::now().date_naive() - chrono::Days::new(10));
    let mut steps = Vec::new();
    let mut ceiling = None;
    let mut refused = None;
    'ramp: for rate in RATES {
        p.set_interval(Source::Yahoo, Duration::from_secs_f64(1.0 / rate));
        for i in 0..PER_STEP {
            let status = p.get(Source::Yahoo, &url)?.status;
            if status != 200 {
                steps.push(json!({"rate_per_s": rate, "ok_before_refusal": i, "status": status}));
                refused = Some(status);
                break 'ramp;
            }
        }
        steps.push(json!({"rate_per_s": rate, "ok": PER_STEP}));
        ceiling = Some(rate);
    }
    p.note("steps", json!(steps));
    p.note("highest_rate_fully_served_per_s", json!(ceiling));
    if refused.is_some() {
        // Recovery: one request a minute for up to 20 minutes.
        p.set_interval(Source::Yahoo, Duration::from_secs(60));
        let t0 = Instant::now();
        let mut recovered = None;
        for _ in 0..20 {
            if p.get(Source::Yahoo, &url)?.status == 200 {
                recovered = Some(t0.elapsed().as_secs());
                break;
            }
        }
        p.note("recovery_seconds", json!(recovered));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test")]

    use super::*;

    // 2020-08-28 and 2020-08-31 09:30 EDT (UTC-4), as the vendor stamps them.
    const FRI: i64 = 1_598_621_400;
    const MON: i64 = 1_598_880_600;

    fn body(close_fri: f64, vol_fri: f64) -> Value {
        json!({"chart": {"result": [{
            "meta": {"symbol": "AAPL", "gmtoffset": -14400, "firstTradeDate": 345_479_400, "instrumentType": "EQUITY"},
            "timestamp": [FRI, MON],
            "events": {"splits": {"x": {"date": MON, "numerator": 4.0, "denominator": 1.0}},
                       "dividends": {"y": {"date": FRI, "amount": 0.205}}},
            "indicators": {"quote": [{"close": [close_fri, 129.04], "volume": [vol_fri, 225_702_700.0]}]}
        }], "error": null}})
    }

    #[test]
    fn parses_bars_events_and_exchange_dates() {
        let c = parse_chart(&body(124.81, 187_630_000.0));
        assert_eq!(c.bars.len(), 2);
        assert_eq!(
            c.bars[0].date,
            NaiveDate::from_ymd_opt(2020, 8, 28).unwrap()
        );
        assert_eq!(
            c.splits[0].date,
            NaiveDate::from_ymd_opt(2020, 8, 31).unwrap()
        );
        assert_eq!(c.first_trade_date, NaiveDate::from_ymd_opt(1980, 12, 12));
        assert_eq!(
            c.dividends,
            [(NaiveDate::from_ymd_opt(2020, 8, 28).unwrap(), 0.205)]
        );
        assert_eq!(c.error, None);
    }

    #[test]
    fn verdict_tells_adjusted_from_as_traded() {
        let adjusted = parse_chart(&body(124.81, 187_630_000.0));
        assert_eq!(
            split_verdict(&adjusted, &adjusted.splits[0])["adjusted"],
            json!(true)
        );
        let traded = parse_chart(&body(499.23, 46_907_500.0));
        assert_eq!(
            split_verdict(&traded, &traded.splits[0])["adjusted"],
            json!(false)
        );
    }

    #[test]
    fn not_found_error_is_reported() {
        let v = json!({"chart": {"result": null, "error": {"code": "Not Found", "description": "No data found, symbol may be delisted"}}});
        let c = parse_chart(&v);
        assert_eq!(
            c.error.as_deref(),
            Some("Not Found: No data found, symbol may be delisted")
        );
        assert!(c.bars.is_empty());
    }
}
