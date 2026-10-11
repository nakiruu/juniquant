# Source Details

Per-source endpoints, parameters, parsing rules and quirks. Everything is
Rust over direct HTTP; no vendor client library, no Python. Prices come from
EODHD (paid, DECISIONS D15); everything else is free. Rate limits are
in `SKILL.md` §5 (the one table); retry constants in `RETRY-POLICY.md`.

**Verification status.** The endpoint shapes, field names and limits below
were written from memory of the public services; none was probed in the
authoring environment. Every line marked **verify** is confirmed or
corrected by the first probe run (`BACKFILL.md` §2), and the probe result is
recorded in the manifest. Do not rely on an unverified line for a decision
that cannot be undone.

---

## EODHD (primary price source, DECISIONS D15)

**Client:** direct HTTP (`reqwest`) to the EODHD REST API. No vendor client
library. **Plan:** "EOD Historical Data, All World" (about $20/month, personal
use): end-of-day prices, splits and dividends, including delisted tickers; no
fundamentals (EDGAR stays the fundamentals source). Raw responses stay on this
machine; nothing is redistributed.

**Auth.** `api_token={EODHD_API_KEY}` query parameter on every request, read
from the environment (`.env`). The key is redacted in every sidecar, log and
probe record, like the FRED key.

**Symbols.** `{CODE}.US`, where `CODE` is the vendor form of the canonical
ticker (`UNIVERSE.md` §3). The `.US` exchange is composite (NYSE, Nasdaq,
NYSE American). Share classes use a hyphen: `BRK-B.US` (P3; `BRK.B.US` is a 404).

**Endpoints** (from the EODHD documentation, 2026-10-11; each costs 1 API call
per request whatever the range, except bulk):

| Endpoint | Returns |
|---|---|
| `GET /api/eod/{CODE}.US?fmt=json&from=YYYY-MM-DD&to=YYYY-MM-DD` | `[{date, open, high, low, close, adjusted_close, volume}]` |
| `GET /api/splits/{CODE}.US?fmt=json&from=...` | `[{date, split: "4.000000/1.000000"}]`, new shares first; `date` is the ex-date |
| `GET /api/div/{CODE}.US?fmt=json&from=...` | `[{date, declarationDate, recordDate, paymentDate, period, value, unadjustedValue, currency}]`; `date` is the ex-date |
| `GET /api/eod-bulk-last-day/US?fmt=json&date=YYYY-MM-DD[&type=splits\|dividends]` | One trading day for the whole exchange; **100 calls** per request. A non-trading day returns the previous trading day |
| `GET /api/exchange-symbol-list/US?fmt=json[&delisted=1][&type=common_stock]` | `[{Code, Name, Country, Exchange, Currency, Type, Isin}]` today; `delisted=1` returns the delisted set instead. A snapshot with no dates: a fetch list, never a point-in-time listing (`listings` #35 stays Nasdaq Trader + EDGAR) |
| `GET /api/user` | Plan and usage counters; costs no call (P4). The body carries the account name and email: never landed |

**What the fields mean** (EODHD documentation, confirmed by P1):

- `open`, `high`, `low`, `close`: **raw, as traded** that day, no adjustment.
- `volume`: **split-adjusted** (restated in today's shares).
- `adjusted_close`: adjusted for splits and dividends; stored as
  `adj_close_vendor` (diagnostic, reconciliation gate only).
- Dividend `unadjustedValue`: the amount actually paid per share on the day;
  `value` is restated in today's shares. `dividends.amount` stores
  `unadjustedValue`.
- A **delisted** ticker keeps its history (P2: TWTR, CELG, RHT, LLTC, TWX,
  LEH to their last day).
- **A code holds its current holder only.** A renamed company's history moves
  to its new code (Facebook from 2012 is under `META.US`). An earlier holder of
  a reused code is renamed `{CODE}_old`, `_old1`, `_old2`, ... in the delisted
  list and identified there by `Name` and `Isin` (`MON_old` is Monsanto 2000-2018;
  `MON.US` is a 2021 holder; `SHLD_old` is Sears, `SHLD.US` a 2023 fund). So the
  vendor code of a historical holder comes from the delisted list (matched by
  ISIN, else by name and dates), never from rendering the old ticker, and every
  fetched series is checked against the holding period it is used for (first and
  last bar against first and last member day). A `_old` code is not a canonical
  ticker (it fails `symbols::normalize`) and is never written to
  `config/symbol_map.toml`; it is stored with the identifier master's holding
  (I2, open).
- Depth: US common stocks from 1962-01-02 at the earliest; several delisted
  series start on 1997-12-31 instead of their listing (P3). The backfill needs
  2005 on (D12).

**Normalization to as-traded.** Prices are already raw (`F = 1` for OHLC).
Volume is undone with the splits fetched in the same run:

    F(d)      = product of ratio_s over splits s with ex_date_s > d and ex_date_s <= eod fetch date
    volume_d  = round(vendor_volume_d / F(d))

The splits request is made right after the eod request; a split whose ex-date
falls after the eod fetch date is not in that eod response's adjustment and is
excluded from `F`.

**Usage.** 100,000 calls per day and 1,000 per minute on this plan (pricing
page, 2026-10-11). Measured costs (P4): eod, splits and dividends 1 call each,
a bulk request 100. The backfill costs 3 calls per code: in scope are 6,059
active and 16,588 delisted common-stock codes on NYSE, Nasdaq and NYSE American
(P2), about 68,000 calls, so it fits in one day under the cap.
The daily update uses the three bulk requests: 300 calls. The client stops for
the day at 90,000 calls (counted in the manifest) rather than run into the cap.

**Point in time.** A close for `t` is usable after 16:00 ET on `t`; the daily
job runs after EODHD publishes the day (time **verify**, P4) and drops a bar
dated today if it is fetched before 16:05 ET.

**Failure modes.** An unknown code returns `200` with `[]` (or `404`): the chunk
is `gone` with reason `vendor_no_data`, as a Yahoo `Not Found` was. A code that
answers with another holder's series (a reused ticker) is caught by the
holding-period check above and logged as `vendor_wrong_holder`. `401`
or `403` (key, plan) stops the run: never retried. `429` follows
`RETRY-POLICY.md`.

---

## Yahoo Finance (chart API): optional secondary source

Since D15 Yahoo is not the price source. It may be registered as a secondary
source behind the price-source interface (`SURVIVORSHIP.md` §2, source id
`yahoo`) for names EODHD does not serve; the rules below apply only then. On
2026-10-11 Yahoo answered every request from this machine with `429`, so it is
unusable until that clears.

**Client:** direct HTTP (`reqwest`) to the chart endpoint. No `yfinance`, no
crate that wraps it.

**Terms.** The chart endpoint is an undocumented web API. Yahoo's terms
restrict automated use and redistribution (verify). This system uses it for
personal research only; it can disappear or change without notice, which is
why every raw response is kept and the `PriceSource` interface exists
(`SURVIVORSHIP.md` §2).

**Endpoint (verify):**

    GET https://query1.finance.yahoo.com/v8/finance/chart/{SYMBOL}
        ?period1={unix}&period2={unix}&interval=1d
        &events=div%2Csplit&includeAdjustedClose=true

Send a browser-like `User-Agent`. `query2.finance.yahoo.com` is an alternate
host. `{SYMBOL}` is the vendor form of the canonical ticker (`BRK.B` ->
`BRK-B`; `UNIVERSE.md` §3).

**Response fields used (verify):**

| Path | Meaning |
|---|---|
| `chart.result[0].meta` | `symbol`, `currency`, `exchangeTimezoneName`, `gmtoffset`, `firstTradeDate`, `instrumentType` |
| `chart.result[0].timestamp[]` | Unix seconds, one per bar. Convert to a calendar date in `exchangeTimezoneName` (a bar is stamped at the open, 09:30 ET) |
| `...indicators.quote[0].{open,high,low,close,volume}[]` | Prices and volume; `null` entries are possible |
| `...indicators.adjclose[0].adjclose[]` | Vendor's adjusted close (stored as `adj_close_vendor`, diagnostic) |
| `...events.splits{ts: {date, numerator, denominator, splitRatio}}` | Split events |
| `...events.dividends{ts: {date, amount}}` | Dividend events |
| `chart.error` | `{code: "Not Found", description: "No data found, symbol may be delisted"}` for an unknown or delisted symbol |

**Window rule.** One request returns the whole history for a symbol
(`period1` at the backfill start or earlier). Every request's window must
**end at the fetch time** (`period2` = now): the normalization below needs
every split after any bar in the response to be in the same response.

### Raw means as-traded: undoing the vendor's split adjustment

The vendor's `open/high/low/close/volume` and its dividend amounts are, to the
best of my recollection, **already adjusted for splits as of the fetch date**
(the "Close" column is split-adjusted but not dividend-adjusted; "Adj Close"
is both). Under that behaviour a stored row would change meaning whenever a
later split happens, and rows fetched on different days would not be
comparable. **Verify with the probe** (`BACKFILL.md` §2): a known split name,
for example AAPL around its 4-for-1 on 2020-08-31, must show the pre-split
closes divided by 4 if my recollection is right; the probe result sets the
config flag `yahoo_ohlcv_split_adjusted = true|false`.

If `true`, the parser undoes the adjustment with the splits of the same
response, so the stored values are the as-traded values and do not depend on
the fetch date:

    F(d)        = product of ratio_s over splits s in this response with ex_date_s > d
                  (ratio = numerator / denominator: new shares per old share)
    price_d     = round4(vendor_price_d * F(d))          # open, high, low, close
    volume_d    = round(vendor_volume_d / F(d))
    dividend_d  = vendor_amount_d * F(d)                 # at the dividend's ex-date
    adj_close_vendor_d = stored as returned (never used except by the reconciliation gate)

`round4` is rounding to 4 decimals (pre-decimalization prices such as 1/16 =
0.0625 survive; verify against a known unadjusted close). If the flag is
`false`, `F` is 1.

Sketch (not normative, not compiled; the parser is a pure function of the raw
bytes so it can be replayed and unit-tested with captured fixtures):

```rust
// Pure: raw body -> normalized rows. No I/O, no clock.
fn normalize(raw: &ChartResponse, split_adjusted: bool) -> Result<Normalized, ParseError> {
    let splits: Vec<Split> = raw.splits();                      // ex_date, numerator, denominator
    let f = |d: NaiveDate| -> f64 {
        if !split_adjusted { return 1.0; }
        splits.iter().filter(|s| s.ex_date > d).map(|s| s.num / s.den).product()
    };
    let mut bars = Vec::new();
    for b in raw.bars() {                                       // date in the exchange time zone
        let (Some(o), Some(h), Some(l), Some(c), Some(v)) = (b.open, b.high, b.low, b.close, b.volume)
            else { continue };                                  // dropped and counted for data_quality
        let k = f(b.date);
        bars.push(Bar { date: b.date, open: round4(o * k), high: round4(h * k),
                        low: round4(l * k), close: round4(c * k),
                        volume: (v as f64 / k).round() as i64, adj_close_vendor: b.adjclose });
    }
    Ok(Normalized { bars, splits, dividends: raw.dividends().map(|d| d.scaled(f(d.ex_date))).collect() })
}
```

**Key rules:**
- Rows with a `null` open, high, low, close or volume are dropped and
  counted (WARN if more than 1% of a series).
- Sanity per row: `high >= low`, `low <= close <= high`, `volume >= 0`;
  violations are dropped and logged.
- `date` is the calendar date of the bar in the exchange time zone, not UTC.
- A response that contains a bar dated after the fetch date has those bars
  dropped.
- The vendor's last bar can be a partial intraday bar when fetched during the
  session. The daily job runs after the close (16:10 ET) and drops a bar
  dated today if the fetch time is before 16:05 ET.

**Known issues:**
- Intermittent 429s under load. Back off 60 s (`SKILL.md` §7).
- Occasional bad ticks and 10x spikes in a few series; the price gates catch
  them (`QUALITY-GATES.md` G2).
- Delisted and acquired symbols frequently return `Not Found`, or return a
  truncated series; a reused symbol returns the **current** holder's history
  under the old holder's name. See `SURVIVORSHIP.md` §1 (verify the
  behaviour with the delisted-symbol probe).
- A symbol's history can begin well after the security's real first trading
  day (`meta.firstTradeDate` helps detect symbol reuse).

**Not ingested (deferred, no destination dataset by decision):**
`Ticker.info` / `quoteSummary` (a snapshot, not a time series, and needs a
cookie/crumb session), `Ticker.calendar` (estimated future earnings dates).
Past earnings dates come from EDGAR 8-K Item 2.02 in `filing_events`
(point-in-time, below). A forward-looking earnings calendar has no free
point-in-time source and is deferred; nothing in GSPQR needs it.

---

## SEC EDGAR

**Client:** direct HTTP. `edgartools` is not used.

**Auth:** `User-Agent: "<ProjectName> <real email>"` from `SEC_EDGAR_EMAIL`.
Requests without it are blocked. Maximum 10 requests/s (verify); we run
5/s (`SKILL.md` §5).

**Endpoints** (P5, 2026-10-11: the two bulk URLs, `company_tickers.json`, `cik-lookup-data.txt` and
the `data.sec.gov` per-filer URLs answered 200; `form.idx` was not probed):

| Use | URL |
|---|---|
| Bulk submissions (all filers; filing lists, 8-K items, tickers, former names) | `https://www.sec.gov/Archives/edgar/daily-index/bulkdata/submissions.zip` |
| Bulk XBRL facts (all filers) | `https://www.sec.gov/Archives/edgar/daily-index/xbrl/companyfacts.zip` |
| One filer's submissions (daily delta) | `https://data.sec.gov/submissions/CIK##########.json` |
| One filer's facts (daily delta) | `https://data.sec.gov/api/xbrl/companyfacts/CIK##########.json` |
| Current ticker -> CIK map | `https://www.sec.gov/files/company_tickers.json`, `.../company_tickers_exchange.json` |
| Name -> CIK lookup (historical names) | `https://www.sec.gov/Archives/edgar/cik-lookup-data.txt` |
| Quarterly filing index (cross-check only) | `https://www.sec.gov/Archives/edgar/full-index/YYYY/QTRn/form.idx` |

The two zips are the backfill source (`BACKFILL.md` §6). P5: `submissions.zip` is
1,568,623,659 bytes and `companyfacts.zip` 1,410,325,968; both send `Accept-Ranges: bytes`
(a 206 range read works, so a zip's central directory can be listed without downloading it).
Their `Last-Modified` was 2026-10-10 04:33 and 04:26 GMT when probed on 2026-10-11 14:05 UTC;
nightly regeneration is plausible but seen once (verify with a `HEAD` on two consecutive
days). Record `Last-Modified` in the sidecar of every download. `CIK##########` is the 10-digit zero-padded CIK.
`filings.recent` in a submissions JSON holds the most recent filings; older
filings are in the extra pages named in `filings.files[]`
(`CIK##########-submissions-00N.json`). The bulk zip includes those pages (P5: 5,394
page files for 4,169 filers, up to page 071, beside 988,520 `CIK##########.json`; both GOOGL
and BRK have theirs), so no per-filer follow-up request is needed for older filings. Fields per filing used: `accessionNumber`,
`filingDate`, `acceptanceDateTime`, `form`, `items` (8-K item numbers,
comma-separated), `reportDate`. Top-level: `name`, `tickers[]`,
`exchanges[]`, `formerNames[{name, from, to}]`, `sic` and `sicDescription` (both strings,
P5: GOOGL `"7370"`, BRK `"6331"`). `tickers[]` can hold tickers we do not trade (GOOGL lists
`GOOGM` and `GOOGN`; BRK spells `BRK-B`): take only tickers the symbol map lists.

**Point-in-time keys:** `filingDate` (-> `filing_date`) is the stamp.
`reportDate` is the period end (-> `period_end`, metadata).
`acceptanceDateTime` (-> `acceptance_datetime`) is the exact timestamp.

### Building `fundamentals` from `companyfacts`

A `companyfacts` JSON has, per taxonomy (`us-gaap`, `dei`) and tag, the list
of facts `{start?, end, val, accn, fy, fp, form, filed, frame?}` in USD (or
shares). Each filing that repeats a figure (as a comparative column) repeats
the fact with its own `accn` and `filed`. `fy` and `fp` describe the
**filing**, not the individual fact (confirmed, P5: GOOGL's balance-sheet date 2025-12-31 is
reported under `(2025, FY)`, `(2026, Q1)` and `(2026, Q2)`); never use them to date a fact.
Dimensioned (segment) facts are not in `companyfacts` (confirmed, P5).

**Multi-class issuers (P5).** Cover-page share counts are reported per class, hence
dimensioned, so `dei:EntityCommonStockSharesOutstanding` is absent from `companyfacts` for
GOOGL and stops in 2011 for BRK (CIK 1067983, classes A and B). GOOGL carries the total of all
classes in `us-gaap:CommonStockSharesOutstanding` (12,230,000,000 at 2026-06-30); BRK carries
neither tag. The fundamentals builder never reads `dei` shares for these names and never treats
a missing value as zero; where shares outstanding come from for a BRK-style issuer is an
owner decision (`DECISIONS.md`).

Rows are built per accession of form `10-K`, `10-Q`, `10-K/A`, `10-Q/A`
(and `10-KT`, treated as `10-K`; verify), from submissions and facts:

1. **Primary period.** `period_end` = the filing's `reportDate`.
2. **Instants** (balance sheet: `total_assets`, `total_liabilities`,
   `assets_current`, `liabilities_current`, `long_term_debt`,
   `retained_earnings`): the fact with `accn` = this accession and
   `end` = `period_end`.
3. **Flows** (`revenues`, `net_income`, `gross_profit`, `ebit`,
   `operating_cash_flow`): facts with `accn` = this accession and
   `end` = `period_end`, chosen by duration `d = end - start`:
   - 10-K: `d` between 350 and 380 days -> the fiscal-year value;
     `fiscal_period = FY`.
   - 10-Q: the discrete quarter (`d` between 80 and 100 days) if reported;
     otherwise derive it as YTD minus the prior YTD (below).
4. **YTD to discrete quarter.** In a Q2 10-Q the six-month figure is
   `YTD2`, in Q3 the nine-month figure is `YTD3`. Cash-flow items are
   reported only as YTD. Then
   `Q2 = YTD2 - Q1`, `Q3 = YTD3 - YTD2`, and the Q4 that has no filing of
   its own: **`Q4 = FY - YTD3`** (equivalently `FY - Q1 - Q2 - Q3`). The
   earlier-period figures are taken as known at this filing's `filing_date`
   (the latest fact for that period with `filed <= filing_date`). Derived
   discrete quarters are internal; the `fundamentals` row for a 10-K holds
   the fiscal year, not Q4.
5. **TTM** (`revenue_ttm`, `net_income_ttm`, `operating_cash_flow_ttm`,
   `gross_profit_ttm`, `ebit_ttm`): for a 10-K row, the fiscal-year value;
   for a 10-Q row, the sum of the four most recent discrete quarters ending
   at `period_end` (`FY_prev + YTD_cur - YTD_prevyear`, the same number).
   Requires four consecutive quarters (period ends spaced three months, give
   or take 20 days); otherwise null. Component values are "known at this
   `filing_date`" (restated values filed before this filing replace the
   original, later restatements do not exist yet). This is the one place
   period arithmetic on `period_end` is allowed (below).
6. **Prior-period columns** (`prior_roa`, `prior_debt_ratio`,
   `prior_current_ratio`, `prior_gross_margin`, `prior_asset_turnover`,
   `prior_shares_outstanding`): computed from the row of the same
   `fiscal_period` one fiscal year earlier (the latest such row with
   `filing_date <=` this row's `filing_date`). Piotroski's year-over-year
   tests need them, and pairing "the same period a year ago" needs
   `period_end`, hence ingestion computes them
   (`07-fundamental-overlay` reads the columns).
7. **Shares outstanding.** `dei:EntityCommonStockSharesOutstanding` (cover
   page; `end` is the cover date, set into `shares_asof_date`, on or before
   `filing_date`), not a `us-gaap` tag (verify the tag). If the cover reports
   several classes, the per-class values are summed; the multi-class
   treatment of market cap is OPEN (`FEATURES.md` §3). Fallback when absent:
   `us-gaap:CommonStockSharesOutstanding` at `period_end`, recorded in
   `tag_used`.
8. **Restatements.** Never overwrite. A row is what its accession reported
   for its own period. An amendment (`10-K/A`, `10-Q/A`) is a separate row
   with its own `filing_date` and `amends_accession`. A restated
   comparative inside a later filing does not create a row; it enters only
   the later TTM and prior-period computations. The earliest `filed` per
   fact is the as-first-reported value and is the one in the original row.

**Tag candidates** (first non-null wins; the winning tag is recorded in
`tag_used`):

| Field | Candidate tags, in order |
|---|---|
| `revenues` | `Revenues`, `RevenueFromContractWithCustomerExcludingAssessedTax`, `SalesRevenueNet`, `RevenueFromContractWithCustomerIncludingAssessedTax`, `SalesRevenueGoodsNet`, `SalesRevenueServicesNet` |
| `net_income` | `NetIncomeLoss`, `ProfitLoss` |
| `total_assets` | `Assets` |
| `total_liabilities` | `Liabilities` (if absent: `LiabilitiesAndStockholdersEquity - StockholdersEquity`, recorded) |
| `operating_cash_flow` | `NetCashProvidedByUsedInOperatingActivities` |
| `gross_profit` | `GrossProfit` (if absent: `revenues - CostOfRevenue`, recorded) |
| `assets_current` | `AssetsCurrent` |
| `liabilities_current` | `LiabilitiesCurrent` |
| `long_term_debt` | `LongTermDebtNoncurrent`, `LongTermDebt` |
| `retained_earnings` | `RetainedEarningsAccumulatedDeficit` |
| `ebit` | `OperatingIncomeLoss` |
| `shares_outstanding` | `dei:EntityCommonStockSharesOutstanding`, then `CommonStockSharesOutstanding` |

**Revenue tag precedence (verify).** `SalesRevenueNet` is the pre-2018
(pre-ASC 606) tag and `RevenueFromContractWithCustomerExcludingAssessedTax`
the post-adoption one; `Revenues` is the broadest and is used by many filers
in both eras. The order above prefers `Revenues`, then the two eras. Filers
can switch tag across the adoption date; a switch can create a level jump in
`revenue_ttm`. Gate G7 lists tickers whose revenue tag changes between
consecutive filings together with a jump above 30% for review
(`QUALITY-GATES.md`). The order is a judgement call; correct it from the
gate's findings, with a migration only if the schema changes (it does not).

**Pre-2009 limits.** XBRL financial data is available for periods ending
after 2009-06-15 for the largest filers, with the rest phased in through
2011 (verify). Before that there is no free structured source and no HTML
parsing is allowed, so `fundamentals` is empty for earlier periods and
`sp_ratio` is NaN. See `02-panel-construction/FEATURES.md` §3 for the first
fully featured panel date.

**Amended filings.** Store both original and amendment (above). The panel
uses the latest `filing_date <= t` row.

### EDGAR events and delistings (`filing_events`, `delistings`)

From the same `submissions` data (no extra requests beyond the bulk zip and
daily deltas), one `filing_events` row per (filing, 8-K item) for the
following, plus the event forms:

| Source | Event | Used for |
|---|---|---|
| 8-K Item 2.02 | Earnings results release | Past earnings dates (`event_type = earnings_release`); `acceptance_datetime` gives before-open vs after-close |
| 8-K Item 3.01 | Notice of delisting or failure to satisfy a listing rule; transfer of listing | Veto input "pending delisting notice" (a superset: it also covers transfers) |
| 8-K Item 4.01 | Changes in certifying accountant (dismissal **or** resignation) | Proxy for "auditor resignation": cannot separate the two without reading the text. OPEN (`07-fundamental-overlay/VETO-RULES.md`) |
| 8-K Item 4.02 | Non-reliance on previously issued financial statements | Veto input "material restatement" (the structured, filed signal) |
| 8-K Item 1.03 | Bankruptcy or receivership | Delisting reason `bankruptcy`; terminal value |
| 8-K Item 2.01 | Completion of acquisition or disposition of assets | Delisting reason `acquired` (filed by the acquirer or the target) |
| 8-K Item 5.01 | Changes in control of registrant | Delisting reason `acquired` |
| 8-K Item 2.04 | Triggering events that accelerate or increase a direct financial obligation | Closest structured proxy for "debt covenant breach disclosed"; not the same thing |
| Form 25 / 25-NSE | Notification of removal from listing | Delisting date estimate (effective about 10 days after filing; verify) |
| Form 15 (15-12B, 15-12G, 15-15D) | Termination of registration | Delisting reason `deregistered` or confirmation of `acquired` |
| NT 10-K, NT 10-Q | Late-filing notice | Warning signal |

The derivation of `delist_class` is in `SURVIVORSHIP.md` §3.

### Deferred inputs (no free point-in-time source)

These veto and conviction inputs have **no** free point-in-time source
here. They are deferred, not faked; the dependent triggers are inactive
until an owner decision supplies a source.

| Input | Why deferred |
|---|---|
| Going-concern qualification in the latest 10-K | Lives in the auditor's report text. Not in structured XBRL for most of the history (an XBRL element for going-concern doubt exists in recent years; coverage unverified). No HTML parsing |
| Debt covenant breach disclosed | Text of notes and 8-Ks. Item 2.04 is a partial proxy only |
| Auditor resignation vs dismissal | Item 4.01 has both; separating needs the text |
| Analyst estimate revisions (`revision_z`) | No free historical estimates. Yahoo analyst pages are snapshots (constitution §3) |
| WACC inputs (beta, cost of debt, risk premium) | No free point-in-time source for market-implied inputs; any value would be a modelled estimate |
| Forward earnings calendar | Estimates only, no point-in-time source |

---

## FRED / ALFRED

**Client:** direct HTTP to `https://api.stlouisfed.org/fred/`. No `fredapi`.

**Auth:** free API key, `FRED_API_KEY`.

**Series** (`config/macro_series.toml` holds this table; `min_lag_days` is
the floor on `known_from - obs_date`, `buffer` the fallback when ALFRED
cannot be trusted, both in `POINT-IN-TIME.md`):

| Series ID | Meaning | Native cadence |
|---|---|---|
| `DGS3MO` | 3-month Treasury constant maturity (the risk-free rate in the targets) | Daily |
| `DGS2` | 2-year Treasury | Daily |
| `DGS10` | 10-year Treasury | Daily |
| `BAMLH0A0HYM2` | ICE BofA US High Yield OAS | Daily |
| `BAMLC0A0CM` | ICE BofA US Corporate OAS | Daily |
| `CPIAUCSL` | CPI, all urban consumers, seasonally adjusted | Monthly |
| `FEDFUNDS` | Effective federal funds rate, monthly average | Monthly |
| `UNRATE` | Unemployment rate | Monthly |
| `VIXCLS` | CBOE VIX close | Daily |

**Caveat (confirmed, P6).** FRED shows only about three years of the ICE BofA series
(`BAMLH0A0HYM2`, `BAMLC0A0CM`) because of the data licence: both start on 2023-10-10 (794
observations to 2026-10-08, one vintage) whatever `observation_start` asks for. A 15-year
history of the credit spreads is not available from FRED, so those two series are
**deferred** for the backfill (the gate reports it; no panel feature uses them).

**Endpoint and vintages (P6):**

    GET /fred/series/observations?series_id={ID}&api_key={KEY}&file_type=json
        &observation_start=2004-01-01
        &realtime_start=1776-07-04&realtime_end=9999-12-31
        &limit=100000&offset={n}

With `realtime_start=1776-07-04` and `realtime_end=9999-12-31` and the
default `output_type=1` ("observations by real-time period"), each returned
observation carries `realtime_start`, `realtime_end`, `date`, `value`: one
row per (observation, vintage interval). That is the long form stored in
`macro` (`obs_date` = `date`). Other `output_type` values (2 = one column per
vintage date, wide; 3 = new and revised observations only; 4 = initial
release only) are not used. The API limits the rows per request to 100,000
(page with `offset`) and **the vintage dates per request to 2,000, for every
`output_type` and for `json` and `xml` alike** (P6: `DGS3MO`, 5,127 vintage dates since
2005-06-28, answers HTTP 400 "exceeds the maximum number of vintage dates allowed for this
file type (2000)"; `CPIAUCSL`, 669, fits). `fred/series/vintagedates` lists a series'
vintage dates (up to 10,000 per call) and drives the windowing below; the gate also uses it
to cross-check coverage. `value` is the string `.` for a missing observation -> null.

**Windowing and stitching (P6).** For each series: list the vintage dates, cut the list into
runs of at most 2,000, and request each run with `realtime_start` = its first date and
`realtime_end` = its last (`DGS3MO` takes three windows; a window of 2,000 vintages was 2,635
rows, 250 KB). A window clips each interval's `realtime_end` to its own last date, so adjacent
windows are stitched: an interval that ends at a window's last date joins the next window's
interval with the same `(date, value)`. Without that the table gets a false revision at every
window boundary. The one-request form above works only for a series with at most 2,000
vintage dates (`CPIAUCSL`).

**What `releases` does and does not give.** The `fred/release/dates` and
`fred/series/release` endpoints give the dates on which a **release** was
published (for example the CPI release), one list per release. They do not
give a release date per observation. The per-observation first-availability
date is ALFRED's `realtime_start` of the observation's first vintage. The
earlier claim that the `releases` endpoint gives the release date of each
observation was wrong.

**Quirk: late-captured history (confirmed, P6).** For observations that predate ALFRED's
capture of the series, the first vintage's `realtime_start` is the capture
date, not the publication date. `DGS3MO`'s first vintage is 2005-06-28, and its 388
observations dated 2004-01-01 to 2005-06-27 all carry that date; `CPIAUCSL` shows real
publication dates (January 2004 CPI: 2004-02-20). Ingestion detects this (first vintage
captured long after the period) and uses the conservative lag-rule buffer
instead (`known_from_basis = lag_rule`; `POINT-IN-TIME.md`).

**Rate limit:** `SKILL.md` §5. About 9 series x 1 to 3 windows plus one vintage-date call
per series: under 40 requests for the whole backfill.

---

## iShares

**Source:** the holdings CSV of each ETF product page. Universe ETFs: IWF
(iShares Russell 1000 Growth) and IWP (iShares Russell Mid-Cap Growth). IJT
is not ingested in this phase (`UNIVERSE.md` §7).

**URL pattern (verify):**

    https://www.ishares.com/us/products/{PRODUCT_ID}/{slug}/1467271812596.ajax
        ?fileType=csv&fileName={ETF}_holdings&dataType=fund&asOfDate={YYYYMMDD}

The `asOfDate` parameter selects a historical holdings date; without it the
file is the latest. **Both the parameter name and how far back it works are
unverified**; the probe requests month-ends back to 2005 and records the
earliest date that returns data (`BACKFILL.md` §2). The product ids and slugs
must be taken from the product page. The id that earlier text gave for IWF
(`239714`) is unverified and may be wrong.

**Parse rules (verify the layout):**
- The CSV has a header block before the table (fund name, "Fund Holdings as
  of", a date such as `Oct 10, 2026`). The date in that block is the
  `snapshot_date`. Never use the fetch date.
- Skip to the row that starts the column header (`Ticker,Name,Sector,Asset
  Class,Market Value,Weight (%),Notional Value,Shares/Quantity,Price,...`).
- Numbers can carry thousands separators and be quoted; strip them. A UTF-8
  byte-order mark and a trailing disclaimer block can occur.
- Keep only equity lines: drop cash, futures, money-market lines (`-` or a
  non-equity `Asset Class`); log the count.
- `Sector` is the file's classification on that date (verify it is GICS).
- Normalize every ticker with the shared rule (`UNIVERSE.md` §3); keep the
  original in `vendor_ticker`.

**Cadence.** Monthly at minimum (the last trading day of the month); the
backfill uses the same grid. Weekly or daily snapshots are an owner option
(`BACKFILL.md` §7). `available_date` = next trading day after
`snapshot_date`.

**Rate limit:** `SKILL.md` §5.

**If the history is shorter than the project needs,** two fallbacks exist and
neither is free of problems: the fund's SEC filings (N-PORT monthly
holdings reported quarterly with a lag, N-Q before that; CUSIP-keyed; verify
availability and format), or accepting a later first universe date.
`BACKFILL.md` §7 sets this as OPEN.

---

## Identifiers

`company_tickers.json` maps **current** tickers to CIKs; it has no history.
Historical (delisted, renamed) names are resolved through the submissions
`name`/`formerNames` and the name lookup file, with an owner-maintained
override table `config/identifier_overrides.csv`. Rules and the unresolved
share gate: `UNIVERSE.md` §4.

---

## Common Patterns

All ingestion follows these patterns. They are sketches (not normative; the
contract is in `SKILL.md` §4, `BACKFILL.md` and `13-parquet-lake/IO-CONTRACT.md`).

**Land, then parse (the raw body is kept before anything else):**

```rust
async fn land(src: Source, entity: &str, chunk: &str, body: Bytes, meta: FetchMeta)
    -> Result<LandedRaw, IngestError>
{
    let sha = sha256_hex(&body);
    let dir = raw_dir().join(src.as_str()).join(entity).join(chunk);
    let final_path = dir.join(format!("{}__{}.{}.zst", meta.fetched_at_compact(), &sha[..8], meta.ext));
    atomic_write(&final_path, &zstd_compress(&body)).await?;        // temp + fsync + rename
    atomic_write(&final_path.with_extension("meta.json"), &meta.to_json_redacted()).await?;
    manifest().mark_fetched(src, entity, chunk, &sha, &final_path, meta.fetched_at).await?;
    Ok(LandedRaw { sha256: sha, path: final_path })
}
```

**Atomic write (same directory, then rename):**

```rust
async fn atomic_write(target: &Path, bytes: &[u8]) -> Result<(), IngestError> {
    let tmp = target.with_file_name(format!(".{}.tmp.{}.{}", file_name(target), pid(), now_ns()));
    write_and_fsync(&tmp, bytes).await?;
    tokio::fs::rename(&tmp, target).await?;
    fsync_dir(target.parent().unwrap()).await
}
```

**Data quality log (keyed by entity and chunk):**

```rust
async fn log_quality(row: QualityRow) -> Result<(), IngestError> {
    // QualityRow { date, source, dataset, entity, chunk, run_id, attempts,
    //              check_id, tickers_expected, tickers_received, missing_pct,
    //              latency_min, status, notes, measured, threshold }
    quality_writer().write_partition_single(row).await   // lake writer, SCHEMAS.md #20
}
```
