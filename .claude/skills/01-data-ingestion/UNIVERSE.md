# Universe, Identifiers and Symbol Rules

Two universes, kept apart (DECISIONS D20):

- **The training universe** is every in-scope common stock of the D12 scope (NYSE,
  NYSE American, Nasdaq) that is point-in-time eligible from our own data (§5): it
  feeds the panel, LightGBM, calibration and backtests.
- **The buy universe** is the individual stocks held by IWF and IWP **today**
  (constitution §6): it only restricts what may be bought, at trading time.

This file fixes how both are defined **point in time**, how securities are identified,
and the single symbol-normalization rule that every component, including the Rust lock
and execution code, reuses.

## 1. Who Writes What (one writer per dataset)

| Dataset | Writer | What it is |
|---|---|---|
| `etf_holdings` #30 | Rust `ingest_holdings` | Raw membership snapshots by "as of" date |
| `security_master` #31 | Rust `ingest_identifiers` | Identifier master |
| `universe` #1 | **Julia universe builder** (`features/src/universe.jl`; `price_coverage` in `features/src/coverage.jl`) | Derived daily membership and eligibility |
| `price_coverage` #34 | Julia universe builder | Survivorship coverage per ticker (`SURVIVORSHIP.md`) |

The earlier text had both "Julia universe builder" (`SCHEMAS.md`) and "Rust
`ingest_universe`" (`SKILL.md`) writing `universe.parquet`. There is no
`ingest_universe`: the fetched part is `etf_holdings`, the derived part is
`universe`.

## 2. Membership Snapshots: the Buy Filter (D20)

IWF/IWP membership is a **buy filter**, not a training population. iShares serves only
the latest file (probe P7), so `etf_holdings` is archived forward from the first snapshot
(2026-10-09, `quant-ingest ishares snapshot`); there is no membership before that date
and none is reconstructed. At trading time the filter is the latest snapshot with
`available_date <= t`. A backtest restricted to the buy list can start only at the first
snapshot; any earlier backtest runs on the training universe.

`etf_holdings` has one row per (snapshot date, ETF, ticker) as the file
published it (`SCHEMAS.md` #30). The primary key includes `etf`: Russell
Mid-Cap Growth names are largely also held by the Russell 1000 Growth fund,
so a ticker is often in both and a key of `(snapshot_date, ticker)` would
collide.

- A snapshot is usable from `available_date`, the next trading day after its
  "as of" date.
- **Membership on date `t`** is read from the latest snapshot of each ETF
  with `available_date <= t`; `in_iwf`, `in_iwp` are set independently;
  `snapshot_date` and `snapshot_age_days` record which snapshot and how old.
  A member is anyone in the union.
- Between snapshots membership is carried forward. A stock acquired between
  two month-end snapshots stays a "member" until the next one; its missing
  prices then show up in the coverage report with the EDGAR delisting reason.
  Weekly snapshots shrink this window (`BACKFILL.md` §7).
- Snapshots older than 45 days at `t` are a gate failure (G5).

Using the **current** holdings for past dates is never allowed (selection of
the winners), for training and for backtests alike; see `BACKFILL.md` §7. Using them
as today's buy filter is what they are for.

## 3. Canonical Symbol Normalization (the one shared definition)

A single function defines the canonical ticker. It lives in a small Rust
crate (`symbols/`, name proposed) that `ingest/`, `execution/` (lock,
orders, risk) and any other Rust code depend on. Julia does **not**
reimplement it: it reads canonical tickers from the lake. The test vectors
below live in `schemas/fixtures/symbol_normalization.csv`; both the crate's
tests and any other consumer's tests load that one file.

**Canonical form `CANON`** (what every lake dataset, order and config file
uses): upper-case ASCII, base of letters/digits, an optional share-class
letter after a dot. Grammar for tradable symbols:

    CANON  := [A-Z0-9]{1,6} ( "." [A-Z] )?

**Normalization `normalize(raw) -> CANON | error`**, in order:

1. Trim; reject empty and non-ASCII; upper-case.
2. Strip a leading `$`.
3. If the string is `BASE` + one separator from `. - / _ space` + **one
   letter**, the result is `BASE.LETTER` (`BRK-B`, `BRK/B`, `BRK B` ->
   `BRK.B`).
4. If the string is only `[A-Z0-9]{1,6}`, it is unchanged.
5. Anything else is an error (`UnparseableSymbol`): the row is skipped and
   counted. Preferred shares, warrants, rights, units and index symbols are
   outside the universe.
6. A vendor ticker without a separator for a class share (an `A` or `B`
   fused to the base, for example `BRKB`) cannot be derived by a rule. It is
   mapped through the `alias` rows of `config/identifier_overrides.csv`
   (`vendor_ticker -> canonical`); without an alias it is treated as a
   different symbol and the unresolved gate (G5) shows it.

**Vendor renderings** `render(CANON, vendor)`, the inverse direction:

| Vendor | `BRK.B` renders as | Status |
|---|---|---|
| EODHD (prices, D15) | `BRK-B`, sent as `BRK-B.US` | confirmed (P3); a historical holder of a reused ticker has its own `_old` code (`SOURCES.md` EODHD) |
| Yahoo (optional secondary) | `BRK-B` (dot -> hyphen) | verify |
| SEC `company_tickers` | `BRK-B` | verify |
| iShares | as published; input only | verify |
| Schwab | its own symbol format for class shares | verify in `10-execution-schwab` before use |

**Superseded securities (symbol reuse).** *Retired as a storage key by DECISIONS
D17*: history is keyed by the US composite FIGI, so a reused ticker needs no
distinct string (`docs/adr/0001-figi-security-key.md`). The rule below stays
only for `is_tradable`. If two different securities (two
CIKs) used the same canonical symbol at different times, the older one is
stored as `SYMBOL~CIK10` (for example `ABC~0000012345`) in the lake's
`ticker` columns, and the newer keeps the plain `SYMBOL`. The `~` form
violates the tradable grammar on purpose: **execution and lock code must
reject it** at order entry (it is a delisted security, never tradable), and
the shared crate offers `is_tradable(&str)` for that.

**Test vectors** (the fixture file is the authority; these must be in it):

| Input | Expected |
|---|---|
| `AAPL` | `AAPL` |
| `aapl ` | `AAPL` |
| `BRK-B` | `BRK.B` |
| `BRK/B` | `BRK.B` |
| `BRK.B` | `BRK.B` |
| `BF B` | `BF.B` |
| `$MSFT` | `MSFT` |
| `GOOGL` | `GOOGL` (no separator: unchanged) |
| `BRKB` | `BRKB` (unchanged; only an alias row can make it `BRK.B`) |
| `BAC-PL` | error (two letters after the separator: a preferred series) |
| `` (empty) | error |
| `ABCDEFG` | error (more than six characters) |
| `ABC~0000012345` | `is_tradable` false; `normalize` rejects it (it is produced only by the master) |

**Fixture format.** `schemas/fixtures/symbol_normalization.csv` has columns
`input,normalized,error,is_tradable_input,note`. `error` is one of `empty`,
`non_ascii`, `unparseable`, `superseded` (empty when `normalized` is set);
`is_tradable_input` is `is_tradable` applied to the raw input, so only
already-canonical inputs are true. Checks apply in this order: non-ASCII,
then empty (after trim and `$`), then `superseded` (any `~`), then the
grammar. `schemas/fixtures/symbol_render.csv` (`canonical,vendor,rendered,error,note`)
holds the `render` vectors; `error` is `unverified_vendor` (Schwab class
shares until verified), `input_only` (iShares) or `not_canonical`.

## 4. The Identifier Master (`security_master`)

One row per security (listed share class), keyed by the US composite FIGI
(DECISIONS D17; `SCHEMAS.md` #31). J4a resolves EODHD codes to FIGIs
(`SOURCES.md` Identifiers); the CIK rules below fill `cik` (W6).

**Resolution of CIK, in order (first success wins; the method is recorded in
`resolution`):**

1. `manual`: a row in `config/identifier_overrides.csv` (owner-maintained,
   committed; columns `ticker,cik,from_date,to_date,note`, plus `alias` rows
   as in §3). Always wins.
2. `exact`: the canonical ticker equals a ticker of the current
   `company_tickers` map, or of a submissions `tickers[]` entry.
3. `name`: the normalized holdings name (upper-case, punctuation and
   corporate suffixes such as INC, CORP, CO, LTD, PLC and share-class words
   removed) equals exactly one submissions `name` or `formerNames` entry whose
   date window contains the snapshot dates in which the ticker appears. More
   than one match is `unresolved`, never a guess.
4. `unresolved`: `cik` null. The row stays and can be eligible (the key is the FIGI,
   D20); only `sp_ratio` is NaN.

**What J4a implements today (2026-10-11).** Rule 2 for active securities only, from the
current `company_tickers_exchange.json` (a delisted symbol may now belong to another
issuer). Rule 3 for every other security, from EDGAR's `cik-lookup-data.txt` (current and
former names of every filer, no dates): the security's normalized name must equal the
names of exactly one CIK. `resolution` keeps the FIGI path; the name method is recorded as
`; cik:name` at the end of `resolution_note`. The date window of rule 3 is checked on the filer's whole filing history: the CIK is kept
only when the filings in its EDGAR submissions JSON (`data.sec.gov/submissions`, landed once
a month) overlap the security's price span, with a year of slack each side; an unrelated
filer with the same name filed in another era. No price span or no filings: null.
Rule 1 is not implemented yet. First run: securities without a CIK fell from 5,165 to 956.

**Renames.** The same CIK appears under ticker X before a date and ticker Y
after (a `formerNames` entry bridges them, or a manual row). It is one
security: the canonical ticker is the **latest** symbol, earlier ones go in
`prior_tickers`. All lake datasets use the canonical ticker for the whole
life of the security, and the price vendor is asked for the vendor symbol of
the canonical ticker (vendors generally keep the history under the new
symbol; verify with P2). The ticker string in a past holdings file is kept
in `etf_holdings.vendor_ticker`.

**Share classes.** Several tickers can share a CIK (for example two Alphabet
classes). `fundamentals` and `filing_events` carry one row per listed class
ticker. The multi-class market-cap treatment is OPEN (`FEATURES.md` §3).

**Symbol reuse.** §3 (`SYMBOL~CIK10`). The vendor series for a reused symbol
belongs to the newer holder; the coverage job guards against attaching it to
the older one (`SURVIVORSHIP.md` §4).

**Unresolved gate G5.** At most 1% of snapshot ticker-rows (0.5% by weight)
may be unresolved. Above that the build fails and the list is the work queue
for `identifier_overrides.csv`. This is expected to need manual work for
delisted names at first.

## 5. Eligibility, Derived Per Date From Trailing Metrics Only

`universe` has a row for every in-scope security (a FIGI in `security_master`) with a
price on that trading day, whether or not it is eligible or in IWF/IWP (`SCHEMAS.md` #1,
D20). `in_iwf` / `in_iwp` record the buy filter where a snapshot exists. Every metric at
date `t` uses data through `t` only:

| Field | Definition |
|---|---|
| `median_dollar_vol_60d` | Median over the 60 trading days ending `t` of `close x volume` (as traded). Null with fewer than 40 valid days |
| `history_days` | Count of valid price days on or before `t` (0 without prices). "Clean" means the row passed the per-row price checks (G2) |
| `in scope` | D12: not an ETF (`security_master.security_type` `ETP`, the reference codes SPY, IWF, IWP) and not a foreign private issuer as of `t` (the filer's latest annual report in `filing_events` filed by `t` is a 20-F or 40-F; a 10-K, or no annual report yet, is in scope). Point in time, so a filer that moves from 20-F to 10-K enters on the 10-K |
| `market_cap` | `close[t] x shares[t]` (`02-panel-construction/FEATURES.md` §0); null without a CIK or before the first filing with a share count, when the count is stated more than 455 days before `t` (a filer that stopped filing), and when `median_dollar_vol_60d` exceeds half the cap (turnover that high means a wrong count; D21 amended 2026-10-12) |
| `mcap_pct` | Percentile rank in [0, 1] of `market_cap` on `t` among in-scope securities with a non-null `market_cap` (ties take the average rank); null when `market_cap` is null, and on a date where fewer than 60% of the in-scope securities with a valid bar have a `market_cap` (D21 amended 2026-10-12: in 2009-2011 the early XBRL filers alone, mostly large caps, would set the bar) |
| `eligible` | In scope and `median_dollar_vol_60d >= 5,000,000` and `history_days >= 756` (3 years of 252 days; the outline's "fewer than 3 years of clean data"; from the business combination for a de-SPACed company, §5a) and `mcap_pct >= 0.30` when `mcap_pct` is not null (D21; a null never excludes) and a FIGI (every security in `prices` has one) and a price on `t`. A missing CIK does not exclude a security; its `sp_ratio` is NaN |
| `exclusion_reason` | Null if eligible, else the first of `scope`, `data_quality` (a failed-G2 row on `t`, or `t` inside a G2 span, `QUALITY-GATES.md` §2a), `history`, `liquidity`, `size`. A security without a FIGI or a price on `t` has no row (`SCHEMAS.md` #1) |

The earlier `data_years` (years of history available, computed over the whole
file) and the single current `eligible` flag looked into the future; they are
removed. The maximum-median-quoted-spread entry filter of
`10-execution-schwab/EXECUTION-WINDOW.md` §8 needs a quote history that does
not exist for the past and is **not** part of the historical universe.

The restricted list (`config/restricted.toml`, `ICE`) does not enter the
universe: it is applied at the tradable layer so cross-sectional ranks do not
change (constitution amendment 1.5).

The panel's eligible set on `t` is `eligible = true` in `universe` for `t`;
the vetoes come later (`07-fundamental-overlay`).

## 5a. SPACs (DECISIONS D19)

A SPAC (special purpose acquisition company, a blank-check company) trades near
its trust value until its business combination. Its bars carry no information
about an operating business, so:

- **A pre-merger shell is out of scope.** It is never eligible, its rows never
  reach training or calibration, and an unresolved shell is not counted as an
  identity gap (`SOURCES.md` Identifiers). FTSE Russell's rules exclude
  blank-check companies from the Russell indexes (verify against
  `etf_holdings`), so a shell should not appear among IWF/IWP members either.
- **A de-SPACed company is in scope from the combination date.** The listing
  often continues through the merger, so its price series starts with shell
  bars. `history_days` counts only valid days on or after the combination date,
  and features are computed only from bars on or after it
  (`02-panel-construction/FEATURES.md` §6).
- **Recognition.** An issuer whose EDGAR SIC code is 6770 ("blank checks") is a
  shell until its combination. The combination date is the effective date of the
  EDGAR name change that ends the shell name (`formerNames`), or the filing date
  of the 8-K with Item 2.01 (completion of acquisition) in `filing_events`,
  whichever is earlier. Until the EDGAR worker (W6) provides SIC and filings, a
  name rule **bans shells outright** (owner, 2026-10-11): `ACQUISITION` or `BLANK CHECK`
  in the normalized name (`ingest::identity::is_spac_name`). J4a applies it before every
  route (ISIN, 13(f) name, rename), so such a code gets no FIGI, hence no prices, no
  `universe` row and no training rows. A de-SPACed company listed under its operating
  name is unaffected; one that kept the shell word in its name (Navios Maritime
  Acquisition, NNA) is banned with the shells and needs an identifier override to come
  back.

## 6. Sector and CIK

- `sector` is the GICS sector in the latest snapshot usable at `t` (from
  `etf_holdings.sector`; verify the file uses GICS). It is what the 25%
  sector cap uses (constitution §6), and `sector_source = gics_snapshot`.
- **SIC fallback.** When the snapshot gives no sector (blank, `Cash and/or
  Derivatives`, or a label outside the 11 GICS sectors), `sector` is mapped
  from `security_master.sic` through `config/sic_to_gics.toml` (the narrowest
  range containing the code wins) and `sector_source = sic_fallback`. SIC
  comes from EDGAR submissions (`sic`, `sicDescription`, confirmed in the 2026-10-11 zip; written by
  `quant-ingest edgar sic`) and is the **current** value only: using it for past dates
  is a mild look-ahead, accepted because it fills gaps only, is flagged per
  row, and a company's industry rarely changes. SIC is per issuer, so every
  share class of a CIK gets the same code. Codes with no range (for example
  9995-9999, non-classifiable) leave `sector` null.
- Null sector on more than 1% of member rows is a G5 failure; `sic_fallback`
  on more than 5% of member rows in a year is a G5 warning (the snapshot
  source may have changed format).
- SIC is a classification, not a lake column per price row: it lives once per
  security in `security_master` and is joined by `ticker`.
- `cik` joins `universe` to `fundamentals` and `filing_events` (through
  `security_master`).

## 7. IJT

IJT (iShares S&P Small-Cap 600 Growth) appears in earlier text as a possible
expansion. In this phase it is **not ingested**: the config list is
`etfs = ["IWF", "IWP"]`, the `etf` column accepts `IJT` for later, and
`universe` has no `in_ijt`. Adding it later means a new column appended to
`universe` (migration) and about 600 more symbols to backfill (an extra half
minutes of EODHD calls, estimate); the $5M dollar-volume floor would remove
many of them. Owner decision, recorded as OPEN in the accompanying report.

## 8. Build Order (Julia, daily)

1. Read `etf_holdings`, `security_master`, `prices`, `splits`, `fundamentals`, the annual
   reports of `filing_events` and `delistings` (columns needed only).
2. One row per `(date, figi)` in `prices` for a FIGI in `security_master`.
3. Join the latest snapshot per ETF on `figi` (`as of` join on `available_date`; no
   snapshot before 2026-10-09: `in_iwf = in_iwp = false`, `snapshot_date` null).
4. Compute the scope (D12) as of each date, `history_days` (running count) and the 60-day
   median of dollar volume, then `market_cap` and its cross-sectional `mcap_pct` per date
   (D21, with its coverage rule).
5. Set `eligible` / `exclusion_reason` by the precedence above.
6. Write `universe` partitions (`replace` per year touched) through the lake
   writer; then derive and write `price_coverage` for every security with prices
   (`SURVIVORSHIP.md` §4).

## 9. Verification

- [ ] A test with a ticker in both ETFs yields one row with
      `in_iwf = in_iwp = true` and both weights
- [ ] A test asserts no field of `universe` at `t` changes when data after
      `t` is altered (the shuffle/truncation test)
- [ ] A security outside IWF/IWP with enough history and liquidity is `eligible` (D20)
- [ ] Rows before the first snapshot have `snapshot_date` null and `in_iwf = in_iwp = false`
- [ ] A row with null `mcap_pct` and enough history and liquidity is `eligible`; one with `mcap_pct < 0.30` is `size` (D21)
- [ ] An ETF, and a filer whose latest annual report is a 20-F or 40-F, is `scope`; a later 10-K brings it back (D12)
- [ ] On a date where fewer than 60% of the in-scope rows have a market cap, `mcap_pct` is null (D21)
- [ ] The symbol fixture file passes in the Rust crate
- [ ] No Julia code implements symbol normalization
- [ ] `universe` has no `effective_date`, `end_date`, `data_years`
