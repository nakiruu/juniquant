//! P6: ALFRED full-vintage fetch (`BACKFILL.md` §2, `SOURCES.md` "FRED / ALFRED"): the vintage
//! count limit, the earliest `realtime_start`, late-captured history, and how far back the ICE
//! BofA credit spreads go. Human-readable result: `docs/probes/p6-alfred.md`.

use serde_json::{Value, json};

use super::{Probe, ProbeError, Source};

const BASE: &str = "https://api.stlouisfed.org/fred";

/// Vintage dates in the window tried when the full range is refused.
const WINDOW: usize = 2000;

/// The series whose vintages are probed: one daily (many vintages), one monthly.
const VINTAGED: [&str; 2] = ["DGS3MO", "CPIAUCSL"];

/// The ICE BofA series, whose history FRED restricts.
const ICE_BOFA: [&str; 2] = ["BAMLH0A0HYM2", "BAMLC0A0CM"];

/// Runs P6; findings are keyed `<series>.<what>`.
///
/// # Errors
///
/// [`ProbeError::EnvUnset`] without `FRED_API_KEY`, and transport or parse errors.
pub fn run(p: &mut Probe) -> Result<(), ProbeError> {
    let key = std::env::var("FRED_API_KEY")
        .ok()
        .filter(|k| !k.is_empty())
        .ok_or(ProbeError::EnvUnset("FRED_API_KEY"))?;
    for id in VINTAGED {
        vintaged(p, &key, id)?;
    }
    for id in ICE_BOFA {
        let (status, v) = observations(p, &key, id, "1990-01-01", "")?;
        p.note(&format!("{id}.history"), summarize(status, &v));
    }
    Ok(())
}

fn vintaged(p: &mut Probe, key: &str, id: &str) -> Result<(), ProbeError> {
    let url = format!(
        "{BASE}/series/vintagedates?api_key={key}&file_type=json&series_id={id}\
         &realtime_start=1776-07-04&realtime_end=9999-12-31&limit=10000"
    );
    let v = p.get(Source::Fred, &url)?.json()?;
    let dates: Vec<String> = v
        .get("vintage_dates")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|d| d.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    p.note(
        &format!("{id}.vintage_dates"),
        json!({ "count": dates.len(), "first": dates.first(), "last": dates.last() }),
    );

    let all = "&realtime_start=1776-07-04&realtime_end=9999-12-31";
    let (status, v) = observations(p, key, id, "2004-01-01", all)?;
    p.note(&format!("{id}.full_vintage"), summarize(status, &v));

    // Refused (the limit): retry with the first WINDOW vintage dates only.
    if status != 200
        && let (Some(first), Some(last)) = (dates.first(), dates.get(WINDOW - 1).or(dates.last()))
    {
        let window = format!("&realtime_start={first}&realtime_end={last}");
        let (status, v) = observations(p, key, id, "2004-01-01", &window)?;
        let mut s = summarize(status, &v);
        // Observations older than the first vintage were captured late: their
        // `realtime_start` is the capture date, not the publication date.
        let late = v
            .get("observations")
            .and_then(Value::as_array)
            .map_or(0, |o| {
                o.iter()
                    .filter(|x| {
                        x.get("date")
                            .and_then(Value::as_str)
                            .is_some_and(|d| d < first.as_str())
                    })
                    .count()
            });
        if let Some(obj) = s.as_object_mut() {
            obj.insert(
                "window".into(),
                json!({ "from": first, "to": last, "vintages": WINDOW }),
            );
            obj.insert("observations_before_first_vintage".into(), json!(late));
        }
        p.note(&format!("{id}.first_window"), s);
    }
    Ok(())
}

/// `series/observations` from `start`; `realtime` is a ready query fragment (or empty).
fn observations(
    p: &mut Probe,
    key: &str,
    id: &str,
    start: &str,
    realtime: &str,
) -> Result<(u16, Value), ProbeError> {
    let url = format!(
        "{BASE}/series/observations?api_key={key}&file_type=json&series_id={id}\
         &observation_start={start}&limit=100000{realtime}"
    );
    let reply = p.get(Source::Fred, &url)?;
    Ok((reply.status, reply.json()?))
}

/// Status, error text, row count, first and last row, earliest `realtime_start`.
fn summarize(status: u16, v: &Value) -> Value {
    let obs = v
        .get("observations")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    json!({
        "status": status,
        "error": v.get("error_message"),
        "rows": obs.len(),
        "first": obs.first(),
        "last": obs.last(),
        "min_realtime_start": obs
            .iter()
            .filter_map(|o| o.get("realtime_start").and_then(Value::as_str))
            .min(),
    })
}
