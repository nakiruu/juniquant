# Forward Return Targets

The prediction targets. Three horizons, each a cumulative excess return, with
explicit rules for names that stop trading before the horizon ends.

## 1. Definition

For a forecast origin at date `t` and horizon `H` (trading days), the
forward excess return is

    y_fwd_Hd[t] = ( tri[T] / tri[t] - 1 ) - rf[t, H]          T = the H-th trading day after t

where

- `tri` is the **total-return index** of `FEATURES.md` §0, built from raw
  `prices` plus the `splits` and `dividends` tables. It is the one defined
  adjusted series; the earlier "use `adj_close` throughout" is withdrawn
  (`prices` no longer has an adjusted close that anything may use). The
  ratio `tri[T] / tri[t]` is a label (future prices are allowed in labels).
- `T` is `t + H` in the **trading calendar** (the NYSE rules plus overrides,
  `09-lock-state-machine` §4), not the H-th row of the ticker's price series.
- `rf[t, H]` is the cumulative risk-free return over the same period, from
  FRED `DGS3MO` (3-month Treasury constant maturity), **as known at `t`**:

        r3m(t)    = value of the latest DGS3MO observation with known_from <= t
                    (macro #4; as-of-t vintage rule, 01-data-ingestion/POINT-IN-TIME.md)
        rf[t, H]  = (1 + r3m(t) / 100)^(H / 252) - 1

  `known_from` for the daily H.15 series is at least one day after the
  observation date (`min_lag_days = 1`, verify the publication practice), so
  for an origin `t` the rate used is the observation of `t - 1` or earlier,
  never the observation dated `t`, and never a value revised or published
  after `t`. Every label depends on this rate, so the as-of rule is part of
  the label's definition. If no `DGS3MO` value known at `t` is at most 7
  calendar days old, the label is NaN with flag `data_gap` (gate G6 makes this
  case a build failure instead of a silent NaN).

  Using the rate known at `t` for the whole horizon is the ex-ante risk-free
  rate of a 3-month bill, compounded over `H` days; it is a convention, not
  the realized bill return. The realized path (the sequence of rates over the
  horizon) is allowed in a label but is not used; changing this is a
  definition change and a new panel version.

If `tri[t]` or the needed price is missing, see §3.

## 2. Horizons

| Column | H (trading days) | Approx. calendar |
|---|---|---|
| `y_fwd_63d` | 63 | 3 months |
| `y_fwd_126d` | 126 | 6 months |
| `y_fwd_252d` | 252 | 12 months |

The three horizons are stored independently. A row can have a valid
`y_fwd_63d` and NaN `y_fwd_252d` (flag `immature`) if the 252-day window
extends past the data end.

## 3. Terminal Values, NaN Rules and Flags

The earlier rule "NaN if the ticker was delisted, merged or removed between
`t` and `t + H`" is **replaced**. It silently dropped exactly the failures
(bankruptcies) and so inflated every result. A name that stops trading gets
a label by what is known about *why*; where nothing is known, the label is
censored and **flagged**, never silently dropped.

Per ticker, `price_coverage` (SCHEMAS #34, `01-data-ingestion/SURVIVORSHIP.md`)
gives `last_price_date` `L`, `terminal_class` ∈ {`none`, `bankrupt`,
`performance_delist`, `acquired`, `unknown`}, `listing_exchange` ∈
{`nyse_amex`, `nasdaq`, `other`, null} and `later_price` (true when the
security has trade prices after its exchange delisting date, for example OTC
bars from a source in `config/price_sources.toml`). `data_end` is the last
trading day with data in the build. `h` is the number of trading days from
`t` to `L`. `G(t, H, h) = (1 + r3m(t)/100)^((H - h)/252)` is the risk-free
growth from `L` to `T` at the rate known at `t`. The label for horizon `H`
and its flag column `label_flag_Hd` (SCHEMAS #5) follow in order:

| # | Condition | `y_fwd_Hd` | `label_flag_Hd` |
|---|---|---|---|
| 1 | `T <= L` and a bar exists on `T` (or the last bar on or before `T` is at most 3 trading days earlier) | ordinary (§1) | null |
| 2 | `T <= L` but no bar within 3 trading days before `T` | NaN | `data_gap` |
| 3 | `T > L`, `terminal_class` is `bankrupt` or `performance_delist`, and `later_price` | `( tri[L] / tri[t] ) * G(t, H, h) - 1 - rf[t, H]`: the last later trade price already contains the delisting loss; then the risk-free rate | `bankrupt` / `performance_delist` |
| 4 | `T > L`, `terminal_class` is `bankrupt` or `performance_delist`, no later price | `( tri[L] / tri[t] ) * (1 + R_d) * G(t, H, h) - 1 - rf[t, H]` with the delisting return `R_d = -0.30` for an NYSE/AMEX listing and `R_d = -0.55` for a Nasdaq listing (also for `other` or unknown, the more pessimistic value); then the risk-free rate | `bankrupt` / `performance_delist` |
| 5 | `T > L`, `terminal_class = acquired` | `( tri[L] / tri[t] ) * G(t, H, h) - 1 - rf[t, H]`: the stock's return to its last price, then the proceeds earn the risk-free rate | `acquired` |
| 6 | `T > L`, `terminal_class = unknown` | NaN (**censored**) | `survivorship_gap` |
| 7 | `T > data_end` and `terminal_class = none` | NaN | `immature` |

**Delisting returns (decision D8, owner 2026-10-10).** The -30% and -55%
values are the average returns after a performance-related delisting that
CRSP does not record, estimated by Shumway (1997, NYSE/AMEX) and Shumway and
Warther (1999, Nasdaq) (**verify** the published values and their samples
before the first build; they are constants in `features/src/targets.jl`, and
a change is a new panel version). "Performance-related" is `delist_class`
`bankruptcy` (8-K Item 1.03) or `delisted_performance` (Form 25 with an 8-K
Item 3.01 continued-listing notice and no acquisition evidence,
`SURVIVORSHIP.md` §3). The earlier rule "bankruptcy = -100%" is withdrawn: it
ignored recoveries and post-delisting trading and was a bound, not an
estimate (it remains one of the sensitivity bounds below).

Rows 3-5 are assumptions, not observations: the delisting returns are
literature averages, not the name's outcome; "acquired" at the last price
ignores the deal spread and the consideration mix (cash or stock, which
EDGAR's structured fields do not separate). All are inferred from EDGAR
filings with stated confidence (`SURVIVORSHIP.md` §3). Row 6 is the honest
statement that nothing is known: any other disappearance (`delisted_other`,
`deregistered`, no rule) is excluded and counted as `survivorship_gap`.

**A target is NaN if and only if** its flag is `data_gap`,
`survivorship_gap` or `immature`. `immature` ("the horizon runs past the data
end") is the only NaN that is not a data problem, and the only one the old
text allowed.

**A target is NOT NaN because:**

- The feature at `t` is NaN. Features and targets are independent.
- The ticker was not in the universe at `t` but is at `T`. Universe
  membership is separate from target availability.
- The name was delisted for a known reason (rows 3-5).

**Training and calibration.** Do not impute targets. Do not drop panel rows
with NaN targets. GSPQR is trained only on rows where the target for the
horizon being modeled is non-NaN **and** the ticker is `eligible` in
`universe` at `t` (decision C3, `SKILL.md` §2), so `survivorship_gap` rows
are excluded from training, TCP calibration, EVT and evaluation **and their
count is reported** (per year and horizon, in the survivorship caveat). Rows
flagged `bankrupt`, `performance_delist` and `acquired` are included; their
counts are reported too (split by "later price" and "delisting return"), so
a result can be recomputed without them.

**Sensitivity (reported with the caveat, never used as targets).** The
evaluation report may add two bound runs that impute the censored rows: with
`-1 - rf` (every disappeared name went to zero: a pessimistic bound) and with
`0` (zero excess return: a neutral reference). They bound the size of the
bias; they are not estimates and are labelled as such.

## 4. Train / Cal / Test Split

The `split` column is assigned by **calendar date**, identically for
every ticker:

    1. Take the sorted distinct trading dates D in the panel.
    2. D_train = D[floor(0.6 * |D|)],  D_cal = D[floor(0.8 * |D|)].
    3. date <= D_train - 252 trading days      -> `train`
    4. D_train - 252 trading days < date <= D_train -> `purge`
    5. D_train < date <= D_cal                  -> `cal`
    6. date > D_cal                              -> `test`

**Why global, not per-ticker.** Cross-sectional ranks, market-wide shocks
and the regime are shared across tickers on a given date. If each ticker
is split by its own row count, a ticker with a short history trains on
dates that another ticker uses as `test`, and the model learns the test
period's market-wide moves. A split that is not the same calendar cut
for every ticker leaks even when each ticker is individually
chronological. Tickers that list late simply have fewer (or no) `train`
rows; they still receive `cal`/`test` rows and are scored.

**Why the `purge` band.** A `train` row at date `t` carries a label that
ends at `t + H`, up to 252 trading days later. A row within 252 trading
days of `D_train` has a label that reaches into `cal`. Those rows are
marked `purge` and are used for neither training nor evaluation. 252 is
the longest horizon, so one `split` value serves all three horizons.
Models for shorter horizons may apply a tighter purge themselves (see
`03-gspqr-model/TRAINING.md` §2) but never a looser one.

Only the end of `train` needs purging: nothing is fitted on `cal`, so its
labels cannot contaminate `test`.

The split depends on the data range, so it changes whenever the panel
gains dates. It is immutable only for a given panel version (see §7).

**What the 15+ year depth gives (estimates).** A panel from about 2008-01 to
2026-09 has about 4,700 dates: `D_train` is about 2019-03, `D_cal` about
2022-12, the purge band starts about 2018-03, so `train` is about 2008-2018
(including the 2008 crisis), `cal` about 3.7 years (including 2020 and
2022), `test` about 3.8 years, whose last 252 days have `immature` 252-day
labels. If the owner starts the panel at the first fully featured date
(about 2011-07, `FEATURES.md` §6) the boundaries move to about 2020-08 and
2023-09. The numbers are recomputed from the panel dates at build time.

## 5. The Forecast Origin Boundary

The `split` column is not the same as the TCP forecast origin. TCP reads
stored as-of predictions and calibrates only on matured dates
(`s <= t - H`, see `04-tcp-calibration/CALIBRATION.md` §1). The `split`
column is used only for the **research evaluation**:

- The one-shot held-out research model (`--mode research`: trained on
  `split = train`, scored once on `split = test`)
- Its evaluation metrics (computed on `split = test`)

**Production does not use `split`.** Production GSPQR trains on all rows
whose label has matured at each month-end cutoff, as monthly walk-forward
generations (`03-gspqr-model/PREDICTION-BACKFILL.md`); the live model is
the latest generation, trained on all matured rows. The CPCV track ignores
`split` too (`14-backtest-protocol` §6). In every mode `survivorship_gap`
rows are excluded (§3) and the survivorship caveat is attached.

A ticker in the `test` phase may still contribute to TCP's rolling window
if the forecast origin is late enough. Do not conflate the two concepts.

## 6. Survivorship

The panel includes tickers that were in the universe on each date. A ticker
that was delisted in 2022 has rows through its last trading day, then no
more; the panel does NOT retroactively remove it from 2021. Do **not**
rebuild the panel using only currently-listed tickers.

That is the intent. Its fidelity depends on whether the free vendor served
the delisted name's prices at all. Members with no prices are not in the
panel (they cannot have features); they are counted as missing in the
coverage report, which is attached to the panel and to every artifact trained
or evaluated on it (`01-data-ingestion/SURVIVORSHIP.md` §4). The labels of
names that stop trading follow §3. What free data cannot fix is listed in
`SURVIVORSHIP.md` §6.

## 7. What Not to Do

- Do not compute forward returns from unadjusted `close` and do not use
  `adj_close_vendor`. Use `tri` (§1).
- Do not use the raw risk-free rate. Convert to a cumulative return over the
  horizon, and use the rate known at `t` (§1).
- Do not drop rows with NaN targets. They are legitimate rows with unknown
  future.
- Do not turn a `survivorship_gap` into a number, a zero, or a dropped row
  without a count.
- Do not reassign `split` after the panel is built. The split is immutable
  for a given panel version.
- Do not use the `split` column for TCP. TCP rolls its own windows.
- Do not assign `split` per ticker. It is a calendar-date cut shared by all
  tickers, with a 252-day `purge` band before the train/cal boundary.

## 8. Verification

- [ ] A test recomputes `y_fwd_63d` from known prices, a split, a dividend
      and the risk-free rate known at `t`
- [ ] A test asserts the label uses the rate dated at most `t - 1` and not a
      later vintage (change the stored vintage after `t`: no change)
- [ ] A test asserts NaN with flag `immature` beyond the data end, for every
      ticker alike
- [ ] A test on a bankrupt NYSE fixture with no later price (last price,
      then -30%, then the risk-free rate), a performance-delisted Nasdaq
      fixture (-55%), a bankrupt fixture with later OTC prices (the later
      price, no extra delisting return), an acquired fixture (return to the
      last price, then the risk-free rate), and an unknown fixture (NaN,
      `survivorship_gap`)
- [ ] A test asserts every NaN target has a non-null flag, and every flagged
      `bankrupt`/`performance_delist`/`acquired` target is finite
- [ ] A test asserts the split boundaries match the 60/20/20 date rule and
      are identical across tickers
- [ ] A test asserts no `train` row has `date + 252 trading days > D_train`
      (no train label reaches into `cal`)
- [ ] A test asserts every `train` date is earlier than every `cal` date,
      and every `cal` date earlier than every `test` date, across all
      tickers
- [ ] A test asserts that delisted tickers retain their historical rows
