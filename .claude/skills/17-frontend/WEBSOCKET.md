# WebSocket

The message contract between the Bun server and the browser client.

## 1. Connection

The client connects to `ws://<lan-ip>:<port>/ws`. The server accepts the
connection and immediately sends the current state:

```json
{
  "type": "initial",
  "holdings": [ ... ],
  "orders": [ ... ],
  "prices": { "AAPL": 187.42, "MSFT": 442.10, ... },
  "server_time": "2026-10-10T16:30:05Z"
}
```

The client renders this state and then listens for updates.

## 2. Message Types

Four message types, all JSON:

| Type | Direction | Payload |
|---|---|---|
| `initial` | server → client | Full state on connect |
| `price_update` | server → client | One or more ticker prices |
| `order_update` | server → client | A new or updated order |
| `stale` | server → client | Indicates the quote source has not updated |

### `price_update`

```json
{
  "type": "price_update",
  "prices": {
    "AAPL": 187.55,
    "MSFT": 442.30
  },
  "as_of": "2026-10-10T16:31:00Z"
}
```

Sent every 30 seconds while at least one client is connected. Contains
only the tickers in the current portfolio.

### `order_update`

```json
{
  "type": "order_update",
  "order": {
    "order_id": "...",
    "ticker": "AAPL",
    "side": "Buy",
    "status": "Filled",
    "fill_price": 187.42,
    "fill_quantity": 25
  }
}
```

Sent when a fill or status change is detected. The client re-renders the
order history and, if the fill changes the position, re-fetches holdings.

### `stale`

```json
{
  "type": "stale",
  "since": "2026-10-10T16:25:00Z",
  "reason": "quote_source_unavailable"
}
```

Sent when the server has not received a new quote within 60 seconds.
The client renders the last-known prices with a stale indicator. When
quotes resume, a normal `price_update` clears the indicator.

## 3. Client Reconnect

The client reconnects with exponential backoff:

| Attempt | Delay |
|---|---|
| 1 | 1s |
| 2 | 2s |
| 3 | 4s |
| 4 | 8s |
| 5+ | 30s (cap) |

On reconnect, the server sends the full `initial` message again. The
client discards its previous state and renders the new one.

## 4. Server Broadcast

The server maintains a `Set<ServerWebSocket>` of connected clients.
Every message is broadcast to all clients. There is no per-client
subscription in v1.

A client that wants fewer updates should close the connection. This is
not a real use case in v1; there is one operator with one tab.

## 5. What Not to Do

- Do not send the full order history on every `order_update`. Send only
  the updated order.
- Do not send prices for tickers not in the portfolio. The set is
  fixed at connect time and updated on a fill.
- Do not exceed one message per second. The quote poll is every 30
  seconds; there is no reason to broadcast faster.
- Do not omit the `as_of` field. The client uses it to compute staleness.