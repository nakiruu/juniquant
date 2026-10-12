# Survivorship Bias: What It Is Here, How It Is Measured, What It Cannot Fix

Prices come from EODHD (paid, DECISIONS D15), which keeps the history of
delisted tickers; filings come from EDGAR. Before D15 the price source was free
Yahoo, which drops most delisted names. EODHD narrows the gap but does not
close it by assumption: whatever it does not serve still biases the history
toward survivors. The bias is not hidden or argued away: it is **measured on every build, reported on every
training and evaluation artifact, and reduced where EDGAR allows**. This file
is the specification. The label rules that depend on it are normative in
`02-panel-construction/TARGETS.md` §3.

**Status (DECISIONS D20, 2026-10-11).** This method counts coverage against a membership
universe. It is kept unchanged because the universe is meant to grow, and any universe
with membership history (IWF/IWP from the first archived snapshot, 2026-10-09, or a later
addition) will need it. Training currently runs on the broad universe (`UNIVERSE.md`),
which has no membership history before that date, so this report applies only from the
first snapshot onward; the price gap of the broad universe is reported by the identity
job (`BACKFILL.md` J4a).

## 1. The Problem, Stated Plainly

- The universe is defined by historical IWF and IWP holdings (`UNIVERSE.md`),
  which include companies that were later acquired, went bankrupt or were
  delisted.
- EODHD keeps a delisted ticker's history (`TWTR.US` returns Twitter until
  2022-10, its documentation says), but its coverage of older and smaller
  delistings is not known, a **renamed** company's old code returns nothing
  (`FB.US`), and a **reused** ticker may return the new holder's history under
  the old holder's code (**verify** with probes P2/P3, `BACKFILL.md` §2). The
  first backfill records what it actually does for a fixed list of known
  delisted, renamed and reused names.
- If the missing names were a random sample, this would only cost data. They
  are not random: delisted names include bankruptcies (large losses) and
  acquisitions (usually a premium at the end). The effect on a long-only
  strategy's measured return is expected to be **upward** (failures missing),
  but its sign and size here are not known until the coverage report is run.
  No number is claimed in this file.

## 2. The `PriceSource` Interface

So that a better source can be added later without touching anything
downstream, every price source implements one trait (Rust, `ingest/`). A
source produces **normalized as-traded rows plus events**; the lake does not
know which source a row came from except through `prices.source`.

```rust
// Sketch (not normative, not compiled).
pub struct SourceCaps {
    pub id: &'static str,              // "eodhd" (primary, D15); "yahoo" if registered as secondary
    pub split_adjusted_ohlcv: bool,    // EODHD: false for prices, true for volume (probe P1)
    pub serves_delisted: Coverage,     // None | Partial | Full
    pub history_start_hint: Option<NaiveDate>,
    pub max_window_years: Option<u32>,
}

#[async_trait]
pub trait PriceSource {
    fn caps(&self) -> SourceCaps;
    /// Land the raw response for one symbol over [from, to]; `to` must be the fetch time.
    async fn fetch(&self, vendor_symbol: &str, from: NaiveDate) -> Result<RawResponse, FetchError>;
    /// Pure: raw bytes -> as-traded bars, splits, dividends, vendor metadata. No I/O, no clock.
    fn parse(&self, raw: &RawResponse, ctx: &ParseCtx) -> Result<Normalized, ParseError>;
}
```

Rules for adding a second source (for example Yahoo as a secondary source, or
a survivor-bias-free vendor):

1. It gets its own `id` and lands raw files under its own `source` directory.
2. `config/price_sources.toml` lists sources by priority per ticker and date
   range. `prices` holds **one row per `(figi, date)`**: the highest-priority
   source that has the bar. The losing source's bars stay only in the landing
   zone.
3. When two sources both have a bar, gate G10 (cross-source close
   disagreement) is active: relative difference of the as-traded close above
   0.5% on more than 0.1% of common days is a WARN, above 1% a FAIL.
   With a single source G10 is reported as "not applicable", never as passed.
4. Nothing downstream changes; the coverage report simply improves.

## 3. The `delistings` Dataset (from EDGAR)

EDGAR is the free source for **why and when** a company stopped trading. The
inputs are `filing_events` rows (`SOURCES.md` "EDGAR events"). One row per
delisted security (`SCHEMAS.md` #32), built by Rust at ingestion. Rules are
applied in this order; the first match sets `delist_class`, `confidence` and
`rule_id`. `E25` = the filing date of the (latest) Form 25 / 25-NSE for the
CIK. `est_last_trading_date` (D22) = `E25` itself for D2-D4 (the delisting takes legal
effect about 10 days after the Form 25, but trading stops on the filing day); the filing date
of the Item 1.03 used for D1, with or without a Form 25; the Form 15 date for D5.

Evidence for P8 (J6 first build, 2026-10-12, securities whose prices stop within 30 days of
the +10 estimate): the last EODHD bar minus the Form 25 filing date has median 0 days for D2
(1,885 securities, p10-p90 -3 to +3) and D4 (679, -11 to +12), and -7 days for D1 (134,
-55 to +33: trading is often suspended at the bankruptcy, months before the Form 25). Within +-5 days of the last bar: `E25` 87% of D2 (`E25` + 10: 6%), 55% of D4 (15%);
the owner chose `E25` (D22).

| Rule | Condition (all dates are `filing_date`) | `delist_class` | Confidence |
|---|---|---|---|
| D1 | 8-K Item 1.03 (bankruptcy or receivership) filed in [`E25` - 365 d, `E25` + 30 d], or, with no Form 25, any time in the 365 days before the last known price | `bankruptcy` | high with a Form 25, medium without |
| D2 | Form 25 present, and an 8-K Item 5.01 (change in control) or Item 2.01 (completion of an acquisition) by this CIK in [`E25` - 45 d, `E25` + 10 d] | `acquired` | high |
| D3 | Form 25 present, an Item 3.01 in [`E25` - 45 d, `E25`], and an Item 2.01 or 5.01 by this CIK within 90 days either side | `acquired` | medium |
| D4 | Form 25 present, none of the above | `delisted_other` (listing-standard failure, voluntary delisting, exchange transfer) | medium |
| D5 | Form 15 (15-12B, 15-12G, 15-15D) with no Form 25 | `deregistered` | low |
| none | No qualifying event | no row | |
| D0 | Applied last: the security's prices outlive the Form 25 (else the Form 15) at the same level of trade: more than 10 bars after it, the median dollar volume of bars 11-70 after it at least half that of the 60 bars to it, and no ten-fold jump without a split after it | the row is dropped | |

D0 (2026-10-12) catches a Form 25 for another security of the filer (a note, a preferred, a
tracking stock: Loews' Carolina Group in June 2008) or an exchange transfer, and an Item
2.01 the filer made as an acquirer or seller (CME 2008, Darden 2015). It dropped 1,414 of
5,258 rows (`acquired` 176, `bankruptcy` 18, `delisted_other` 1,100, `deregistered` 120). It
misses a security whose volume fell by half around the form for other reasons (Take-Two and
Leidos keep a false row; harmless while the series is live: their coverage stays `complete`)
and drops a few real ones whose stock traded on at volume (Merrill Lynch 2009: 158 bars of
another security follow). It needs a bar on or before the form; without one the row stays.
Rebuild alone with `quant-ingest edgar delistings`.

Limits, stated:

- Item 2.01 is filed by the **acquirer** as often as by the target; D2/D3
  look for the item under the target's CIK, which is incomplete. A missed
  acquisition falls to D4, which is conservative (it leads to a censored
  label, not a wrong terminal value).
- Whether an acquisition was for **cash or stock** is not in structured
  fields. This does not matter to the terminal value rule (the last price
  proxies the deal value in both cases), and `acquired` is not split.
- Form 25 is also filed for an **exchange transfer** (the security keeps
  trading under the same symbol) and for the filer's other securities. D0
  drops such a row at the source when the prices outlive it. Without prices
  this error cannot be seen: it is one reason the vendor's history is needed.
- The classification is a heuristic on filing metadata. Confidence is
  recorded, and the evidence accessions are stored for audit
  (`evidence_accessions`).

## 4. The Coverage Report

For **every ticker that appears in any historical IWF or IWP snapshot**
(`etf_holdings`), either prices exist from its first membership date through
its delisting date, or the gap is logged with a reason. The report is the
dataset `price_coverage` (`SCHEMAS.md` #34, written by the Julia universe
builder, one row per security: per FIGI, or per ticker for a member without one) plus the yearly table below; both are
regenerated by every universe build.

**Per ticker.** Compare the member interval [`first_member_date`,
`last_member_date`] (from `etf_holdings`, which includes members without prices or a FIGI)
with [`first_price_date`, `last_price_date`] and the delisting record:

| `coverage_status` | Meaning |
|---|---|
| `complete` | Prices on all member days (within a 2% internal-hole tolerance), through the last expected day: the data end for a live name, `est_last_trading_date` for a delisted one |
| `terminal_known` | The series ends within 10 trading days of the `est_last_trading_date` of an `acquired` row, or reaches within 30 days of the `bankruptcy_filing_date` of a `bankruptcy` row (D22: a bankrupt stock stops trading near the 8-K or keeps trading over the counter, so no single date fits; about 90% of D1 series meet the 30-day rule, 29% the 10-day one) |
| `gap` | Anything else; `gap_reason` is one of `vendor_no_data`, `vendor_wrong_holder` (the code answers with a different holder's series, `SOURCES.md` EODHD), `vendor_truncated` (series ends more than 10 trading days before the expected end), `late_start` (series starts after the first member day), `symbol_reused` (the vendor series starts after the security's first member day by more than 30 days and the security is old, or `firstTradeDate` contradicts the holdings), `identity_unresolved` (no CIK / vendor symbol), `internal_gaps` |

`vendor_wrong_holder` and `symbol_reused` come from the G2 spans (`QUALITY-GATES.md` G2): a member whose member days overlap such a span is a `gap` with that reason, checked right after `vendor_no_data`; the coverage counts no rejected bar as priced.

`terminal_class` (used by the labels): `none` (alive at the data end),
`bankrupt` (class `bankruptcy`, the series reaches the bankruptcy filing
within 30 days), `acquired` (class `acquired`, `terminal_known`), `unknown`
(the series ends and no rule applies).

**By year.** The SURVIVORSHIP CAVEAT table is the query in
`13-parquet-lake/QUERY-PATTERNS.md` ("Survivorship: missing share by year"):
member days, member days without a price, the share by count and by ETF
weight, split by `gap_reason`. The denominator is membership, which does not
depend on prices, so a name that vanished from the vendor is counted, not
silently dropped from the universe.

**Label counts.** The panel build adds, per year and horizon, the number of
labels by `label_flag` (`bankrupt`, `acquired`, `survivorship_gap`,
`immature`, `data_gap`; `02-panel-construction/TARGETS.md` §3), so the
number of censored labels is reported next to the missing-price share.

**Bias bound (an illustration, not an estimate).** For a year with weight
share `m` of members without prices and survivors' mean excess return `mu`,
the universe mean differs from `mu` by `m * (mu - r_missing)`. The report
prints this for `r_missing` of -100%, -50% and 0% (assumptions, labelled as
such) so a reader sees the order of magnitude of the worst plausible effect.

### The caveat artifact

The panel build writes `reports/survivorship_caveat.json`:

```json
{
  "caveat_version": 1,
  "generated_at": "2026-11-01T12:00:00Z",
  "data_end": "2026-10-30",
  "price_source": "eodhd",
  "other_price_sources": [],
  "gates_passed": ["G0", "G1", "..."],
  "years": [
    {"year": 2012, "member_days": 0, "missing_days": 0,
     "missing_pct_by_count": 0.0, "missing_pct_by_weight": 0.0,
     "by_reason": {"vendor_no_data": 0, "vendor_truncated": 0, "late_start": 0,
                   "symbol_reused": 0, "identity_unresolved": 0, "internal_gaps": 0}}
  ],
  "labels": {"63d": {"by_year": {"2012": {"bankrupt": 0, "acquired": 0,
             "survivorship_gap": 0, "immature": 0, "data_gap": 0}}}},
  "bias_bound": {"r_missing": [-1.0, -0.5, 0.0], "by_year": {}},
  "text": "SURVIVORSHIP CAVEAT: ..."
}
```

(The zeros are placeholders; the values come from the first run.)

**It is attached to every training and evaluation artifact.** A model
directory under `models/` contains a copy `SURVIVORSHIP_CAVEAT.json` and the
model registry entry records its SHA-256; a backtest or evaluation report
embeds the caveat's `text` and the yearly table and records the same hash; a
metric published without its caveat is invalid. The training script and the
backtest runner refuse to start if the file for the panel they read is
missing (`03-gspqr-model`, `14-backtest-protocol`; they implement the check).

## 5. Labels (summary; normative in TARGETS.md §3)

| Situation at origin `t`, horizon `H` | Label |
|---|---|
| Prices exist through `t + H` | Ordinary excess return |
| Series ends before `t + H`, class `bankruptcy` and the series reached the bankruptcy filing | -100% terminal: `y = -1 - rf`, flag `bankrupt` |
| Series ends before `t + H`, class `acquired` (`terminal_known`) | Return to the last price, proceeds earn the risk-free rate, flag `acquired` |
| Series ends before `t + H`, anything else | **Censored**: `y` NaN, flag `survivorship_gap`; excluded from training and calibration; counted in the report |
| `t + H` is after the data end | NaN, flag `immature` (not a survivorship issue) |

## 6. What the Data Cannot Fix

Said plainly, because the machinery above only reduces and measures the
problem:

1. **Delisted names that EODHD does not serve have no prices.** No rule
   creates history. Their member days are missing from `prices`, and
   everything trained on `prices` sees only the survivors of those names,
   except where EDGAR gives a terminal value for a name whose earlier prices
   EODHD does serve.
2. **Symbol reuse** makes some names unrecoverable and, if not caught,
   dangerous (a different company's prices attached to the old name). The
   guard (series-start vs first-member-day and EDGAR listing-date checks; Yahoo's
   `firstTradeDate` if Yahoo is a secondary source) catches
   the obvious cases, not necessarily all.
3. **Terminal values are assumptions.** Bankruptcy at -100% ignores
   post-delisting trading and recoveries; an acquisition at the last price
   ignores the deal spread and the consideration mix. Failures that EDGAR
   does not classify (`delisted_other`, `deregistered`, unmatched names) stay
   censored.
4. **Dropping censored labels biases training toward survivors.** That is
   the bias; the report quantifies how many labels are involved.
5. **The membership history itself** may be shorter than 15 years
   (`BACKFILL.md` §7), and sector and identifier history is as good as the
   iShares files and the name matching.
6. **Results are optimistic estimates** for a long-only strategy, by a margin
   this system reports but cannot eliminate. Treat out-of-sample numbers
   accordingly.

The only fix is a better data source (a point-in-time database with
delisting returns); the `PriceSource` interface (§2) is the seam. Whether to
buy one is an owner decision recorded as OPEN in the report accompanying this
change.

## 7. Verification

- [ ] Every ticker in `etf_holdings` has a `price_coverage` row
- [ ] `coverage_status = gap` always has a `gap_reason`
- [ ] The yearly table's denominator is membership, not eligibility (a test
      with a member that has no prices counts it as missing)
- [ ] A fixture with a known bankruptcy, a known cash acquisition and an
      exchange transfer classifies as `bankruptcy`, `acquired` and
      `ignored-transfer`
- [ ] The caveat file is present next to every model directory and every
      evaluation report, with a matching hash
- [ ] G10 reports "not applicable" while only one price source exists
