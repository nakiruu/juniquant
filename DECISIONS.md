# Decisions (owner)

Answered by the account owner on 2026-10-10. Everything below is RESOLVED and applied
in the skills.

## A. Constitutional amendments

| # | Section | Status | Decision |
|---|---|---|---|
| A1 | §6a | RESOLVED (amendment 1.13) | (a) In the Standard tier a Market order may sell only a legacy fractional remainder (0 < qty < 1 share) of a position bought in an earlier tier, as part of a full exit. (b) In the Bootstrap tier any reduce-only exit of a held position may be a Market order. Both stay subject to the lock, the restricted list, the halt flag and the audit; neither permits a buy. |
| A2 | §9 | RESOLVED (amendment 1.7) | TCP outputs are the endpoints of a 95%-coverage interval, not 5%/95% quantiles. |
| A3 | §7 | RESOLVED (1.8) | Vetoed names never bought; held vetoed names sold on the first permitted day; triggers without a point-in-time source are inactive. |
| A4 | §4 | RESOLVED (1.9) | Read-only frontend; Julia sole writer of `gspqr_predictions`; no Python. |
| A5 | §3, §8 | RESOLVED (1.10, 1.11) | Vendor-snapshot wording; as-of predictions; audit journal is the system of record; companion docs linked. |
| B3 | §1 rule 6 | RESOLVED (1.12) | Add-only backfill of missed closures from daily bars of IWF, IWP and SPY. |

## B. Compliance (lock)

| # | Decision |
|---|---|
| B1 | Share classes of one issuer are one lock group (a GOOGL buy blocks a GOOG sell). |
| B2 | An order that ended with zero fills is not locked; the retrospective audit does not flag overlaps with it (INFO count only). |
| B3 | Missed closures are filled from daily bars (add-only), and the check runs once over the whole historical backfill. |
| B4 | Alerts stay on the dashboard (no other channel chosen). |
| B5 | Ticker reuse after a rename is allowed through dated holdings: overlapping holdings of one ticker string fail the map load; aliases must match `security_master.prior_tickers`; history resolves by placement date; within the lock lookback of a handover a record counts against both securities (09 §3.8). |

## C. Modeling (best practice where asked)

| # | Decision |
|---|---|
| C1 | CPCV: purge on both sides of each test block by the label horizon H, plus an embargo of max(21, ceil(0.01 T)) trading days after each block (about 38 for 15 years). AFML ch. 7. |
| C2 | First generation no earlier than 504 matured days AND only once it beats a naive empirical-quantile baseline on its validation window. |
| C3 | **Amended by D20** (the eligible names are the broad universe, not IWF/IWP). Panel keeps rows from the first date with at least 50 eligible names; training and calibration use only point-in-time eligible names ("train on what you would trade"). |
| C4 | A generation more than 10% worse than the previous one aborts, also in the backfill; the previous generation serves for at most 3 months. |

## D. Data

| # | Decision |
|---|---|
| D1 | Probe P7 (iShares history depth) runs first; the universe start follows its result; today's membership is never used for past dates. |
| D2 | Month-end snapshots, plus one after the June Russell reconstitution, plus extra snapshots after large holdings turnover (`config/quality_gates.toml`). |
| D3 | Survivorship report always produced; WARN above 2% missing member-days in any year; above 10% a model is not promoted without owner acknowledgement. |
| D4 | Multi-class shares: sum of cover-page shares (default kept). |
| D5 | Auditor trigger only on an actual resignation or declined re-election (8-K 4.01 text rule); covenant breach via 8-K 2.04 as a labelled proxy; going-concern, analyst revisions and WACC inputs inactive. |
| D6 | IJT off. |
| D7 | Prices from 2005-01-01, macro from 2004-01-01; historical closures 2001-09-11..14, 2004-06-11, 2007-01-02, 2012-10-29..30, 2018-12-05, 2025-01-09 seeded (verify). |
| D8 | Delisted names: later price if any; else performance delistings -30% (NYSE/AMEX) / -55% (Nasdaq) per Shumway; cash acquisitions at last price; other disappearances excluded and counted. |
| D9 | `config/fractional_eligible.toml` is seeded by the owner; until then Bootstrap trades nothing. |
| D10 | Unverified external facts are settled by probes P1-P8 and the live tests. |
| D11 | Altman Z is not applied to `Financials` or `Real Estate` (GICS `universe.sector` at `t`); when the sector is null, SIC 6000-6799 is excluded. Different sectors get different treatment (owner, 2026-10-10). |
| D12 | **Amended by D20 for training.** Data scope is wider than the trading universe. I2 ingests every common stock listed on NYSE, NYSE American and Nasdaq from 2005 (ADRs and other foreign filers, ETFs, closed-end funds, preferreds, warrants, rights and units excluded) so a later universe change is a re-run, not a second backfill. The trading and training universe stays IWF/IWP (constitution §6) until a backtest supports a constitutional amendment (owner, 2026-10-10). |
| D13 | **Amended by D15 for prices.** Free sources only, reaffirmed (owner, 2026-10-10). Consequence: prices for stocks delisted before the backfill are unavailable for the wider set as for IWF/IWP; a point-in-time list of all listed stocks exists only from the day the system starts archiving the Nasdaq Trader symbol directories, and earlier listing history is reconstructed from EDGAR (Form 25, submissions) and labelled as such. Any backtest on the wider set carries the survivorship caveat. |
| D14 | `regime_states` and `gspqr_predictions` are `append_only` in the registry: a re-run must reproduce the stored rows, and a row that differs is an error, never a silent overwrite (constitution §3). A deliberate recompute (a stale generation after a panel rebuild, corrected HMM inputs) rewrites the affected partitions with `Mode::Replace` (owner, 2026-10-11). |
| D15 | EODHD "EOD Historical Data, All World" (paid, about $20/month, personal plan) is the **primary price source**: daily OHLCV, splits and dividends, including delisted US tickers, which keep their history (owner, 2026-10-11). It replaces Yahoo, which becomes an optional secondary source behind the price-source interface (`01-data-ingestion/SURVIVORSHIP.md` §2). The plan has no fundamentals: EDGAR, FRED/ALFRED, iShares and the Nasdaq Trader directories stay the sources of everything else, and stay free. Consequence for D13: delisted names are no longer missing by construction; the survivorship gap is still measured, now as EODHD's coverage, not assumed. The key is `EODHD_API_KEY` in `.env`, never logged. |
| D16 | Credit spreads: the ICE BofA series (`BAMLH0A0HYM2`, `BAMLC0A0CM`) stay deferred (FRED serves them only from 2023-10-10, P6). If a credit-spread input is ever needed, use FRED `BAA10Y` (Moody's seasoned Baa yield minus the 10-year Treasury, daily from 1986-01-02) instead. Not fetched now: nothing uses spreads (owner, 2026-10-11). Probed that day: 5,941 observations from 2004, 2,979 vintage dates from 2014-01-27, so it needs two windows of at most 2,000 vintages and its pre-2014 observations are late-captured (`known_from_basis = lag_rule`). |
| D17 | History is keyed by the **US composite FIGI** (one code per security, never reused, unchanged by ticker or name changes) with the **CIK** as the issuer code; no internal identifier scheme (owner, 2026-10-11, option B: free sources). FIGIs come from OpenFIGI through the EODHD ISIN, or for delisted codes without one through a CUSIP from the SEC 13(f) lists matched by name; an unresolved code is kept raw, not compacted, and counted. The paid EODHD Fundamentals route is declined (owner, 2026-10-11): the remainder is judged by gate G5 against IWF/IWP membership and closed with overrides. Execution stays on the tradable ticker. The design is `docs/adr/0001-figi-security-key.md`. |
| D18 | The GSPQR feature set stays the 12 of `02-panel-construction` for the first run. `panel` also stores four candidate columns that the model does not read (`log_mcap`, `r_size`, `amihud_60d`, `r_amihud`); after the baseline, a pre-registered ablation compares exactly three configurations on the same CPCV paths (baseline, + `r_size`, + `r_size` + `r_amihud`; N = 3 for the Deflated Sharpe Ratio), and a candidate is adopted only by the rule in `FEATURES.md` §7a. Short-term reversal is not added: it is `mom_20d`. No automated feature selection (owner, 2026-10-11). |
| D19 | Pre-merger SPAC shells (blank-check companies) are not training data and not an identity gap: they are out of scope. A de-SPACed company is in scope, but its history starts at the business combination: shell bars before it never count toward features or `history_days` (owner, 2026-10-11). Shells are recognized by EDGAR SIC 6770 once W6 fills `security_master.sic` (by name, `ACQUISITION` / `BLANK CHECK`, until then); the combination date comes from the issuer's EDGAR name change (`formerNames`) or the 8-K Item 2.01 in `filing_events`. Rules in `01-data-ingestion/UNIVERSE.md` §5a. **Amended 2026-10-11 (owner): SPACs are banned outright**: the name rule runs before every J4a route, so a shell never gets a FIGI, even through its ISIN. |
| D20 | **Training on the broad universe; IWF/IWP is the live buy filter only** (owner, 2026-10-11). The panel, LightGBM training, calibration and backtests use every in-scope common stock of the D12 scope (NYSE, NYSE American, Nasdaq) that is point-in-time eligible from our own data (a FIGI, a price on `t`, the trailing liquidity floor and the history rule). IWF/IWP membership only restricts what may be bought: the current snapshot at trading time (constitution §6 unchanged). So no historical holdings are needed: `etf_holdings` is archived forward from the first snapshot (2026-10-09), and the SEC N-PORT / N-Q routes of `BACKFILL.md` §7 are dropped. A backtest restricted to the buy list is possible only from that date; earlier backtests run on the broad eligible universe. Rules: `01-data-ingestion/UNIVERSE.md` §2, §5; `02-panel-construction/SKILL.md`. |
| D21 | **Market-cap percentile floor** (owner, 2026-10-11). Eligibility at `t` also requires `mcap_pct >= 0.30`: the percentile rank of `market_cap[t]` (`02-panel-construction/FEATURES.md` §0: as-traded close x split-aligned shares from the latest filing with `filing_date <= t`) among all in-scope securities with a known market cap on `t`. A percentile, not a dollar amount, so the bar means the same in 2006 and 2026. **Missing data never counts as below the floor**: a security with no market cap on `t` (no CIK, or no share count filed yet; XBRL share counts start about 2009) has `mcap_pct` null and the rule does not apply, so the dollar-volume floor alone decides; otherwise the floor would drop mostly delisted names and keep survivors. Labels are unaffected: a row eligible at `t` keeps its forward return after the security falls below the floor (`TARGETS.md` §3). 0.30 is fixed before any fit and is not tuned on test data; changing it is a new decision. Rules in `01-data-ingestion/UNIVERSE.md` §5 and `SCHEMAS.md` #1 |
| D22 | **Delisting date estimate** (owner, 2026-10-12; answers probe P8). `est_last_trading_date` is the Form 25 filing date itself for D2-D4, not 10 days later: the 10 days are the legal effect, trading stops on the filing day. Over all delistings whose prices stop near the filing, the last bar is within +-5 days of the Form 25 date for 87% of D2 (6% with +10) and 55% of D4 (15%). For D1 (bankruptcy) the estimate is the Item 1.03 filing date, and `terminal_known` for a bankruptcy is "the series reaches within 30 days of the bankruptcy filing" (as `terminal_class = bankrupt` already is): a bankrupt stock stops near the 8-K or keeps trading over the counter, so no single date fits (about 90% of D1 series meet the 30-day rule, 29% a 10-trading-day point test; sample 161). Rules: `01-data-ingestion/SURVIVORSHIP.md` §3, §4 |
| D23 | **G7 index misses split by source** (owner, 2026-10-12). A 10-K/10-Q in the quarterly `form.idx` without a `fundamentals` row counts against the HARD 1% only when `companyfacts` has XBRL facts for that accession (an ingestion loss: 68 of 230,699, 0.03%, on 2026-10-12). One with no facts at the source (5,616, 2.43%: XBRL phase-in, missing exhibits, filers such as royalty trusts with no facts at all) is listed under `index_no_xbrl` as WARN; its cost to the model is measured by the staleness check, and whether such filers are eligible is a universe question, not a gate threshold. Neither the threshold nor a name list was changed. Rules: `01-data-ingestion/QUALITY-GATES.md` G7 |

Also decided: `evt_tail_bounds` is partitioned by year (about 14M rows over 15 years).

## E. Placeholders (tune on the walk-forward track, not by decision)

TCP `M_d`, `omega`, `n_min`; EVT `W_full`, `W_min`, `B_min`; execution gate, ladder and band
parameters; REST/streamer intervals. All are marked `[placeholder]` in the skills.
