# Channel Configuration

Every channel in the event bus, its type, capacity, subscribers, and
overflow policy.

## 1. Channel Inventory

| Channel | Type | Capacity | Subscribers | Overflow |
|---|---|---|---|---|
| `market_data` | `broadcast` | 1000 | 3 (quote cache, regime monitor, tail monitor) | Drop oldest |
| `order_events` | `mpsc` | 100 | 1 (order manager: tracks the order lifecycle; there is no lock state machine) | Block sender |
| `lock_events` | `mpsc` | 50 | 1 (lock monitor) | Block sender |
| `regime` | `mpsc` | 10 | 1 (portfolio gate) | Block sender |
| `filing` | `mpsc` | 20 | 1 (conviction updater) | Block sender |
| `audit` | `mpsc` | 10000 | 1 (audit writer) | Block sender |

## 2. Why `broadcast` for Market Data

`market_data` has three subscribers that all need every quote. A
`broadcast` channel clones the value for each receiver .
The tradeoff: if one subscriber lags, it returns `RecvError::Lagged`
and skips the oldest messages. For market data, this is
correct — stale quotes are worse than dropped quotes.

The other channels have one subscriber each, so `mpsc` is the right
choice. It has lower overhead and no cloning.

## 3. Priority and Latency

| Priority | Channels | Latency budget |
|---|---|---|
| P0 | `order_events`, `lock_events`, `audit` | < 1 second |
| P1 | `regime`, `filing` | < 5 minutes |
| P3 | `market_data` | Best effort |

P0 channels are on the critical path. A slow handler here delays
execution. P3 is best-effort — if the market data channel fills, the
oldest quotes are dropped, and the downstream consumers catch up on
the next quote.

## 4. Overflow Policies

**Block sender (default).** When the channel is full, `send().await`
waits. The producer is blocked, not the consumer. This applies
backpressure upstream.

**Drop oldest (market_data only).** The `broadcast` channel's native
behavior. When capacity is reached, the oldest message is released
and the sender is not blocked. The lagging receiver gets
`RecvError::Lagged(n)` on its next `recv()` .

## 5. Capacity Rationale

| Channel | Capacity | Rationale |
|---|---|---|
| `market_data` | 1000 | Burst of quotes during volatile open; 1000 covers ~10 seconds at peak |
| `order_events` | 100 | Order events are rare (a few per day); 100 is generous |
| `lock_events` | 50 | Lock transitions are rare; 50 covers a busy day |
| `regime` | 10 | Regime flips are rare (weekly at most) |
| `filing` | 20 | Filings are rare; 20 covers a quarterly cluster |
| `audit` | 10000 | Audit events are frequent; 10000 covers hours of operation |

## 6. What Not to Do

- Do not increase `market_data` capacity above 10000. The memory cost
  grows linearly, and the drop-oldest policy handles overflow.
- Do not set `audit` capacity below 5000. If it fills, all trading
  halts.
- Do not add a subscriber to an `mpsc` channel without changing it to
  `broadcast`. `mpsc` supports one consumer only.
- Do not use `try_send` on P0 channels. If the channel is full, block,
  do not drop.