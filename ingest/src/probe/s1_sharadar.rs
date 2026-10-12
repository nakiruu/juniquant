//! S1: Sharadar (Nasdaq Data Link) on the ten-fold jumps G2 still counts HARD, as a possible
//! second price source (`QUALITY-GATES.md` G10, `SOURCES.md` "Sharadar"). Per jump: the
//! `TICKERS` rows of its ticker, the `SEP` bars from 14 days before to 14 after, the `ACTIONS`
//! from 30 days before to 30 after, and a verdict comparing Sharadar's unadjusted close ratio
//! across the jump with EODHD's. Key `NASDAQ_DATA_LINK_API_KEY`, sent as `api_key=` (redacted).

use arrow::array::AsArray;
use chrono::{Days, NaiveDate};
use serde_json::{Map, Value, json};

use super::{Probe, ProbeError, Source};

const BASE: &str = "https://data.nasdaq.com/api/v3/datatables/SHARADAR";

/// A ten-fold jump from the latest G2 run: FIGI, ticker, the bar's date, EODHD's close ratio.
type Jump = (String, String, NaiveDate, f64);

/// Runs S1; one finding per jump, keyed `{ticker} {date}`.
///
/// # Errors
///
/// [`ProbeError::EnvUnset`] without `NASDAQ_DATA_LINK_API_KEY`; lake, transport and parse errors.
pub fn run(p: &mut Probe) -> Result<(), ProbeError> {
    let key = std::env::var("NASDAQ_DATA_LINK_API_KEY")
        .ok()
        .filter(|k| !k.is_empty())
        .ok_or(ProbeError::EnvUnset("NASDAQ_DATA_LINK_API_KEY"))?;
    let jumps = hard_jumps()?;
    p.note("jumps", json!(jumps.len()));
    let mut tally: Map<String, Value> = Map::new();
    for (figi, ticker, d, r) in &jumps {
        let window = |n| (*d - Days::new(n), *d + Days::new(n));
        let (a, b) = window(14);
        let sep = table(
            p,
            &key,
            "SEP",
            &format!("ticker={ticker}&date.gte={a}&date.lte={b}"),
        )?;
        if matches!(sep.status, 401 | 403) {
            // A key without the SEP table: every other call would fail the same way.
            p.note("access", json!({"status": sep.status, "error": sep.error}));
            break;
        }
        let tickers = table(p, &key, "TICKERS", &format!("ticker={ticker}&table=SEP"))?;
        let (a, b) = window(30);
        let actions = table(
            p,
            &key,
            "ACTIONS",
            &format!("ticker={ticker}&date.gte={a}&date.lte={b}"),
        )?;
        let verdict = verdict(&sep.rows, *d, *r);
        *tally.entry(verdict.0.to_owned()).or_insert(json!(0)) =
            json!(tally.get(verdict.0).and_then(Value::as_u64).unwrap_or(0) + 1);
        p.note(
            &format!("{ticker} {d}"),
            json!({
                "figi": figi,
                "eodhd_ratio": r,
                "verdict": verdict.0,
                "sharadar": verdict.1,
                "sep_status": sep.status,
                "sep_error": sep.error,
                "tickers": tickers.rows.iter().map(|t| json!({
                    "permaticker": t.get("permaticker"),
                    "name": t.get("name"),
                    "isdelisted": t.get("isdelisted"),
                    "firstpricedate": t.get("firstpricedate"),
                    "lastpricedate": t.get("lastpricedate"),
                    "relatedtickers": t.get("relatedtickers"),
                })).collect::<Vec<_>>(),
                "actions": actions.rows,
            }),
        );
    }
    p.note("verdicts", Value::Object(tally));
    Ok(())
}

/// Sharadar's view of the jump: a verdict and the bars either side of it.
fn verdict(rows: &[Map<String, Value>], d: NaiveDate, r: f64) -> (&'static str, Value) {
    let day = |m: &Map<String, Value>| {
        m.get("date")
            .and_then(Value::as_str)
            .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
    };
    let num = |m: &Map<String, Value>, k: &str| m.get(k).and_then(Value::as_f64);
    let mut bars: Vec<&Map<String, Value>> = rows.iter().filter(|m| day(m).is_some()).collect();
    bars.sort_by_key(|m| day(m));
    let i = bars.partition_point(|m| day(m) < Some(d));
    let (Some(prev), Some(bar)) = (i.checked_sub(1).and_then(|j| bars.get(j)), bars.get(i)) else {
        return (
            "no Sharadar bars around the jump",
            json!({"bars": bars.len()}),
        );
    };
    let unadj = |m: &Map<String, Value>| num(m, "closeunadj").or_else(|| num(m, "close"));
    let ratio = |f: &dyn Fn(&Map<String, Value>) -> Option<f64>| Some(f(bar)? / f(prev)?);
    let (Some(ru), ra) = (ratio(&unadj), ratio(&|m| num(m, "close"))) else {
        return ("Sharadar bars without a close", json!({}));
    };
    let shown = json!({
        "prev": {"date": prev.get("date"), "closeunadj": unadj(prev), "close": num(prev, "close"), "volume": num(prev, "volume")},
        "bar": {"date": bar.get("date"), "closeunadj": unadj(bar), "close": num(bar, "close"), "volume": num(bar, "volume")},
        "ratio_unadj": ru,
        "ratio_split_adjusted": ra,
    });
    let v = if day(bar) != Some(d) {
        "Sharadar has no bar on the jump day"
    } else if ru.ln().abs() < 1.8_f64.ln() {
        "Sharadar shows no jump"
    } else if (ru.ln() - r.ln()).abs() < 0.22 {
        if ra.is_some_and(|a| a.ln().abs() < 1.8_f64.ln()) {
            "Sharadar shows the jump as a split"
        } else {
            "Sharadar shows the same jump"
        }
    } else {
        "Sharadar shows a different jump"
    };
    (v, shown)
}

/// One datatable reply: HTTP status, rows as column-name maps, and the API's error message.
struct Table {
    status: u16,
    rows: Vec<Map<String, Value>>,
    error: Option<String>,
}

fn table(p: &mut Probe, key: &str, name: &str, query: &str) -> Result<Table, ProbeError> {
    let reply = p.get(
        Source::Sharadar,
        &format!("{BASE}/{name}.json?{query}&api_key={key}"),
    )?;
    let v = reply.json().unwrap_or(Value::Null);
    let error = v
        .pointer("/quandl_error/message")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let cols: Vec<String> = v
        .pointer("/datatable/columns")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|c| c.get("name").and_then(Value::as_str).map(str::to_owned))
        .collect();
    let rows = v
        .pointer("/datatable/data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_array)
        .map(|r| cols.iter().cloned().zip(r.iter().cloned()).collect())
        .collect();
    Ok(Table {
        status: reply.status,
        rows,
        error,
    })
}

/// The `ten_fold` offenders of the latest G2 run, with each FIGI's ticker.
fn hard_jumps() -> Result<Vec<Jump>, ProbeError> {
    let err = |e: lake::LakeError| ProbeError::Parse(e.to_string());
    let lake = lake::Lake::from_env().map_err(err)?;
    let m = lake
        .read_dataset("security_master", Some(&["figi", "ticker"]), &[], None)
        .map_err(err)?;
    let (f, t) = (
        m.column(0).as_string::<i32>(),
        m.column(1).as_string::<i32>(),
    );
    let ticker: std::collections::HashMap<&str, &str> = (0..m.num_rows())
        .map(|i| (f.value(i), t.value(i)))
        .collect();
    let q = lake
        .read_dataset(
            "data_quality",
            Some(&["check_id", "run_id", "entity", "chunk"]),
            &[],
            None,
        )
        .map_err(err)?;
    let col = |i: usize| q.column(i).as_string::<i32>();
    let (check, run, entity, chunk) = (col(0), col(1), col(2), col(3));
    let g2: Vec<usize> = (0..q.num_rows())
        .filter(|&i| check.value(i) == "G2")
        .collect();
    let latest = g2.iter().map(|&i| run.value(i)).max().unwrap_or_default();
    let mut out = Vec::new();
    for &i in g2.iter().filter(|&&i| run.value(i) == latest) {
        let words: Vec<&str> = chunk.value(i).split(' ').collect();
        let (Some(&"ten_fold"), Some(d), Some(r)) = (words.first(), words.get(1), words.get(4))
        else {
            continue;
        };
        let (Ok(d), Ok(r)) = (
            NaiveDate::parse_from_str(d, "%Y-%m-%d"),
            r.trim_end_matches(';').parse::<f64>(),
        ) else {
            continue;
        };
        let figi = entity.value(i);
        let tk = ticker.get(figi).copied().unwrap_or_default().to_owned();
        out.push((figi.to_owned(), tk, d, r));
    }
    out.sort_by(|a, b| (&a.1, a.2).cmp(&(&b.1, b.2)));
    Ok(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test")]

    use super::*;

    fn bars(closes: &[(&str, f64, f64)]) -> Vec<Map<String, Value>> {
        closes
            .iter()
            .map(|(d, unadj, adj)| {
                let mut m = Map::new();
                m.insert("date".into(), json!(d));
                m.insert("closeunadj".into(), json!(unadj));
                m.insert("close".into(), json!(adj));
                m
            })
            .collect()
    }

    #[test]
    fn verdicts() {
        let d = NaiveDate::from_ymd_opt(2021, 10, 27).unwrap();
        let v = |b: &[(&str, f64, f64)]| verdict(&bars(b), d, 0.004).0;
        let flat = [("2021-10-26", 18.0, 18.0), ("2021-10-27", 18.2, 18.2)];
        assert_eq!(v(&flat), "Sharadar shows no jump");
        let same = [("2021-10-26", 4885.0, 4885.0), ("2021-10-27", 18.2, 18.2)];
        assert_eq!(v(&same), "Sharadar shows the same jump");
        let split = [("2021-10-26", 4885.0, 18.3), ("2021-10-27", 18.2, 18.2)];
        assert_eq!(v(&split), "Sharadar shows the jump as a split");
        let late = [("2021-10-26", 18.0, 18.0), ("2021-10-28", 18.2, 18.2)];
        assert_eq!(v(&late), "Sharadar has no bar on the jump day");
        assert_eq!(v(&late[1..]), "no Sharadar bars around the jump");
    }
}
