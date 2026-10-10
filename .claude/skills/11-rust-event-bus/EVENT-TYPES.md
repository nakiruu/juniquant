# Event Types

The nine event classes dispatched by the bus. Each has a stable
`event_id`, a timestamp, and a payload.

## 1. MarketData

```rust
struct MarketData {
    event_id: Uuid,
    timestamp: DateTime<Utc>,
    ticker: String,
    bid: Decimal,
    ask: Decimal,
    last: Decimal,
    volume: u64,
    // appended (additive): needed by the spread gate and the quote recorder
    bid_size: Option<u64>,        // SHARES (stream lots x 100, normalised in streamer.rs)
    ask_size: Option<u64>,
    security_status: Option<String>, // raw LEVELONE field 32 ("Normal", "Halted", ...); anything but "Normal" defers sequences
    quote_time_ms: Option<i64>,   // field 34
    source: QuoteSource,          // Stream | Rest
}
```

Source: Schwab Streamer `LEVELONE_EQUITIES` (and REST `/marketdata/v1/quotes`
for names outside the stream set), conflated to the latest state per symbol
before it enters the bus, so the channel's drop-oldest policy cannot lose
recorder data: the quote recorder (`10-execution-schwab/EXECUTION-WINDOW.md`
§2.1) reads the per-symbol state inside `schwab/streamer.rs`, not from this
channel. Subscribers: quote cache, regime monitor, tail monitor.

## 2. OrderSubmitted

```rust
struct OrderSubmitted {
    event_id: Uuid,
    timestamp: DateTime<Utc>,
    order_id: Uuid,
    schwab_order_id: Option<String>,
    ticker: String,
    side: Side,
    quantity: Decimal,          // fractional in Bootstrap/Transition
    order_type: OrderType,      // Limit / Market / ...
    limit_price: Option<Decimal>, // None for Market
    tier: Tier,                 // tier the order was generated under
}
```

Source: execution engine, after Schwab accepts the order. `tier` is the
tier read at submission time (`10-execution-schwab` §2a).

## 3. OrderFilled

```rust
struct OrderFilled {
    event_id: Uuid,
    timestamp: DateTime<Utc>,
    order_id: Uuid,
    ticker: String,
    fill_price: Decimal,
    fill_quantity: Decimal,     // fractional in Bootstrap/Transition
    tier: Tier,                 // tier of the originating order
    venue: Option<String>,
}
```

Source: the REST order read (`executionLegs[]`), triggered by an `ACCT_ACTIVITY`
stream message or a poll. A stream message alone never emits `OrderFilled`: the
Streamer is a notifier and the REST read is the anchor
(`09-lock-state-machine` §3.2). Fill time is REST `executionLegs[].time`.

## 4. OrderCancelled

```rust
struct OrderCancelled {
    event_id: Uuid,
    timestamp: DateTime<Utc>,
    order_id: Uuid,
    reason: Option<String>,
}
```

Source: the REST order read, triggered by an `ACCT_ACTIVITY` message or a poll
(same rule as `OrderFilled`). An order cancelled with zero fills does not count
for the lock; one cancelled after a partial fill does.

## 5. LockTransition (derived)

```rust
struct LockTransition {
    event_id: Uuid,
    timestamp: DateTime<Utc>,
    ticker: String,
    blocked_side_before: Option<Side>, // side that was blocked before the refresh
    blocked_side_after: Option<Side>,  // side that is blocked after it
    first_permitted: Option<NaiveDate>, // D + 5 trading days of the latest blocking order
    trigger_order_id: Option<String>,   // the counted order that created or extended the block
}
```

Source: lock monitor, **after it recomputes the lock from order history**
(for example when a refresh shows a new counted order, wherever it was
placed). This is a **derived, display-and-notification event**. There is no
stored lock state any more (constitution §1, amendment 1.4), so a
`LockTransition` is not a state change that the system persists or reads
back: nothing may authorize an order from it, and `lock::authorize` never
consumes it. It feeds the dashboard and the audit trail. The event class and
the `lock_events` channel are kept so that the nine-class count and
`CHANNELS.md` stay valid.

## 6. LockExpiry (derived)

```rust
struct LockExpiry {
    event_id: Uuid,
    timestamp: DateTime<Utc>,
    ticker: String,
    side: Side,  // the side that is now permitted
    first_permitted: NaiveDate, // renamed from lock_expiry_date: the first permitted day
}
```

Source: lock monitor, at the start of the execution window (10:00 ET) on `first_permitted` (computed from
history), and again when a pending order is re-dated because the block was
extended. Derived and informational, like `LockTransition`; the unlock
condition is `today >= first_permitted`, evaluated by `authorize`, not by
this event's arrival.

## 7. RegimeFlip

```rust
struct RegimeFlip {
    event_id: Uuid,
    timestamp: DateTime<Utc>,
    from_regime: Regime,
    to_regime: Regime,
    probabilities: [f64; 3],
}
```

Source: regime detector, after Viterbi state changes.

## 8. FilingEvent

```rust
struct FilingEvent {
    event_id: Uuid,
    timestamp: DateTime<Utc>,
    ticker: String,
    form_type: String,
    filing_date: NaiveDate,
}
```

Source: SEC EDGAR poll, after a new filing is detected.

## 9. AuditEvent

```rust
struct AuditEvent {
    event_id: Uuid,
    timestamp: DateTime<Utc>,
    event_type: String,
    ticker: Option<String>,
    actor: String,
    details: serde_json::Value,
    severity: Severity,
}
```

Source: every handler, twice per event: an **intent** record before the
side effect and an **outcome** record after it (`SKILL.md` §6). Both carry
the event's `event_id`; `details.phase` is `"intent"` or `"outcome"`.
Subscriber: audit writer. On the `audit` channel the event travels inside
an `AuditRequest { event, ack }`; the `oneshot` ack is what makes
`append_durable` return only after commit (`SKILL.md` §5). Julia-originated
events (`veto_triggered`, `regime_flip`, `model_retrain_aborted`) reach the
writer as JSON spool files carrying the same fields, with `event_id`
generated by Julia.

Lock and compliance event types (all `actor = system`, details are JSON):

| `event_type` | Severity | When |
|---|---|---|
| `lock_violation_detected` | `CRITICAL` | The daily compliance audit (`09-lock-state-machine` §8) found an opposite-side order inside a block (including an order placed outside the system). Halts trading (kind `VIOLATION`, owner-only clear). **Not** written for an unreadable history: that is `lock_audit_incomplete` |
| `restricted_security_blocked` | `CRITICAL` | An order for a ticker on `config/restricted.toml` was rejected pre-trade; or the audit/reconciliation found restricted activity or a position in a Schwab account (halt, no auto-sell) |
| `lock_audit_completed` | `INFO` | The daily audit finished and its dated report was written; `details.outcome` is `COMPLIANT` / `VIOLATION` / `INCOMPLETE` |
| `lock_violation_blocked` | `WARN` | `authorize` denied an order (`details.reason`: `blocked_until`, `blocked_open_order`, `history_incomplete`, `halted`, `non_trading_day`, `unmapped`, `nothing_to_sell`) |
| `lock_transition` | `INFO` | Derived `LockTransition` (§5) |
| `order_history_disagreement` | `WARN` | A stream message and the REST read disagree, or the aggregate `/orders` and the per-account union disagree; history marked dirty and re-read (`09-lock-state-machine` §3.4) |
| `lock_audit_incomplete` | `WARN` / `CRITICAL` | The audit (or reconciliation) **could not verify** (REST or token outage, truncation, unreadable source). Distinct from a violation: `WARN` at the first failure, `CRITICAL` when the retry window ends; sets an `INCOMPLETE` halt that a clean re-run clears (`09` §8c) |
| `trading_halted` | `CRITICAL` (`WARN` for `INCOMPLETE` until the retry ends) | The halt flag was written; `details.kind` is the halt kind (`09` §8b) |
| `halt_cleared` | `WARN` | The halt flag was cleared: `actor = manual` (owner command) or `system` with `details.auto = true` (`INCOMPLETE` / `AUTH_EXPIRED` only) |
| `kill_switch_activated` | `CRITICAL` | The owner's kill switch: working orders cancelled, halt `KILL_SWITCH`; never liquidates (`10` §6d) |
| `manual_override` | `WARN` | Owner supplied a fact or restarted the system; `details.kind` is `halt_clear`, `calendar_add` or `journal_resolve`. Never refers to the lock; no `kind` bypasses `authorize` |
| `calendar_override_added` | `WARN` | `calendar-sync` appended a closure on the first sighting (`details.source`: `schwab_marketdata` / `same_day_signal`) |
| `calendar_sync_suspect_source` | `WARN` | More than 3 calendar mismatches in one run; **every one is still written** (`09-lock-state-machine` §4) |
| `unknown_enum_value` | `WARN` | An order with an unknown status / instruction / asset type was counted for its ticker only (`09` §3.6) |
| `journal_order_not_in_broker` | `WARN` | A journal order that Schwab does not list is counted open (`09` §3.7) |
| `fill_without_order` | `WARN` / `CRITICAL` | A TRADE transaction whose order is not in the history (`09` §3.1); `CRITICAL` inside a block |
| `symbol_unmapped` | `WARN` | A ticker the system wants to trade is not in the canonical symbol map: not tradable (`09` §3.8) |
| `broker_auth_expiring` | `WARN` | The refresh token is within 48 h / 24 h / 6 h of its assumed expiry (`10` OAUTH.md) |
| `quote_recorder_gap` | `WARN` | The quote recorder lost the stream or a REST poll for a period (start, end, reason) |

## 10. Event ID Rules

- Every event has a unique `event_id` (UUID v4).
- The `event_id` is generated by the producer, not the bus.
- The `event_id` is the idempotency key.
- Handlers do not modify `event_id`.
- The audit writer preserves the `event_id` in the audit record.

## 11. What Not to Do

- Do not reuse an `event_id` across events. It is the idempotency key.
- Do not generate `event_id` in the handler. The producer owns it.
- Do not omit `timestamp`. Every event has one.
- Do not add a field to an event without updating this file and the
  audit writer's schema mapping.