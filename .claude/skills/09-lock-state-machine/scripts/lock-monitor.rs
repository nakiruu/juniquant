// SKETCH, not compiled, not normative. Crate home: execution/src/locks/monitor.rs (scaffold.md).
// Lock monitor / pending-order timer. Runs at the start of the execution window (10:00 ET) every
// trading day: orders are placed only in-session (10-execution-schwab/EXECUTION-WINDOW.md), never
// at the open.
//
// There is NO stored lock state. The monitor does not flip states, does not write lock rows, and
// does not decide what is allowed. It (1) RELEASES due pending intents to the normal order
// generator on their first permitted day, each one pre-checked against an order history freshly
// read over REST, and (2) emits DERIVED display events (LockTransition / LockExpiry) computed from
// that history. Nothing may authorize an order from those events.
//
// What changed from the earlier sketch (09 SKILL.md sections 6a, 12; PENDING-ORDERS.md section 3):
//  * The monitor does NOT submit. It releases an intent; the generator recomputes the quantity from
//    the fresh price and NAV, applies the spread gate and the ladder, and the placement goes through
//    the per-ticker OrderGate and `authorize_and_reserve` (authoritative). A permit is never carried
//    out of the gate, never queued, never sent through a channel.
//  * The halt flag is checked first: halted => release nothing, leave every row Pending.
//  * No ANCHOR_MAX_AGE: the run's pre-checks share one REST read taken at the start of the run
//    (N = 0), refreshed per ticker after a release; every placement does its own fresh read.

use chrono::NaiveDate;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::events::{LockEvent, LockExpiry, OrderEvent};
use crate::locks::authorize::{authorize, Denied};
use crate::locks::halt::HaltState;
use crate::locks::order_history::OrderHistory;
use crate::locks::pending::PendingStore;
use crate::risk::restricted::RestrictedList;
use crate::locks::calendar::TradingCalendar;

pub struct LockMonitor {
    pending: PendingStore,          // pending_orders.parquet; a queue, not an authority
    calendar: TradingCalendar,      // the single calendar: NYSE rules + overrides (09 SKILL.md section 4)
    restricted: RestrictedList,     // config/restricted.toml
    history: OrderHistory,          // REST-anchored copy of the orders (never a lock verdict)
    lock_event_tx: mpsc::Sender<LockEvent>,
    order_event_tx: mpsc::Sender<OrderEvent>,  // carries OrderEvent::ReleaseIntent, never a permit
}

impl LockMonitor {
    /// Run at the window start (10:00 ET). `today` is the America/New_York session date from the
    /// clock, not from a price file.
    pub async fn daily_check(&mut self, today: NaiveDate) -> Result<(), LockError> {
        // 0. Halt flag: a halted system releases nothing (SKILL.md section 8b).
        let halt = HaltState::read();                 // unreadable/unparseable => treated as halted
        if let Some(kind) = halt.active() {
            self.pending.alert_all_unresolved(&format!("halted: {kind:?}")).await?;
            return Ok(());
        }

        let due = self.pending.due(today).await?; // earliest_execution <= today, status = Pending

        // One REST read serves every due row (N = 0 at the start of the run). It is re-read only
        // for a ticker that was just released, since that release changes that ticker's history
        // (a due Buy and a due Sell of one name). One read per row would serialise a REST round
        // trip per account in front of every release, at the moment the window opens.
        let mut cached = None;
        let mut released = std::collections::HashSet::new();

        for order in due {
            // A pre-check only; the authoritative check is the one at placement, inside the
            // ticker gate, which always reads fresh.
            if cached.is_none() || released.contains(&order.ticker) {
                match self.history.ready_for_submission(chrono::Utc::now(), &self.calendar).await {
                    Ok(h) => cached = Some(h),
                    Err(_incomplete) => {
                        cached = None;
                        self.pending.alert_unresolved(&order.pending_id).await?;
                        continue;
                    }
                }
            }
            let Some(history) = cached.as_ref() else { continue };
            let shares_held = self.pending.broker_shares(&order.ticker).await?;

            match authorize(
                &order.ticker, order.side, today, history,
                &self.restricted, &halt, shares_held, &self.calendar,
            ) {
                // Still blocked (a new same-direction counted order, possibly placed outside
                // the system, extended the block): re-date, do not release.
                Err(Denied::BlockedUntil(d)) => {
                    self.pending.extend(&order.pending_id, d).await?;
                    self.lock_event_tx.send(LockEvent::Expiry(self.derived_expiry(&order, d))).await?;
                }
                // An opposite-side order is open with no end date: never release; alert.
                Err(Denied::BlockedOpenOrder(_)) => {
                    self.pending.alert_unresolved(&order.pending_id).await?;
                }
                Err(Denied::Restricted) => self.pending.cancel(&order.pending_id, "restricted").await?,
                Err(Denied::NothingToSell) => self.pending.cancel(&order.pending_id, "nothing_to_sell").await?,
                // Fail closed: leave Pending, release nothing, alert. Retry next run.
                Err(Denied::Halted(_)) | Err(Denied::NonTradingDay) | Err(Denied::Unmapped)
                | Err(Denied::HistoryIncomplete(_)) => {
                    self.pending.alert_unresolved(&order.pending_id).await?;
                }
                // The pure verdict allows it today. Re-validate the signal, then RELEASE.
                Ok(_verdict) => match self.revalidate_signal(&order).await? {
                    RevalidationDecision::Release => {
                        // The generator recomputes quantity, runs the spread gate and ladder, takes
                        // the ticker gate and asks authorize_and_reserve again at placement.
                        // Mark first, then send: a crash between the two loses a release (the row
                        // is Fired with no order, which the owner sees) but can never repeat one.
                        // The handler's dedup set is in memory and does not survive a restart.
                        self.pending.mark_fired(&order.pending_id).await?; // = released
                        if let Err(e) = self.order_event_tx
                            .send(OrderEvent::ReleaseIntent {
                                pending_id: order.pending_id,
                                ticker: order.ticker.clone(),
                                side: order.side,
                                // The generator derives a veto exit from portfolio_targets (veto_active && w_current > 0).
                                // NO quantity field, NO permit field.
                            })
                            .await
                        {
                            self.pending.alert_unresolved(&order.pending_id).await?;
                            return Err(e.into());
                        }
                        released.insert(order.ticker.clone());
                    }
                    RevalidationDecision::Cancel(reason) => {
                        self.pending.cancel(&order.pending_id, &reason).await?;
                    }
                },
            }
        }

        // Pending rows past earliest_execution by more than 5 trading days.
        self.pending.flag_expired(today, &self.calendar).await?;
        Ok(())
    }

    /// Derived event: `first_permitted` is a function of history. Emitted at
    /// open on the day it is reached, or when a block is extended. Display
    /// only; never read back by `authorize`.
    fn derived_expiry(&self, order: &PendingOrder, first_permitted: NaiveDate) -> LockExpiry {
        LockExpiry {
            event_id: Uuid::new_v4(),
            timestamp: chrono::Utc::now(),
            ticker: order.ticker.clone(),
            side: order.side,
            first_permitted,
        }
    }

    async fn revalidate_signal(
        &self,
        order: &PendingOrder,
    ) -> Result<RevalidationDecision, LockError> {
        let latest = self.pending.latest_signal(&order.ticker).await?;
        let snapshot: SignalSnapshot = serde_json::from_str(&order.signal_snapshot)?;

        // A veto never cancels a queued SELL, and a vetoed name's sell does not go
        // through the signal comparison at all: the exit is released on the first
        // permitted day. A veto cancels a queued BUY (and the generator cancels the
        // name's working buys).
        if latest.veto_active {
            return Ok(match order.side {
                Side::Sell => RevalidationDecision::Release,
                Side::Buy => RevalidationDecision::Cancel("veto_triggered".into()),
            });
        }
        // Crisis blocks new capital only; it cancels BUYS and never a sell.
        if latest.regime_factor <= 0.0 && order.side == Side::Buy {
            return Ok(RevalidationDecision::Cancel("regime_crisis".into()));
        }
        // Comparison rules (PENDING-ORDERS.md section 4)
        // Product, not signum: signum(0.0) is 1.0, so a zero forecast would read as a reversal.
        if latest.q_50 * snapshot.q_50 < 0.0 {
            return Ok(RevalidationDecision::Cancel("signal_reversed".into()));
        }
        if latest.q_50.abs() < 0.5 * snapshot.q_50.abs() {
            return Ok(RevalidationDecision::Cancel("signal_weakened".into()));
        }
        Ok(RevalidationDecision::Release)
    }
}

pub enum RevalidationDecision {
    Release,
    Cancel(String),
}

#[derive(Clone)]
pub struct PendingOrder {
    pub pending_id: Uuid,
    pub ticker: Ticker,
    pub side: Side,
    pub target_quantity: Decimal,        // INFORMATIONAL ONLY: never submitted
    pub earliest_execution: NaiveDate,   // first permitted day
    pub signal_snapshot: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub status: String,
}

// Derived events (see 11-rust-event-bus/EVENT-TYPES.md sections 5-6). Emitted by this
// monitor after it recomputes from history; display and notification only.
// LockTransition { ticker, blocked_side_before, blocked_side_after,
//                  first_permitted, trigger_order_id } is emitted when a
// refresh of the order history shows a counted order that newly creates or
// extends a block.
