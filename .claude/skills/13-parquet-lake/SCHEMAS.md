# Schema Definitions

Every column, every type, every writer. This file is normative. If code
disagrees with this file, the code is wrong.

Types use Arrow notation. `date32` = days since epoch. `timestamp[ns, UTC]`
= nanosecond precision, UTC timezone. `utf8` = string. `bool` = boolean.

**Registry type strings.** The schema registry hashes DuckDB's rendering of
the column types (`13-parquet-lake/SKILL.md` §6). The mapping, confirmed
with DuckDB 1.5.6 against the Rust writer (the generator fails if `DESCRIBE`
disagrees):
`utf8` -> `VARCHAR`, `date32` -> `DATE`, `float64` -> `DOUBLE`,
`int64` -> `BIGINT`, `int32` -> `INTEGER`, `int16` -> `SMALLINT`,
`bool` -> `BOOLEAN`, `timestamp[ns, UTC]` -> `TIMESTAMP WITH TIME ZONE`.

**Layout.** Single file or hive directory, per the table in `SKILL.md` §3.
Headings below say "dataset" where the old text said "file".

**Row-count estimates** are for a 15+ year history (about 2026-10 minus 15 to
18 years) with about 600 eligible names and about 2,500 distinct tickers over
the whole period. They are estimates, not measurements, and are to be
replaced by the first backfill's real counts. Rows for #7-#10 and #11-#12 are
owned by their skills and are not revised here.

**Pre-first-write corrections.** #1-#5 and #20 were rewritten on 2026-10-10
before any file existed. They are recorded in `migrations/2026-10-10-initial.md`.
Columns marked "Appended" were added to the right of the earlier column list;
from the first write they are frozen like the rest.

---

## 1. `universe` (derived daily membership and eligibility)

Rows: ~25M (one row per in-scope security per trading day with a price, eligible or not; estimate).
Writer: Julia (universe builder). Update: daily, after prices, fundamentals and the identifier master.
Partitioned: `year`. Spec: `01-data-ingestion/UNIVERSE.md`.

One row per `(date, figi)` for **every** in-scope security (a FIGI in `security_master`)
with a `prices` row on `date`, whether or not it is eligible or held by IWF/IWP
(DECISIONS D20). This is the training population; `in_iwf` / `in_iwp` are the buy filter
and exist only from the first archived snapshot (2026-10-09). Securities without a FIGI
or without prices have no row; their gap is reported by the identity job (`data_quality`)
and the survivorship report (`01-data-ingestion/SURVIVORSHIP.md`). Every column at `date`
uses only information available at `date`: the latest holdings snapshot with
`available_date <= date`, and prices through `date`.

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `date` | `date32` | no | Trading day (NYSE calendar rules, not the dates present in `prices`) |
| `figi` | `utf8` | no | US composite FIGI (`security_master` #31, D17) |
| `ticker` | `utf8` | no | Canonical symbol on `date` (`UNIVERSE.md` §3), from `prices` |
| `cik` | `utf8` | yes | 10-digit zero-padded; null if unresolved |
| `sector` | `utf8` | yes | GICS sector from the snapshot used; needed for the 25% sector cap. When there is no snapshot or it has none, the SIC fallback (`UNIVERSE.md` §6) |
| `sector_source` | `utf8` | yes | `gics_snapshot` / `sic_fallback`; null when `sector` is null |
| `in_iwf` | `bool` | no | Member of IWF in the snapshot used; false when no snapshot is available at `date` |
| `in_iwp` | `bool` | no | Member of IWP in the snapshot used. A ticker can be in both |
| `snapshot_date` | `date32` | yes | `snapshot_date` of the `etf_holdings` snapshot used; null before the first snapshot |
| `snapshot_age_days` | `int32` | yes | `date - snapshot_date` in calendar days; null with `snapshot_date` |
| `weight_iwf` | `float64` | yes | Percent, from the snapshot; null if not a member |
| `weight_iwp` | `float64` | yes | Percent, from the snapshot; null if not a member |
| `median_dollar_vol_60d` | `float64` | yes | Median of `close x volume` (as traded) over the 60 trading days ending `date`; null with fewer than 40 valid days |
| `history_days` | `int32` | no | Count of valid price days on or before `date` |
| `market_cap` | `float64` | yes | USD, `close x` split-aligned shares from the latest filing with `filing_date <= date` (`02-panel-construction/FEATURES.md` §0); null without a CIK or a filed share count |
| `mcap_pct` | `float64` | yes | Percentile rank in [0, 1] of `market_cap` among rows of `date` with a non-null `market_cap`; null with `market_cap` (D21) |
| `eligible` | `bool` | no | `median_dollar_vol_60d >= 5,000,000` and `history_days >= 756` and `mcap_pct >= 0.30` when not null (D21; a null never excludes) and no `data_quality` FAIL for the security at `date`. IWF/IWP membership is not a condition (D20) |
| `exclusion_reason` | `utf8` | yes | Null if eligible, else the first failing of `data_quality` / `history` / `liquidity` / `size` |

Primary key: `(date, figi)`.

Removed from the earlier layout: `etf_source` (replaced by `in_iwf`/`in_iwp`;
the old single value could not represent a ticker held by both ETFs),
`effective_date`/`end_date` (spans are derived, `QUERY-PATTERNS.md`),
`data_years` (looked forward by construction; replaced by the trailing
`history_days`), the current-only `eligible` flag, and the `delisted` reason
(a delisting is a fact about the future of `date`; it never enters an
eligibility flag). `IJT` is not ingested in this phase (`UNIVERSE.md` §7).

---

## 2. `prices` (raw as-traded OHLCV)

Rows: ~8-10M (about 2,500 tickers, each from its first available day to its last; estimate). Writer: Rust (ingest). Update: daily. Partitioned: `year/month`.

**Raw, not adjusted.** `open`/`high`/`low`/`close` are the prices as they
traded on `date`, and `volume` is the shares traded on `date`, neither
adjusted for later splits nor dividends. EODHD returns raw OHLC and a
split-adjusted volume (confirmed by probe P1); ingestion undoes the volume
adjustment with the splits fetched in the same run, so a stored row does not
depend on when it was fetched (`01-data-ingestion/SOURCES.md` EODHD). Adjusted series
are computed in Julia from `splits` (#28) and `dividends` (#29)
(`02-panel-construction/FEATURES.md` §0). Nothing reads `adj_close_vendor`
except the reconciliation gate.

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `figi` | `utf8` | no | US composite FIGI of the security (`security_master`, DECISIONS D17); first column since the 2026-10-11 correction |
| `ticker` | `utf8` | no | Canonical form of the EODHD code the row was fetched under (`MON_old` -> `MON`); not a key |
| `date` | `date32` | no | NYSE trading day in the exchange time zone of the bar |
| `open` | `float64` | no | As traded, USD |
| `high` | `float64` | no | As traded |
| `low` | `float64` | no | As traded |
| `close` | `float64` | no | As traded |
| `volume` | `int64` | no | Shares traded on `date`, as traded (not split-adjusted) |
| `adj_close_vendor` | `float64` | yes | The vendor's own adjusted close as of `fetched_at`. Diagnostic only (reconciliation gate); never a feature, never a label input. Replaces the earlier `adj_close` |
| `source` | `utf8` | no | Appended. Price source id (`eodhd`, DECISIONS D15); a second source uses its own id (`yahoo`) |
| `fetched_at` | `timestamp[ns, UTC]` | no | Appended. When the raw response was fetched |
| `raw_sha256` | `utf8` | no | Appended. SHA-256 of the raw response body in the landing zone; links the row to its replayable source |

Primary key: `(figi, date)`. One source per row; with more than one price
source the highest-priority source wins and the loser is kept only in the raw
zone (`01-data-ingestion/SURVIVORSHIP.md` §2).

---

## 3. `fundamentals`

Rows: ~150K (about 2,500 companies x about 60 periodic filings over 15+ years, plus amendments and one copy per listed share class; estimate). Writer: Rust (ingest). Update: on filing. Partitioned: no.
Specification of every derivation below: `01-data-ingestion/SOURCES.md` (EDGAR) and `01-data-ingestion/POINT-IN-TIME.md`.

One row per `(ticker, accession)`: one periodic filing (10-K, 10-Q or an
amendment). A company with several listed share classes has one row per
class ticker (identical numbers). 8-K events are **not** here; they are in
`filing_events` (#33). **Rows are never updated.** A restatement is either a
new amendment row with its own `filing_date`, or a later filing's
comparative figures, which are used only inside later TTM and prior-period
computations. The value in a row is what that accession reported for its
own primary period, as first reported there.

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `ticker` | `utf8` | no | Canonical symbol, via `security_master` |
| `filing_date` | `date32` | no | **Point-in-time stamp.** EDGAR filing date of the accession |
| `period_end` | `date32` | no | Metadata only, never a decision variable. Ingestion is the only code that reads it (`POINT-IN-TIME.md` "period_end exception") |
| `form_type` | `utf8` | no | `10-Q` / `10-K` / `10-Q/A` / `10-K/A` (a transition report `10-KT` is treated as `10-K`; verify) |
| `revenues` | `float64` | yes | USD, primary period. 10-Q: the discrete fiscal quarter (derived as YTD minus prior YTD when only YTD is tagged); 10-K: the fiscal year. `duration_days` says which |
| `net_income` | `float64` | yes | USD, same rule |
| `total_assets` | `float64` | yes | USD, instant at period end |
| `total_liabilities` | `float64` | yes | USD, instant |
| `operating_cash_flow` | `float64` | yes | USD, same duration rule as `revenues` (cash-flow statements are cumulative YTD in 10-Qs; the discrete quarter is derived) |
| `gross_profit` | `float64` | yes | USD, same duration rule |
| `assets_current` | `float64` | yes | USD, instant |
| `liabilities_current` | `float64` | yes | USD, instant |
| `long_term_debt` | `float64` | yes | USD, instant |
| `retained_earnings` | `float64` | yes | USD, instant |
| `ebit` | `float64` | yes | USD, same duration rule as `revenues` |
| `shares_outstanding` | `float64` | yes | Count from the cover page (`dei:EntityCommonStockSharesOutstanding`), stated as of `shares_asof_date`; not `us-gaap` (verify). Not adjusted for later splits |
| `accession` | `utf8` | no | Appended. EDGAR accession number, `0000320193-24-000006` form. Primary-key part |
| `cik` | `utf8` | no | Appended. 10-digit zero-padded |
| `fiscal_year` | `int32` | yes | Appended. Fiscal year of the filing's own period, from the filer's DEI data (`fy`), checked against the period; null if inconsistent |
| `fiscal_period` | `utf8` | yes | Appended. `Q1` / `Q2` / `Q3` / `FY` (`fp`). There is no `Q4` row: a 10-K row is `FY`; the discrete Q4 is derived inside ingestion for TTM only |
| `duration_days` | `int32` | yes | Appended. Length of the primary period for flow items (about 91 for a quarter, about 364 for a year) |
| `acceptance_datetime` | `timestamp[ns, UTC]` | yes | Appended. EDGAR acceptance timestamp (`acceptanceDateTime` in submissions); null when unavailable |
| `amends_accession` | `utf8` | yes | Appended. For `/A` forms, the accession of the filing being amended; null otherwise |
| `tag_used` | `utf8` | yes | Appended. JSON object `{field: xbrl_tag}` recording which candidate tag supplied each non-null field (for example `{"revenues":"Revenues","shares_outstanding":"dei:EntityCommonStockSharesOutstanding"}`) |
| `shares_asof_date` | `date32` | yes | Appended. The date the cover-page share count is stated for (at or before `filing_date`). Used to align splits to the share count; not a decision date |
| `revenue_ttm` | `float64` | yes | Appended. Trailing four discrete fiscal quarters ending at this filing's period, using values known at `filing_date`; null unless four consecutive quarters are available |
| `net_income_ttm` | `float64` | yes | Appended. Same rule |
| `operating_cash_flow_ttm` | `float64` | yes | Appended. Same rule |
| `gross_profit_ttm` | `float64` | yes | Appended. Same rule |
| `ebit_ttm` | `float64` | yes | Appended. Same rule |
| `prior_roa` | `float64` | yes | Appended. `net_income_ttm / total_assets` of the same-fiscal-period filing one year earlier (latest such row with `filing_date <=` this row's `filing_date`) |
| `prior_debt_ratio` | `float64` | yes | Appended. `long_term_debt / total_assets`, same prior row |
| `prior_current_ratio` | `float64` | yes | Appended. `assets_current / liabilities_current`, same prior row |
| `prior_gross_margin` | `float64` | yes | Appended. `gross_profit_ttm / revenue_ttm`, same prior row |
| `prior_asset_turnover` | `float64` | yes | Appended. `revenue_ttm / total_assets`, same prior row |
| `prior_shares_outstanding` | `float64` | yes | Appended. `shares_outstanding` of the same prior row, as stored (not split-adjusted) |

Primary key: `(ticker, accession)`. The earlier key `(ticker, filing_date,
form_type)` could not distinguish two filings of one form on one day or an
amendment and its original filed the same day.

Pre-2009 coverage: structured XBRL data exists only for fiscal periods ending
after 2009-06-15 for the largest filers, with smaller filers phased in
through 2011 (verify). Before that the dataset has no rows, and `sp_ratio`
is NaN (`02-panel-construction/FEATURES.md` §3). This is a limit of free
structured data, not a bug.

---

## 4. `macro` (long form, one row per vintage of each observation)

Rows: ~50K (9 series; observations from 2004 on; one row per vintage of each observation, so revised series such as CPI have several rows per observation; estimate). Writer: Rust (ingest). Update: daily. Partitioned: no.

Source: ALFRED (FRED's archival vintage interface), `observations` with the
full real-time range (`01-data-ingestion/SOURCES.md` FRED). The earlier wide
table (one column per series, indexed by `date`) could not say when a value
was known and is replaced; the wide shape is a query (`QUERY-PATTERNS.md`
"Macro as of a date"), not a stored dataset.

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `series_id` | `utf8` | no | FRED series id, for example `DGS3MO`, `CPIAUCSL` |
| `obs_date` | `date32` | no | The period the value describes (start of period for monthly series). Metadata for the row, never the availability date |
| `realtime_start` | `date32` | no | First day this value was the published value (ALFRED vintage start) |
| `realtime_end` | `date32` | no | Last day this value was the published value; `9999-12-31` while still current |
| `value` | `float64` | yes | Null where FRED reports a missing observation (`.`) |
| `known_from` | `date32` | no | **Point-in-time stamp.** First date a decision may use this row: `max(realtime_start, obs_date + min_lag)` for a contemporaneously captured vintage; otherwise `obs_date` plus the conservative buffer for that series (`POINT-IN-TIME.md`). Computed in ingestion |
| `known_from_basis` | `utf8` | no | `alfred` (vintage dates trusted) / `lag_rule` (first vintage was captured long after publication, so the buffer table is used and revision history before capture is unavailable) |
| `fetched_at` | `timestamp[ns, UTC]` | no | When the raw response was fetched |

Primary key: `(series_id, obs_date, realtime_start)`.

Use (decision code): the row for `(series_id, obs_date)` with `known_from <= t`
and the greatest `realtime_start` is the value known at `t`; the
observation to use is the one with the greatest `obs_date` among those. For
the risk-free rate in the targets, `series_id = DGS3MO`
(`02-panel-construction/TARGETS.md` §1).

---

## 5. `panel`

Rows: ~20M (every in-scope security with a price from about 2008, D20; estimate). Writer: Julia (feature engine). Update: daily. Partitioned: `year/month`.

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `ticker` | `utf8` | no | |
| `date` | `date32` | no | |
| `split` | `utf8` | no | `train` / `purge` / `cal` / `test`. Global date cut; see `02-panel-construction/TARGETS.md` §4 |
| `y_fwd_63d` | `float64` | yes | Excess return with terminal-value rules (`02-panel-construction/TARGETS.md` §3); NaN only when `label_flag_63d` is `immature`, `survivorship_gap` or `data_gap` |
| `y_fwd_126d` | `float64` | yes | Same |
| `y_fwd_252d` | `float64` | yes | Same |
| `mom_20d` | `float64` | yes | |
| `mom_60d` | `float64` | yes | |
| `mom_12m_1m` | `float64` | yes | |
| `vol_60d_res` | `float64` | yes | |
| `sp_ratio` | `float64` | yes | |
| `turn_20d` | `float64` | yes | |
| `r_mom_20d` | `float64` | yes | Cross-sectional rank |
| `r_mom_60d` | `float64` | yes | |
| `r_mom_12m_1m` | `float64` | yes | |
| `r_vol_60d` | `float64` | yes | |
| `r_sp` | `float64` | yes | |
| `r_turn` | `float64` | yes | |
| `label_flag_63d` | `utf8` | yes | Appended. Null for an ordinary label. Else `bankrupt` / `acquired` (terminal-value rules applied, label is finite) or `immature` / `survivorship_gap` / `data_gap` (label is NaN). `TARGETS.md` §3 |
| `label_flag_126d` | `utf8` | yes | Appended. Same |
| `label_flag_252d` | `utf8` | yes | Appended. Same |
| `log_mcap` | `float64` | yes | Appended (2026-10-11, before first write; DECISIONS D18). **Candidate, not a feature**: `ln(market_cap[t])` (`02-panel-construction/FEATURES.md` §0, §7a); NaN when `market_cap` is NaN or not positive |
| `r_size` | `float64` | yes | Appended (D18). Candidate: cross-sectional rank of `market_cap` (`CROSS-SECTIONAL.md`); the stationary form a model would use |
| `amihud_60d` | `float64` | yes | Appended (D18). Candidate: mean over the last 60 bars of `abs(ret[s]) / dollar_vol[s]`, per USD million (`FEATURES.md` §7a); NaN below 40 valid days |
| `r_amihud` | `float64` | yes | Appended (D18). Candidate: cross-sectional rank of `amihud_60d` (higher = less liquid) |
| `figi` | `utf8` | no | Appended (2026-10-11, before first write; D20). US composite FIGI, the security key (D17); joins `universe` on `(date, figi)` |

Primary key: `(figi, date)`. Feature column order is frozen. The 18 earlier
columns are unchanged; the three flags, the four candidate columns (D18) and `figi` (D20) are appended (26 columns). Models read
the 12 features by name and ignore the flags, the candidates and the identifiers; the flags exist so calibration
and evaluation can exclude `survivorship_gap` rows and report their count.

---

## 6. `regime_states.parquet`

Rows: about 3.3K for a 15-year walk-forward backfill (one per date from about day 526), plus one per day. Writer: Julia (regime). Update: daily, **append only**: a row is never rewritten, and a refit applies only to later dates. Partitioned: no.

Every row is **causal**: the probabilities are filtered (forward-only) from an HMM fitted on data up to a month-end before `date` (`fit_date < date`), and `viterbi_state` is the online Viterbi state (the last element of the path decoded on data through `date`). Smoothed (forward-backward) probabilities are never stored (`06-regime-detection/HMM-PROCEDURE.md` §3).

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `date` | `date32` | no | |
| `regime_low_vol_prob` | `float64` | no | Filtered probability; sums to 1.0 with others |
| `regime_high_vol_prob` | `float64` | no | |
| `regime_crisis_prob` | `float64` | no | |
| `viterbi_state` | `utf8` | no | `low_vol` / `high_vol` / `crisis`; the raw online state before the 2-day confirmation |
| `regime_factor` | `float64` | no | 1.0 / 0.5 / 0.0, from the confirmed state |
| `fit_date` | `date32` | yes | Appended. Month-end of the HMM fit used for this row; always before `date`. Null before the migration |
| `confirmed_state` | `utf8` | yes | Appended. The state after the 2-day confirmation (`low_vol` / `high_vol` / `crisis`); `regime_factor` is its mapping |

Primary key: `date`.

---

## 7. `gspqr_predictions.parquet`

Rows: about 3.9M for a 15-year as-of backfill (450 tickers x 3 horizons x the days after each horizon's first generation, `03-gspqr-model/PREDICTION-BACKFILL.md` §7), plus about 1.4K per day thereafter. Writer: Julia (GSPQR), the **sole writer**: daily batch inference and the historical backfill; Rust only reads. Update: daily. Partitioned: `year/month`.

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `ticker` | `utf8` | no | |
| `date` | `date32` | no | Forecast origin |
| `horizon` | `utf8` | no | `63d` / `126d` / `252d` |
| `q_01` | `float64` | no | |
| `q_05` | `float64` | no | |
| `q_25` | `float64` | no | |
| `q_50` | `float64` | no | Median, used by optimizer |
| `q_75` | `float64` | no | |
| `q_95` | `float64` | no | |
| `q_99` | `float64` | no | |
| `crossed` | `bool` | yes | Appended. True if the raw model outputs were not non-decreasing in quantile order before rearrangement. Null for rows written before the migration |
| `generation` | `utf8` | yes | Appended. Id of the model generation that produced the row: the ISO cutoff date `m` of the walk-forward generation (trained on rows matured at `m` only). Each `(ticker, date, horizon)` comes from exactly one generation, so the primary key stays unique |
| `panel_version` | `utf8` | yes | Appended. The as-of panel the generation saw (its cutoff date; the hash of the slice is in the backfill manifest) |

The seven quantile columns hold the **monotone-rearranged** values
(sorted ascending per row), so `q_01 <= q_05 <= ... <= q_99` always holds.
TCP scores are computed on these stored values. Migration required
(`MIGRATIONS.md`): additive, nullable, not backfilled for `crossed`. `generation` and `panel_version` are written for every row of the as-of backfill. Rows for a month come from one generation per horizon; the `year/month` partition equals the generation's served dates `(m, m_next]`. `generation` is the cutoff date, never a training date later than the row's `date`.

Primary key: `(ticker, date, horizon)`.

---

## 8. `tcp_calibration.parquet`

Rows: about 3.5M for a 15-year as-of backfill (450 tickers x 3 horizons x the days from each horizon's first TCP origin, `03-gspqr-model/PREDICTION-BACKFILL.md` §8), plus about 1.4K per day thereafter. Writer: Julia (TCP). Update: daily. Partitioned: `year/month`. The Robbins-Monro columns are sequential state per `(ticker, horizon)`: a backfill is written in date order and resumes from the last row of each pair (`04-tcp-calibration/CALIBRATION.md` §5).

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `ticker` | `utf8` | no | |
| `forecast_date` | `date32` | no | |
| `horizon` | `utf8` | no | |
| `alpha` | `float64` | no | 0.05 nominal |
| `m_cal` | `int32` | no | Number of calibration dates in the window: `M_d = H` (63 / 126 / 252) for the pooled default, 60 for the per-ticker option |
| `k_order_stat` | `int32` | no | `ceil((n+1)(1-alpha))` with `n` the number of scores (see `n_cal_scores`; `n = m` per ticker) |
| `q_hat_lower` | `float64` | no | Lower endpoint of the calibrated interval, whose target coverage is 95% (`1 - alpha`). **Not** the 5% quantile: the raw `[q_05, q_95]` is a 90% base interval and TCP widens it toward 95% |
| `q_hat_upper` | `float64` | no | Upper endpoint of the same 95%-coverage interval. **Not** the 95% quantile |
| `rm_offset` | `float64` | no | Robbins-Monro offset `C_t` (state) |
| `calib_width` | `float64` | no | `q_hat_upper - q_hat_lower` |
| `rm_n` | `int32` | yes | Appended. Number of matured Robbins-Monro updates behind `rm_offset` (the `n` of `gamma_n = gamma_0 / sqrt(n + 1)`); persisted state, 0 before the first update. Null before the migration |
| `calib_method` | `utf8` | yes | Appended. `pooled_norm` (default) / `per_ticker` / `per_ticker_fallback_pooled` |
| `n_cal_scores` | `int32` | yes | Appended. Number of conformity scores behind the order statistic |

Primary key: `(ticker, forecast_date, horizon)`.

---

## 9. `evt_residuals.parquet`

Rows: about 3.5M for a 15-year as-of backfill (one per TCP row once matured). Writer: Julia (EVT). Update: daily. Partitioned: `year/month`.

A row for calibration date `date` is written only once it has matured
(`date + H <= today` on the global trading-calendar index, the same rule as TCP;
`realized_return` is the realized forward excess return), in date order. A
residual exists only from the horizon's first TCP origin. `calibrated_mid`
and `calibrated_halfwidth` are from the TCP interval **as issued on `date`**
(the half-width of the 95%-coverage interval, so `abs(std_residual) = 1` is
the interval edge).

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `ticker` | `utf8` | no | |
| `date` | `date32` | no | |
| `horizon` | `utf8` | no | |
| `realized_return` | `float64` | no | Forward return |
| `calibrated_mid` | `float64` | no | TCP interval midpoint at `date` |
| `residual` | `float64` | no | `realized - mid` |
| `exceed_lower` | `bool` | no | `std_residual < u_lower` of the pooled fit dated the maturity day `date + H` (diagnostic; the fit recomputes thresholds from `std_residual`); `false` when no fit existed that day |
| `exceed_upper` | `bool` | no | `std_residual > u_upper`, same convention |
| `calibrated_halfwidth` | `float64` | yes | Appended. `(q_hat_upper - q_hat_lower) / 2` at `date`; null before the pooled-EVT migration |
| `std_residual` | `float64` | yes | Appended. `residual / calibrated_halfwidth`; the quantity that is pooled across tickers |

Primary key: `(ticker, date, horizon)`.

---

## 10. `evt_tail_bounds.parquet`

Rows: up to about 600 eligible names x 3 horizons x 2 tails per fit date (about 3,600; about 2,700 at 450 names), so a daily 15-year fit history is about 14M rows (about 3,800 fit dates; fewer early on, when a horizon has no row, `05-evt-tails/EVT-PROCEDURE.md` §8; denormalized: the pooled parameters repeat per ticker). Writer: Julia (EVT). Update: daily. Partitioned: `year` (decided before first write, 2026-10-10: at about 14M rows a single file would be rewritten daily; layout `P(year)` in `SKILL.md` §3).

A `(fit_date, horizon, tail)` with no usable fit has **no row** (window shorter than one label block, or fewer than 5000 pooled residuals); a `data_quality` row records it and consumers use the TCP bounds (`05-evt-tails/EVT-PROCEDURE.md` §8).

The GPD is fitted **pooled across tickers, per `(fit_date, horizon, tail)`**
(`05-evt-tails/EVT-PROCEDURE.md`). `threshold_u`, `n_exceedances`,
`gpd_shape_xi`, `gpd_scale_sigma` and the appended pooled-fit columns are
therefore identical for all tickers of one `(fit_date, horizon, tail)`;
only `q_extreme` is per ticker.

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `ticker` | `utf8` | no | |
| `fit_date` | `date32` | no | |
| `horizon` | `utf8` | no | |
| `tail` | `utf8` | no | `lower` / `upper` |
| `threshold_u` | `float64` | no | Pooled threshold in **standardized** units (upper: 95th pct of `std_residual`; lower: 5th pct) |
| `n_exceedances` | `int32` | no | Number of distinct dates in the window with an exceedance. Informational only: at 450+ tickers it is about the window length and is **not** a gate (the gate is `n_episode_blocks`, `window_days` and the raw count) |
| `gpd_shape_xi` | `float64` | no | Pooled; NaN in the fallback |
| `gpd_scale_sigma` | `float64` | no | Pooled, standardized units; NaN in the fallback |
| `q_extreme` | `float64` | no | 1% or 99% bound in forward-return units: `mid_i + halfwidth_i * q_pooled`, never narrower than the TCP bound |
| `n_pooled` | `int32` | yes | Appended. Pooled sample size `n` |
| `n_exceed_raw` | `int32` | yes | Appended. Raw pooled exceedances `N_u` (used in the quantile formula; not declustered) |
| `q_pooled` | `float64` | yes | Appended. Pooled 99th (upper) / 1st (lower) percentile of `std_residual`; lower is negative |
| `fit_method` | `utf8` | yes | Appended. `gpd` / `pooled_empirical` |
| `n_episode_blocks` | `int32` | yes | Appended. Non-overlapping `H`-day blocks of the window containing at least one exceedance (the effective-sample criterion; must be ≥ 3 for `gpd`). Raw counts are not an effective sample size |
| `window_days` | `int32` | yes | Appended. Pooling window actually used, `W_avail`; below the horizon's `W_full` it is a partial window |

Primary key: `(ticker, fit_date, horizon, tail)`.

Both changes in this section and §9 are additive; the meaning of `n_exceedances`
and `threshold_u` changed from per-ticker to pooled; any rows written under the earlier
per-ticker procedure must be refit or discarded. They need a migration file and a registry regeneration
(`MIGRATIONS.md`).

---

## 11. `conviction_scores.parquet`

Rows: ~150K. Writer: Julia (conviction). Update: filing + daily. Partitioned: no.

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `ticker` | `utf8` | no | |
| `as_of_date` | `date32` | no | |
| `filing_date` | `date32` | no | Most recent filing used |
| `f_score_z` | `float64` | yes | Cross-sectional z |
| `roic_spread_z` | `float64` | yes | |
| `accruals_z` | `float64` | yes | Sign-flipped |
| `gm_trend_z` | `float64` | yes | |
| `rev_persist_z` | `float64` | yes | |
| `revision_z` | `float64` | yes | |
| `composite_z` | `float64` | yes | Weighted sum |
| `conviction` | `float64` | no | `clip(1.0 + 0.12*z, 0.50, 1.25)` |
| `components_used` | `int32` | no | Non-missing components |

Primary key: `(ticker, as_of_date)`.

---

## 12. `veto_flags.parquet`

Rows: ~650. Writer: Julia (conviction). Update: filing + daily. Partitioned: no.

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `ticker` | `utf8` | no | |
| `as_of_date` | `date32` | no | |
| `filing_date` | `date32` | yes | Null if no filing yet |
| `altman_z` | `float64` | yes | |
| `going_concern` | `bool` | no | |
| `neg_book_neg_ocf` | `bool` | no | Both conditions |
| `delisting_notice` | `bool` | no | |
| `restatement_12m` | `bool` | no | |
| `auditor_resigned` | `bool` | no | |
| `covenant_breach` | `bool` | no | |
| `veto_active` | `bool` | no | OR of all triggers + `altman_z < 1.8` |

Primary key: `(ticker, as_of_date)`.

---

## 13. `portfolio_targets.parquet`

Rows: ~650 per rebalance. Writer: Julia (portfolio). Update: event-driven. Partitioned: no.

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `rebalance_date` | `date32` | no | |
| `ticker` | `utf8` | no | |
| `expected_return` | `float64` | no | GSPQR 50th percentile `q_50` over horizon `H` (63d by default); an excess return over the risk-free rate |
| `downside_risk` | `float64` | no | The stock's GSPQR 5th-percentile forecast `q_05` over `H`. Diagnostic only: **not** a semi-deviation and not an optimizer input. Column name kept for schema stability |
| `w_optimized` | `float64` | no | Pre-conviction |
| `conviction` | `float64` | no | From `conviction_scores` |
| `w_conviction` | `float64` | no | `w_optimized * conviction` |
| `w_final` | `float64` | no | Renormalized, capped; before the regime gate |
| `veto_active` | `bool` | no | |
| `cap_applied` | `bool` | no | 5% single-name hit |
| `regime_factor` | `float64` | no | Confirmed factor on `rebalance_date` (1.0 / 0.5 / 0.0) |
| `w_target` | `float64` | no | After the regime gate: `min(w_current, w_final) + regime_factor * max(w_final - w_current, 0)`. The order generator reads this, not `w_final` |
| `w_current` | `float64` | yes | Appended. The current weight used by the optimizer |
| `lock_status` | `utf8` | yes | Appended. `none` / `no_sell` / `no_buy` |
| `lock_first_permitted` | `date32` | yes | Appended. `D + 5` trading days, `D` = the blocking order's last fill date; null if `none` or the blocking order is still open |
| `lock_binding` | `bool` | yes | Appended. True if the lock-free diagnostic solve wanted a trade the lock forbade |
| `band_skipped` | `bool` | yes | Appended. True if the no-trade band replaced the weight by `w_current` |
| `cost_buy_bps` | `float64` | yes | Appended. Buy cost estimate used |
| `cost_sell_bps` | `float64` | yes | Appended. Sell cost estimate used |
| `opt_status` | `utf8` | yes | Appended. `optimal` / `infeasible_locks` / `no_positive_excess` / `solver_failed` |

Primary key: `(rebalance_date, ticker)`.

---

## 14. `position_locks.parquet`

**Derived view. NOT authoritative.** The 5-day lock is a pure function of
order history (constitution §1, amendment 1.4, `09-lock-state-machine`).
This file is a convenience snapshot for the dashboard, rebuilt by the Rust lock
monitor from the recomputed history. **No order path may read it to authorize
anything**, and `lock::authorize` ignores it; if it disagrees with the history,
the history wins. It may be deleted and regenerated at any time. The columns
are unchanged for schema stability (§5); their meaning is now as stated below.

Rows: ~650. Writer: Rust (lock monitor). Update: daily and on history refresh. Partitioned: no.

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `ticker` | `utf8` | no | Primary key |
| `state` | `utf8` | no | Derived display label only (`IDLE` / `LONG_LOCKED` / `LONG_UNLOCKED` / `FLAT_LOCKED` / `FLAT_UNLOCKED`); not a stored state machine |
| `last_buy_date` | `date32` | yes | `D` (ET placement date) of the latest counted buy |
| `last_sell_date` | `date32` | yes | `D` of the latest counted sell |
| `lock_expiry_date` | `date32` | yes | Display: the first permitted day for the blocked side (`D + 5 trading days`). The day itself is permitted |
| `shares_held` | `float64` | no | Fractional in Bootstrap/Transition |
| `pending_side` | `utf8` | yes | `Buy` / `Sell` / null |

Primary key: `ticker`.

---

## 15. `pending_orders.parquet`

Rows: ~50. Writer: Rust (order manager). Update: event-driven. Partitioned: no.

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `pending_id` | `utf8` | no | UUID |
| `ticker` | `utf8` | no | |
| `side` | `utf8` | no | `Buy` / `Sell` |
| `target_quantity` | `float64` | no | Fractional permitted |
| `earliest_execution` | `date32` | no | First permitted day (`D + 5 trading days`), recomputed from order history |
| `signal_snapshot` | `utf8` | no | JSON |
| `created_at` | `timestamp[ns, UTC]` | no | |
| `status` | `utf8` | no | `Pending` / `Fired` / `Cancelled` / `Expired` |
| `cancelled_reason` | `utf8` | yes | `signal_reversed` / `signal_weakened` / `veto_triggered` / `regime_crisis` / `restricted` / `nothing_to_sell` / `lock_extended` / null (free-text enum, no schema change) |

Primary key: `pending_id`.

---

## 16. `orders.parquet`

Rows: ~5K/year. Writer: Rust (execution). Update: event-driven. Partitioned: no.

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `order_id` | `utf8` | no | UUID |
| `schwab_order_id` | `utf8` | yes | Null until accepted |
| `ticker` | `utf8` | no | |
| `side` | `utf8` | no | `Buy` / `Sell` |
| `quantity` | `float64` | no | Fractional permitted |
| `order_type` | `utf8` | no | v1 writes only `Limit` / `Market`. `Market` only in Bootstrap, or a Transition fractional remainder. `StopLimit` / `TrailingStop` / `OCO` / `LOC` are reserved (disabled until a lock analysis exists); `Bracket` / `OTO` are never valid |
| `limit_price` | `float64` | yes | Required for Limit |
| `status` | `utf8` | no | `Pending` / `Filled` / `Partial` / `Cancelled` / `Rejected` |
| `submitted_at` | `timestamp[ns, UTC]` | no | |
| `filled_at` | `timestamp[ns, UTC]` | yes | |
| `fill_price` | `float64` | yes | VWAP across fills |
| `fill_quantity` | `float64` | no | Cumulative; fractional permitted |
| `slippage_bps` | `float64` | yes | vs mid at submission |
| `tier` | `utf8` | no | `Bootstrap` / `Transition` / `Standard`, as of submission (appended column) |
| `arrival_mid` | `float64` | yes | Appended. Mid at the quote read that passed the spread gate; shared by all legs of a sequence |
| `spread_bps_at_submit` | `float64` | yes | Appended. Quoted spread at this order's own submission |
| `window_arm` | `utf8` | yes | Appended. `A` / `B` / `C`, or null outside the experiment |
| `ladder_arm` | `utf8` | yes | Appended. `cross` / `post` / `market` |
| `time_to_first_fill_s` | `float64` | yes | Appended. From the sequence's first submission |
| `time_to_complete_s` | `float64` | yes | Appended. Null if not completed |
| `n_reprices` | `int32` | yes | Appended. Cancel/replace count in the sequence |
| `fill_rate` | `float64` | yes | Appended. Filled / intended quantity, sequence level |
| `slippage_vs_arrival_bps` | `float64` | yes | Appended. This order's fills vs the sequence's `arrival_mid`; the benchmark for laddered orders (`slippage_bps` is vs the order's own submission mid) |
| `slippage_vs_vwap_bps` | `float64` | yes | Appended |
| `deferral_count` | `int32` | yes | Appended. Spread-gate deferrals before this order |
| `gate_reason` | `utf8` | yes | Appended. `ok` / `no_history` / `bad_quote` / `spread` / `stale_move` / `deadline_gate` / `deadline_not_due` / `special_session:<tag>` |
| `sequence_id` | `utf8` | yes | Appended. Groups the legs of one ladder |
| `markout_5m_bps` | `float64` | yes | Appended. Optional |

Primary key: `order_id`.

---

## 17. `positions.parquet`

Rows: ~650. Writer: Rust (execution). Update: daily. Partitioned: no.

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `ticker` | `utf8` | no | Primary key |
| `shares` | `float64` | no | Fractional permitted |
| `avg_cost` | `float64` | no | |
| `current_price` | `float64` | no | |
| `market_value` | `float64` | no | |
| `unrealized_pnl` | `float64` | no | |
| `weight` | `float64` | no | `market_value / portfolio_value` |
| `entry_date` | `date32` | yes | Null if `shares == 0` |
| `holding_days` | `int32` | no | Trading days since entry |

Primary key: `ticker`.

---

## 18. `fills.parquet`

Rows: ~8K/year. Writer: Rust (execution). Update: event-driven. Partitioned: no.

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `fill_id` | `utf8` | no | UUID |
| `order_id` | `utf8` | no | FK to `orders` |
| `ticker` | `utf8` | no | |
| `side` | `utf8` | no | |
| `quantity` | `float64` | no | Fractional permitted |
| `price` | `float64` | no | |
| `filled_at` | `timestamp[ns, UTC]` | no | |
| `venue` | `utf8` | yes | Exchange |
| `commission` | `float64` | no | |

Primary key: `fill_id`.

---

## 19. `evaluation_metrics.parquet`

Rows: about 250/year for the walk-forward track, plus per-path rows for each CPCV run (36 paths x 3 horizons x spread-source rows per run). Writer: Julia (backtest). Update: daily (walk-forward) / per run (CPCV). Partitioned: no.

Two tracks write here (`14-backtest-protocol/SKILL.md` §2a). Walk-forward rows (`track = walk_forward`, `path_id` and `fold` null) carry the interval and tail metrics of the calibrated pipeline. CPCV rows (`track = cpcv`) carry one row per `(run_id, path_id, horizon)` with `eval_date` the last date of the path, the raw-model metrics and the portfolio metrics. A metric a track does not compute is stored as NaN in the existing non-null columns and null in the appended columns.

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `eval_date` | `date32` | no | |
| `horizon` | `utf8` | no | |
| `coverage_95` | `float64` | no | Empirical coverage of the calibrated interval `[q_hat_lower, q_hat_upper]` (95% target). Walk-forward track; NaN in CPCV rows |
| `coverage_90` | `float64` | no | Empirical coverage of the raw `[q_05, q_95]` interval (90% nominal) |
| `avg_width` | `float64` | no | Mean interval width of the calibrated interval (walk-forward); NaN in CPCV rows |
| `winkler_score` | `float64` | no | Of the calibrated 95% interval, alpha = 0.05. Lower is better. Walk-forward; NaN in CPCV rows |
| `pinball_loss` | `float64` | no | Of the **raw** quantiles `q_tau`, averaged over the seven tau. Never of `q_hat_lower/upper` |
| `crps` | `float64` | no | Approximate, from the seven raw quantiles (`14-backtest-protocol/METRICS.md` §4) |
| `rank_ic` | `float64` | no | Spearman, cross-sectional |
| `n_obs` | `int32` | no | Universe size |
| `qhat_zero_frac` | `float64` | yes | Appended. Fraction of rows with the conformal `Q̂` exactly 0 (TCP inactive); `04-tcp-calibration` §4a. Null before the migration |
| `qhat_applied_nonpos_frac` | `float64` | yes | Appended. Fraction with applied `Q̂ + rm_offset <= 0` |
| `run_id` | `utf8` | yes | Appended. Backtest run id. Null for the legacy daily rows |
| `track` | `utf8` | yes | Appended. `walk_forward` / `cpcv` |
| `path_id` | `int32` | yes | Appended. CPCV path 1..36; null for walk-forward rows |
| `fold` | `int32` | yes | Appended. CPCV split id 1..120 when the row is per split (raw-model metrics); null for path-level and walk-forward rows |
| `spread_source` | `utf8` | yes | Appended. `all` / `measured_1m` / `proxy` for cost-dependent portfolio rows (`METRICS.md` §8); null otherwise |
| `sortino` | `float64` | yes | Appended. CPCV portfolio metric of the path |
| `sharpe` | `float64` | yes | Appended. As above |
| `max_drawdown` | `float64` | yes | Appended. As above, negative |
| `turnover_ann` | `float64` | yes | Appended. Annualized one-way turnover (`METRICS.md` §7b) |
| `lock_binding_freq` | `float64` | yes | Appended. LBF (`METRICS.md` §7c) |
| `shortfall_bps` | `float64` | yes | Appended. Implementation shortfall in bps of average NAV per year (`METRICS.md` §7a) |

Primary key: `(eval_date, horizon)` for the legacy and walk-forward rows; for run rows the logical key is `(run_id, track, path_id, fold, eval_date, horizon, spread_source)` with nulls comparing equal (`IO-CONTRACT.md` §3), so legacy rows with null run columns stay unique on `(eval_date, horizon)` (a uniqueness test, not a database constraint). Migration required (`MIGRATIONS.md`): additive, nullable.

---

## 20. `data_quality`

Rows: ~5K/year in steady state, plus about 5K from the backfill (one aggregate row per run and dataset, one row per failed or warned entity or chunk, one row per gate check; estimate). Writer: Rust (ingest). Update: per run. Partitioned: no.

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `date` | `date32` | no | Run date (ET) |
| `source` | `utf8` | no | `EODHD` / `Yahoo` / `SEC` / `FRED` / `Schwab` / `iShares` / `Gate` (rows written by the quality-gate run, `01-data-ingestion/QUALITY-GATES.md`) |
| `dataset` | `utf8` | no | |
| `tickers_expected` | `int32` | no | Entities expected in this row's scope (1 for a per-entity row) |
| `tickers_received` | `int32` | no | |
| `missing_pct` | `float64` | no | |
| `latency_min` | `int32` | no | From scheduled time |
| `status` | `utf8` | no | `OK` / `WARN` / `FAIL` |
| `notes` | `utf8` | yes | |
| `entity` | `utf8` | no | Appended. Ticker, CIK, series id or ETF the row is about; `*` for a run-level aggregate |
| `chunk` | `utf8` | no | Appended. Chunk id as in the ingestion manifest (`01-data-ingestion/BACKFILL.md` §4); `*` when not chunk-specific |
| `run_id` | `utf8` | no | Appended. Run identifier (UTC start time + mode) |
| `attempts` | `int32` | yes | Appended. Fetch attempts for the entity or chunk |
| `check_id` | `utf8` | no | Appended. Gate id (`G1` ... `G9`) for `source = Gate`; `*` for a fetch row |
| `measured` | `float64` | yes | Appended. The value a gate measured (a share, a count) |
| `threshold` | `float64` | yes | Appended. The threshold it was compared with |

Primary key: `(date, source, dataset, entity, chunk, check_id, run_id)`. The
earlier key `(date, source, dataset)` allowed one row per dataset per day and
so collided as soon as two tickers or two retries of one dataset were logged.

---

## 21. `audit_log` (dataset)

Rows: ~20K/year. Writer: Rust (event bus). Update: nightly export. Partitioned: `date=YYYY-MM-DD` (hive, one `data.parquet` per closed UTC day; leaf name per `13-parquet-lake/IO-CONTRACT.md` §2).

`audit_log` is a **partitioned dataset**: `lake/audit_log/date=YYYY-MM-DD/data.parquet` (registry key `audit_log`). The columns below are unchanged; `date` is a partition key, not a stored column. The **system of record is the SQLite journal** `audit.db` (`$QUANT_STATE/audit.db`, WAL, `synchronous=FULL`, trigger-enforced append-only), written only by the Rust audit writer. Parquet cannot be appended row by row, so a nightly export job (18:00 slot, before the backup) writes each closed day via temp path + atomic rename, after checking row count and a content hash against the journal. For **unexported days (including today)** the journal is the only copy; the Parquet dataset is complete only up to the last exported day. Journal-only columns (`seq`, `dedup_key`, `phase`, `day`) are not exported. Julia-originated events reach the journal through the spool directory; see `11-rust-event-bus` §5.

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `event_id` | `utf8` | no | UUID |
| `timestamp` | `timestamp[ns, UTC]` | no | |
| `event_type` | `utf8` | no | See enum below |
| `ticker` | `utf8` | yes | Null for non-ticker events |
| `actor` | `utf8` | no | `system` / `schwab` / `manual` |
| `details` | `utf8` | no | JSON |
| `severity` | `utf8` | no | `INFO` / `WARN` / `CRITICAL` |

Append-only. No primary key. Ordering by `timestamp` (the journal breaks ties by `seq`). The `details.phase` convention (`intent` / `outcome`) is described in `11-rust-event-bus` §6.

Event types: `order_submitted`, `order_filled`, `order_cancelled`,
`lock_transition` (derived), `lock_violation_blocked` (an order was denied by
`authorize`), `lock_violation_detected` (CRITICAL: the daily audit found a
violation or could not prove compliance), `lock_audit_completed`,
`restricted_security_blocked` (CRITICAL: an order for a ticker on
`config/restricted.toml`, or restricted activity/position found in a Schwab
account), `veto_triggered`, `regime_flip`, `position_reconciled`,
`manual_override`, `kill_switch_activated`, `model_retrain_aborted`,
`audit_spool_rejected`, `order_history_disagreement` (WARN: stream and REST, or aggregate and per-account, disagree), `calendar_sync_suspect_source` (WARN), `quote_recorder_gap` (WARN), `lock_audit_incomplete` (WARN/CRITICAL: could not verify, not a violation), `trading_halted`, `halt_cleared`, `calendar_override_added` (WARN), `unknown_enum_value` (WARN), `journal_order_not_in_broker` (WARN), `fill_without_order` (WARN/CRITICAL), `symbol_unmapped` (WARN), `broker_auth_expiring` (WARN), `reconciliation_incomplete` (WARN). `manual_override` never refers to the lock: no override
of the lock exists.

## 22. `portfolio_ledger.parquet`

Rows: ~252/year. Writer: Julia (ledger). Update: daily. Partitioned: `year`.

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `date` | `date32` | no | Primary key |
| `cash` | `float64` | no | USD |
| `cash_pct` | `float64` | no | |
| `market_value` | `float64` | no | |
| `total_value` | `float64` | no | `cash + market_value` |
| `cost_basis` | `float64` | no | |
| `unrealized_pnl` | `float64` | no | |
| `realized_pnl_today` | `float64` | no | |
| `realized_pnl_ytd` | `float64` | no | |
| `n_positions` | `int32` | no | |
| `daily_return` | `float64` | no | vs prior day |
| `cumulative_return` | `float64` | no | since inception |
| `max_drawdown` | `float64` | no | trailing 252d |
| `sortino_252d` | `float64` | no | trailing 252d |
| `regime_factor` | `float64` | no | |
| `reconciled` | `bool` | no | |
| `backfilled` | `bool` | no | false for live rows |

Primary key: `date`.

## 23. `tax_lots.parquet`

Rows: ~1000s total. Writer: Rust (execution). Update: event-driven. Partitioned: no.

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `lot_id` | `utf8` | no | Primary key |
| `ticker` | `utf8` | no | |
| `open_date` | `date32` | no | |
| `open_price` | `float64` | no | |
| `open_quantity` | `float64` | no | Reduced on partial close; fractional permitted |
| `close_date` | `date32` | yes | Null if open |
| `close_price` | `float64` | yes | |
| `close_quantity` | `float64` | no | 0 if open; fractional permitted |
| `realized_pnl` | `float64` | yes | Null if open |
| `holding_days` | `int32` | no | Calendar days |
| `order_id_open` | `utf8` | no | FK to orders |
| `order_id_close` | `utf8` | yes | FK to orders |

Primary key: `lot_id`.

## 24. `trade_summary.parquet`

Rows: ~500/year. Writer: Julia (ledger). Update: event-driven (on close). Partitioned: `year` (of `exit_date`).

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `trade_id` | `utf8` | no | Primary key |
| `ticker` | `utf8` | no | |
| `entry_date` | `date32` | no | Earliest open date |
| `exit_date` | `date32` | no | Latest close date |
| `entry_price` | `float64` | no | Weighted-average |
| `exit_price` | `float64` | no | Weighted-average |
| `quantity` | `float64` | no | Fractional permitted |
| `gross_return_bps` | `float64` | no | |
| `slippage_bps` | `float64` | no | Quantity-weighted |
| `net_return_bps` | `float64` | no | |
| `benchmark_return_bps` | `float64` | no | Equal-weighted universe |
| `alpha_bps` | `float64` | no | |
| `holding_days` | `int32` | no | Trading days |
| `return_per_day_bps` | `float64` | no | |
| `downside_deviation_bps` | `float64` | yes | Null if no down days |
| `trade_sortino` | `float64` | yes | Null if downside deviation is 0 |
| `regime_at_entry` | `utf8` | no | |
| `regime_at_exit` | `utf8` | no | |
| `conviction_at_entry` | `float64` | no | |
| `tier_at_entry` | `utf8` | no | |
| `lock_overridden` | `bool` | no | Reserved; constant `false`. No lock override exists (amendment 1.4); kept for schema stability until removed by migration |

Primary key: `trade_id`.

---

## 25. `order_history.parquet`

Rows: ~10K/year (a daily snapshot of the audited window). Writer: Rust (compliance audit). Update: daily (append). Partitioned: no.

**Evidence, not authority.** An append-only record of the order history that
the daily compliance audit scanned (`09-lock-state-machine` §8), so that the
owner can show what was checked. `lock::authorize` never reads it: it loads
the order history live from Schwab, the audit journal and `lock_external.csv`.
`lock_external.csv` itself is **not** in the lake: it is an owner-maintained
CSV at `$QUANT_STATE/lock_external.csv` (directly under `QUANT_STATE`, `IO-CONTRACT.md` §1)
(`account,ticker,side,placed_date,status,filled_qty`), read directly by Rust at
every check and hashed into each compliance report.

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `snapshot_date` | `date32` | no | Audit date (ET) |
| `fetched_at` | `timestamp[ns, UTC]` | no | |
| `source` | `utf8` | no | `Schwab` / `Journal` / `External` |
| `account` | `utf8` | no | Account hash or label (`FIDELITY`); never a raw account number |
| `schwab_order_id` | `utf8` | yes | Null for External |
| `ticker` | `utf8` | no | |
| `side` | `utf8` | no | `Buy` / `Sell` |
| `entered_time_utc` | `timestamp[ns, UTC]` | yes | Null for External (only `placed_date`) |
| `placed_date_et` | `date32` | no | ET placement date, before any roll-forward (for External rows: the file's `placed_date`, which for a late-filled order is its last fill date) |
| `d_effective` | `date32` | no | `D` after rolling to the next session (from the last fill date if the order filled, else the placement date) |
| `status` | `utf8` | no | Raw Schwab status (or the External file's text) |
| `quantity` | `float64` | yes | |
| `filled_qty` | `float64` | no | |
| `counted` | `bool` | no | The rule's verdict at snapshot time |
| `last_fill_date_et` | `date32` | yes | ET date of the order's last fill (`executionLegs`), null if never filled. Appended column |
| `first_permitted` | `date32` | yes | `block_end`: last fill date (or placement date) + 5 trading days; null while the order is open (no end date). Appended column |


Primary key: `(snapshot_date, source, account, schwab_order_id, ticker, placed_date_et, side)`. Never updated, never deleted.

---

## 26. `execution_costs.parquet`

Rows: ~600 names x tiers x arms per session, append-only by `date`. Writer: Rust (execution), after each session. Read by Julia at 17:00 (cost-aware optimizer, `08-portfolio-optimization` §5c). Update: daily. Partitioned: `year`.

Per-name execution cost estimates (`10-execution-schwab/EXECUTION-WINDOW.md` §5). Tiers and arms are never pooled.

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `date` | `date32` | no | Session date |
| `ticker` | `utf8` | no | |
| `tier` | `utf8` | no | `Bootstrap` / `Transition` / `Standard` |
| `ladder_arm` | `utf8` | no | `cross` / `post` / `market` |
| `spread_bucket` | `int32` | no | Quintile (1-5) of the universe's median quoted spread |
| `median_spread_bps` | `float64` | yes | |
| `ewma_slip_bps` | `float64` | yes | EWMA of realized slippage vs arrival mid |
| `n_fills` | `int32` | no | |
| `cost_buy_bps` | `float64` | no | The estimate the optimizer reads |
| `cost_sell_bps` | `float64` | no | |
| `cost_source` | `utf8` | no | `fills` / `prior` |
| `sigma_daily` | `float64` | yes | Daily volatility used by the stale-move rule |

Primary key: `(date, ticker, tier, ladder_arm)`. New dataset: register it in `schemas/registry.json`; no migration file (`MIGRATIONS.md`).

---

## 27. `quote_spread_1m.parquet`

Rows: ~600 names x 390 regular-session minutes = about 234,000 rows/day, about 59 million rows/year (**estimate**, one row per name per minute including carried rows). Writer: Rust (quote recorder, `schwab/quotes.rs`). Update: daily, written once after the close from an on-disk spool (Parquet cannot be appended row by row). Partitioned: `session=YYYY-MM-DD/data.parquet` (one leaf per ET session date, written once after the close; the key is `session` because the stored `date` column would collide with a `date=` directory key; `13-parquet-lake/IO-CONTRACT.md` §2).

Intraday quoted-spread history, built **forward only** (the Market Data API has no historical bid/ask; `pricehistory` is OHLCV only). Specification, subscription set, filters and failure behaviour: `10-execution-schwab/EXECUTION-WINDOW.md` §2.1. Read by the spread gate (time-of-day medians), the spread-bucket quintiles, the per-name cost model (`execution_costs`, #26) and the backtest spread source. Regular hours only (09:30-16:00 ET). Size estimate: ~0.6-1.2 GB/year at an assumed 10-20 bytes/row after ZSTD (an estimate, not measured).

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `date` | `date32` | no | ET session date |
| `minute_et` | `int16` | no | Minutes since midnight America/New_York, 570 (09:30) to 959 (15:59) |
| `ticker` | `utf8` | no | |
| `n_updates` | `int32` | no | Valid quote updates in the minute (stream) or 1 per REST poll; 0 for a carried row |
| `bid_last` | `float64` | yes | Last valid bid in the minute |
| `ask_last` | `float64` | yes | |
| `spread_bps_median` | `float64` | yes | Median of `(ask-bid)/mid*10000` over the minute's valid updates; null for a carried row or no valid update |
| `spread_bps_p90` | `float64` | yes | 90th percentile, same population |
| `bid_size_last` | `int64` | yes | **Shares** (stream lots x 100) |
| `ask_size_last` | `int64` | yes | **Shares** |
| `last_price` | `float64` | yes | |
| `volume_delta` | `int64` | yes | Change in cumulative total volume over the minute; null when unknown |
| `status_flags` | `int32` | no | Bit mask: 1 crossed/locked seen, 2 zero bid/ask or size seen, 4 delayed/NFL seen, 8 halted status seen, 16 non-`Normal` status seen, 32 `STALE_CARRY`, 64 stream gap inside the minute, 128 `SIZE_UNITS_UNVERIFIED` |
| `source` | `utf8` | no | `stream` / `rest` |

Primary key: `(date, minute_et, ticker)`. A minute with no row means the recorder was down for that name (a `quote_recorder_gap` audit row records the period). New dataset: register it in `schemas/registry.json`; no migration file (`MIGRATIONS.md`). Additive changes only.

---

## 28. `splits`

Rows: ~5K (estimate). Writer: Rust (ingest). Update: daily. Partitioned: no.

Split events from the price vendor's `events=split` (`01-data-ingestion/SOURCES.md`). Used to compute split-adjusted prices and to align the share count in `fundamentals` to a date (`02-panel-construction/FEATURES.md` §0).

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `figi` | `utf8` | no | US composite FIGI of the security (`security_master`, DECISIONS D17); first column since the 2026-10-11 correction |
| `ticker` | `utf8` | no | Canonical form of the EODHD code the row was fetched under (`MON_old` -> `MON`); not a key |
| `ex_date` | `date32` | no | First trading day at the new share count |
| `numerator` | `float64` | no | New shares (4 in a 4-for-1) |
| `denominator` | `float64` | no | Old shares (1 in a 4-for-1; 8 in a 1-for-8 reverse split) |
| `ratio` | `float64` | no | `numerator / denominator`: new shares per old share. A price before `ex_date` is divided by it |
| `source` | `utf8` | no | `eodhd` (or a secondary source id) |
| `fetched_at` | `timestamp[ns, UTC]` | no | |
| `raw_sha256` | `utf8` | no | Landing-zone body this row came from |

Primary key: `(figi, ex_date)`. New dataset: register it in `schemas/registry.json`; no migration file (`MIGRATIONS.md`).

---

## 29. `dividends`

Rows: ~100K (estimate). Writer: Rust (ingest). Update: daily. Partitioned: no.

Cash dividends from the vendor's `events=div`. Used for the dividend-adjusted (total-return) series in Julia.

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `figi` | `utf8` | no | US composite FIGI of the security (`security_master`, DECISIONS D17); first column since the 2026-10-11 correction |
| `ticker` | `utf8` | no | Canonical form of the EODHD code the row was fetched under (`MON_old` -> `MON`); not a key |
| `ex_date` | `date32` | no | Ex-dividend date (EODHD `date`, documented as the ex-date) |
| `amount` | `float64` | no | USD per share **as paid** (not split-adjusted): EODHD `unadjustedValue` |
| `currency` | `utf8` | yes | From the response metadata, normally `USD` |
| `source` | `utf8` | no | `eodhd` (or a secondary source id) |
| `fetched_at` | `timestamp[ns, UTC]` | no | |
| `raw_sha256` | `utf8` | no | |

Primary key: `(figi, ex_date)`. New dataset: register it in `schemas/registry.json`; no migration file (`MIGRATIONS.md`).

---

## 30. `etf_holdings`

Rows: ~130K at monthly snapshots (IWF about 400 and IWP about 270 holdings x about 190 months; estimate), up to about 3M at daily snapshots. Writer: Rust (ingest). Update: monthly at minimum, plus the backfill. Partitioned: `year` (of `snapshot_date`).

Historical constituents of the two ETFs by "as of" date, as published
(`01-data-ingestion/UNIVERSE.md`). Raw facts only: no eligibility here.
Cash, futures and other non-equity lines are dropped at parse time (their
count is logged).

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `snapshot_date` | `date32` | no | The "as of" date stated in the file header (not the fetch date) |
| `etf` | `utf8` | no | `IWF` / `IWP` (`IJT` reserved, not ingested) |
| `ticker` | `utf8` | no | Canonical symbol after normalization |
| `vendor_ticker` | `utf8` | no | The symbol exactly as in the file |
| `name` | `utf8` | no | |
| `sector` | `utf8` | yes | GICS sector as the file states it on that date (verify the file's classification) |
| `asset_class` | `utf8` | yes | |
| `weight` | `float64` | yes | Percent of the ETF |
| `shares` | `float64` | yes | Shares of the stock held by the ETF |
| `price` | `float64` | yes | The file's price for the stock (informational; `prices` is the price source) |
| `market_value` | `float64` | yes | |
| `exchange` | `utf8` | yes | |
| `location` | `utf8` | yes | |
| `available_date` | `date32` | no | First trading day the snapshot may be used: the next trading day after `snapshot_date` (conservative; verify the publication time) |
| `fetched_at` | `timestamp[ns, UTC]` | no | |
| `raw_sha256` | `utf8` | no | |
| `figi` | `utf8` | yes | Appended 2026-10-11 before first write (DECISIONS D17): US composite FIGI of the active security that held `ticker` on `snapshot_date` (`security_master`); null if unresolved (gate G5) |

Primary key: `(snapshot_date, etf, ticker)`. A ticker held by both ETFs has two
rows (the key includes `etf`). Duplicate lines for one `(snapshot_date, etf,
ticker)` in a file are summed (weights, shares, market value) and counted in
`data_quality`. New dataset: register it in `schemas/registry.json`; no migration file (`MIGRATIONS.md`).

---

## 31. `security_master`

Rows: about 20,000 (one per resolved security of the D12 scope; estimate). Writer: Rust (ingest, identity job J4a). Update: weekly, and on new codes. Partitioned: no.

The identifier master, keyed by the **US composite FIGI** (DECISIONS D17,
`docs/adr/0001-figi-security-key.md`). One row per security (a listed share
class on US venues) over its whole life. Redefined before first write on
2026-10-11 (was keyed by ticker; `migrations/2026-10-10-initial.md`).

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `figi` | `utf8` | no | US composite FIGI (`BBG` + 9 characters), from OpenFIGI |
| `share_class_figi` | `utf8` | yes | OpenFIGI share-class FIGI |
| `cik` | `utf8` | yes | 10-digit zero-padded issuer CIK; null until resolved. Active: the current ticker in the SEC ticker file. Otherwise (every delisted security): the one CIK whose EDGAR name or former name (`cik-lookup-data.txt`) has the same normalized name; a name of two CIKs stays null. `resolution_note` ends `; cik:name` for those (J4a, `BACKFILL.md` §5) |
| `ticker` | `utf8` | no | Canonical form of the latest ticker (from the latest EODHD code) |
| `name` | `utf8` | no | EODHD `Name` of the latest code |
| `eodhd_code` | `utf8` | no | The latest EODHD code without `.US` (`MON_old`, `BRK-B`) |
| `exchange` | `utf8` | yes | EODHD `Exchange` of the latest code |
| `delisted` | `bool` | no | True if every code of the security is in the delisted list |
| `security_type` | `utf8` | yes | OpenFIGI `securityType` (`Common Stock`, `REIT`, ...) |
| `figi_name` | `utf8` | yes | OpenFIGI `name` |
| `resolution` | `utf8` | no | How the FIGI was found: `override` / `isin` / `cusip_13f` / `rename` (a renamed code or vendor twin of a resolved code, `01-data-ingestion/SOURCES.md` Identifiers) |
| `resolution_note` | `utf8` | yes | Evidence: the ISIN or CUSIP used, the normalized names compared |
| `updated_at` | `timestamp[ns, UTC]` | no | |
| `sic` | `utf8` | yes | 4-digit SIC code of the issuer (CIK) from EDGAR submissions `sic`; null if unresolved or absent. Current value only (`UNIVERSE.md` §6) |
| `sic_description` | `utf8` | yes | EDGAR `sicDescription` |
| `sic_asof` | `date32` | yes | Date the `sic` value was fetched; a change is logged, never back-dated |

Primary key: `figi`. An EODHD code that does not resolve has no row here: it
is counted (gate G5, `identity_unresolved`) and its prices are not compacted.

---

## 32. `delistings`

Rows: ~800 (estimate). Writer: Rust (ingest). Update: daily. Partitioned: no.

Delisting events derived from EDGAR filings (`filing_events`), one row per
delisted security (`01-data-ingestion/SURVIVORSHIP.md` §3). It records what
EDGAR says and when EDGAR said it; the last price date comes from `prices`
and is combined in `price_coverage`.

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `ticker` | `utf8` | no | Canonical symbol |
| `cik` | `utf8` | no | |
| `delist_class` | `utf8` | no | `bankruptcy` / `acquired` / `delisted_other` / `deregistered` |
| `confidence` | `utf8` | no | `high` / `medium` / `low` |
| `rule_id` | `utf8` | no | The classification rule that fired (`SURVIVORSHIP.md` §3) |
| `form25_filing_date` | `date32` | yes | Form 25 / 25-NSE filing date |
| `est_last_trading_date` | `date32` | yes | Estimate: Form 25 filing date plus 10 days (verify), else the date of the earliest evidence |
| `form15_filing_date` | `date32` | yes | Form 15 filing date |
| `bankruptcy_filing_date` | `date32` | yes | Filing date of the 8-K with Item 1.03 |
| `acquisition_filing_date` | `date32` | yes | Filing date of the 8-K with Item 2.01 or 5.01 |
| `evidence_accessions` | `utf8` | no | JSON array of the accessions used |
| `built_at` | `timestamp[ns, UTC]` | no | |

Primary key: `ticker`. New dataset: register it in `schemas/registry.json`; no migration file (`MIGRATIONS.md`).

---

## 33. `filing_events`

Rows: ~300K (8-K items of about 2,500 companies over 15+ years; estimate). Writer: Rust (ingest). Update: daily. Partitioned: no.

Event-type filings from EDGAR submissions (`01-data-ingestion/SOURCES.md`
EDGAR events). One row per (filing, 8-K item); non-8-K event forms have the
item `-`. Usable from `filing_date` (the same point-in-time rule as
`fundamentals`).

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `accession` | `utf8` | no | |
| `item` | `utf8` | no | 8-K item number (`2.02`) or `-` |
| `ticker` | `utf8` | no | Canonical symbol, via `security_master` (one row per listed class) |
| `cik` | `utf8` | no | |
| `form_type` | `utf8` | no | `8-K`, `8-K/A`, `25`, `25-NSE`, `15-12B`, `15-12G`, `15-15D`, `NT 10-K`, `NT 10-Q` |
| `filing_date` | `date32` | no | **Point-in-time stamp** |
| `acceptance_datetime` | `timestamp[ns, UTC]` | yes | |
| `event_type` | `utf8` | no | `earnings_release` (2.02) / `delisting_notice` (3.01) / `bankruptcy` (1.03) / `acquisition_completed` (2.01) / `change_in_control` (5.01) / `auditor_change` (4.01) / `non_reliance_restatement` (4.02) / `obligation_acceleration` (2.04) / `delisting_filing` (Form 25) / `deregistration` (Form 15) / `late_filing_notice` (NT) / `other` |

Primary key: `(accession, item, ticker)`. New dataset: register it in `schemas/registry.json`; no migration file (`MIGRATIONS.md`).

---

## 34. `price_coverage`

Rows: ~2,500 (one per security that was ever a member of a membership universe; estimate). Writer: Julia (universe builder). Update: daily, after the universe; rewritten in full. Partitioned: no.

The survivorship coverage report in data form
(`01-data-ingestion/SURVIVORSHIP.md` §4). It is derived and may be rebuilt at
any time. The yearly missing-share table is a query over `universe` and this
dataset (`QUERY-PATTERNS.md`).

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `figi` | `utf8` | yes | US composite FIGI (D17); null for a member the identity job could not resolve (`gap_reason = identity_unresolved`) |
| `ticker` | `utf8` | no | Canonical symbol: the latest one for a FIGI, the holdings symbol when `figi` is null |
| `cik` | `utf8` | yes | |
| `first_member_date` | `date32` | no | First trading day as a member of IWF or IWP, from `etf_holdings` (`available_date`); a member without a FIGI has no `universe` row |
| `last_member_date` | `date32` | no | Last such date |
| `first_price_date` | `date32` | yes | Null if no prices |
| `last_price_date` | `date32` | yes | |
| `n_member_days` | `int32` | no | Member days (trading days) |
| `n_member_days_priced` | `int32` | no | Member days with a price row |
| `delist_class` | `utf8` | yes | From `delistings`, null if none |
| `est_last_trading_date` | `date32` | yes | From `delistings` |
| `coverage_status` | `utf8` | no | `complete` (priced through the last expected day) / `terminal_known` (series ends at a delisting with a terminal-value rule) / `gap` (priced days missing, cause logged) |
| `gap_reason` | `utf8` | yes | `vendor_no_data` / `vendor_truncated` / `late_start` / `symbol_reused` / `identity_unresolved` / `internal_gaps` / null |
| `terminal_class` | `utf8` | no | `none` (alive) / `bankrupt` / `acquired` / `unknown` (disappeared without a rule: labels are censored and flagged `survivorship_gap`) |
| `data_end` | `date32` | no | Last price date the report is computed against |
| `built_at` | `timestamp[ns, UTC]` | no | |

Primary key: `(figi, ticker)`: one row per FIGI, and per ticker among members without one (nulls compare equal, `IO-CONTRACT.md` §3). Re-keyed from `ticker` 2026-10-11, before first write (D17, D20). New dataset: register it in `schemas/registry.json`; no migration file (`MIGRATIONS.md`).


---

## 35. `listings`

Rows: about 8,000 symbol-directory lines per archived day (about 2M per year
from the day archiving starts; estimate), plus the reconstructed history
(`source = edgar_reconstructed`, size set in I2). Writer: Rust (ingest). Update: daily. Partitioned: `year` (of `date`).

Which securities were listed on each date, the data scope of DECISIONS D12
(every NYSE, NYSE American and Nasdaq common stock; trading stays IWF/IWP).
Raw facts from the Nasdaq Trader symbol directories (`nasdaqlisted.txt`,
`otherlisted.txt`) as archived each day, plus earlier history reconstructed
from EDGAR (Form 25, submissions) and labelled as such (D13). Test issues and
symbols that do not normalize (most preferred series, warrants, rights,
units) are dropped at parse time and counted in `data_quality`. `listings` is
a superset of the ingest scope: ETFs, ADRs, funds and preferreds whose symbol
does normalize (`BAC-A` -> `BAC.A`) are kept and flagged by `is_etf` and
`security_class`. The files have no security-type field, so `security_class` is a
rule-based classification whose rule is recorded; every rule is a **verify**
item until checked against a sample.

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `date` | `date32` | no | Listing date the row describes: the `America/New_York` date of the file's creation stamp, or the reconstructed date |
| `ticker` | `utf8` | no | Canonical symbol after normalization (`01-data-ingestion/UNIVERSE.md` §3) |
| `vendor_symbol` | `utf8` | no | The symbol exactly as in the file (`Symbol` or `ACT Symbol`) |
| `exchange` | `utf8` | no | `NASDAQ` / `NYSE` / `NYSE_AMERICAN` / `NYSE_ARCA` / `CBOE_BZX` / `IEX` (from the file's exchange code) |
| `security_name` | `utf8` | no | As in the file |
| `is_etf` | `bool` | no | The file's ETF flag |
| `market_category` | `utf8` | yes | Nasdaq market category (`Q` / `G` / `S`); null for other exchanges |
| `financial_status` | `utf8` | yes | Nasdaq financial status code; null for other exchanges |
| `round_lot` | `int32` | yes | Round lot size as in the file |
| `security_class` | `utf8` | no | `common` / `adr` / `fund` / `preferred` / `other` / `unknown`; D12's ingest scope is `common` on `NYSE`, `NYSE_AMERICAN` or `NASDAQ` |
| `class_rule` | `utf8` | no | Id of the classification rule that decided `security_class` |
| `cik` | `utf8` | yes | 10-digit zero-padded, from SEC `company_tickers_exchange.json` or the reconstruction; null if unresolved |
| `source` | `utf8` | no | `nasdaqtrader` / `edgar_reconstructed` |
| `file_created_at` | `timestamp[ns, UTC]` | yes | The file's creation stamp; null for reconstructed rows |
| `fetched_at` | `timestamp[ns, UTC]` | yes | When the file was fetched; null for reconstructed rows |
| `raw_sha256` | `utf8` | yes | SHA-256 of the raw file in the landing zone; null for reconstructed rows |

Primary key: `(date, source, ticker)`. New dataset: register it in `schemas/registry.json`; no migration file (`MIGRATIONS.md`).

---

## 36. `security_spells`

Rows: about 60,000 (several identifiers per security; estimate). Writer: Rust (ingest, identity job J4a). Update: with `security_master`. Partitioned: no.

Dated vendor identifiers of each security (DECISIONS D17): which EODHD code,
ticker, ISIN or CUSIP pointed at a FIGI, and when. "Which security was `FB` on
2015-06-30" and "which code serves this FIGI" are as-of lookups here. A
lookup table of existing identifiers, not a new identifier.

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `figi` | `utf8` | no | `security_master.figi` |
| `id_type` | `utf8` | no | `eodhd_code` / `ticker` / `isin` / `cusip` |
| `value` | `utf8` | no | The identifier (an EODHD code without `.US`; a canonical ticker) |
| `valid_from` | `date32` | yes | First date it is known to apply; null when unknown (symbol lists are snapshots; W5 fills code and ticker spans from the price series) |
| `valid_to` | `date32` | yes | Last such date; null when open or unknown |
| `source` | `utf8` | no | `eodhd_symbols` / `openfigi` / `sec_13f` / `override` |

Primary key: `(figi, id_type, value)`. New dataset: register it in `schemas/registry.json`; no migration file (`MIGRATIONS.md`).
