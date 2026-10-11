---
name: rust-event-bus
description: Tokio-based event infrastructure for Rust modules (later
  phase). Load when touching execution/src/eventbus/, execution/src/audit/,
  channels, event handlers or the audit journal.
---

# Rust Event Bus

The event bus is the spine of the Rust execution engine. Every module
communicates through it: market data arrives, order events fire, the
derived lock events are published, and risk gates react. It is built on Tokio and uses bounded
channels throughout.

## 1. Architecture

The bus is not a generic pub/sub broker. It is a set of **typed channels**,
one per event class, with explicit backpressure and idempotency rules.

```
┌──────────────────────────────────────────────────────────────────┐
│                    EVENT BUS (Tokio)                              │
│                                                                   │
│  Channel: market_data  (bounded, capacity 1000)                  │
│  Channel: order_events (bounded, capacity 100)                   │
│  Channel: lock_events  (bounded, capacity 50)                    │
│  Channel: regime       (bounded, capacity 10)                    │
│  Channel: filing       (bounded, capacity 20)                    │
│  Channel: audit        (bounded, capacity 10000)                 │
└──────────────────────────────────────────────────────────────────┘
```

Each channel has one or more producers and one or more subscribers. The
subscriber count per channel is fixed at compile time. Dynamic
subscription is not supported.

## 2. Bounded Channels, Always

Every channel is bounded. Unbounded channels mask backpressure and lead
to OOM under load . The bus uses `tokio::sync::mpsc` for
single-consumer channels and `tokio::sync::broadcast` for multi-consumer
channels.

See `CHANNELS.md` for the capacity per channel and the overflow policy.

## 3. Event Types

See `EVENT-TYPES.md`. The bus dispatches nine event classes:

| Event | Priority | Channel |
|---|---|---|
| `MarketData` | P3 | `market_data` |
| `OrderSubmitted` | P0 | `order_events` |
| `OrderFilled` | P0 | `order_events` |
| `OrderCancelled` | P0 | `order_events` |
| `LockTransition` (derived) | P0 | `lock_events` |
| `LockExpiry` (derived) | P0 | `lock_events` |
| `RegimeFlip` | P1 | `regime` |
| `FilingEvent` | P1 | `filing` |
| `AuditEvent` | P0 | `audit` |

`LockTransition` and `LockExpiry` are **derived** events: the lock is a pure
function of order history with no stored state (`09-lock-state-machine`), and
no handler may authorize an order from them. The audit event types
`lock_violation_detected`, `restricted_security_blocked` and
`lock_audit_completed` travel as `AuditEvent` (`EVENT-TYPES.md` §9); the first
two are `CRITICAL` and halt trading.

**Streamer messages feed the existing classes; no new class is added.**
`LEVELONE_EQUITIES` updates become `MarketData` (conflated per symbol);
`ACCT_ACTIVITY` messages are notifiers handled inside the order-history layer
(`09-lock-state-machine` §3.2): each one triggers a REST re-read of the order and
only the REST result emits `OrderSubmitted` / `OrderFilled` / `OrderCancelled`.
An unknown message type, sequence gap, disconnect or missed heartbeat marks the
history dirty (and writes `order_history_disagreement` or an INFO/WARN audit
row); it never emits a lock event by itself. The Streamer session
(`schwab/streamer.rs`) is one connection per user, shared by both services.
Confidence labels for every API fact: `10-execution-schwab/SCHWAB-API-REFERENCE.md`.

Priority determines latency budget, not channel. See `CHANNELS.md` §3.

Halting is not an event: the halt flag (`$QUANT_STATE/TRADING_HALTED`,
`09-lock-state-machine` §8b) is written by the `halt` module and read directly by
`authorize`, `submit_*`, the pending timer and the ladder. The audit event types
that accompany it (`trading_halted`, `halt_cleared`, `kill_switch_activated`,
`manual_override`, `lock_audit_incomplete`, ...) travel as `AuditEvent`
(`EVENT-TYPES.md` §9). The handler's `halt_trading(...)` in §6 below calls that
module.

## 4. Idempotency

Every event carries an `event_id` (UUID). Handlers are idempotent: if
the same `event_id` is processed twice, the second processing is a no-op.

The bus maintains a deduplication set of processed `event_id`s with a
TTL of 24 hours. The set is a `DashSet<Uuid>` (concurrent hash set). A
handler checks the set before processing and inserts after.

**Why:** Tokio channels do not guarantee exactly-once delivery under all
conditions. A retry on error, a duplicate WebSocket frame, or a
reconnect can deliver the same event twice. Idempotency at the handler
boundary is the fix.

## 5. The Audit Channel

`audit` is a special channel. It has one subscriber (the audit writer)
and high capacity (10000). Every other channel's handlers write two
`AuditEvent`s to this channel per event: an intent record before the
side effect and an outcome record after it (§6).

The audit writer is the only component that writes the audit record, and
the record is append-only. Its system of record is a dedicated SQLite
journal, `audit.db` (`$QUANT_STATE/audit.db`, `13-parquet-lake/IO-CONTRACT.md` §1;
host `/mnt/cache/appdata/quant/audit.db`), not Parquet. Parquet cannot be appended row by row, and
a batch held in memory is not durable. See `scripts/audit-writer.rs`.

- **Journal settings.** `journal_mode=WAL`, `synchronous=FULL`. The writer
  reads both back at startup and refuses to start if either is not in
  effect. The file lives on the cache pool, never on `/mnt/user` or the
  array.
- **`append_durable`.** One insert, committed before the call returns. A
  handler sends an `AuditRequest { event, ack }` on the bounded `audit`
  channel and awaits the `oneshot` ack. The writer drains whatever is
  queued (at most 256) into one transaction, so concurrent appends share
  one fsync (group commit). Every ack is sent after `COMMIT`, never
  before. A failed commit acks every waiter with an error and stops the
  writer; the caller halts trading.
- **Append-only, mechanically.** SQL triggers on `audit_events` (and on
  `audit_export`) `RAISE(ABORT)` on every `UPDATE` and `DELETE`. The writer
  never uses `INSERT OR REPLACE` (it deletes), sets
  `recursive_triggers=ON`, and checks at startup that all four triggers
  exist. Triggers do not stop `DROP TABLE`; every other process opens the
  file read-only (`mode=ro`), and only the writer's container has it
  read-write.
- **No truncation, no deletion.** Exported days stay in the journal
  (~20K rows/year). The `audit_export` table marks what has been exported.
- **Julia-originated events** (`veto_triggered`, `regime_flip`,
  `model_retrain_aborted`) never touch `audit.db`. Julia writes
  `<event_id>.json.tmp` into the spool directory
  `$QUANT_STATE/audit-spool/` (`13-parquet-lake/IO-CONTRACT.md` §1), fsyncs,
  and renames it to `<event_id>.json` (atomic on one filesystem). The Rust
  spool ingester reads `*.json`, calls `append_durable`, and deletes the
  file only after the ack. A crash between ack and delete re-reads the file
  as a `Duplicate` (unique `event_id` + phase key). A malformed file is
  moved to `rejected/` and a `CRITICAL` row is written. Rust stays the sole
  audit writer.
- **Nightly export.** At the 18:00 slot, before the rsync backup, an export
  job writes each closed UTC day (`day < today`) to
  `$QUANT_ROOT/lake/audit_log/date=YYYY-MM-DD/data.parquet`: write to the
  dot-temp `.data.parquet.tmp.<pid>.<unix_ns>` in the same directory
  (`13-parquet-lake/IO-CONTRACT.md` §2),
  fsync, read back, verify row count and a SHA-256 content digest against
  the journal, rename atomically, fsync the directory, and only then insert
  the day into `audit_export`. The Parquet schema is unchanged
  (`13-parquet-lake/SCHEMAS.md` #21). `audit_log` is now a partitioned
  dataset; the journal is the system of record for unexported days.

If the audit channel is full, the bus applies **backpressure to all
other channels**. Trading halts. This is intentional: no order can be
processed without an audit record.

## 6. Handler Lifecycle

Every handler follows the same pattern: claim the event, write the
**intent** to the audit log, act, write the **outcome**, and release the
claim only if the action failed.

```rust
async fn handle_event(
    event: Event,
    dedup: &DashSet<Uuid>,
    audit: &AuditHandle,
) -> Result<(), HandlerError> {
    let id = event.event_id();

    // 1. Idempotency: claim the event. A duplicate is a no-op.
    if !dedup.insert(id) {
        return Ok(());
    }

    // 2. Write-ahead audit. The intent record is durable before any side
    //    effect. If it cannot be written, nothing happens (constitution §8).
    if let Err(e) = audit.append_durable(AuditEvent::intent(&event)).await {
        dedup.remove(&id); // nothing was done; allow a later retry
        return Err(e.into());
    }

    // 3. Process: the only step with side effects.
    let result = process(&event).await;

    // 4. Outcome record. The side effect has already happened, so a failure
    //    here is never answered by re-running `process`. It sets the global
    //    halt flag and surfaces the error. Queued, not awaited: the durable intent
    //    already guards a crash (startup resolves an intent with no outcome against
    //    the broker), and awaiting a second fsync would delay the next order.
    //    A failed commit stops the writer, which halts trading.
    if let Err(e) = audit.append_deferred(AuditEvent::outcome(&event, &result)).await {
        halt_trading("audit outcome write failed");
        return Err(e.into());
    }

    // 5. Release the claim only if processing failed, so the sender's retry
    //    can run. A success keeps the claim for the dedup TTL.
    if result.is_err() {
        dedup.remove(&id);
    }
    result
}
```

Rules this ordering enforces:

- **Audit before action.** The earlier ordering (process, then audit)
  could submit an order to Schwab and then fail to record it. With the
  intent record first, an action that happened is always on record.
- **The claim does not outlive a failure.** Inserting into the dedup set
  and then failing used to turn the retry into a silent no-op, losing the
  event.
- **`append_durable` means durable.** It returns only after the record is
  committed to the SQLite journal with `synchronous=FULL` (§5), not when
  it is queued in the channel. A queued-but-uncommitted record does not
  satisfy step 2.
- **An intent with no outcome is an unresolved action.** If the process
  crashes or panics between steps 2 and 4, the audit log holds an intent
  with no matching outcome. On startup, scan for these and resolve each
  one against the broker (`10-execution-schwab` reconciliation) before
  trading resumes. The scan runs on the journal (`unresolved_intents` in
  `scripts/audit-writer.rs`: `phase = 'intent'` with no row of the same
  `event_id` and `phase = 'outcome'`), covers all days including exported
  ones, and the resolution is itself an appended outcome row
  (`details.resolution`). Trading stays blocked until the list is empty. The in-memory dedup set is empty after a restart, so
  this scan is what prevents a replayed event from acting twice.
- **A retry can follow a partly completed side effect.** Handlers that
  call the broker must therefore be idempotent at the broker. Schwab's Order
  schema has **no client order id** (only a free-text `tag`, whether it can be
  set and echoed is UNVERIFIED): idempotence comes from the journal (`intent_id`
  recorded before the send; `send_unknown` when the outcome is ambiguous) and
  the adoption rule that matches an ambiguous send to a Schwab order by account,
  symbol, side, quantity and time (`09-lock-state-machine` §3.7), never from a
  blind re-POST and never from a made-up client id.

Both records for one event share its `event_id` and are distinguished by
`details.phase` (`"intent"` / `"outcome"`). The Parquet export has no
primary key, so no schema change is needed; the journal copies the phase
into its own `phase` column and enforces uniqueness on `event_id` + phase.

## 7. Backpressure

Bounded channels provide backpressure. When a channel is full, `send()`
awaits until space is available. The sender is blocked, not the receiver.

The overflow policy per channel is in `CHANNELS.md` §4. The default is
**block the sender**. For `market_data`, the policy is **drop oldest**,
because stale quotes are worse than no quotes.

## 8. What Not to Do

- Do not use unbounded channels. Every channel is bounded.
- Do not use `tokio::sync::broadcast` for single-consumer channels. Use
  `mpsc` for lower overhead.
- Do not skip the idempotency check. Duplicate events are real.
- Do not process an event before checking `event_id`. The check is first.
- Do not perform a side effect before the intent record is durable.
- Do not leave an event in the dedup set after `process` failed.
- Do not re-run `process` because the outcome record failed to write.
- Do not write the audit record from any other process or language. Julia
  uses the spool; Rust handlers use `append_durable`.
- Do not truncate, rewrite, or `DELETE` from the journal, and do not use
  `INSERT OR REPLACE` on it.
- Do not batch audit rows in memory across a commit. The group is whatever
  is already queued, and it is committed before any ack.
- Do not copy `audit.db` with `rsync` or `cp` while the writer is running.
  Use `sqlite3 audit.db ".backup ..."` (see `13-parquet-lake/UNRAID-STORAGE.md`).
- Do not poll channels where events belong. The bus is push-based.
- Do not add a new channel without updating `CHANNELS.md` and
  `EVENT-TYPES.md` in the same commit.

## 9. Verification

- [ ] A test asserts every channel is bounded
- [ ] A test asserts idempotency (same event twice produces one result)
- [ ] A test asserts backpressure (full channel blocks sender)
- [ ] A test asserts the intent record is durable before `process` runs,
      and that a failing intent write prevents `process` from running
- [ ] A test asserts a failed `process` releases the dedup claim so a
      retry runs, while a successful one keeps it
- [ ] A test asserts a failing outcome write sets the halt flag and does
      not re-run `process`
- [ ] A test asserts startup finds an intent with no outcome and does not
      resume trading until it is resolved
- [ ] A test asserts `halt_trading` writes the halt flag through the `halt` module even when the audit journal is unwritable
- [ ] A test asserts the writer does not truncate: restart with existing
      rows, and the row count and `seq` values are preserved
- [ ] A test asserts `UPDATE` and `DELETE` on `audit_events` and
      `audit_export` fail (trigger abort), and that the writer refuses to
      start if a trigger is missing
- [ ] A test asserts group commit: N concurrent `append_durable` calls
      commit in fewer than N transactions, and no ack arrives before
      `COMMIT` (kill the process mid-group; acked rows survive, unacked
      may not)
- [ ] A test asserts a row acked by `append_durable` survives `kill -9`
      of the process
- [ ] A test asserts a spool file is ingested once: crash between ack and
      delete, restart, and the journal has one row; a `*.json.tmp` file is
      ignored; a malformed file is quarantined with a `CRITICAL` row
- [ ] A test asserts the export: row count and digest of
      `date=YYYY-MM-DD/data.parquet` equal the journal's, the temp file is
      never visible at the final path, a corrupted temp file blocks the
      `audit_export` mark, and the current UTC day is never exported
- [ ] A test asserts duplicate `event_id` is a no-op
- [ ] No `unwrap()` on `send()` calls outside of `main.rs`

## 10. Related Skills

- `09-lock-state-machine` — produces the derived `LockTransition` and `LockExpiry`; the compliance audit writes `lock_violation_detected` / `restricted_security_blocked`
- `10-execution-schwab` — produces `OrderSubmitted`, `OrderFilled`
- `06-regime-detection` — produces `RegimeFlip`
- `00-construction` §8 — the audit log contract
- `13-parquet-lake` — `audit_log` export dataset (`SCHEMAS.md` #21)
- `00-construction/ENFORCEMENT.md` §8 — how the contract is enforced