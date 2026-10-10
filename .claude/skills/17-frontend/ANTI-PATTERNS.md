# Anti-Patterns

Frontend mistakes that violate the system's rules. If you catch yourself
writing one of these, stop.

## Data Access

### AP-1: Writing to the Parquet lake
**Why it is wrong:** The frontend is read-only. The mount is `:ro`. A
write attempt fails, and if it ever succeeded it would corrupt the
system's book of record.
**Fix:** All writes go through Julia or Rust. The frontend displays, it
does not mutate.

### AP-2: Calling Schwab directly from Bun
**Why it is wrong:** OAuth tokens live in the Rust process. Putting them
in Bun duplicates a credential, expands the attack surface, and risks
rate-limit exhaustion from a frontend bug.
**Fix:** Read from the Rust quote cache. If the cache does not exist
yet, read the last row of `prices.parquet` and label it end-of-day.

### AP-3: Querying DuckDB on every render
**Why it is wrong:** A dashboard that re-queries on every React render
will hammer the cache pool and slow the whole system. The lake is a
single NVMe; concurrent reads during a model training run matter.
**Fix:** Query once on page load, once on a WebSocket update that
changes the underlying data. Cache the result in the component state.

### AP-4: Reading the entire order history
**Why it is wrong:** `orders.parquet` grows unbounded. A query without
a date filter reads every order ever placed.
**Fix:** Default to the last 30 days. Let the user expand the range
explicitly.

## Transport

### AP-5: Polling faster than 30 seconds
**Why it is wrong:** Schwab's rate limit is per OAuth app. The dashboard
sharing an app with the execution engine means a faster poll steals
budget from order submission.
**Fix:** 30 seconds. If a faster update is needed, subscribe to the
Rust event bus in v2.

### AP-6: Polling when no client is connected
**Why it is wrong:** Wasted queries, wasted CPU, no benefit. The
dashboard is idle 99% of the time.
**Fix:** The poll loop exits when `clients.size === 0` and restarts on
connect.

### AP-7: Sending the full state on every update
**Why it is wrong:** The client already has the state. Re-sending it on
every 30-second tick wastes bandwidth and forces a re-render.
**Fix:** Send only the delta. `price_update` sends changed prices.
`order_update` sends one order.

### AP-8: Assuming the WebSocket never drops
**Why it is wrong:** Laptops sleep, networks drop, tabs are closed.
A client that assumes a live connection will show stale data
indefinitely.
**Fix:** Reconnect with exponential backoff. On reconnect, request the
full state.

## Rendering

### AP-9: Rendering a stale price as live
**Why it is wrong:** A price older than 60 seconds is not live. Showing
it without a stale indicator misleads the operator.
**Fix:** Compute staleness from the `as_of` field. Gray at 60s. Red at
5 minutes.

### AP-10: Rendering a price without a timestamp
**Why it is wrong:** Staleness is only detectable if the timestamp is
present. A price without a timestamp cannot be marked stale.
**Fix:** Every price carries its `as_of`. Every render displays it or
uses it to compute the stale class.

### AP-11: Blocking the UI on a slow query
**Why it is wrong:** A DuckDB query against a partitioned Parquet file
can take a second. A synchronous render blocks the whole page.
**Fix:** Render a skeleton or a spinner. The data appears when the
query resolves. The dashboard is not a trading interface; a 1-second
load is acceptable.

## Scope

### AP-12: Adding views without a skill amendment
**Why it is wrong:** The three views are the v1 scope. Adding a fourth
without documenting it breaks the boundary between the frontend and
the rest of the system.
**Fix:** Propose a skill amendment. Update `VIEWS.md`. Then build.

### AP-13: Adding write paths
**Why it is wrong:** The frontend has no write paths in v1. Adding one
(for example, a "cancel order" button) requires a redesign of the
security boundary and the audit trail.
**Fix:** Writes stay in Rust. If the operator needs to cancel an order,
that is a Rust endpoint with its own checks, not a button in the
dashboard.

### AP-14: Adding authentication on the LAN
**Why it is wrong:** The v1 assumption is that the LAN is trusted. Adding
auth for a single-operator local dashboard adds complexity without a
threat model.
**Fix:** If the dashboard is exposed to the WAN, stop. Add auth before
exposure, not after.

### AP-15: Exposing the dashboard to the WAN
**Why it is wrong:** No auth in v1. A WAN-exposed dashboard is an open
door to the portfolio.
**Fix:** Bind to the LAN. Do not port-forward. If remote access is
needed, use a VPN, not a public endpoint.

### AP-16: Storing credentials in the frontend
**Why it is wrong:** The frontend has no credentials to store. Any
credential in the Bun process is a duplicate of a Rust-held token and
a new leak surface.
**Fix:** Keep OAuth tokens in Rust. The frontend reads from the cache.

## Correctness

### AP-17: Recomputing P&L on the client
**Why it is wrong:** P&L is computed in the ledger (`16-portfolio-ledger`)
and in `positions.parquet`. A second computation on the client will
diverge from the source of truth.
**Fix:** Display the values from the source. Do not recompute.

### AP-18: Using the client's clock for staleness
**Why it is wrong:** Clock skew between the server and the client
produces incorrect staleness classifications. A client 30 seconds
behind will mark a fresh price as stale.
**Fix:** Use the server's `as_of` timestamp. The server is the
authority.

### AP-19: Formatting numbers with locale-dependent code
**Why it is wrong:** A dashboard rendered in a browser with a European
locale will show `1,234.56` as `1.234,56`. Confusing for a US
brokerage account.
**Fix:** Format numbers explicitly. Use `Intl.NumberFormat('en-US')` or
a fixed formatter. Do not rely on the browser default.

### AP-20: Assuming the order history is append-only on the client
**Why it is wrong:** Orders change status. A pending order becomes
filled or cancelled. A client that appends without updating shows
duplicates.
**Fix:** Key orders by `order_id`. Update in place on `order_update`.
Do not append.