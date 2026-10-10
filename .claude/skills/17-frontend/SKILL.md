---
name: frontend
description: Local dashboard for price charts, holdings, and order
  history. Load when touching frontend/ or any Bun/TypeScript code.
---

# Frontend

The frontend is a read-only dashboard served by Bun. It displays a
price chart, the current holdings, and the order history. It polls
Schwab for price updates every 30 seconds while a browser tab is
connected.

**Scope of v1.** Three views. No manual override. No order submission.
No configuration UI. No multi-user. No authentication. It binds to the
LAN and serves a single operator.

**Expansion.** The frontend is expected to grow. Views for predictions,
regime state, conviction, locks, and the ledger are planned but not
built in v1. This skill defines the v1 scope and the constraints that
the expansion must respect. When a lock view is added it renders a derived label
from the lock snapshot / `position_locks`; a **null `first_permitted` /
`lock_expiry_date` means an open order with no end date ("blocked, open
order"), never "unlocked"**, and a halted system (`TRADING_HALTED`, shown from a
read-only export) is displayed prominently.

## 1. The Three Views

See `VIEWS.md` for the layout and the data each reads.

| View | Data source | Refresh |
|---|---|---|
| Price chart | Schwab quotes (polled) + `prices.parquet` (history) | 30s while connected |
| Holdings | `positions.parquet` | On page load + on order fill |
| Order history | `orders.parquet` | On page load + on order fill |

## 2. Transport

**REST** for the initial page load. Bun serves JSON from DuckDB queries
against the Parquet files.

**WebSocket** for updates. When a browser connects, the frontend
subscribes to price updates for the tickers in the portfolio. The
backend polls Schwab every 30 seconds and pushes the new prices to all
connected clients.

See `WEBSOCKET.md` for the message contract and `POLLING.md` for the
polling loop.

## 3. Read-Only

The frontend does not write to any Parquet file. It reads from DuckDB
and forwards Schwab quotes from the backend's cache.

The container mounts the lake read-only:

    /mnt/cache/quant:/data:ro

A write attempt fails at the filesystem level. This is intentional and
is the first line of defense against a frontend bug corrupting the lake.

## 4. Local Access Only

The Bun server binds to the LAN interface, not to `0.0.0.0`. The
Unraid Docker network type is ipvlan. The port is not forwarded from
the WAN.

There is no authentication in v1. The assumption is that the LAN is
trusted and the dashboard is not reachable from the internet. If that
assumption ever changes, authentication becomes mandatory before the
dashboard is exposed.

## 5. What the Frontend Does Not Do

- Does not place orders.
- Does not change or bypass the lock. The lock is derived from order history
  (`09-lock-state-machine`); the dashboard only displays derived labels and
  `position_locks`, which are not authoritative.
- Does not override vetoes.
- Has **no manual override of any kind** and no kill switch or halt-clear button.
  The kill switch (`quant-exec kill`: cancel working orders and halt, never
  liquidate) and every owner action (`halt clear`, `calendar add`, `journal
  resolve`) are Rust commands (`10-execution-schwab` §6d). The constitution's §4
  table lists "manual override" among the Bun/TS layer's responsibilities; that
  contradicts this skill and §1 rule 8 (no exceptions), and is reported for the
  owner to correct in the constitution. This skill governs until then.
- Does not change the regime factor.
- Does not write to any Parquet file.
- Does not store credentials.
- Does not subscribe to the Rust event bus. It talks to the Bun
  server, which talks to DuckDB and to the Schwab quote cache.

The frontend is a viewer. Every write path in the system lives in Rust
or Julia.

## 6. The Schwab Quote Path

The Bun server does not call Schwab directly. It reads from a quote
cache that the Rust execution engine maintains.

**Why:** OAuth tokens live in the Rust process. The Bun server has no
credentials. If Bun held the tokens, a frontend bug could leak them or
exhaust the rate limit.

**How:** The Rust execution engine writes the current quote for each
held ticker to a small in-memory cache and, on every fill or price
update, appends to a lightweight `live_quotes.parquet` file (or an
in-process shared memory region). The Bun server reads the cache or the
file every 30 seconds and forwards the values to connected clients.

**v1 simplification.** The Rust cache may not exist yet. In that case,
the Bun server reads the most recent row from `prices.parquet` for each
ticker and displays that. The price is then end-of-day, not live. This
is acceptable for v1. The live quote path is a v2 addition and is
documented here so the boundary is clear from the start.

## 7. WebSocket Reconnect

Browser tabs disconnect. Laptops sleep. Networks drop. The WebSocket
client reconnects automatically with exponential backoff, capped at 30
seconds. On reconnect, the client re-requests the full state (holdings,
order history, latest prices) rather than assuming it is current.

The server does not maintain per-client state. It broadcasts updates to
all connected clients. A client that missed a message re-requests the
full state on reconnect.

## 8. What Not to Do

- Do not write to the Parquet lake from the frontend.
- Do not call Schwab directly from Bun. Read from the Rust cache.
- Do not add authentication-less WAN exposure. If the dashboard becomes
  reachable from outside the LAN, stop and add auth.
- Do not subscribe to the Rust event bus. The frontend talks to Bun,
  not to Rust.
- Do not add a fourth view without updating `VIEWS.md` and this skill.
- Do not cache quotes on the client longer than 30 seconds. The price
  shown must be the price polled within the last 30 seconds.
- Do not render a price older than 5 minutes without marking it stale.
- Do not store OAuth tokens, API keys, or account numbers in the
  frontend.

## 9. Verification

- [ ] A test asserts the server binds to the LAN IP, not 0.0.0.0
- [ ] A test asserts the container mounts the lake read-only
- [ ] A test asserts a write attempt from the frontend fails
- [ ] A test asserts the WebSocket reconnects after a simulated drop
- [ ] A test asserts no OAuth token is present in any Bun process
- [ ] A test asserts the three views render on load

## 10. Related Skills

- `13-parquet-lake` — schemas for `positions.parquet`, `orders.parquet`,
  `prices.parquet`
- `10-execution-schwab` — the Rust process that holds OAuth tokens and
  maintains the quote cache
- `00-construction` §4 — language boundaries