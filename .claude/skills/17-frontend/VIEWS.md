# Views

The three v1 views. Each reads from one or two Parquet files and, in
the price case, from the Rust quote cache.

## 1. Price Chart

**Purpose:** Show the price history for a ticker and the current price.

**Data:**
- History: `prices.parquet`, trailing 252 trading days for the selected
  ticker.
- Live: the Rust quote cache, polled every 30 seconds.

**Rendering:** A line chart of daily closes. A live marker for the
current price. The live marker is updated on each WebSocket message.
If the last WebSocket message is older than 30 seconds, the marker
turns gray to indicate staleness.

**Ticker selection:** A dropdown listing the tickers in the current
portfolio (from `positions.parquet`) plus any ticker in the order
history. Default is the largest holding by market value.

**What it does not show in v1:** Prediction intervals, TCP bounds, EVT
bounds, regime overlays, conviction scores. These are v2.

## 2. Holdings

**Purpose:** Show the current portfolio.

**Data:** `positions.parquet`.

**Columns:** ticker, shares, average cost, current price, market value,
unrealized P&L, weight, entry date, holding days.

**Refresh:** On page load. On order fill (via a WebSocket message).

**What it does not show in v1:** Conviction multiplier, veto status,
sector breakdown, cash balance. Cash is shown as a single line at the
bottom. Conviction and veto are v2.

## 3. Order History

**Purpose:** Show the order lifecycle.

**Data:** `orders.parquet`, trailing 90 days.

**Columns:** submitted time, ticker, side, quantity, order type, limit
price, status, fill price, fill quantity, slippage in bps.

**Refresh:** On page load. On order fill (via a WebSocket message).

**Filters:** ticker, side, status, date range. Default: last 30 days,
all tickers, all sides, all statuses.

**What it does not show in v1:** Pending orders that have not been
submitted. Those are v2 and belong in a separate "Locked / Pending"
view.

## 4. Layout

A single page. Three panels:

    +--------------------------------------------------+
    |  Header: NAV, cash, regime state (from ledger)   |
    +----------------------------+---------------------+
    |                            |                     |
    |   Price Chart              |   Holdings          |
    |                            |                     |
    |                            |                     |
    +----------------------------+---------------------+
    |                                                  |
    |   Order History                                  |
    |                                                  |
    +--------------------------------------------------+

The header is read once on page load from `portfolio_ledger.parquet`
(most recent row). It is not live in v1.

## 5. What Not to Do

- Do not add views without a skill amendment.
- Do not render a stale price as live.
- Do not show historical prices older than 252 trading days by default.
  The trailing window keeps the query fast.
- Do not allow the user to edit any field. The views are read-only.