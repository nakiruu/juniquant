// SKETCH, not compiled, not normative. Crate home: execution/src/locks/audit.rs (scaffold.md).
// Daily post-close lock-compliance audit and report. Runs at the 16:40 ET slot.
// Verify every Schwab API detail against the developer portal.
//
// Spec: 09-lock-state-machine/SKILL.md sections 8, 8b, 8c. Contract (constitution section 1, 6):
//   - rescan ALL order history (Schwab, every account, including orders placed
//     outside the system; the audit journal; lock_external.csv; the TRADE transactions net)
//   - any violation of the 5-trading-day opposite-side rule halts trading (halt kind VIOLATION,
//     owner-only clear) and writes a CRITICAL `lock_violation_detected` row
//   - any order (ANY status, zero-fill included), fill or position in a restricted ticker in a
//     Schwab account or the journal halts trading and writes a CRITICAL row; the system NEVER
//     sells to "fix" it. The External source is excluded (Fidelity ICE is irrelevant).
//   - a dated compliance report is ALWAYS written, clean or not
//   - COULD NOT VERIFY IS NOT A VIOLATION: an unreadable history is retried for a bounded
//     period, then the outcome is INCOMPLETE with a distinct event type (`lock_audit_incomplete`)
//     and an INCOMPLETE halt that a clean automatic re-run clears. It never writes
//     `lock_violation_detected` and never sets a VIOLATION halt.
//   - violations the owner has acknowledged (acknowledged_violations.json) are reported as
//     ACKNOWLEDGED and do not re-halt while the pair stays inside the 45-day window
//
// Exit codes: 0 compliant, 2 violation or anomaly (trading halted), 3 incomplete.

use std::collections::{BTreeMap, BTreeSet};
use chrono::{NaiveDate, Utc};
use serde::Serialize;

use crate::audit::AuditHandle;
use crate::locks::calendar::TradingCalendar;
use crate::locks::halt::{self, HaltKind};
use crate::locks::order_history::{BlockEnd, HistorySource, HistoricalOrder, OrderHistory};
use crate::risk::restricted::RestrictedList;

const LOOKBACK_DAYS: i64 = 45; // same safe window as authorize; never above 60 (window conflict is a live test)
// Retry schedule after the 16:40 start, in minutes (bounded; placeholder).
const RETRY_MINUTES: [u64; 5] = [0, 5, 15, 30, 60];

#[derive(Serialize)]
struct Violation {
    id: String,         // sha256(ticker | a.source | a.account | a.order_id | b.source | b.account | b.order_id | b.placed_et)
    kind: &'static str, // "opposite_side_inside_window" | "restricted_ticker_activity"
    ticker: String,
    first: OrderRef,    // the earlier counted order
    second: Option<OrderRef>, // the later opposite-side order (None for restricted)
    first_permitted: Option<NaiveDate>, // None while `first` is open (no end date)
    acknowledged: bool,
    detail: String,
}

#[derive(Serialize, Clone)]
struct OrderRef {
    account: String,    // label or hash, never a raw account number
    source: String,     // Schwab | Journal | External | Transactions
    order_id: Option<String>,
    side: String,
    placed_et: NaiveDate,
    status: String,
    filled_qty: String,
}

#[derive(Serialize)]
struct Report {
    audit_date: NaiveDate,
    generated_at: String,
    outcome: &'static str, // COMPLIANT | VIOLATION | INCOMPLETE
    lookback_from: NaiveDate,
    sources: Vec<SourceStatus>,
    accounts_covered: Vec<String>,
    orders_scanned: usize,
    orders_counted: usize,
    calendar_rules_version: String,   // git hash of locks/calendar.rs
    calendar_overrides_sha256: String,
    calendar_overrides: Vec<String>,  // dates, for the evidence trail
    last_calendar_sync_ok: Option<String>,
    external_file_sha256: String,
    external_file_modified: String,
    restricted_list_sha256: String,
    symbol_map_sha256: String,
    violations: Vec<Violation>,
    acknowledged: Vec<String>,
    anomalies: Vec<String>, // SELL_SHORT, unknown enum value, working > 1 session, fill without order, unknown account
    known_limit: &'static str,
}

pub async fn run(
    today: NaiveDate, // ET session date of the audit (after the close)
    cal: &TradingCalendar,
    restricted: &RestrictedList,
    audit: &AuditHandle,
) -> anyhow::Result<i32> {
    let from = today - chrono::Duration::days(LOOKBACK_DAYS);

    // Fresh REST-only read (no stream): accountNumbers, then per-account orders over the audit
    // window and the open-status queries, plus journal (merged as in 09 section 3.7) and
    // lock_external.csv; also `GET /accounts/{hash}/transactions?types=TRADE` as the fill net.
    // Same parsing/status mapping as `lock::authorize`; never "partial ok".
    // BOUNDED RETRY (09 section 8c): a failed read is retried at RETRY_MINUTES before it is final.
    let history = {
        let mut last_err = None;
        let mut got = None;
        for (i, wait) in RETRY_MINUTES.iter().enumerate() {
            if *wait > 0 { tokio::time::sleep(std::time::Duration::from_secs(*wait * 60)).await; }
            match OrderHistory::load_range(Utc::now(), from, today, cal).await.and_then(|h| h.is_complete_for(today).map(|_| h)) {
                Ok(h) => { got = Some(h); break; }
                Err(e) => {
                    if i == 0 {
                        audit.append_durable(audit_row("lock_audit_incomplete", "WARN", None,
                            serde_json::json!({ "attempt": i, "error": e.to_string() }))).await?;
                    }
                    last_err = Some(e);
                }
            }
        }
        match got { Some(h) => Ok(h), None => Err(last_err.expect("at least one attempt")) }
    };

    let mut violations: Vec<Violation> = vec![];
    let mut anomalies: Vec<String> = vec![];
    let acked: BTreeSet<String> = halt::acknowledged_violation_ids()?; // append-only file

    let outcome = match &history {
        Err(e) => {
            // COULD NOT VERIFY: not a violation. Distinct event type, INCOMPLETE halt
            // (auto-clearable by a clean re-run; the hourly re-run and the 09:00 refresh try).
            audit.append_durable(audit_row(
                "lock_audit_incomplete", "CRITICAL", None,
                serde_json::json!({ "error": e.to_string(), "retries": RETRY_MINUTES }),
            )).await?;
            halt::set(HaltKind::Incomplete, "lock audit: history could not be verified", audit).await?;
            "INCOMPLETE"
        }
        Ok(history) => {
            // 1) Opposite-side pairs. Group by canonical ticker across ALL accounts.
            //    Only COUNTED orders are `a`; OPEN orders are NOT skipped (an open `a` blocks
            //    every later placement). `b` is any counted opposite-side order (or a
            //    blocks_both order on either side).
            let mut by_ticker: BTreeMap<&str, Vec<&HistoricalOrder>> = BTreeMap::new();
            for o in history.orders().filter(|o| o.counted()) {
                by_ticker.entry(o.ticker.as_str()).or_default().push(o);
            }
            for (ticker, orders) in &by_ticker {
                for a in orders {
                    // block_end(a): BlockEnd::Open while `a` is open (no end date), otherwise the
                    // first permitted day = last fill date (placement date if none) + 5 trading days.
                    let end = a.block_end(cal);                     // BlockEnd, never an Option of Option
                    let first_permitted = match end { BlockEnd::Open => None, BlockEnd::FirstPermitted(d) => Some(d) };
                    for b in orders {
                        let opposite = a.blocks_both || b.blocks_both || b.side == a.side.opposite();
                        if !opposite || std::ptr::eq(*a, *b) { continue; }
                        // b is inside a's block: placed on/after a's placement day and before
                        // block_end(a) (open `a`: every later placement; a late-filling `a` blocks
                        // b placed between its placement and its last fill, and until fill + 4
                        // trading days).
                        let inside = b.placed_et >= a.placed_et
                            && first_permitted.map_or(true, |f| b.placed_et < f);
                        if inside {
                            violations.push(mk_violation("opposite_side_inside_window", ticker, a, Some(b),
                                first_permitted, &acked,
                                "opposite-side order placed inside the block (open order: no end date)"));
                        }
                    }
                }
            }
            dedup_symmetric(&mut violations); // each pair once

            // 1b) Owner decision B2 (2026-10-10): an order that ended with ZERO fills is not
            //     locked, so an opposite-side order placed while it was open is NOT a finding.
            //     Only an INFO count goes into the report; no anomaly, no WARN, no halt.
            let zero_fill_overlaps = history.zero_fill_overlaps(cal).count();

            // 2) Restricted tickers: ANY order (every status, zero-fill, cancelled, rejected),
            //    fill or position, in a Schwab account or the journal. NOT filtered by `counted()`.
            //    (Fidelity ICE activity is irrelevant and is excluded via the External source.)
            for o in history.orders() {
                if restricted.contains(&o.ticker) && o.source != HistorySource::External {
                    violations.push(mk_violation("restricted_ticker_activity", o.ticker.as_str(), o, None,
                        None, &acked,
                        "order/fill in a restricted ticker in a Schwab account: anomaly; halt; do NOT auto-sell"));
                }
            }
            // A restricted position is also found by reconciliation (same row type); checked here too.
            for p in history.broker_positions().filter(|p| restricted.contains(&p.ticker) && p.has_quantity()) {
                violations.push(mk_position_violation(p, today, &acked));
            }

            // 3) Anomalies (WARN; CRITICAL if inside a block): short instructions, unknown enum
            //    values, stale working orders (GTC), a fill whose orderId is not in the history,
            //    unknown account.
            for o in history.orders() {
                if o.is_short_instruction() { anomalies.push(format!("{}: short instruction in {}", o.ticker, o.account)); }
                if o.status.is_unknown()    { anomalies.push(format!("{}: unknown status counted open ({})", o.ticker, o.status.raw())); }
                if o.is_working_older_than_one_session(today, cal) {
                    anomalies.push(format!("{}: order working longer than one session (DAY only)", o.ticker));
                }
            }
            for w in history.fills_without_order() {
                anomalies.push(format!("{}: fill without an order in the history ({})", w.ticker, w.order_id));
            }

            let live: Vec<&Violation> = violations.iter().filter(|v| !v.acknowledged).collect();
            if live.is_empty() {
                "COMPLIANT"
            } else {
                for v in &live {
                    audit.append_durable(audit_row(
                        "lock_violation_detected", "CRITICAL", Some(&v.ticker),
                        serde_json::to_value(v)?,
                    )).await?;
                    if v.kind == "restricted_ticker_activity" {
                        audit.append_durable(audit_row(
                            "restricted_security_blocked", "CRITICAL", Some(&v.ticker),
                            serde_json::json!({ "source": "daily_audit", "detail": v.detail }),
                        )).await?;
                    }
                }
                let kind = if live.iter().any(|v| v.kind == "restricted_ticker_activity") { HaltKind::Restricted } else { HaltKind::Violation };
                halt::set(kind, "lock audit: violation detected", audit).await?;  // owner-only clear
                "VIOLATION"
            }
        }
    };

    // Snapshot of what was scanned (append-only evidence, 13-parquet-lake #25), then the
    // report; both before the "audit completed" row. A failure here is INCOMPLETE (halt kind
    // INCOMPLETE after the retry), not a violation.
    if let Ok(h) = &history { h.append_snapshot_to_order_history_parquet(today).await?; }
    let report = build_report(today, from, outcome, &history, cal, restricted, violations, anomalies);
    write_report(&report)?; // temp + fsync + atomic rename

    audit.append_durable(audit_row(
        "lock_audit_completed", "INFO", None,
        serde_json::json!({ "outcome": outcome }),
    )).await?;

    // A clean pass clears an INCOMPLETE-only halt automatically (never a VIOLATION halt): 09 section 8b.
    if outcome == "COMPLIANT" { halt::auto_clear_incomplete(audit).await?; }

    Ok(match outcome { "COMPLIANT" => 0, "VIOLATION" => 2, _ => 3 })
}

fn write_report(r: &Report) -> anyhow::Result<()> {
    let dir = format!("{}/compliance", std::env::var("QUANT_STATE")?);   // $QUANT_STATE/compliance (IO-CONTRACT §1)
    std::fs::create_dir_all(&dir)?;
    let base = format!("{dir}/lock-compliance-{}", r.audit_date);
    // Never overwrite: a re-run on the same date writes `.rerun-N`.
    let mut path = format!("{base}.json");
    let mut n = 1;
    while std::path::Path::new(&path).exists() { path = format!("{base}.rerun-{n}.json"); n += 1; }
    let tmp = format!("{path}.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(r)?)?;
    std::fs::File::open(&tmp)?.sync_all()?;
    std::fs::rename(&tmp, &path)?;
    // A short human-readable .md with the same content is written the same way.
    Ok(())
}

// Helpers elided in this sketch: mk_violation, mk_position_violation, order_ref, dedup_symmetric,
// audit_row, build_report; OrderHistory::{zero_fill_overlaps, fills_without_order, broker_positions}.
// The halt module (locks/halt.rs) owns TRADING_HALTED: set / clear / auto_clear_incomplete /
// acknowledged_violation_ids (09 section 8b); nothing else writes that file.
