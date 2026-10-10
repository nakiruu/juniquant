# Schwab API Reference (order history, streamer, quotes, fractional, market hours)

Compiled 2026-10-10 by web/source research. developer.schwab.com is login-gated, so facts come from
(a) Schwab's own OpenAPI/Streamer files as mirrored in open-source repos, (b) client source, (c) public Schwab pages.

## Source keys and confidence labels

| Key | What it is | URL |
|---|---|---|
| SPEC-T | Schwab Trader API OpenAPI JSON (title "Trader API - Account Access and User Preferences", v1.0.0, contact TraderAPI@Schwab.com). Mirror in `major/schwab-rs` `docs/` (README there says: originally sourced from the `schwab-go` project's docs/). Repo HEAD checked: e79d1a8d (2026-06-20). | https://github.com/major/schwab-rs/blob/main/docs/trader_api.openapi.json |
| SPEC-M | Schwab Market Data OpenAPI JSON (same mirror) | https://github.com/major/schwab-rs/blob/main/docs/market_data.openapi.json |
| STREAM-DOC | Schwab "Streamer API" HTML documentation (same mirror) | https://github.com/major/schwab-rs/blob/main/docs/streaming_documentation.html |
| PY | alexgolec/schwab-py (HEAD 43c46149) | https://github.com/alexgolec/schwab-py (`schwab/client/base.py`, `schwab/streaming.py`) |
| DEV | tylerebowers/Schwabdev (HEAD ab15d10f, 2026-08-10) docs | https://github.com/tylerebowers/Schwabdev/tree/main/docs/pages-raw (`stream.md`, `api.md`, `orders.md`, `client.md`) |
| RS | major/schwab-rs Rust models | https://github.com/major/schwab-rs (`src/models/trader.rs`, `src/models/streaming/*.rs`) |

Labels: **CONFIRMED-OFFICIAL** = in a Schwab-authored spec/doc file (via mirror; not re-verified on the gated portal).
**CONFIRMED-CLIENT-SOURCE** = in client code/tests/docs of a named repo. **COMMUNITY-REPORTED** = third-party statements.
**UNVERIFIED** = not found; must be tested live. In this file "(official)" in a table means CONFIRMED-OFFICIAL via SPEC-T/SPEC-M/STREAM-DOC.

Key conflicts found are collected in section 7.

---

## 1. Trader API REST (base `https://api.schwabapi.com/trader/v1`)

Base URL: SPEC-T `servers` = `https://api.schwabapi.com/trader/v1` (CONFIRMED-OFFICIAL).

### 1.1 Endpoint list (SPEC-T paths, all CONFIRMED-OFFICIAL)

| Method | Path | Summary |
|---|---|---|
| GET | `/accounts` | linked accounts balances (positions only with `fields=positions`) |
| GET | `/accounts/accountNumbers` | list of plain account number + encrypted (hash) value |
| GET | `/accounts/{accountNumber}` | one account |
| GET | `/accounts/{accountNumber}/orders` | orders for one account |
| POST | `/accounts/{accountNumber}/orders` | place order (201, `Location` header = link to new order) |
| GET | `/accounts/{accountNumber}/orders/{orderId}` | one order |
| PUT | `/accounts/{accountNumber}/orders/{orderId}` | replace order (201, `Location` header) |
| DELETE | `/accounts/{accountNumber}/orders/{orderId}` | cancel order (200, empty body) |
| POST | `/accounts/{accountNumber}/previewOrder` | preview |
| GET | `/accounts/{accountNumber}/transactions` | transactions (see 1.6) |
| GET | `/accounts/{accountNumber}/transactions/{transactionId}` | one transaction |
| GET | `/orders` | orders for ALL linked accounts |
| GET | `/userPreference` | user preference incl. `streamerInfo` |

### 1.2 Account discovery and which identifier goes in URLs

| Fact | Source / confidence |
|---|---|
| `GET /accounts/accountNumbers` returns array of `AccountNumberHash {accountNumber: string, hashValue: string}`. | SPEC-T `AccountNumberHash` - CONFIRMED-OFFICIAL |
| Official description: "Account numbers in plain text cannot be used outside of headers or request/response bodies. As the first step consumers must invoke this service to retrieve the list of plain text/encrypted value pairs, and use encrypted account values for all subsequent calls for any accountNumber request." | SPEC-T operation `getAccountNumbers` - CONFIRMED-OFFICIAL |
| The URL path parameter is literally named `accountNumber` but is described as "The encrypted ID of the account", i.e. the `hashValue`. The owner's `GET /accounts/{accountNumber}/orders` is the spec path; the value to put there is the hash. The real path is `/trader/v1/accounts/{hashValue}/orders`. | SPEC-T; PY `get_orders_for_account` uses `/trader/v1/accounts/{account_hash}/orders` (`schwab/client/base.py` ~line 250) - CONFIRMED-OFFICIAL + CONFIRMED-CLIENT-SOURCE |
| The `Order` response body contains `accountNumber` as int64 (plain number, not hash) - use it to attribute orders from `GET /orders` to an account; map via `/accounts/accountNumbers`. | SPEC-T `Order.accountNumber` - CONFIRMED-OFFICIAL. Population on every order: UNVERIFIED |
| Hash values are stable per account/app: UNVERIFIED (cache but re-fetch at startup). | UNVERIFIED |
| `GET /userPreference` `accounts[]` items: `accountNumber` (string), `primaryAccount` (bool), `type`, `nickName`, `displayAcctId`, `accountColor`, `autoPositionEffect`. | SPEC-T `UserPreferenceAccount` - CONFIRMED-OFFICIAL |

### 1.3 Order list queries

| Item | `GET /accounts/{hash}/orders` | `GET /orders` (all linked accounts) |
|---|---|---|
| `maxResults` | optional int64. "Default is 3000." | same |
| `fromEnteredTime` | spec marks `required: true`; ISO-8601 `yyyy-MM-dd'T'HH:mm:ss.SSSZ`, example `2024-03-29T00:00:00.000Z`; "'toEnteredTime' must also be set" | spec marks required; "Date must be within 60 days from today's date" |
| `toEnteredTime` | spec marks required; example `2024-04-28T23:59:59.000Z` | spec marks required |
| `status` | optional enum `apiOrderStatus` (21 values, section 1.5) | same |
| Max range | "Maximum date range is 1 year." (account endpoint description) | "within 60 days from today's date" (fromEnteredTime text) |
| Errors | 400 BadRequest, 401 NotAuthorized, 403 Forbidden, 404 NotFound, 500 InternalServerError, 503 ServiceUnavailable; body `ServiceError {message: string, errors: [string]}`; header `Schwab-Client-CorrelId` | same |
| Source | SPEC-T `getOrdersByPathParam` - CONFIRMED-OFFICIAL | SPEC-T `getOrdersByQueryParam` - CONFIRMED-OFFICIAL |

Client-source cross-checks:
- PY `get_orders_for_account` and `get_orders_for_all_linked_accounts` docstrings both say range "must be within 60 days from today's date" and default to the last 60 days when unset; both use `yyyy-MM-dd'T'HH:mm:ss.SSSZ` (`schwab/client/base.py` lines ~250-300) - CONFIRMED-CLIENT-SOURCE. This CONTRADICTS the spec's "1 year" for the per-account endpoint (see section 7). Treat the effective per-account window as UNVERIFIED between 60 days and 1 year; test live.
- DEV `account_orders` / `account_orders_all`: `fromEnteredTime`, `toEnteredTime` are positional (required), `maxResults` default 3000, `status` optional (`api.md`) - CONFIRMED-CLIENT-SOURCE.
- Other items:

| Question | Answer |
|---|---|
| Paging (cursor/offset/page token)? | None in spec (only `maxResults`). UNVERIFIED how truncation behaves above 3000; mitigate by splitting the time window (e.g. per day) and checking result count < maxResults. |
| Result ordering | UNVERIFIED (not specified). Sort client-side by `enteredTime`, then `orderId`. |
| Is date filter on `enteredTime` only (not `closeTime`)? | Yes by parameter name ("no orders entered before/after"). A GTC order entered long ago and filled today will NOT be in a short window - so for the 5-day lock query a window at least as long as the longest GTC lifetime you allow, or additionally fetch by `status` (WORKING etc.) and `GET /orders/{id}`. Inference from the parameter text - CONFIRMED-OFFICIAL text, consequence is our deduction. |
| Rate limit | COMMUNITY-REPORTED: 120 API requests/minute, 120 orders/minute and 4000 order calls/day, HTTP 429 beyond (DEV `client.md`, `orders.md`, `api.md`). Not in the OpenAPI spec. |
| Time zone of query params | `Z` (UTC) in the official examples. |

### 1.4 The `Order` object (SPEC-T `Order`, CONFIRMED-OFFICIAL; every property below exists in the schema)

JSON key order is irrelevant. `date-time` fields are strings; sample payloads in client repos show the form `2021-01-01T12:01:00+0000` (UTC, `+0000` WITHOUT a colon; CONFIRMED-CLIENT-SOURCE: DEV `api.md` example `'enteredTime': 'YYYY-MM-DDT00:00:00+0000'`; RS test fixtures `"enteredTime": "2024-01-15T10:30:00+0000"` in `src/models/trader.rs`; PY tests `2020-03-30T15:36:12+0000`). Parse with a lenient offset parser (accept `+0000`, `+00:00`, `Z`). Never seen with a non-UTC offset in samples; assume UTC but UNVERIFIED.

| Field | Type | Notes |
|---|---|---|
| `orderId` | int64 | unique order id |
| `accountNumber` | int64 | plain account number |
| `status` | enum `status` | section 1.5 |
| `statusDescription` | string | free text (e.g. reject reason); contents UNVERIFIED |
| `enteredTime` | date-time | placement time |
| `closeTime` | date-time | when order reached terminal state (fill/cancel/expire); semantics UNVERIFIED |
| `cancelTime` | date-time | scheduled cancel time (GTC expiry / cancel time), NOT the time of a user cancel (DEV sample shows it on GTC LIMIT order); UNVERIFIED |
| `releaseTime` | date-time | for AWAITING_RELEASE_TIME |
| `session` | enum | NORMAL, AM, PM, SEAMLESS |
| `duration` | enum | DAY, GOOD_TILL_CANCEL, FILL_OR_KILL, IMMEDIATE_OR_CANCEL, END_OF_WEEK, END_OF_MONTH, NEXT_END_OF_MONTH, UNKNOWN |
| `orderType` | enum | section 1.5 |
| `complexOrderStrategyType` | enum | NONE, ... |
| `orderStrategyType` | enum | SINGLE, CANCEL, RECALL, PAIR, FLATTEN, TWO_DAY_SWAP, BLAST_ALL, OCO, TRIGGER |
| `quantity` | double | ordered quantity (decimal-capable type) |
| `filledQuantity` | double | |
| `remainingQuantity` | double | |
| `price` | double | limit price |
| `stopPrice` | double | |
| `stopPriceOffset` | double | |
| `stopPriceLinkBasis` | enum | MANUAL, BASE, TRIGGER, LAST, BID, ASK, ASK_BID, MARK, AVERAGE |
| `stopPriceLinkType` | enum | VALUE, PERCENT, TICK |
| `stopType` | enum | STANDARD, BID, ASK, LAST, MARK |
| `priceLinkBasis` | enum | same 9 values as stopPriceLinkBasis |
| `priceLinkType` | enum | VALUE, PERCENT, TICK |
| `activationPrice` | double | |
| `specialInstruction` | enum | ALL_OR_NONE, DO_NOT_REDUCE, ALL_OR_NONE_DO_NOT_REDUCE |
| `taxLotMethod` | enum | FIFO, LIFO, HIGH_COST, LOW_COST, AVERAGE_COST, SPECIFIC_LOT, LOSS_HARVESTER |
| `requestedDestination` | enum | INET, ECN_ARCA, CBOE, AMEX, PHLX, ISE, BOX, NYSE, NASDAQ, BATS, C2, AUTO |
| `destinationLinkName` | string | free text (DEV sample `SOHO`) |
| `cancelable` | bool | |
| `editable` | bool | |
| `tag` | string | client tag |
| `orderLegCollection` | array<OrderLegCollection> | 1.4.1 |
| `orderActivityCollection` | array<OrderActivity> | 1.4.2 (fills live here) |
| `replacingOrderCollection` | array<Order> | recursive; semantics UNVERIFIED (see 1.7) |
| `childOrderStrategies` | array<Order> | recursive; OCO/TRIGGER children |

Not present in the `Order` schema: `quantityType` (only on legs), `accountHash`, `cancelUser`, any "fill time" at order level. `Order.price` is the limit; average fill price is NOT an order-level field - compute from `executionLegs`.

#### 1.4.1 `OrderLegCollection` items (SPEC-T, CONFIRMED-OFFICIAL)

| Field | Type / enum |
|---|---|
| `orderLegType` | string enum: EQUITY, OPTION, INDEX, MUTUAL_FUND, CASH_EQUIVALENT, FIXED_INCOME, CURRENCY, COLLECTIVE_INVESTMENT |
| `legId` | int64 |
| `instrument` | `AccountsInstrument` (discriminator `assetType`; oneOf AccountCashEquivalent, AccountEquity, AccountFixedIncome, AccountMutualFund, AccountOption) |
| `instruction` | enum: BUY, SELL, BUY_TO_COVER, SELL_SHORT, BUY_TO_OPEN, BUY_TO_CLOSE, SELL_TO_OPEN, SELL_TO_CLOSE, EXCHANGE, SELL_SHORT_EXEMPT |
| `positionEffect` | string enum: OPENING, CLOSING, AUTOMATIC |
| `quantity` | double |
| `quantityType` | string enum: ALL_SHARES, DOLLARS, SHARES |
| `divCapGains` | string enum: REINVEST, PAYOUT |
| `toSymbol` | string |

Instrument base fields (`AccountsBaseInstrument`): `assetType` (EQUITY, OPTION, INDEX, MUTUAL_FUND, CASH_EQUIVALENT, FIXED_INCOME, CURRENCY, COLLECTIVE_INVESTMENT), `cusip`, `symbol`, `description`, `instrumentId` (int64), `netChange` (double). `AccountOption` adds `optionDeliverables`, `optionMultiplier`, `putCall` (PUT/CALL/UNKNOWN), `type` (VANILLA/BINARY/BARRIER/UNKNOWN), `underlyingSymbol`. (Top-level `assetType` enum used elsewhere also lists FUTURE, FOREX, PRODUCT.)

#### 1.4.2 `OrderActivity` and `ExecutionLeg` (SPEC-T, CONFIRMED-OFFICIAL)

| `OrderActivity` field | Type |
|---|---|
| `activityType` | enum EXECUTION, ORDER_ACTION |
| `executionType` | enum FILL (only value listed) |
| `quantity` | double (quantity of this activity/fill) |
| `orderRemainingQuantity` | double |
| `executionLegs` | array<ExecutionLeg> |

| `ExecutionLeg` field | Type |
|---|---|
| `legId` | int64 (matches `orderLegCollection[].legId`) |
| `price` | double (execution price) |
| `quantity` | double |
| `mismarkedQuantity` | double |
| `instrumentId` | int64 |
| `time` | date-time - THE per-fill timestamp (location of fill time in REST) |

Fill-time location (REST): `orderActivityCollection[].executionLegs[].time`. There is no `Order`-level fill time; `closeTime` is only the order's terminal time. One order can have several `OrderActivity` items (partial fills). Timezone form of `executionLegs[].time`: same UTC pattern assumed, UNVERIFIED.

### 1.5 Enumerations (SPEC-T unless stated; CONFIRMED-OFFICIAL)

| Enum | Values (exact spelling) |
|---|---|
| `status` / `apiOrderStatus` (21) | AWAITING_PARENT_ORDER, AWAITING_CONDITION, AWAITING_STOP_CONDITION, AWAITING_MANUAL_REVIEW, ACCEPTED, AWAITING_UR_OUT, PENDING_ACTIVATION, QUEUED, WORKING, REJECTED, PENDING_CANCEL, CANCELED, PENDING_REPLACE, REPLACED, FILLED, EXPIRED, NEW, AWAITING_RELEASE_TIME, PENDING_ACKNOWLEDGEMENT, PENDING_RECALL, UNKNOWN (identical list in PY `Order.Status`, `schwab/client/base.py` ~line 190-215, and DEV `api.md` - CONFIRMED-CLIENT-SOURCE) |
| `session` | NORMAL, AM, PM, SEAMLESS |
| `duration` | DAY, GOOD_TILL_CANCEL, FILL_OR_KILL, IMMEDIATE_OR_CANCEL, END_OF_WEEK, END_OF_MONTH, NEXT_END_OF_MONTH, UNKNOWN |
| `orderType` | MARKET, LIMIT, STOP, STOP_LIMIT, TRAILING_STOP, CABINET, NON_MARKETABLE, MARKET_ON_CLOSE, EXERCISE, TRAILING_STOP_LIMIT, NET_DEBIT, NET_CREDIT, NET_ZERO, LIMIT_ON_CLOSE, UNKNOWN (`orderTypeRequest` = same minus UNKNOWN) |
| `instruction` | BUY, SELL, BUY_TO_COVER, SELL_SHORT, BUY_TO_OPEN, BUY_TO_CLOSE, SELL_TO_OPEN, SELL_TO_CLOSE, EXCHANGE, SELL_SHORT_EXEMPT |
| `assetType` | EQUITY, MUTUAL_FUND, OPTION, FUTURE, FOREX, INDEX, CASH_EQUIVALENT, FIXED_INCOME, PRODUCT, CURRENCY, COLLECTIVE_INVESTMENT |
| `orderStrategyType` | SINGLE, CANCEL, RECALL, PAIR, FLATTEN, TWO_DAY_SWAP, BLAST_ALL, OCO, TRIGGER |
| `complexOrderStrategyType` | NONE, COVERED, VERTICAL, BACK_RATIO, CALENDAR, DIAGONAL, STRADDLE, STRANGLE, COLLAR_SYNTHETIC, BUTTERFLY, CONDOR, IRON_CONDOR, VERTICAL_ROLL, COLLAR_WITH_STOCK, DOUBLE_DIAGONAL, UNBALANCED_BUTTERFLY, UNBALANCED_CONDOR, UNBALANCED_IRON_CONDOR, UNBALANCED_VERTICAL_ROLL, MUTUAL_FUND_SWAP, CUSTOM |
| `requestedDestination` | INET, ECN_ARCA, CBOE, AMEX, PHLX, ISE, BOX, NYSE, NASDAQ, BATS, C2, AUTO |
| `stopType` | STANDARD, BID, ASK, LAST, MARK |
| `priceLinkBasis` / `stopPriceLinkBasis` | MANUAL, BASE, TRIGGER, LAST, BID, ASK, ASK_BID, MARK, AVERAGE |
| `priceLinkType` / `stopPriceLinkType` | VALUE, PERCENT, TICK |
| `specialInstruction` | ALL_OR_NONE, DO_NOT_REDUCE, ALL_OR_NONE_DO_NOT_REDUCE |
| `taxLotMethod` | FIFO, LIFO, HIGH_COST, LOW_COST, AVERAGE_COST, SPECIFIC_LOT, LOSS_HARVESTER |
| leg `positionEffect` / `quantityType` / `divCapGains` / `orderLegType` | see 1.4.1 |
| `OrderActivity.activityType` / `executionType` | EXECUTION, ORDER_ACTION / FILL |

Real-world values may exceed these lists (the spec is not guaranteed complete); model every enum fail-closed (section 9).

### 1.6 Alternative fill source: transactions endpoint (SPEC-T, CONFIRMED-OFFICIAL)

`GET /accounts/{hash}/transactions`: required `startDate`, `endDate` (ISO-8601 `yyyy-MM-dd'T'HH:mm:ss.SSSZ`), required `types` (enum `TransactionType`: TRADE, RECEIVE_AND_DELIVER, DIVIDEND_OR_INTEREST, ACH_RECEIPT, ACH_DISBURSEMENT, CASH_RECEIPT, CASH_DISBURSEMENT, ELECTRONIC_FUND, WIRE_OUT, WIRE_IN, JOURNAL, MEMORANDUM, MARGIN_CALL, MONEY_MARKET, SMA_ADJUSTMENT), optional `symbol`. "Maximum number of transactions in response is 3000. Maximum date range is 1 year." `Transaction` fields: `activityId`, `accountNumber` (string), `activityType` (ACTIVITY_CORRECTION, EXECUTION, ORDER_ACTION, TRANSFER, UNKNOWN), `orderId`, `positionId`, `time`, `tradeDate`, `settlementDate`, `netAmount`, `status` (VALID, INVALID, PENDING, UNKNOWN), `subAccount`, `type`, `description`, `transferItems[]` (`amount`, `cost`, `price`, `feeType`, `positionEffect`, `instrument`). Useful as an independent cross-check of fills (trade date/time, orderId). Whether TRADE rows appear same-day (settlement lag) is UNVERIFIED.

### 1.7 Cancel and replace as they appear in history

| Question | Finding | Confidence |
|---|---|---|
| Replace API behavior | "Replace an existing order for an account. The existing order will be replaced by the new order. Once replaced, the old order will be canceled and a new order will be created." Response is HTTP 201 with `Location` header "Link to the newly created order". | CONFIRMED-OFFICIAL (SPEC-T `replaceOrder`); same text in PY `replace_order` docstring (`schwab/client/base.py` line 320) |
| New `orderId` on replace? | Yes by the above text ("a new order will be created"); the new id is in the `Location` header. COMMUNITY-REPORTED the same. | CONFIRMED-OFFICIAL (text) |
| Old order status after replace | Enum has `PENDING_REPLACE` and `REPLACED`; expectation: old order ends as `REPLACED` (or `CANCELED`) - actual terminal value UNVERIFIED. Handle BOTH. | UNVERIFIED |
| Linkage field | `Order.replacingOrderCollection` (array of Order) exists in the schema, plus `Order.statusDescription`; which side carries the link (old order lists new order, or new lists old) and whether it is populated is UNVERIFIED. No `parentOrderId`/`replacedOrderId` field exists in the schema. | UNVERIFIED |
| Cancel | `DELETE .../orders/{orderId}` -> 200 empty. Order goes to `PENDING_CANCEL` then `CANCELED` (status enum values). Cancel timestamp: `closeTime` expected; UNVERIFIED. | enum CONFIRMED-OFFICIAL; behavior UNVERIFIED |
| Partial fill then cancel | Schema permits `filledQuantity > 0` with `status = CANCELED` (separate fields, `orderActivityCollection` lists the fills). Actual rendering UNVERIFIED but we MUST NOT infer "no fill" from CANCELED; always sum `executionLegs[].quantity` / read `filledQuantity`. | schema CONFIRMED-OFFICIAL; behavior UNVERIFIED |
| Partial fill then replace | Fills on the old order stay on the old order's `orderActivityCollection`; replacement quantity semantics (original vs remaining) UNVERIFIED. |  UNVERIFIED |
| Rejects | `REJECTED`, with reason probably in `statusDescription` - UNVERIFIED. | UNVERIFIED |
| 5-day lock implication | Count a leg as "placed" at `enteredTime` for ANY status except proven never-live (REJECTED); count as "filled" from `executionLegs`. A replaced/cancelled order that had partial fills still counts for those fills. Rule choice is ours (policy), not an API fact. | design note |

---

## 2. Streamer Service

### 2.1 Connecting (STREAM-DOC and SPEC-T; CONFIRMED-OFFICIAL)

| Step | Detail |
|---|---|
| Get stream params | `GET /trader/v1/userPreference` -> JSON. In SPEC-T the 200 schema is an ARRAY of `UserPreference`, each `{accounts[], offers[], streamerInfo[]}`; `streamerInfo` is an array of `StreamerInfo {streamerSocketUrl, schwabClientCustomerId, schwabClientCorrelId, schwabClientChannel, schwabClientFunctionId}` (all strings). `offers[]`: `level2Permissions` (bool), `mktDataPermission` (string). Real responses are reported as a single object with `streamerInfo` as a one-element list (PY `get_user_preferences` + `streaming.py` login reads `[0]`) - take the first element; verify live. |
| Open | WebSocket to `streamerSocketUrl` (wss). |
| LOGIN | Envelope below. Must succeed (response `code: 0`) before any other command. Wait for the LOGIN response before sending SUBS (docs: race condition gives code 20 STREAM_CONN_NOT_FOUND). |
| Token | `parameters.Authorization` = the plain OAuth access token (the doc's `"PN"` in one example is a placeholder inconsistency). |
| Do not alter | Changing `SchwabClientCustomerId`/`SchwabClientCorrelId` after login causes code 20. |

Request envelope (official examples; a request object may be wrapped as `{"requests":[ {...}, {...} ]}`; a bare object is also shown):

```
{"requests":[{
  "requestid": "1",               // unique; string or int accepted in doc examples
  "service": "ADMIN",              // or LEVELONE_EQUITIES, ACCT_ACTIVITY, CHART_EQUITY, ...
  "command": "LOGIN",              // LOGIN | SUBS | ADD | UNSUBS | VIEW | LOGOUT
  "SchwabClientCustomerId": "<schwabClientCustomerId>",
  "SchwabClientCorrelId": "<schwabClientCorrelId>",
  "parameters": {"Authorization":"<access token>","SchwabClientChannel":"<channel>","SchwabClientFunctionId":"<function id>"}
}]}
```
SUBS/ADD parameters: `{"keys":"AAPL,SPY","fields":"0,1,2,3,4,5"}`. Source: STREAM-DOC sections 2-3; CONFIRMED-OFFICIAL. LOGIN `SchwabClientChannel` is 2 chars, `SchwabClientFunctionId` 5 alphanumerics (from userPreference).

Commands (CONFIRMED-OFFICIAL): `SUBS` overwrites the whole subscription for that service; `ADD` appends (OK as first command); `UNSUBS`; `VIEW` changes the field set for all symbols of a service; `LOGOUT` (server closes the connection). ACCT_ACTIVITY supports only SUBS and UNSUBS.

### 2.2 Response shapes (CONFIRMED-OFFICIAL, STREAM-DOC section 3; all frames are JSON with an array under one top-level key)

| Type | Shape |
|---|---|
| response | `{"response":[{"service","command","requestid","SchwabClientCorrelId","timestamp"(ms epoch),"content":{"code":<int>,"msg":"..."}}]}` |
| notify (heartbeat) | `{"notify":[{"heartbeat":"1668715930582"}]}` (ms epoch as string). Cadence UNVERIFIED. |
| data | `{"data":[{"service","timestamp","command","content":[ {...}, ... ]}]}` ; each content item has `key` (symbol, or the ACCT_ACTIVITY key) and, for level one, `delayed` (bool), plus numeric-string field keys `"1"`, `"2"` ... |
| LOGIN ok | `content: {"code":0,"msg":"server=s0166bdv-1;status=PN"}` ; status PN = Non-Pro non-paying? (doc: "PN (Non-Paying Pro), NP (Non-Pro), PP (Paying-Pro)"; no entitlements -> delayed/NFL quotes) |
| LOGIN denied | `code: 3`, e.g. "Login Denied.: token is invalid or has expired." |

### 2.3 Response codes (STREAM-DOC section 4, CONFIRMED-OFFICIAL)

| Code | Name | Connection severed | Notes |
|---|---|---|---|
| 0 | SUCCESS | no | |
| 3 | LOGIN_DENIED | yes | reconnect and re-login with a NEW token |
| 9 | UNKNOWN_FAILURE | TBD | contact TraderAPI@Schwab.com with CorrelId |
| 11 | SERVICE_NOT_AVAILABLE | no | unsupported service or source down |
| 12 | CLOSE_CONNECTION | yes | "maximum number of connections allowed. A limit of 1 Streamer connection at any given time from a given user" |
| 19 | REACHED_SYMBOL_LIMIT | no | total subscription symbol limit reached (number NOT in the official doc) |
| 20 | STREAM_CONN_NOT_FOUND | TBD | no connection for CustomerId+CorrelId; common causes: command before LOGIN completes, altered ids, already disconnected |
| 21 | BAD_COMMAND_FORMAT | no | |
| 22 / 23 / 24 / 25 | FAILED_COMMAND_SUBS / UNSUBS / ADD / VIEW | no | parallel commands can cause failures |
| 26 / 27 / 28 / 29 | SUCCEEDED_COMMAND_SUBS / UNSUBS / ADD / VIEW | no | success codes (the example in the doc shows `code:0 "SUBS command succeeded"` for SUBS) |
| 30 | STOP_STREAMING | yes | "administrator action, inactivity, or slowness"; typically no subscriptions |

Treat both `0` and `26-29` as success for subscription acks. schwab-rs reconnects on code 30 (`src/stream_session/mod.rs` ~line 835), CONFIRMED-CLIENT-SOURCE.

### 2.4 Limits, reconnect, token refresh

| Fact | Source / confidence |
|---|---|
| 1 streamer connection per user at a time (code 12 otherwise). A second client with the same user kicks/blocks. | STREAM-DOC - CONFIRMED-OFFICIAL |
| 500 tickers concurrently streamed; beyond that errors (HTTP 429 phrasing there is for REST). | COMMUNITY-REPORTED (DEV `client.md`) |
| Stream closes after ~30 s with no subscriptions. | COMMUNITY-REPORTED (DEV `stream.md` "The stream will close after ~30 seconds if there are no subscriptions") |
| DEV reconnect: exponential backoff 2 s doubling to a max of 120 s, client-side websocket ping interval 20 s / timeout 30 s; reconnect re-sends recorded subscriptions. | CONFIRMED-CLIENT-SOURCE (DEV `schwabdev/stream.py`) |
| The access token is only used at LOGIN. Token expiry mid-session: no documented server push; after reconnect/login use a fresh token (code 3 = expired token). Whether an established stream survives access-token expiry (30 min) is UNVERIFIED - design for forced periodic refresh/reconnect and detect via heartbeat gap. | UNVERIFIED |
| Service hours: ACCT_ACTIVITY availability outside market hours UNVERIFIED. LEVELONE_EQUITIES streams in pre/post market (see field 8 "including pre/post market hours"). DEV `start_auto` defaults start 09:29 ET / stop 16:00 ET as a client convenience only. Doc resets: volume to 0 at 7:28 ET; high/low/open/close reset at 3:30 ET; descriptions loaded 7:29:50 ET. | STREAM-DOC (resets, CONFIRMED-OFFICIAL); hours otherwise UNVERIFIED |
| Delivery types: LEVELONE_* = "Change" (only changed fields are sent, conflated); NYSE_BOOK/NASDAQ_BOOK/OPTIONS_BOOK/SCREENER_* = "Whole" (throttled); CHART_* and ACCT_ACTIVITY = "All Sequence" (not conflated, with sequence number). | STREAM-DOC - CONFIRMED-OFFICIAL |
| Service names: ADMIN, LEVELONE_EQUITIES, LEVELONE_OPTIONS, LEVELONE_FUTURES, LEVELONE_FUTURES_OPTIONS, LEVELONE_FOREX, NYSE_BOOK, NASDAQ_BOOK, OPTIONS_BOOK, CHART_EQUITY, CHART_FUTURES, SCREENER_EQUITY, SCREENER_OPTION, ACCT_ACTIVITY (no TIMESALE_* service exists in the Schwab doc; it was TDA-only). | CONFIRMED-OFFICIAL |

### 2.5 ACCT_ACTIVITY

Official request (STREAM-DOC section 7, CONFIRMED-OFFICIAL):
`{"service":"ACCT_ACTIVITY","requestid":"2","command":"SUBS","SchwabClientCustomerId":..., "SchwabClientCorrelId":..., "parameters":{"keys":"Account Activity","fields":"0,1,2,3"}}`
- `keys`: "A client-provided string that streamer will populate updates with. Only first key is used if multiple are provided." Convention in clients: `"Account Activity"` (DEV `stream.md`; PY uses the correl id as key). `fields`: doc table says `"0" expected`; the example and all clients send `"0,1,2,3"`.
- Commands: SUBS, UNSUBS only.
- Scope: the key is arbitrary and no account is named, so the subscription is per USER/token, and the `1` field tells which account the activity is on. Whether it covers ALL accounts linked to the app/token is UNVERIFIED (PY docstring says "the account id associated with this streaming client"; no doc states it). MUST test with a second account.

Response item fields (CONFIRMED-OFFICIAL STREAM-DOC; same in PY `AccountActivityFields` `schwab/streaming.py` line 412 and RS `src/models/streaming/acct_activity.rs`):

| Key in item | Meaning | Type |
|---|---|---|
| `"seq"` | message sequence number; "If client reconnects and receives the same seq number again, it can choose to ignore the duplicate" -> use for de-dup | integer |
| `"key"` | the client key echoed back | string |
| `"1"` | Account number the activity occurred on (plain account number, NOT hash) | string |
| `"2"` | Message type; "dictates the format of the Message Data field" | string |
| `"3"` | Message data: "Either JSON-formatted data describing the update, NULL in some cases, or plain text in case of ERROR" | string (JSON-encoded string) |
(PY enumerates 0 SUBSCRIPTION_KEY, 1 ACCOUNT, 2 MESSAGE_TYPE, 3 MESSAGE_DATA; on the wire `seq`/`key` are named keys. RS test fixture also shows `"2":"Heartbeat"` with `"3":null` - synthetic, UNVERIFIED.)

Message types (field `"2"`) - the official doc does NOT enumerate them. Observed (CONFIRMED-CLIENT-SOURCE, DEV `docs/pages-raw/stream.md` "Account activity" section, includes real redacted payloads from Dec 2025 / Mar 2025):

| `"2"` value | When seen | Payload root under `BaseEvent` |
|---|---|---|
| `OrderCreated` | order placed | `OrderCreatedEventEquityOrder` |
| `OrderAccepted` | order accepted | `OrderAcceptedEvent` (`Status:"Open"`, `TradingSessionCodeOnOrderEntry:"REG"`, `QuoteOnOrderEntry[]`) |
| `OrderMonitorCreated`, `OrderMonitorUpdated`, `OrderMonitorCompleted` | order monitoring lifecycle | not shown |
| `ExecutionRequested` | routed to venue (also for cancel route, `RouteRequestedType:"Cancel"`) | `ExecutionRequestedEventRoutedInfo` |
| `ExecutionRequestCreated` | route created | `ExecutionRequestCreatedEvent` |
| `ExecutionRequestCompleted` | venue ack | `ExecutionRequestCompletedEvent` |
| `ExecutionCreated` | execution record (incl. `ExecutionTransType:"UROut"` for a cancel) | `ExecutionCreatedEventExecutionInfo` |
| `OrderFillCompleted` | fill (full) | `OrderFillCompletedEventOrderLegQuantityInfo` |
| `CancelAccepted` | cancel accepted | `CancelAcceptedEvent` |
| `OrderUROutCompleted` | cancel completed ("UROut") | `OrderUROutCompletedEvent` |

NOT confirmed for Schwab: the TDA-era type names `OrderEntryRequest`, `OrderFill`, `OrderPartialFill`, `OrderRejected`, `CancelRequest`, `UROut`, `CancelReplaceRequest`, `SUBSCRIBED`, `ERROR` (only `OrderEntryRequest` appears, in a schwab-py unit-test fixture inherited from TDA, `tests/streaming_test.py` line ~529 - synthetic). Names for partial fill, rejection, replace events are UNVERIFIED -> treat any unknown `"2"` as "unknown activity, trigger a REST re-poll". Do not key logic on a closed set.

`"3"` structure (CONFIRMED-CLIENT-SOURCE, DEV `stream.md`): a JSON string (not XML). Common envelope:
`{"SchwabOrderID":"<id>","AccountNumber":"<acct>","BaseEvent":{"EventType":"<same as field 2>","<EventSpecificKey>":{ ... }}}`.
Numeric values use a fixed-point object `{"lo":"<digits>","signScale":12}`; empty/zero = `{"signScale":12}` (no `lo`). From the samples: quantity 1 share = `"lo":"1000000"`, limit price 220.47 = `"lo":"220470000"`, fill price 213.4091 = `"lo":"213409100"` => value = lo / 10^6 (INFERRED; the meaning of `signScale` (12, 13) and negative numbers is UNVERIFIED - `signScale:13` appears on `EstimatedPrincipalAmount`). Timestamps: `{"DateTimeString":"2025-03-20 13:43:45.620"}` (no zone given; zone UNVERIFIED).

Fill event `OrderFillCompleted` fields (all in DEV sample): `SchwabOrderID` (== REST `orderId`, as string), `AccountNumber`, `BaseEvent.OrderFillCompletedEventOrderLegQuantityInfo{ LegId, LegStatus:"LegClosed", LegSubStatus:"LegSubStatusFilled", QuantityInfo{ExecutionID, CumulativeQuantity, LeavesQuantity, AveragePrice}, PriceImprovement, ExecutionInfo{ExecutionSequenceNumber, ExecutionId, VenueExecutionID, ExecutionQuantity, ExecutionPrice, ExecutionTimeStamp{DateTimeString}, ExecutionTransType:"Fill", ExecutionBroker, ExecutionCapacityCode, RouteName, RouteSequenceNumber, VenuExecutionTimeStamp (sic, "Venu"), ReportingCapacityCode, PrincipalAmmount (sic), ActualChargedCommissionAmount, ActualChargedFeesCommissionAndTax{...}, ClientOrderID}, OrderInfoForTransactionPosting{LimitPrice,...} }`. So fill time = `ExecutionInfo.ExecutionTimeStamp.DateTimeString`, fill qty = `ExecutionInfo.ExecutionQuantity`, price = `ExecutionInfo.ExecutionPrice`, remaining = `QuantityInfo.LeavesQuantity`. A partial fill's type name is UNVERIFIED (could be the same `OrderFillCompleted` with `LeavesQuantity > 0` and `LegStatus != LegClosed`). Cancel: `CancelAccepted` (`CancelAcceptedEvent.LegCancelRequestInfoList[]{LegID, IntendedOrderQuantity, RequestedAmount, LegStatus, LegSubStatus:"LegSubStatusCancelled", CancelAcceptedTime{DateTimeString}}`, `CancelRequestType:"ClientCancel"`) then `OrderUROutCompleted` (`CancelQuantity`, `LeavesQuantity`, `LegStatus:"LegClosed"`, `OutCancelType`).
Warning: sample `ResponseType` and `RouteStatus` appear as strings ("Accepted","RouteVenueAccepted") in one event and as integers (7, 8) in another -> model as untyped/`serde_json::Value`.

Redundancy rule recommended: use the stream as a low-latency NOTIFIER (account, `SchwabOrderID`, type) and re-read the order via `GET /accounts/{hash}/orders/{orderId}` for the authoritative state. Seq de-dup by `seq`. The DEV sample numbers two consecutive different messages with the same `seq` (6) in separate batches - `seq` uniqueness across batches is UNVERIFIED.

### 2.6 LEVELONE_EQUITIES

Request: service `LEVELONE_EQUITIES`, commands SUBS/ADD/UNSUBS/VIEW, `keys` = comma-separated uppercase symbols, `fields` = comma-separated numbers. Response content item: `key` (symbol), `delayed` (false = SIP real-time; true = NFL non-fee-liable subset/delayed), `assetMainType`, `assetSubType`, `cusip`, then requested fields as `"<n>": value`. Because delivery is "Change", the client MUST keep per-symbol state and merge deltas (a missing field means unchanged). Official example responses show only the requested fields. Source for all rows: STREAM-DOC section 3.1 (CONFIRMED-OFFICIAL); names in the last column are the PY enum names (`LevelOneEquityFields`, `schwab/streaming.py` line 636; CONFIRMED-CLIENT-SOURCE, number and order identical for 0-51).

| # | Official name | Type | Notes (official) | PY enum |
|---|---|---|---|---|
| 0 | Symbol | string | uppercase ticker | SYMBOL |
| 1 | Bid Price | double | current bid | BID_PRICE |
| 2 | Ask Price | double | current ask | ASK_PRICE |
| 3 | Last Price | double | last trade price | LAST_PRICE |
| 4 | Bid Size | int | "Units are 'lots' (typically 100 shares per lot)"; for NFL data can be 0 with non-zero bid = less than 100 shares | BID_SIZE |
| 5 | Ask Size | int | same as bid size | ASK_SIZE |
| 6 | Ask ID | char | exchange with the ask | ASK_ID |
| 7 | Bid ID | char | exchange with the bid | BID_ID |
| 8 | Total Volume | long | includes pre/post; reset 07:28 ET | TOTAL_VOLUME |
| 9 | Last Size | long | units shares | LAST_SIZE |
| 10 | High Price | double | regular session only; reset 03:30 ET | HIGH_PRICE |
| 11 | Low Price | double | regular session only | LOW_PRICE |
| 12 | Close Price | double | previous close; loaded 03:30 ET | CLOSE_PRICE |
| 13 | Exchange ID | char | primary listing exchange (A AMEX, Q NASDAQ, N NYSE, P Pacific, 9 Pinks, U OTCBB, 0 Indices, 3 Mutual Fund, : Indicator) | EXCHANGE_ID |
| 14 | Marginable | boolean | | MARGINABLE |
| 15 | Description | string | | DESCRIPTION |
| 16 | Last ID | char | exchange of last trade | LAST_ID |
| 17 | Open Price | double | regular session only; 0 if none | OPEN_PRICE |
| 18 | Net Change | double | last - close | NET_CHANGE |
| 19 | 52 Week High | double | | HIGH_PRICE_52_WEEK |
| 20 | 52 Week Low | double | | LOW_PRICE_52_WEEK |
| 21 | PE Ratio | double | | PE_RATIO |
| 22 | Annual Dividend Amount | double | | DIVIDEND_AMOUNT |
| 23 | Dividend Yield | double | | DIVIDEND_YIELD |
| 24 | NAV | double | mutual funds | NAV |
| 25 | Exchange Name | string | | EXCHANGE_NAME |
| 26 | Dividend Date | string | | DIVIDEND_DATE |
| 27 | Regular Market Quote | boolean | is last quote a regular quote | REGULAR_MARKET_QUOTE |
| 28 | Regular Market Trade | boolean | is last trade a regular trade | REGULAR_MARKET_TRADE |
| 29 | Regular Market Last Price | double | | REGULAR_MARKET_LAST_PRICE |
| 30 | Regular Market Last Size | integer | "Currently realize/100" | REGULAR_MARKET_LAST_SIZE |
| 31 | Regular Market Net Change | double | | REGULAR_MARKET_NET_CHANGE |
| 32 | Security Status | string | "Normal, Halted, Closed" (exact other spellings UNVERIFIED) | SECURITY_STATUS |
| 33 | Mark Price | double | | MARK |
| 34 | Quote Time in Long | long | ms since epoch UTC; "Last time a bid or ask updated" | QUOTE_TIME_MILLIS |
| 35 | Trade Time in Long | long | ms epoch | TRADE_TIME_MILLIS |
| 36 | Regular Market Trade Time in Long | long | ms epoch | REGULAR_MARKET_TRADE_MILLIS |
| 37 | Bid Time | long | ms epoch | BID_TIME_MILLIS |
| 38 | Ask Time | long | ms epoch | ASK_TIME_MILLIS |
| 39 | Ask MIC ID | string | 4-char MIC | ASK_MIC_ID |
| 40 | Bid MIC ID | string | | BID_MIC_ID |
| 41 | Last MIC ID | string | | LAST_MIC_ID |
| 42 | Net Percent Change | double | | NET_CHANGE_PERCENT |
| 43 | Regular Market Percent Change | double | | REGULAR_MARKET_CHANGE_PERCENT |
| 44 | Mark Price Net Change | double | | MARK_CHANGE |
| 45 | Mark Price Percent Change | double | | MARK_CHANGE_PERCENT |
| 46 | Hard to Borrow Quantity | integer | -1 = NULL | HTB_QUANTITY |
| 47 | Hard To Borrow Rate | double | null = NULL | HTB_RATE |
| 48 | Hard to Borrow | integer | -1 NULL, 1 true, 0 false | HARD_TO_BORROW |
| 49 | shortable | integer | -1 NULL, 1 true, 0 false | IS_SHORTABLE |
| 50 | Post-Market Net Change | double | | POST_MARKET_NET_CHANGE |
| 51 | Post-Market Percent Change | double | | POST_MARKET_NET_CHANGE_PERCENT |

Notes:
- The official request examples in the same doc request fields up to `54` and the sample response includes `"46"`; fields 52-54 are NOT defined in the doc -> UNVERIFIED (request only 0-51).
- Quote time = field 34 (also 37/38 per-side). Trade time = 35. Halt indication = field 32 `Security Status` ("Halted"); there is no LULD/limit-up-limit-down or short-sale-circuit-breaker field in the table -> LULD indication UNVERIFIED/absent.
- Size units: the streamer doc says LOTS (100 shares); the REST `QuoteEquity.bidSize/askSize` description says "Number of shares" (SPEC-M). Whether the REST value is also in lots is UNVERIFIED -> compare both against a known stock live before using sizes as share counts. In the doc sample AAPL bid size is `4` (lots, i.e. ~400 shares), consistent with lots.
- Crossed/locked quotes, zero bid or ask, and `delayed:true` records must be filtered when building a spread history.

### 2.7 CHART_EQUITY (1-minute bars) and others

| Item | Finding | Confidence |
|---|---|---|
| Official fields | 0 key (symbol), 1 Open Price, 2 High Price, 3 Low Price, 4 Close Price, 5 Volume (double), 6 Sequence (long, "Identifies the candle minute"), 7 Chart Time (ms epoch), 8 Chart Day (int). "Opening/Highest/Lowest/Closing price for the minute". Updates in regular and AM/PM hours. | STREAM-DOC - CONFIRMED-OFFICIAL; RS `src/models/streaming/chart_equity.rs` uses the same numbering - CONFIRMED-CLIENT-SOURCE |
| schwab-py differs | PY `ChartEquityFields` (`schwab/streaming.py` line 468): 0 SYMBOL, 1 SEQUENCE, 2 OPEN, 3 HIGH, 4 LOW, 5 CLOSE, 6 VOLUME, 7 CHART_TIME_MILLIS, 8 CHART_DAY (the old TDA order). CONFLICT -> trust the official Schwab doc (RS matches it) but TEST live which numbering arrives. | CONFLICT; see section 7 |
| TIMESALE_EQUITY | No such service in the Schwab doc. | CONFIRMED-OFFICIAL (absence) |
| NYSE_BOOK / NASDAQ_BOOK | Exist ("Level Two book for Equities", Whole delivery). Level 2 needs `offers[].level2Permissions`. Field table not extracted here. | STREAM-DOC - CONFIRMED-OFFICIAL (existence only) |

---

## 3. Fractional shares

| Statement | Source | Confidence |
|---|---|---|
| Product is "Schwab Stock Slices": invest in "any of America's leading companies from the S&P 500" for "as little as $5 each". | Schwab press release May 2020 (summarised via fetch of the pressroom page): https://pressroom.aboutschwab.com/press-releases/press-release/2020/Introducing-Schwab-Stock-Slices--Designed-to-Make-Investing-Easier-and-More-Accessible/default.aspx (also BusinessWire 20200505005240) | CONFIRMED-OFFICIAL (public Schwab page; read through a fetch summarizer) |
| Eligible universe S&P 500 stocks only (no ETFs). Current (2026) universe unchanged? | forum/third-party: https://mutualfundobserver.com/discuss/showthread.php?tid=55772 , https://wealthvieu.com/investing/schwab/fractional-shares/ | COMMUNITY-REPORTED; any post-2020 expansion UNVERIFIED |
| Orders are market orders; "You will not be permitted to enter limit or other conditional orders for Stock Slices purchases." Fractional share orders are "market 'not held' orders, and will be executed at market. A 'not held order' means that you grant Schwab time and price discretion to transact your order(s) on a best-efforts basis." "There may be a delay between the time when your order(s) are executed and when they appear in your account." | Quoted from the Stock Slices Terms and Conditions (https://www.schwab.com/public/file/P-12533231 , could not be fetched - access error) as quoted on https://mutualfundobserver.com/discuss/showthread.php?tid=55772 | COMMUNITY-REPORTED quoting Schwab T&C |
| Shares round to four decimal places ("slices of shares can round as narrowly as four decimal places"). | Schwab press release (pressroom URL above) | CONFIRMED-OFFICIAL (via summary) |
| Each stock is a separate trade order, no commission; dollar amount split equally across selected stocks (up to 30 stocks per purchase per third party). | Schwab T&C text as relayed by search results; wealthvieu | COMMUNITY-REPORTED |
| Regular-hours only: "Fractional orders at market price during trading hours. After-hours orders are queued for the next open" (quoted by a third party as Schwab text). Press release: "trade shares in real-time throughout the trading day using market orders". | https://wealthvieu.com/investing/schwab/fractional-shares/ ; pressroom release | COMMUNITY-REPORTED (queue behavior) / CONFIRMED-OFFICIAL (market orders, trading day) |
| Selling: fractions can be sold (the fraction, the whole share, or whole+fraction; not a fraction of a fraction). Schwab's X account (2026-01) said "Only Limit and Market orders are accepted for fractional shares" on selling. This CONTRADICTS "market only" for sells. | https://x.com/CharlesSchwab/status/2016174341294088541 (visible only in a search snippet; the page could not be fetched - HTTP 402) | COMMUNITY-REPORTED (search snippet); contradiction flagged |
| Batching/pricing detail ("not held": Schwab may aggregate/time orders) - no official numeric specification (no fixed execution times) found. | - | UNVERIFIED |
| Trader API acceptance of fractional quantity: `Order.quantity` and `orderLegCollection[].quantity` are `double` and a leg has `quantityType` enum `ALL_SHARES`, `DOLLARS`, `SHARES` (SPEC-T). But DEV states "Fractional orders are not supported" (`orders.md`) and a search summary says the Trader API does not allow fractional order entry. Whether `quantity: 0.5` or `quantityType: DOLLARS` is accepted for EQUITY is UNVERIFIED. | SPEC-T (types); DEV `docs/pages-raw/orders.md` | schema CONFIRMED-OFFICIAL; behavior COMMUNITY-REPORTED (not supported) / UNVERIFIED |
| How a fractional fill shows in history: schema types are `double` for `quantity`, `filledQuantity`, `remainingQuantity`, `executionLegs[].quantity` and `Position.longQuantity`, so decimals are representable; actual decimals/precision UNVERIFIED. Streamer fixed-point `lo`/10^6 gives six decimal places (inferred). | SPEC-T | schema CONFIRMED-OFFICIAL; actual UNVERIFIED |
| Owner's statement "fractional shares can only be filled as MARKET orders and only during regular hours". | - | Matches Schwab's buy-side T&C (market only, regular hours) - CONSISTENT for purchases; for SELLS Schwab's X reply says Limit is also accepted - UNVERIFIED which is right for the API. |

---

## Owner-confirmed facts (2026-10-10)

Stated by the account owner on 2026-10-10. They supersede the doubt expressed
in section 3 and section 7 about API acceptance; no research label elsewhere in
this file was changed.

| Fact | Label |
|---|---|
| The Trader API accepts fractional orders (the owner has used it). | OWNER-CONFIRMED |
| Fractional orders are `MARKET` only and only in regular trading hours. | OWNER-CONFIRMED |
| Fractional quantities are limited to 2 decimal places. (Schwab's press release says Stock Slices round to four; the API limit may differ.) Confirm with `previewOrder` (live test 5); the system floors to 2 decimals meanwhile. | OWNER-REPORTED, UNVERIFIED |
| Fractional trading covers only some stocks (Stock Slices: S&P 500 stocks, no ETFs). Eligibility comes from the owner-seeded `config/fractional_eligible.toml`; `Unknown` = not eligible. The current eligible list is not known from the API. | OWNER-REPORTED, UNVERIFIED (list) |

---

## 4. Market Data API (base `https://api.schwabapi.com/marketdata/v1`, SPEC-M servers; CONFIRMED-OFFICIAL)

### 4.1 Market hours

| Item | Finding | Confidence |
|---|---|---|
| `GET /markets` | query `markets` (required, array, unique; enum `equity`, `option`, `bond`, `future`, `forex`; comma separated), `date` (optional, `YYYY-MM-DD`). | SPEC-M - CONFIRMED-OFFICIAL |
| `GET /markets/{market_id}` | path `market_id` (same enum), `date` optional. | SPEC-M |
| Date limits | Description: "Valid date range is from currentdate to 1 year from today. It will default to current day if not entered." => PAST dates are NOT supported by the documented contract (behavior for a past date - error 400 or data - UNVERIFIED). PY `get_market_hours` doc: date "up to one year from today" (`schwab/client/base.py` line 1050). | SPEC-M; CONFIRMED-CLIENT-SOURCE |
| Response `/markets` | map: market key (`equity`) -> product key (`EQ`) -> `Hours`. `/markets/{id}` returns the same nesting. | SPEC-M |
| `Hours` fields | `date` (string), `marketType` (enum BOND, EQUITY, ETF, EXTENDED, FOREX, FUTURE, FUTURE_OPTION, FUNDAMENTAL, INDEX, INDICATOR, MUTUAL_FUND, OPTION, UNKNOWN), `product` (e.g. `EQ`), `productName` (e.g. `equity`), `exchange`, `category`, `isOpen` (bool), `sessionHours` (map session name -> array of `{start, end}` ISO strings with offset) | SPEC-M |
| Session keys seen | `preMarket` 07:00-09:30, `regularMarket` 09:30-16:00, `postMarket` 16:00-20:00 (all `-04:00` in the example); equity-option `EQO` regular 09:30-16:00 and index option `IND` to 16:15 | SPEC-M example `GetMarketHours`; DEV `api.md` |
| Reflects holidays? | `isOpen` field exists and `sessionHours` is a map that may be empty on closed days (expected, not shown in any sample). Real holiday / early close (13:00 close) behavior - whether `isOpen=false` and whether early-close `regularMarket.end` is 13:00 - is UNVERIFIED. No public confirmation found. Recommended use (revised): query dates in `[today, today + 30 days]` (**today is included**: the documented range starts at the current date; what it returns for *today* on a closure morning is UNVERIFIED), never past dates; treat `isOpen=false` OR missing `regularMarket` as closed; cross-check against the computed NYSE calendar; AUTO-WRITE an add-only override for a weekday that the rules treat as a session **on the first sighting** (a false closure can only lengthen a lock block, a missed one is a breach; `09-lock-state-machine` §4). The earlier "2 consecutive daily polls" recommendation is withdrawn: it let a closure announced the evening before pass unseen. Early-close sessions (`regularMarket.end` = 13:00) are not closures but set the execution window's close time. | UNVERIFIED |
| Rate | counts against the 120 requests/minute reported limit. | COMMUNITY-REPORTED |

### 4.2 Quotes REST

`GET /quotes` params: `symbols` (comma-separated), `fields` (comma-separated root nodes from `quote, fundamental, extended, reference, regular`; default all), `indicative` (bool). `GET /{symbol_id}/quotes` for one symbol. Response: map symbol -> object; equity objects (`EquityResponse`): `assetMainType`, `assetSubType`, `quoteType` (NBBO | NFL | null), `realtime` (bool), `ssid`, `symbol`, `quote`, `reference`, `regular`, `extended`, `fundamental`. Errors per symbol via `QuoteError {invalidSymbols[], invalidCusips[], invalidSSIDs[]}`. (SPEC-M, CONFIRMED-OFFICIAL)

`quote` (`QuoteEquity`) fields: `bidPrice`, `askPrice`, `bidSize` (int32, "Number of shares for bid"), `askSize`, `bidTime` (ms epoch), `askTime`, `bidMICId`, `askMICId`, `lastPrice`, `lastSize`, `lastMICId`, `mark`, `markChange`, `markPercentChange`, `closePrice`, `openPrice`, `highPrice`, `lowPrice`, `netChange`, `netPercentChange`, `totalVolume`, `quoteTime` (ms epoch), `tradeTime`, `securityStatus` (string), `52WeekHigh`, `52WeekLow`, `volatility`.
`regular` (`RegularMarket`): `regularMarketLastPrice`, `regularMarketLastSize`, `regularMarketNetChange`, `regularMarketPercentChange`, `regularMarketTradeTime`. `extended`: `askPrice`, `bidPrice`, `askSize`, `bidSize`, `lastPrice`, `lastSize`, `mark`, `quoteTime`, `tradeTime`, `totalVolume`. `reference`: `cusip`, `description`, `exchange`, `exchangeName`, `isShortable`, `isHardToBorrow`, `htbQuantity`, `htbRate`, `fsiDesc`, `otcMarketTier`.
Price history for intraday prefill: `GET /pricehistory` (`symbol`, `periodType`=day, `period` in {1,2,3,4,5,10}, `frequencyType`=minute, `frequency` in {1,5,10,15,30}, `startDate`/`endDate` ms epoch, `needExtendedHoursData`) gives OHLCV only - NO bid/ask. A bid/ask spread history cannot be backfilled from REST; it must be built from the stream or REST polls from startup (SPEC-M). Pre-open "prefill" of spread history is therefore UNAVAILABLE historically.

---

## 5. Rate limits and auth (all COMMUNITY-REPORTED unless noted)

| Fact | Source |
|---|---|
| Access token valid 30 minutes; refresh token valid 7 days (fixed, no sliding extension reported; re-authorization in browser needed weekly). | DEV `docs/pages-raw/welcome.md`/`setupguide.md` (text "Refresh token - valid for 7 days... Access token - valid for 30 minutes"); COMMUNITY-REPORTED |
| 120 API requests/minute; 120 order requests/minute; 4000 order-related calls/day; 500 symbols streamed; HTTP 429 when exceeded. | DEV `client.md`, `orders.md`, `api.md`; COMMUNITY-REPORTED (not in OpenAPI spec) |
| Third-party article quoting "2-4 trade requests per second" and "maximum 120 requests per minute per account" for PUT/POST/DELETE. | https://grokipedia.com/page/Schwab_Trader_API (cites forexbrokers.com; low quality) |
| Callback URL must be HTTPS with explicit port (schwab-rs requires host 127.0.0.1). OAuth details are in `OAUTH.md` of this skill. | https://github.com/major/schwab-rs/blob/main/docs/auth.md |
| App needs both "Accounts and Trading Production" and "Market Data Production" products for full API use. | DEV `client.md` |
| `Schwab-Client-CorrelId` response header on REST: log it for support tickets. | SPEC-T headers - CONFIRMED-OFFICIAL |

---

## 6. Recommended data flow (design consequences of the above)

1. Startup / 09:00: `GET /accounts/accountNumbers`; then for each hash `GET /accounts/{hash}/orders?fromEnteredTime=..&toEnteredTime=..&maxResults=3000` in windows no larger than 60 days (safe under both documented limits), covering >= the 5-day lock window plus the longest GTC horizon; also `GET /orders` over <= 60 days as a cross-check. Count < maxResults per window to detect truncation.
2. Login to Streamer once (one connection per user), subscribe ACCT_ACTIVITY (`keys:"Account Activity"`, `fields:"0,1,2,3"`), and LEVELONE_EQUITIES for needed symbols with fields `0,1,2,3,4,5,32,34,35,37,38`.
3. On every ACCT_ACTIVITY message: parse `"1"` account, `"2"` type, `"3"` JSON; extract `SchwabOrderID`; re-fetch the order over REST for authoritative state; de-dup by `seq`.
4. On reconnect or heartbeat gap: re-run step 1 for the gap window (events may be lost; no replay is documented).
5. At EVERY order authorization (freshness bound `N = 0`, owner decision): re-read each linked account's order window (step 1, <= 60 days) and `GET /accounts/{hash}/transactions?types=TRADE` for the last 14 days before the lock is evaluated; the stream (steps 2-3) never substitutes for this read (`09-lock-state-machine` §3.2).

---

## 7. Contradictions with the owner's statements / assumptions

| Topic | Owner / assumption | Finding |
|---|---|---|
| Order path | `GET /accounts/{accountNumber}/orders` | Spec path is exactly that, but `{accountNumber}` must be the ENCRYPTED HASH from `/accounts/accountNumbers`. Full path `/trader/v1/accounts/{hashValue}/orders`. |
| Order history window | (unspecified) | Spec: 1 year (per-account), 60 days (`/orders`); schwab-py says 60 days for both. `fromEnteredTime`/`toEnteredTime` are marked REQUIRED in the spec though clients treat them as optional (default 60 days). |
| Fractional | market only, regular hours only | Purchase side matches Schwab's T&C. Schwab's X reply says Limit AND Market orders are accepted for SELLING fractions (search snippet). Whether the API accepts fractional quantity at all is NOT confirmed (Schwabdev says unsupported). |
| TDA-era ACCT_ACTIVITY names (OrderEntryRequest, OrderFill, OrderPartialFill, UROut, CancelAccepted, ...) | assumed | Real Schwab types observed: OrderCreated, OrderAccepted, ExecutionRequested, ExecutionRequestCreated, ExecutionRequestCompleted, ExecutionCreated, OrderFillCompleted, CancelAccepted, OrderUROutCompleted, OrderMonitor*. Payload is JSON (not XML) with a `BaseEvent` envelope and fixed-point `{lo, signScale}` numbers. |
| Replace linkage | REPLACED + link | Enum REPLACED and field `replacingOrderCollection` exist, but no `replacedOrderId`-style field; populated linkage UNVERIFIED. New orderId on replace is confirmed by the official text. |
| Fill-time location | assumed on order | REST: only `orderActivityCollection[].executionLegs[].time`. Stream: `ExecutionInfo.ExecutionTimeStamp.DateTimeString` (zone unknown). |
| CHART_EQUITY numbering | schwab-py enum | Official doc and schwab-rs: 1 open, 2 high, 3 low, 4 close, 5 volume, 6 sequence. schwab-py: 1 sequence, 2 open ... 6 volume. Test live. |
| Streamer sizes | shares | Doc: lots of 100 shares. |
| Market hours for past dates | assumed supported (for holiday overrides file) | Spec says valid range is today to +1 year. Auto-maintenance probes today and the next 30 days (add-only, first sighting) and compares with the rule-based calendar; past dates are never queried. |
| Spread history prefill | assumed from REST | No historical bid/ask endpoint; `pricehistory` is OHLCV only. |

---

## 8. Open items we could not confirm (test against a live account)

| # | Item | How to test |
|---|---|---|
| 1 | Effective max window of `GET /accounts/{hash}/orders` (60 days vs 1 year), behavior when `from/to` omitted, truncation when > maxResults, result ordering | query windows of 30/59/61/90/365/400 days; compare counts; omit params |
| 2 | Does ACCT_ACTIVITY deliver events for ALL linked accounts on one subscription? | place/cancel tiny order in account B while subscribed |
| 3 | Partial fill message type(s) and shape; reject message type; replace event types; `LeavesQuantity` semantics | place limit orders for 1-2 shares, partial-fill if possible, replace, cancel, reject (e.g. invalid price) |
| 4 | `signScale` meaning and fixed-point scale for fractional/negative values; `DateTimeString` timezone | compare stream fill to REST `executionLegs.time` for the same order |
| 5 | Replace/cancel representation in history: old order status (REPLACED vs CANCELED), `replacingOrderCollection` population, `closeTime` values, partial fill + cancel (`filledQuantity>0`, status CANCELED) | exercise each and dump the JSON |
| 6 | Time zone/offset format of `enteredTime`, `closeTime`, `executionLegs[].time` (`+0000`?) | compare to wall clock |
| 7 | Fractional: does POST accept `quantity: 0.5` or `quantityType: DOLLARS`; how fills appear; sell limit vs market; hours | preview-order (`/previewOrder`) first, then tiny live order |
| 8 | Streamer behaviors: heartbeat cadence, session survival across 30-min access-token expiry, symbol limit value (500?), ACCT_ACTIVITY availability outside market hours, code on duplicate connection | long soak run |
| 9 | LEVELONE_EQUITIES: size units vs REST, fields 52-54, `Security Status` spellings (Halted/Closed/...), LULD/halt representation, behavior of `Change` delivery (stale-field handling) | subscribe SPY/halted symbol, log raw |
| 10 | `/markets` for holiday and early-close dates (isOpen false? sessionHours empty? 13:00 end?), behavior for past dates, response for Saturday | query next Thanksgiving/Christmas, a half-day, a Saturday, and yesterday |
| 11 | CHART_EQUITY actual field numbering | subscribe and compare against known candle |
| 12 | `userPreference` shape: array vs object; `streamerInfo` element count | dump |
| 13 | Is `Order.accountNumber` populated in `GET /orders` and in per-account responses; are hashes stable | dump |
| 14 | Rate-limit actuals and 429 `Retry-After` headers | back-off test |

Five most important to test first: (1) order-history window and truncation (item 1); (2) ACCT_ACTIVITY scope across accounts (item 2); (3) partial fill / cancel / replace representation in REST and in the stream (items 3 and 5); (4) fill-time field and time zone (items 4 and 6); (5) fractional order acceptance via API and its sell-side order types (item 7).

---

## 9. Rust modelling notes (names and field lists only; fail closed)

General: `#[serde(rename_all = "camelCase")]` for REST; every field `Option<_>` except identity keys; every enum has `#[serde(other)] Unknown` (or a `Raw(String)` capture) and any `Unknown` in a decision path (lock accounting, status) must block trading / force a REST re-read instead of being ignored. Numbers: use `rust_decimal::Decimal` (deserialize from f64 with care) for quantity/price; `i64` ids; `time`/`chrono` parse for `date-time` tolerant of `+0000`, `+00:00`, `Z`.

| Type | Fields |
|---|---|
| `Order` | `order_id: i64`, `account_number: Option<i64>`, `status: OrderStatus`, `status_description`, `entered_time`, `close_time`, `cancel_time`, `release_time`, `session: Session`, `duration: Duration`, `order_type: OrderType`, `complex_order_strategy_type`, `order_strategy_type`, `quantity`, `filled_quantity`, `remaining_quantity`, `price`, `stop_price`, `activation_price`, `special_instruction`, `tax_lot_method`, `requested_destination`, `destination_link_name`, `cancelable`, `editable`, `tag`, `order_leg_collection: Vec<OrderLeg>`, `order_activity_collection: Vec<OrderActivity>`, `replacing_order_collection: Vec<Order>`, `child_order_strategies: Vec<Order>`, `#[serde(flatten)] extra: Map` (to log unknown fields) |
| `OrderLeg` | `leg_id`, `order_leg_type`, `instrument: Instrument{asset_type, symbol, cusip, description, instrument_id}`, `instruction: Instruction`, `position_effect`, `quantity`, `quantity_type` |
| `OrderActivity` | `activity_type: ActivityType {Execution, OrderAction, Unknown}`, `execution_type: ExecutionType {Fill, Unknown}`, `quantity`, `order_remaining_quantity`, `execution_legs: Vec<ExecutionLeg>` |
| `ExecutionLeg` | `leg_id`, `price`, `quantity`, `mismarked_quantity`, `instrument_id`, `time` |
| `OrderStatus` | the 21 values in 1.5 + `Unknown` via `#[serde(other)]` (note API itself has `UNKNOWN`; map both to one variant) |
| `Session`, `Duration`, `OrderType`, `Instruction`, `AssetType`, `OrderStrategyType` | enums per 1.5 each with `#[serde(other)] Unknown` |
| `AccountNumberHash` | `account_number: String`, `hash_value: String` |
| `StreamEnvelope` (untagged top level) | `response: Vec<StreamResponse>`, `notify: Vec<Notify{heartbeat: Option<String>}>`, `data: Vec<StreamData>` |
| `StreamResponse` | `service`, `command`, `requestid`, `schwab_client_correl_id` (rename `SchwabClientCorrelId`), `timestamp: i64`, `content: {code: i32, msg: String}` |
| `StreamData` | `service`, `command`, `timestamp: i64`, `content: Vec<serde_json::Value>` (parsed per service) |
| `AcctActivityMsg` | `seq: i64` (accept string or int), `key: String`, `account: String` (JSON key `"1"`), `message_type: AcctActivityType` (`"2"`), `message_data: Option<String>` (`"3"`, raw), then parsed `AcctActivityEvent` |
| `AcctActivityType` | `OrderCreated, OrderAccepted, OrderMonitorCreated, OrderMonitorUpdated, OrderMonitorCompleted, ExecutionRequested, ExecutionRequestCreated, ExecutionRequestCompleted, ExecutionCreated, OrderFillCompleted, CancelAccepted, OrderUROutCompleted, Unknown(String)` (capture the raw string; Unknown -> REST re-poll) |
| `AcctActivityEvent` | `schwab_order_id: String` (key `SchwabOrderID`), `account_number: String` (`AccountNumber`), `base_event: BaseEvent{event_type: String, payload: serde_json::Value}`; typed helper `FillInfo{leg_id, execution_id, execution_quantity, execution_price, execution_time_string, cumulative_quantity, leaves_quantity, average_price, leg_status, leg_sub_status}` parsed from `OrderFillCompletedEventOrderLegQuantityInfo`; `Fixed{lo: String, sign_scale: i32}` with `value = lo / 10^6` (inferred, UNVERIFIED - keep raw) |
| `LevelOneEquity` (all `Option`, keys are numeric strings; merge deltas into per-symbol state) | `key: String` (symbol), `delayed: bool`, `asset_main_type`, `asset_sub_type`, `cusip`, `f1 bid_price`, `f2 ask_price`, `f3 last_price`, `f4 bid_size (lots)`, `f5 ask_size (lots)`, `f8 total_volume`, `f9 last_size`, `f32 security_status: SecurityStatus{Normal, Halted, Closed, Unknown(String)}`, `f34 quote_time_ms`, `f35 trade_time_ms`, `f36 regular_market_trade_time_ms`, `f37 bid_time_ms`, `f38 ask_time_ms`, `f33 mark`, `f12 close_price` (plus any other of 0-51 as needed); deserialize via `HashMap<String, Value>` with explicit field-number constants, not derive-rename, since keys are digits |
| `ChartEquity` | `key`, `f1..f8` per the official numbering with a runtime sanity check (high >= low) because of the schwab-py conflict |
| `MarketHours` | `date: String`, `market_type`, `product`, `product_name`, `is_open: bool`, `session_hours: Map<String, Vec<{start, end}>>` (empty or missing = closed) |
| `Quote` (REST) | `bid_price`, `ask_price`, `bid_size`, `ask_size`, `bid_time`, `ask_time`, `quote_time`, `trade_time`, `security_status`, `mark`, `last_price`; envelope `{symbol, realtime, quote_type, quote}` |
