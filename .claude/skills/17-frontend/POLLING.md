# Polling

The Bun server polls the Rust quote cache every 30 seconds while at
least one client is connected.

## 1. Why Not Push From Rust

The Rust execution engine could push quotes to the Bun server over an
internal channel. This would eliminate the 30-second poll.

**v1 decision: poll.** The Rust side does not need to know the frontend
exists. The Rust event bus is for order, lock, regime, and filing
events. Market data is on its own channel with drop-oldest behavior,
which means a subscriber that is slow gets dropped messages. The Bun
server polling a shared cache is simpler and decoupled.

**v2 possibility:** A dedicated `frontend_quotes` channel on the Rust
event bus, with the Bun server as a subscriber. This is deferred.

## 2. The Poll Loop

```typescript
async function pollQuotes(
  cache: QuoteCache,
  broadcast: (msg: object) => void,
): Promise<void> {
  while (clients.size > 0) {
    const quotes = await cache.read();
    if (quotes.size === 0) {
      broadcast({ type: "stale", since: lastGoodQuoteTime, reason: "quote_source_unavailable" });
    } else {
      broadcast({ type: "price_update", prices: Object.fromEntries(quotes), as_of: new Date().toISOString() });
      lastGoodQuoteTime = new Date().toISOString();
    }
    await sleep(30_000);
  }
}
```

The loop runs only while `clients.size > 0`. When the last client
disconnects, the loop exits. When a new client connects, the loop
restarts.

## 3. The Quote Cache

In v1, the cache is the most recent row from `prices.parquet` for each
ticker in the portfolio. This is end-of-day data. It does not update
during the trading day.

In v2, the cache is the Rust execution engine's in-memory quote map,
written to a shared location (a file, a memory-mapped region, or a
small IPC channel) that the Bun server reads.

The Bun server does not know which version it is reading. It calls
`cache.read()` and gets back a map of ticker → price. Whether that map
is live or end-of-day is a property of the cache implementation, not of
the frontend.

## 4. Staleness Rules

A price is **stale** if its `as_of` timestamp is older than 60 seconds.
A price is **dead** if it is older than 5 minutes.

| Age | Display |
|---|---|
| < 30s | Normal |
| 30s – 60s | Normal, but re-poll is imminent |
| 60s – 5m | Gray, labeled "stale" |
| > 5m | Red, labeled "stale", with the age shown |

The client computes staleness from the `as_of` field, not from its own
clock. Clock skew between the server and the client is not a problem
because the timestamp is server-authoritative.

## 5. What Not to Do

- Do not poll faster than 30 seconds. The rate limit on Schwab is per
  app, not per user, and the dashboard is not a trading system.
- Do not poll when no client is connected. The loop exits.
- Do not poll Schwab directly from Bun. Poll the cache.
- Do not assume the cache is live. Treat it as opaque and use the
  timestamps.
- Do not render a price without a timestamp. Staleness is only
  detectable if the timestamp is present.