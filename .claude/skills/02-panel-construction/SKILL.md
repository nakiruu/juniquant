---
name: panel-construction
description: Building the 12-feature panel that feeds GSPQR. Load when
  touching features/src/ (raw.jl, ranks.jl, targets.jl, splits.jl, panel.jl,
  universe.jl), or any code computing momentum, volatility,
  sales-to-price, turnover, forward returns, ranks, terminal-value labels or
  the survivorship caveat.
---

# Panel Construction

The panel is the training and inference matrix. One row per in-scope security
(a FIGI in `security_master`) with prices per trading day, from the first trading day with at least 50
eligible names; training and calibration use only the rows that are
point-in-time eligible (§2, decisions C3, D20). Twenty-six columns: three identifiers, three forward-return
targets, six raw features, six cross-sectional rank features, three
appended label-flag columns, four appended candidate columns (not features, D18) and the
appended security key `figi` (D20).

## 0. Preconditions

The panel is **not** built until the data-quality gates pass
(`01-data-ingestion/QUALITY-GATES.md`; `preflight()` is the first stage of the
build). The build writes the survivorship caveat
(`01-data-ingestion/SURVIVORSHIP.md` §4) next to the panel and refuses to
finish without it.

## 1. What the Panel Contains

The exact schema is in `13-parquet-lake/SCHEMAS.md` §5. The breakdown:

| Group | Columns | Count |
|---|---|---|
| Identifiers | `ticker`, `date`, `split` | 3 |
| Targets | `y_fwd_63d`, `y_fwd_126d`, `y_fwd_252d` | 3 |
| Raw features | `mom_20d`, `mom_60d`, `mom_12m_1m`, `vol_60d_res`, `sp_ratio`, `turn_20d` | 6 |
| Rank features | `r_mom_20d`, `r_mom_60d`, `r_mom_12m_1m`, `r_vol_60d`, `r_sp`, `r_turn` | 6 |
| Label flags (appended) | `label_flag_63d`, `label_flag_126d`, `label_flag_252d` | 3 |
| Candidates (appended, not features; `FEATURES.md` §7a) | `log_mcap`, `r_size`, `amihud_60d`, `r_amihud` | 4 |
| Security key (appended, D20) | `figi` | 1 |

Total: 26 columns (the first 18 are the frozen model-facing schema; the
three flags, the four candidates and `figi` are appended and are not features). Feature column order is
frozen (constitution §5).

## 2. Non-Negotiables

Inherited from the constitution (§2, §3):

- **No feature uses data after `date`.** A feature computed "for date `t`"
  uses prices and events through `t` and fundamentals with
  `filing_date <= t`. Adjusted-price levels are not used; ratios are
  (`FEATURES.md` §0).
- **No random splits.** Train/cal/test assignment is chronological and
  global by date, with a 252-day purge band. The assignment rule is in
  `TARGETS.md` §4.
- **No cross-sectional ranks with fewer than 50 names, per feature.** Each
  rank column is computed per date across the eligible names that have that
  feature; if that count is below 50 the column is NaN on that date
  (`CROSS-SECTIONAL.md` §4). The eligible set is the point-in-time
  `universe` (every in-scope common stock of the D12 scope that passes the trailing
  liquidity, history and market-cap percentile rules, D20, D21; the $5M
  median dollar-volume floor is computed trailing at `t`; the entry-only
  maximum-median-quoted-spread filter of
  `10-execution-schwab/EXECUTION-WINDOW.md` §8 is docs only and is not part
  of the historical panel).
- **Train on the broad universe; buy from IWF/IWP (decision C3 as amended by D20).** The
  panel starts on the first trading day on which at least 50 names are
  `eligible` in `universe` (about 2008-01, `FEATURES.md` §6), and from then on
  holds a row for every in-scope security that has a price on that date,
  eligible or not. IWF/IWP membership is not a panel filter: it is applied only at
  trading time, as the buy filter (`01-data-ingestion/UNIVERSE.md` §2), and exists only
  from the first archived snapshot (2026-10-09). The schema has no eligibility column: every consumer that
  fits or calibrates (GSPQR generations and the research model, CPCV, TCP
  conformity scores, EVT residuals, the evaluation metrics) joins `universe`
  on `(date, figi)` and keeps only rows with `eligible = true` at `t`.
  Predictions are made for eligible names only. Ineligible rows (history
  under 756 days, liquidity, data quality) stay in the panel for diagnostics
  only, with NaN rank columns (`CROSS-SECTIONAL.md` §4); their labels are
  computed like any other row and are never used for a fit or a score.
- **NaN is a valid value, and every NaN label is explained.** A target is NaN
  only with a flag: `immature` (the horizon passes the data end),
  `survivorship_gap` (a name vanished with no known terminal value) or
  `data_gap`. Names that stop trading for a known reason get terminal values
  (`TARGETS.md` §3). Do not forward-fill targets.
- **No dropping rows with NaN features.** GSPQR uses LightGBM, which handles
  NaN natively. Dropping rows biases the training set.
- **No survivorship hiding.** The missing-price share and the censored-label
  counts are reported with every panel (`01-data-ingestion/SURVIVORSHIP.md`).

## 3. Build Order

The panel is built in six stages. Each stage is a separate function and can
be tested in isolation.

    0. Preflight: gates passed (QUALITY-GATES.md), coverage report available
    1. Load prices, splits, dividends, fundamentals, universe
    2. Compute raw features (per ticker, time-series; vol_60d_res uses the
       leave-one-out eligible market return)
    3. Compute cross-sectional ranks (per date, per feature)
    4. Compute forward returns and flags (targets, terminal values)
    5. Assign train/cal/test splits

Do not interleave these stages. A function that both computes a feature and
assigns a split is a bug waiting to happen.

## 4. Raw Features

See `FEATURES.md` for exact formulas. The six:

- `mom_20d` — 20-bar return of the total-return index
- `mom_60d` — 60-bar return
- `mom_12m_1m` — 12-month return skipping the most recent month
- `vol_60d_res` — 60-day residual volatility (idiosyncratic, against the
  leave-one-out point-in-time equal-weighted eligible market)
- `sp_ratio` — sales-to-price (precomputed `revenue_ttm` / market cap of
  as-traded close times split-aligned shares)
- `turn_20d` — 20-day mean turnover (as-traded volume / aligned shares)

Every feature is computed per ticker in time order, except that
`vol_60d_res` also uses the cross-section's returns on each past day. No
feature looks forward.

## 5. Cross-Sectional Ranks

See `CROSS-SECTIONAL.md`. The six raw features each produce a rank column.
Ranks are computed per date and per feature, from the **raw** feature (there
is no z-score step):

    r_mom_20d[i, t] = ( rank_avg(mom_20d[i, t]) - 1 ) / ( N_f(t) - 1 )

`N_f(t)` is the number of eligible tickers with a non-NaN value of that
feature on `t`; ties are averaged. The floor is 50 per feature; below it the
column is NaN on that date.

## 6. Targets

See `TARGETS.md`. Three forward excess returns of the total-return index:

- `y_fwd_63d` = 63-trading-day forward return minus the risk-free return
  over the same period
- `y_fwd_126d` = 126-trading-day forward excess return
- `y_fwd_252d` = 252-trading-day forward excess return

The risk-free rate is `DGS3MO` **as known at `t`** (ALFRED vintage,
`known_from <= t`). Names that stop trading before the horizon get terminal
values (decision D8: a later trade price if one exists; a performance-related
delisting or bankruptcy with no later price: -30% for an NYSE/AMEX listing,
-55% for a Nasdaq listing, then the risk-free rate; an acquisition: the last
price, then the risk-free rate) or are censored and flagged
`survivorship_gap`; only a horizon past the data end is an
unflagged-by-cause NaN (`immature`). `TARGETS.md` §3 is normative.

## 7. Train / Cal / Test Split

The split column is assigned by calendar date, the same cut for every ticker
(`TARGETS.md` §4):

| Split | Rule |
|---|---|
| `train` | Date is in the earliest 60% of panel dates, at least 252 trading days before the train/cal boundary |
| `purge` | The 252 trading days before the train/cal boundary. Unused: their labels reach into `cal` |
| `cal` | Date is in the next 20% of panel dates |
| `test` | Date is in the final 20% of panel dates |

A per-ticker split (each ticker's own earliest 60% of rows) is not allowed.
It puts one ticker's training dates inside another ticker's test dates and
leaks market-wide information through the cross-sectional ranks. Late-listing
tickers have fewer `train` rows; that is accepted.

**The forecast origin boundary** (used by TCP, see `04-tcp-calibration`) is
separate from the split column. TCP rolls its own windows; the split column is
for model training and backtest evaluation only.

**15+ years.** The panel spans about 2008-01 to the present, about 4,700
dates (estimate): `train` is about the first 10 years, `cal` and `test` about
3.7 years each (`TARGETS.md` §4). The panel starts at the first date with at
least 50 eligible names (decision C3, RESOLVED; `FEATURES.md` §6).

## 8. What Not to Do

- Do not compute features using future prices. Use `shift(N)` (backward),
  never `shift(-N)` (forward).
- Do not use adjusted-price levels, backward-adjusted prices or
  `adj_close_vendor`. Use ratios of the total-return index.
- Do not forward-fill targets. NaN means "unknown," not "carry forward."
- Do not drop rows with NaN features. LightGBM handles them.
- Do not compute cross-sectional ranks with fewer than 50 names for that
  feature.
- Do not z-score features before ranking, with an expanding or a rolling
  window. There is no normalization step before the ranks.
- Do not use `period_end` for any fundamental join. Use `filing_date`.
- Do not reorder columns. The order in `SCHEMAS.md` §5 is frozen.
- Do not overwrite `panel` partitions in place. Write through the lake
  writer (temp, atomic rename; `13-parquet-lake/IO-CONTRACT.md`).
- Do not fit, calibrate or score on a row whose ticker is not `eligible` in
  `universe` on that date, and do not rank it. Ineligible member rows are
  diagnostics only (§2).
- Do not start the panel before the first date with at least 50 eligible
  names.
- Do not build the panel when a HARD gate has failed.

## 9. Verification

Before committing any panel change:

- [ ] A test recomputes a known feature value from a known price series,
      including a split and a dividend
- [ ] A test asserts no feature uses future data (shuffle / truncation test)
- [ ] A test asserts each rank column is NaN when that feature's eligible
      count is below 50
- [ ] A test asserts targets are flagged `immature` beyond the data end, and
      that every NaN target has a flag
- [ ] A test asserts the terminal-value rules (later price, bankrupt and
      performance delisting at -30% NYSE/AMEX and -55% Nasdaq, acquired,
      survivorship_gap)
- [ ] A test asserts the first panel date is the first date with at least
      50 eligible names, and that the training and calibration row sets
      contain no row with `universe.eligible = false` at its date
- [ ] A test asserts the split is a global date cut (identical across
      tickers) with the 252-day `purge` band before `cal`
- [ ] `scripts/validate-panel.jl` passes
- [ ] The panel's schema hash matches the registry
- [ ] Re-running on the same input produces the same content hash
      (`13-parquet-lake/IO-CONTRACT.md` §4)
- [ ] The survivorship caveat is written and its hash recorded in the build
      report

## 10. Related Skills

- `00-construction` §2, §3 — point-in-time and no-lookahead
- `01-data-ingestion` — produces the inputs; `SURVIVORSHIP.md`,
  `QUALITY-GATES.md`, `UNIVERSE.md`
- `03-gspqr-model` — consumes the panel
- `13-parquet-lake` — schema definition and I/O contract
- `14-backtest-protocol` — uses split column for evaluation
