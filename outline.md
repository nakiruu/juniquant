# Quant Trading System: Functional Outline

A single-stock and small-universe quant trading system. It produces probabilistic price ranges for 3–12 month horizons, builds a portfolio, places orders through the Schwab API, enforces a 5-day transaction lock, and keeps a daily ledger. It runs on an Unraid server inside Docker containers. The languages are Julia (analytics), Rust (execution), and Bun/TypeScript (frontend).

## Project phase and build order

**Current phase: data collection.** Collect 15+ years of every dataset (prices, filings, macro, universe, and the rest) from EODHD (prices) and free sources (everything else, `DECISIONS.md` D15), with the remaining survivorship bias documented and quantified, then train GSPQR, TCP calibration and EVT tails. **Next:** panel validation, then regime, conviction and portfolio, then the ledger, then the frontend. **Execution (Schwab) and live trading come last and are out of scope for now.** The execution documents (`10-execution-schwab`, `11-rust-event-bus` execution parts, the lock enforcement code) are kept as the specification for that later phase; do not build or refine them unless asked. The phase order is the Build Order table in `scaffold.md`.

## Universe

The universe is the individual stocks held by two iShares ETFs: IWF (large-cap growth) and IWP (mid-cap growth). The ETFs themselves are not traded. IJT (small-cap growth) is a potential expansion. Membership is point-in-time and daily: on each date `t` a name is a member if the latest historical IWF/IWP holdings snapshot with an as-of date on or before `t` lists it. Eligibility uses trailing data only: a member is eligible on `t` if its median daily dollar volume is at least $5M and it has at least 756 trading days of clean history at `t` (`01-data-ingestion/UNIVERSE.md`).

## Data

Four sources are used, fetched by Rust over direct HTTP (`ingest/`; no Python anywhere). EODHD is the only paid one (`DECISIONS.md` D15):

- **EODHD** ("EOD Historical Data, All World", about $20/month) for raw as-traded daily OHLCV plus split and dividend events, including delisted US tickers. No fundamentals in the plan. API key `EODHD_API_KEY`. Yahoo Finance (chart API) is an optional secondary price source.
- **SEC EDGAR** for fundamentals (bulk XBRL), 8-K events and delistings (Form 25/15). Point-in-time stamped with filing dates. No API key, but requires a User-Agent email header.
- **FRED** for macro series (treasury yields, credit spreads, CPI, VIX), with ALFRED vintages so each value is known only from its release date. Free API key.
- **iShares** for ETF holdings CSVs by as-of date. No API key.

EODHD keeps the history of delisted tickers, but its coverage of old delistings is not complete (probes P1-P4 measure it), so the history can still carry survivorship bias. It is not hidden: it is quantified (`01-data-ingestion/SURVIVORSHIP.md`) and the caveat is attached to every artifact built on the data.

Every data point is timestamped with its publication date, not its observation date. SEC filings use `filing_date`, never `period_end`. This is the system's most important rule and is enforced by a linter and a validation script.

## Prediction Engine

Three layers:

1. **GSPQR** — 21 LightGBM models (3 horizons × 7 quantiles) trained on 12 features. Six raw features (momentum at 20/60 days and 12-month-minus-1-month, residual volatility, sales-to-price, turnover) and six cross-sectional ranks of those features. Trained monthly in Julia as walk-forward **generations**: at each month-end cutoff a new set of 21 models is trained on all rows whose label has matured by that cutoff, and the historical backfill repeats the same procedure for every past month-end, so every stored prediction is an as-of prediction (`03-gspqr-model/PREDICTION-BACKFILL.md`). Production trains on all matured rows; the panel's `split` column is for the research evaluation only. Rows flagged `survivorship_gap` are excluded from training and calibration, and every model artifact carries the survivorship caveat. The seven quantiles are independent models, so after inference they are sorted ascending per (ticker, date, horizon) before being stored (monotone rearrangement); a `crossed` flag records when the raw output was not monotone.
2. **TCP** — Rolling split-conformal calibration, pooled across tickers by date. Conformity scores of all tickers are normalized by the raw interval width `q_95 - q_05` and pooled over a window of the `M_d = H` most recent *matured* dates (those whose forward return is already realized, so the window is shifted back by the horizon). The raw `[q_05, q_95]` is a 90% base interval; the outputs `q_hat_lower`/`q_hat_upper` are the endpoints of a 95%-coverage interval built on it (about the 2.5%/97.5% quantiles, not the 5%/95%). A Robbins-Monro offset steers coverage toward the nominal 95%; its state is persisted with each row so a restart resumes it.
3. **EVT** — Peaks-over-threshold with a Generalized Pareto Distribution. Extends the calibrated 95% interval to 1%/99%. The GPD is fitted by native Julia maximum likelihood on exceedances above the 95th percentile of matured calibrated residuals standardized by each ticker's TCP half-width and pooled across tickers per horizon over a horizon-specific window (504 / 756 / 1008 matured dates), then mapped back to each ticker. A fit requires a minimum number of distinct tail episodes (non-overlapping `H`-day blocks containing an exceedance) and a minimum window; early in the history a partial window is allowed, and below the gates the fallback is the pooled empirical 1%/99% (`05-evt-tails`).

Output per stock per horizon: 7 raw quantiles, 2 calibrated bounds, 2 extreme bounds.

## Regime Gate

A 3-state Gaussian Hidden Markov Model on daily equal-weighted universe returns, where the universe return on `t` is taken over the point-in-time eligible set of `t`. The model is refitted monthly as a walk-forward (each refit sees only data up to its fit date), and the stored probabilities are filtered (forward-only) values, never smoothed ones, so the 15-year history has no lookahead. States are labeled low-volatility, high-volatility, and crisis by emission variance. Sizing multiplier on new capital: 1.0, 0.5, or 0.0, applied after the optimizer and conviction (it never forces a sale). Requires 2 consecutive days in a new state before acting.

## Fundamental Overlay

Two distinct jobs:

- **Veto** (binary, before optimizer): seven triggers — Altman Z below 1.8, going-concern qualification, negative book plus negative operating cash flow, pending delisting, material restatement, auditor resignation, or covenant breach. A vetoed stock is removed from the universe entirely. New buys of a vetoed name are blocked immediately. A vetoed name that is already held gets target weight 0 unless it is lock-bound; if the lock blocks the sell, the sell is queued for the first permitted day like any other sell. The veto never overrides the lock.
- **Conviction** (bounded scalar, after optimizer): six quality components — Piotroski F-Score, ROIC minus WACC spread, accruals ratio, gross margin trend, revenue growth persistence, analyst revision breadth. Cross-sectionally standardized via median/MAD. Mapped to a multiplier in [0.50, 1.25] via `1.0 + 0.12 * z`.

Valuation is excluded from conviction because it is already a GSPQR feature.

## Portfolio Optimization

Post-Modern Portfolio Theory. Maximizes the Sortino ratio (return over downside deviation) instead of the Sharpe ratio. The ratio is made a Second-Order Cone Program by Charnes–Cooper homogenization (solved with Clarabel; `PortfolioOptimisers.jl` is the intended front end, API to be verified). Scenarios, expected returns and the target return share one horizon; the only budget rule is total weight at most 0.98 (cash earns the target). GSPQR `q_05` and the Ledoit-Wolf covariance are diagnostics, not optimizer inputs. Constraints: 5% maximum per name, 25% maximum per GICS sector, 2% minimum cash, long-only.

Conviction is applied after optimization, then weights are renormalized and caps are redistributed iteratively.

The optimizer produces weights, not share quantities. The conversion to quantities happens in the Rust order generator, which knows the execution tier.

## Execution Tiers

The order type is determined by the trailing 5-day mean NAV at submission time. Three tiers:

| Tier | Trailing 5-day mean NAV | Order policy |
|---|---|---|
| **Bootstrap** | < $2,500 | Fractional market orders only. Limit orders are not submitted. |
| **Transition** | $2,500 ≤ NAV < $10,000 | Limit orders are the default. Market orders permitted only for fractional share quantities that cannot be expressed as whole-share limits. |
| **Standard** | ≥ $10,000 | Limit orders only. Market orders are not submitted. |

The Bootstrap tier exists because whole-share limit orders are unaffordable at small NAV, and fractional fills require market orders in regular hours. The Trader API accepts fractional orders (owner-confirmed, 2026-10-10). Fractional quantities are market orders only, placed only in regular hours, with at most 2 decimals (owner-reported, to be verified before any live order, a later phase) and rounded down. Fractional trading covers only some stocks (Stock Slices: S&P 500 stocks, no ETFs); eligibility comes from the owner-seeded `config/fractional_eligible.toml` and `Unknown` is not eligible. The system never uses a fractional limit order (a fractional limit sell is unverified).

The tier is checked at order submission time, not cached. A NAV crossing a tier boundary takes effect on the next order. The 5-day lock applies in all tiers.

In the Transition tier, the order generator produces a whole-share limit for the integer portion and, for fractional-eligible names only, a fractional market for the remainder (rounded down to 2 decimals); a non-eligible name trades whole shares only and the sub-share remainder stays in cash. In the Bootstrap tier only fractional-eligible names are tradable. In the Standard tier, quantities are rounded down to whole shares and the residual returns to cash.

The Rust `OrderType` enum contains both `Limit` and `Market` variants. A `MarketOrder` cannot be built without a `MarketPermit`, which only the tier check issues, and only in the Bootstrap tier, for a Transition-tier fractional remainder, or for the two reduce-only exits of amendment 1.13 (a Standard-tier legacy fractional remainder that completes a full exit; any Bootstrap exit of a held position). The tier is runtime data, so the permit makes the check unavoidable at compile time; the tier decision itself is made at runtime.

## The 5-Day Lock

The lock is a **contractual obligation of the account owner**, not a risk preference. A breach is a breach of contract, so it outranks latency, slippage and profit, and the system fails closed. It is a pure function of order history, not a state machine.

- A **counted order** is open (working) or has at least one fill. Cancelled, rejected or expired orders with zero fills are not counted (a partial fill then cancel is counted).
- `D` is the order's last fill date in America/New_York (its placement date if it filled the same day), regardless of time of day; a `D` on a non-trading day rolls forward to the next session. An order still open blocks with no end date; a late fill (a GTC order) counts from its last fill.
- A counted order blocks the **opposite** side on trading days `D .. D+4`; the first permitted day is `D + 5 trading days` (buy Monday filled Monday, first sell the following Monday). Same-direction orders are always allowed and extend the block (buy Monday and Wednesday: first sell the following Wednesday).
- It applies across **every account the owner trades**: all Schwab accounts' order history (including manual trades in the Schwab app), the audit journal, and an owner-maintained external-account file (`lock_external.csv`, for example the Fidelity account's trades in other tickers).
- Long-only: a sell requires shares held, else `NothingToSell` (a rejected request, not a lock event).
- Trading days come from the NYSE holiday rules computed for any year, plus an overrides set for ad-hoc closures (a checked-in seed of historical closures, `config/nyse_calendar_overrides.toml`, unioned with a runtime file). A `calendar-sync` job checks Schwab's market-hours data for the dates `[today, today + 30 days]`, today included, at the 09:00 refresh. A mismatch is written to the runtime overrides on its **first sighting**, because a false closure can only lengthen a block while a missed one is a breach; more than 3 mismatches in one run is a `WARN` (suspect source) that never suppresses a write. A same-day signal at 10:00 ET (REST quotes of reference symbols) catches a closure the endpoint missed, and the owner has a manual-add path (`quant-exec calendar add`). The overrides are add-only and the endpoint's holiday behaviour is unverified. The schedule never expires, its upkeep never halts trading, and it never comes from the `prices` dataset.
- **No exceptions.** No override exists. A veto never forces a sale inside the window: a vetoed held name blocks new buys and queues a sell for the first permitted day. A veto never cancels a queued sell.

It is enforced at three layers:

1. **Permit gate** in Rust: order submission requires a `BuyPermit` or `SellPermit` (private fields, no `Clone`/`Copy`, bound to ticker and date), issued only by `lock::authorize_and_reserve`, which takes the per-ticker gate and calls `lock::authorize`; `authorize` **recomputes the lock from order history at submission time** and never answers from a cached row. If any history source is unreadable, stale or unparseable, opposite-side orders are blocked (fail closed). If the halt flag `TRADING_HALTED` is present, unreadable or unparseable, every order is denied. The executable definition of the rule is the set of golden fixtures (`09-lock-state-machine/fixtures/*.json`) with the Julia oracles (`scripts/lock_rule_oracle.jl`, `scripts/nyse_calendar_oracle.jl`, checked by `scripts/run_fixtures.jl`); the Rust implementation must reproduce them (`execution/tests/lock_fixtures.rs`) and is property-tested against them. The compiler guarantees no order without a permit, not that the history is complete.
2. **Per-ticker gate and pre-submit order-history read**: the per-ticker gate makes check-then-send atomic. Under it, **every `authorize` performs a REST read** of every linked account's order window and recent transactions; there is no cache of order history and no cache of lock verdicts, ever. Concurrent requests share one in-flight fetch (single flight, the `moka` crate) only if that fetch started after they did. The Schwab Streamer `ACCT_ACTIVITY` subscription is only a dirty trigger and early warning; correctness never depends on it. If REST fails, the order is denied. The journal and the external file are read in the same step.
3. **Daily post-close compliance audit** (16:40 ET): all order history is rescanned for violations of the rule, including orders placed outside the system. A violation halts trading (`TRADING_HALTED`) and writes a CRITICAL audit row; a dated compliance report is produced every day as evidence of compliance. Position reconciliation also runs at close; mismatch halts trading.

Orders blocked by the lock are written to `pending_orders.parquet` with `earliest_execution` = the first permitted day. On that day a Rust timer re-authorizes, re-validates the signal against the queued snapshot, and either submits or cancels (a veto does not cancel a queued sell).

### Restricted Securities

`config/restricted.toml` lists tickers the system never orders, in either direction. The initial entry is `ICE` (the account owner may not trade it). Enforced by the portfolio eligibility filter (weight 0; the data/model panel and cross-sectional ranks are unchanged), the Rust pre-trade check and `lock::authorize` (reject, CRITICAL `restricted_security_blocked`), a test that `ICE` is in the file, and the daily audit. The Schwab account never holds ICE (the owner's Fidelity account is the only holder); an ICE position, order or fill found in a Schwab account is an anomaly: halt, CRITICAL, no auto-sell.

## Execution Engine

Rust only.

- **Schwab API** for order submission, account data, and Streamer quotes (`LEVELONE_EQUITIES` for held, targeted and highest-turnover names up to the stream's symbol cap, REST quotes for the remainder). A quote recorder aggregates quotes into 1-minute spread rows (dataset `quote_spread_1m`, one `session=YYYY-MM-DD/data.parquet` leaf per session) from before the first live trade; no historical bid/ask exists, so the history is built forward only. Facts and confidence labels (CONFIRMED-OFFICIAL / CLIENT-SOURCE / COMMUNITY / UNVERIFIED): `10-execution-schwab/SCHWAB-API-REFERENCE.md`.
- **OAuth 2.0**: access token ~30 minutes, refresh token ~7 days.
- **No paper environment**: none found (UNVERIFIED). A shadow engine simulates fills alongside real submissions and flags divergence. The shadow engine's slippage model is calibrated per tier, ladder arm and spread bucket against the arrival mid, with the half-spread as the prior; market orders cross the full spread (fill at ask for buys, bid for sells).
- **Order types**: Limit and Market only (Market only under a `MarketPermit`, see Execution Tiers). StopLimit, TrailingStop, OCO and LOC are reserved and disabled. Bracket, OTO, Stop and MOC are unusable: Bracket and OTO place opposite-side legs with the entry, which would violate the 5-day lock, and Stop and MOC produce market orders outside the tier rules (`10-execution-schwab/ORDER-TYPES.md`).

Pre-trade checks in Rust: restricted-securities list, order-history lock check (`lock::authorize`), position cap, sector cap, cash buffer, execution tier. All must pass before an order reaches Schwab.

## Event Bus

Tokio. Six typed bounded channels: `market_data`, `order_events`, `lock_events`, `regime`, `filing`, `audit`. Every handler checks a deduplication set (UUID) before processing. The audit channel is load-bearing: if it fills, trading halts. All state changes write an audit row before returning. The audit record's system of record is a SQLite journal (`audit.db`, WAL, `synchronous=FULL`, trigger-enforced append-only, group-committed); a nightly export at 18:00 writes closed days to hive-partitioned Parquet. Julia-originated events go through an atomic-file spool that Rust ingests.

`OrderSubmitted` and `OrderFilled` events carry a `tier` field and use `Decimal` for quantity to support fractional orders.

## Ledger

Two files:

- **`portfolio_ledger.parquet`** — one row per trading day, written after reconciliation. Contains cash, market value, cost basis, unrealized P&L, realized P&L (today and year-to-date), number of positions, daily return, cumulative return, trailing drawdown, trailing Sortino, regime factor, and reconciliation status. All quantities are `float64`.
- **`tax_lots.parquet`** — one row per purchase, updated on sale via FIFO matching. Partial closes split the lot into two rows: one for the closed portion, one for the remainder. Full closes update the row in place.

Realized P&L is `(close_price − open_price) × close_quantity`. Holding period is calendar days (for short-term vs long-term classification). The ledger is the source of truth for tax reporting, drawdown calculation, and broker reconciliation.

In the Bootstrap and Transition tiers, `daily_return` reflects the spread cost of market orders. A bootstrap-tier account's returns are systematically lower than a standard-tier account's returns by the average spread per trade. The backtest models the same cost structure, so live and simulated results are comparable.

## Language Boundaries

| Layer | Language |
|---|---|
| Analytics, model training and inference, calibration, portfolio, regime, conviction, ledger, backtest, universe builder and price coverage | Julia |
| Execution, data ingestion (with the shared `symbols` crate, also used by execution and the lock), event bus, risk, lot tracking | Rust |
| Dashboard, API, visualization | Bun/TypeScript |
| Data storage | Parquet + DuckDB |
| Julia ↔ Rust exchange | Parquet files; Arrow IPC; RustCall.jl and jlrs are later-phase options |

The execution hot path does not cross the Julia boundary. Julia is the sole writer of `gspqr_predictions` (training and daily inference both run in Julia); Rust only reads it. In-process FFI between the two (RustCall.jl, jlrs) is a later-phase option, not part of the current design (`12-julia-rust-bridge`). No Python anywhere.

## Data Lake

35 datasets (`13-parquet-lake/SKILL.md` §3), ZSTD-compressed, schema-immutable. A dataset is either a single file `lake/<name>.parquet` or a hive-partitioned directory whose every leaf is one `data.parquet` (large daily datasets by year/month; `audit_log` by `date`; `quote_spread_1m` by `session`). Paths come from `QUANT_ROOT`, `QUANT_STATE` and `QUANT_CONFIG` (`13-parquet-lake/IO-CONTRACT.md`, the single definition). Single-writer rule: each dataset has exactly one writing module. Schemas are hashed in the repository file `schemas/registry.json` and verified by CI. Additive changes only.

## Backtest

Two tracks. The **walk-forward track** replays the production pipeline over history (monthly generations, as-of predictions, TCP, EVT, regime); it is the only source of the TCP/EVT interval and tail metrics. **Combinatorial Purged Cross-Validation (CPCV)** evaluates the raw model and the portfolio, with purging on both sides of each test block by the label horizon H and an embargo of max(21, ceil(0.01 T)) trading days after each block (about 38 for 15 years; owner decision C1, AFML ch. 7). Ten groups, three test groups per fold, 120 splits, 36 backtest paths. CPCV metrics are reported as distributions across paths, not point estimates: coverage, Winkler score, pinball loss, CRPS, Sortino, max drawdown, Sharpe. The Deflated Sharpe Ratio is reported only when the number of tuning trials is logged. Every report carries the survivorship caveat (the measured survivorship gap and the free-source header).

The execution simulator is tier-aware. It reads the tier from the trailing 5-day mean of the simulated NAV at each rebalance and applies the corresponding fill model: market orders fill at ask (buy) or bid (sell), and limit orders use the per-name spread-based cost model of `EXECUTION-WINDOW.md` (a fill at mid is only an optimistic bound). The post-then-cross arm is modeled with an assumed fill probability until real fills replace it. This is documented as a known limitation.

Two starting-NAV configurations are reported: $1,000 (Bootstrap-realistic) and $100,000 (Standard-research).

## Deployment

Unraid host. Two service containers plus a debug container:

- **`quant-core`**: Julia and Rust co-located. The only writer of the Parquet lake. Mounts the data root (`QUANT_ROOT`, host `/mnt/cache/quant`) and the state directory (`QUANT_STATE`, host `/mnt/cache/appdata/quant`) read-write and the repository `config/` (`QUANT_CONFIG`) read-only (`13-parquet-lake/IO-CONTRACT.md` §1).
- **`quant-api`**: Bun. Reads Parquet read-only. Serves the dashboard.
- **`duckdb`**: read-only mount of the data root, for debug queries only (`scaffold.md` `docker/duckdb/`, `13-parquet-lake/UNRAID-STORAGE.md`).

All live data lives on the XFS cache pool. The array is a nightly rsync backup with checksum verification. XFS provides no snapshots and no block checksums, so the mitigation is a monthly full read-through validation job. Containers run as UID 99 / GID 100. Docker network type is ipvlan.

Daily schedule (America/New_York). Decide after the close of session `t`: prices at 16:10, reconciliation at 16:15, tax lots at 16:20, trade summary at 16:22, ledger at 16:25, model inference at 16:30 (Julia), lock-compliance audit at 16:40, lock snapshot at 16:50 (`$QUANT_STATE/lock_snapshot.json`, advisory, for the optimizer), portfolio optimization at 17:00 (targets for session `t+1`), next-session order plan at 17:30 (intents only; nothing is sent to Schwab), audit export then backup at 18:00. Execute in session `t+1`: pre-open refresh at 09:00 (token, calendar-sync including today, lock history refresh, cost and quote-cache warm-up), no placements from 09:30 to 10:00, same-day closure check at 10:00, execution window from 10:00 to `C - 30 min` where `C` is the session close (15:30 on a normal day; spread gate, order ladder, re-validation against the fresh price), deadline fallback to `C - 10 min` (15:50), no placements after that. All orders are placed during the regular session only, so the placement date is the fill date for same-session fills.

## Governance

The system is defined by 19 `.claude/skills/` documents (`00` to `18`). Each skill covers one layer, with parameters, procedures, anti-patterns, and verification checklists. A constitution at `00-construction` defines the invariants that no code may violate. Six amendments (1.1 to 1.6) have been made since ratification, all merged:

1. **Amendment 1.1** — permits in-place update of `tax_lots.parquet` on full close and reduction of `open_quantity` on partial close.
2. **Amendment 1.2** — replaces "limit orders only" with the three-tier NAV execution model.
3. **Amendment 1.3** — clarifies amendment 1.2: Standard begins at exactly $10,000 and Standard-tier quantities round down.
4. **Amendment 1.4** — replaces the five-state lock model by a rule over order history (placement-or-last-fill clock, counted orders, every account, fail closed, no exceptions, daily compliance audit); the proposed veto-forced-liquidation override is withdrawn.
5. **Amendment 1.5** — restricted securities: the system never orders a ticker on `config/restricted.toml` (initial entry `ICE`).
6. **Amendment 1.6** — wording corrections, no change of intent: the universe is the individual stocks held by IWF and IWP (not the ETFs); fractional quantities fill only as market orders in regular hours (owner-confirmed), limited to 2 decimal places (owner-reported).
7. **Amendments 1.7-1.12** — TCP output wording; veto exits and inactive triggers; read-only frontend, Julia sole writer of predictions, no Python; vendor-snapshot and as-of wording; audit journal; add-only closure backfill from daily bars.
8. **Amendment 1.13** — reduce-only exits: a Standard-tier legacy fractional remainder may be sold at market as part of a full exit; any Bootstrap exit of a held position may be a market order. Never a buy; the lock still applies.

A pre-commit hook catches some invariants mechanically; the rest require review. An append-only amendment log records every constitutional change.

## What the System Does Not Do

- No shorting, no options, no futures.
- No intraday rebalancing. The lock makes it impossible and the horizon makes it unnecessary.
- Never trades `ICE` (or any ticker on `config/restricted.toml`), in either direction.
- No exceptions to the 5-day lock: no override, no veto-forced sale, no manual bypass.
- No directional regime prediction. The HMM identifies volatility states, not bull or bear markets.
- No high-frequency execution. The signal half-life is weeks; the execution latency budget is seconds.
- No news sentiment, no LLM decisions, no discretionary overrides of any rule.
- No market orders at or above $2,500 NAV except for fractional remainders in the Transition tier.