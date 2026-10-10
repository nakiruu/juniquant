# Feature Formulas

Every raw feature, its exact computation, its edge cases, and its
verification test.

Notation: `t` is a trading day of the panel calendar; "`t-k`" is `k` bars
earlier in a ticker's own series. All inputs are the lake datasets of
`13-parquet-lake/SCHEMAS.md`: `prices` (as traded), `splits`, `dividends`,
`fundamentals`, `universe`.

---

## 0. Price, Share and Return Series (defined once)

`prices` holds **raw as-traded** open/high/low/close/volume. Nothing is
pre-adjusted. All adjusted quantities are derived here, in Julia, from the
event tables. Notation per ticker: `c[t]` = as-traded close, `v[t]` =
as-traded volume.

**Split ratio.** A `splits` row has `ratio = numerator / denominator` (new
shares per old share; 4 for a 4-for-1, 0.125 for a 1-for-8 reverse split) and
`ex_date`. `rho(a, b]` = the product of `ratio` over splits with
`a < ex_date <= b` (1 if none).

**Total-return index `tri[t]`** (the "adjusted series"; split- and
dividend-adjusted, forward-chained so that it uses only events up to `t`):

    G[t]   = ( rho(t', t] * c[t]  +  sum over dividends d in (t', t] of a_d * rho(t', d] ) / c[t']
    tri[t] = tri[t'] * G[t],       tri[first bar] = 1

where `t'` is the previous bar of the ticker and `a_d` the dividend per
share as paid on its ex-date `d`. For the usual case of consecutive bars
this is `G[t] = rho_t * (c[t] + a_t) / c[t-1]`. Dividends are reinvested at
the ex-date close (a convention; the vendor's own adjusted close uses the
prior close and differs only in the second order, which the reconciliation
gate G2 tolerates). `ret[t] = tri[t] / tri[t'] - 1` is the daily total return. It is defined
only when `t'` is the immediately preceding trading day of the calendar
(one-bar return); across a hole in the ticker's data it is NaN, so a missing
bar never turns into a multi-day return inside the volatility regression.

**Point-in-time property.** `tri[t]` is a function of bars and events on or
before `t`: the *level* of `tri` is therefore stable as data is added. A
vendor-style "adjusted close" scaled to equal today's price
(`tri[t] * c[T] / tri[T]`) is **not** point-in-time as a level (it uses all
events after `t`); a ratio of two such values at dates `<= t` is identical to
the ratio of `tri` values and so is point-in-time. Rule: ratios of adjusted
prices are not lookahead; levels of backward-adjusted prices are, and no
feature uses them. Never compare adjusted levels across tickers.

**Shares aligned to `t`.** For ticker `i` and day `t`, take the most recent
`fundamentals` row with `filing_date <= t` and non-null `shares_outstanding`,
with cover date `s` = `shares_asof_date` (or `filing_date` if null):

    n[t] = shares_outstanding * rho(s, t]

so the share count is carried through any split between the cover date and
`t` (only splits with `ex_date <= t`). Not adjusted for buybacks or issuance
until the next filing.

**Market cap and dollar volume.**

    market_cap[t]  = c[t] * n[t]                      # as-traded price x aligned shares
    dollar_vol[t]  = c[t] * v[t]                      # as traded

The identity `c[t] * n[t] = (vendor split-adjusted close[t]) * (reported
shares) * F(s)` holds, so using raw prices with split-aligned shares is
exactly equal to the vendor-adjusted calculation without using any split
after `t` (checked in the tests).

**Volume.** Stored volume is as traded, so `v[t] / n[t]` compares shares
traded with shares outstanding on the same share basis. No volume adjustment
is applied after the fact (that would create a level that depends on future
splits).

**Multi-class companies (OPEN).** Fundamentals are per company; several
tickers can share them (`01-data-ingestion/UNIVERSE.md` §4). For a company
whose share classes trade at different prices, `n[t]` x one class's price is
wrong. Default rule until the owner decides: `n[t]` is the sum of all class
counts reported on the cover page; if the company has two or more listed
class tickers in `prices` and their closes on `t` differ by more than 20%,
`market_cap[t]` and `turn_20d` are NaN for those tickers; otherwise the sum
is used with the ticker's own price (the standard approximation, e.g. for
share classes with equal economics). The alternative is to be exact only
where the class split is known from `fundamentals` facts, which needs a
per-class source that is not yet specified. Owner decision.

---

## 1. Momentum Features

All use `tri` (§0): they are ratios of adjusted prices.

### `mom_20d`

    tri[t] / tri[t-20] - 1

**Edge cases:**
- If `tri[t-20]` is NaN, the feature is NaN.
- If fewer than 21 bars of history, NaN.

### `mom_60d`

    tri[t] / tri[t-60] - 1

Same edge cases.

### `mom_12m_1m`

    12-month return skipping the most recent month:
    tri[t-21] / tri[t-252] - 1

The skip-1-month convention avoids contaminating momentum with short-term
reversal, which is a well-documented effect (Jegadeesh 1990, Asness 2013).

**Edge cases:**
- Requires 253 bars of history. NaN below that.
- If `tri[t-252]` is NaN, NaN.

`t-k` counts bars of the ticker's own series, which equals trading days when
the series has no holes. A hole longer than 5 trading days inside the window
makes the feature NaN (the bar count would not mean what the name says).

---

## 2. Volatility Feature

### `vol_60d_res`

Residual volatility from a rolling regression of daily total returns on the
equal-weighted return of the point-in-time eligible set, **excluding the stock
itself**, over a 60-bar window.

**Market return for ticker `i` on day `s`.** Let `E(s)` be the tickers with
`universe.eligible = true` on day `s` and a non-NaN `ret[s]`; let
`n_s = |E(s)|` and `S_s = sum of ret_j[s]` over `E(s)`. Then

    mkt_i[s] = (S_s - ret_i[s] * 1[i in E(s)]) / (n_s - 1[i in E(s)])

(the leave-one-out mean; if `i` is not in `E(s)` it is the plain mean). It is
NaN if the count in the denominator is below 30. `E(s)` is the eligible set
**on day `s`**, not on day `t` and not today's constituents: no survivor
selection and no use of later membership.

**Procedure** for each date `t` and ticker `i`:

1. Take `ret_i[s]` and `mkt_i[s]` for the last 60 bars `s = t-59 ... t`.
2. Keep the pairs where both are non-NaN. Require at least 40.
3. Fit `ret_i = alpha + beta * mkt_i + eps` by OLS.
4. `sigma_eps = sqrt( sum(eps^2) / (n - 2) )` with `n` the number of valid
   pairs, and `vol_60d_res[t] = sigma_eps * sqrt(252)`.

**Why residual (not raw):** Raw volatility captures both market and
idiosyncratic variance. The GSPQR paper uses residual volatility to isolate
the stock-specific risk component.

**Why leave-one-out and a point-in-time set:** including the stock in its own
benchmark biases beta toward one by `1/n` and shrinks the residual; using a
fixed or current set selects survivors.

**Edge cases:**
- Fewer than 40 valid pairs: NaN.
- Zero variance of `mkt_i` or of `ret_i` in the window: NaN.

**Verification:** Regress a synthetic series with known beta and residual std
on a synthetic market (with a membership list that changes), confirm
`vol_60d_res` matches `sigma_eps * sqrt(252)` within 1e-9, and that removing
the stock from `E(s)` changes the result when the stock is large.

---

## 3. Valuation Feature

### `sp_ratio`

    Sales-to-price = revenue_ttm / market_cap[t]

`revenue_ttm` is the trailing-four-quarter revenue **precomputed by
ingestion** (`fundamentals.revenue_ttm`, `01-data-ingestion/SOURCES.md`); the
panel does not redo period arithmetic and never reads `period_end`.

**Point-in-time discipline:**
- Take the most recent `fundamentals` row with `filing_date <= t` and
  non-null `revenue_ttm` (as-of join on `filing_date`, never on
  `period_end`). Amendments are separate rows; the latest `filing_date <= t`
  wins, ties by `accession`.
- `market_cap[t]` is §0 (as-traded `c[t]` x split-aligned shares from the
  latest row with shares).

**Edge cases:**
- If no such row exists before `t`, NaN. This is the case before the XBRL
  era: the structured data starts around 2009-2011 (SCHEMAS #3), so
  `sp_ratio` and `r_sp` are NaN earlier. That is a limit of free data.
- If `revenue_ttm` is null (fewer than four consecutive quarters), negative
  or zero, NaN.
- If `market_cap[t]` is NaN or non-positive, NaN.

**Verification:** Take a known filing and a known price, compute by hand,
compare; include a split between the cover date and `t`.

---

## 4. Liquidity Feature

### `turn_20d`

    20-day mean turnover:
    mean over the last 20 bars of  v[s] / n[s]

Each day's turnover uses that day's as-traded volume and that day's
split-aligned share count `n[s]` (§0). The mean is over the trailing 20 bars.

**Edge cases:**
- If a day in the window has NaN volume or shares, that day is skipped and
  the mean is over the valid days. Requires at least 15 valid days.
- Below 15 valid days, NaN.
- Multi-class tickers: §0.

**Verification:** Take a known 20-day window including a split, compute the
mean of daily turnover ratios, compare.

---

## 5. No Time-Series Normalization

The earlier draft normalized each raw feature by a rolling 60-day Z-score
"before the ranks" and said the Z-scores were intermediate and not stored.
That step is **removed**. The rank features are cross-sectional percentile
ranks of the **raw** features on each date (`CROSS-SECTIONAL.md`); a ranking
is invariant to any per-date monotone rescaling, and a per-ticker rolling
z-score would change the cross-section's ordering on a date for no reason the
model can use. There is no Z-score in the panel and no code for it. The
LightGBM model receives the six raw features and the six ranks.

---

## 6. History, Warm-Up and the First Usable Date

| Feature | Bars needed |
|---|---|
| `mom_20d` | 21 |
| `mom_60d` | 61 |
| `mom_12m_1m` | 253 |
| `vol_60d_res` | 60 (and at least 40 valid pairs) |
| `turn_20d` | 20 (and at least 15 valid days) |
| `sp_ratio` | a filing with `revenue_ttm` |

The universe rule `history_days >= 756` (3 years) is stricter than every
feature, so an **eligible** row always has its warm-up; the feature NaN rules
above only matter after gaps (and for the ineligible diagnostic rows,
`SKILL.md` §2, which may lack a warm-up and are never fitted on).

**What the 15+ year depth means for the first panel date** (dates are
estimates; the real values come from the backfill):

- Prices start at `BACKFILL_START = 2005-01-01`, so a name has 756 bars of
  history from about early January 2008: the first date with enough eligible
  names for ranks (at least 50) is about **2008-01** (it can be later if the
  iShares history starts later, `01-data-ingestion/BACKFILL.md` §2 probe P7).
- `sp_ratio` needs XBRL: `fundamentals` starts with periods ending after
  2009-06 for the largest filers and is phased in through 2011; a trailing
  four-quarter figure adds a year. So `sp_ratio` and `r_sp` are NaN for most
  names until about **2010-2011**, and broadly populated from about
  **2011-07**. The other five features are available from 2008.
- Today is 2026-10, so 15 years back is 2011-10. The fully featured window
  (all six features and ranks for most names) is therefore about 15 years;
  2008-2011 adds a partial-feature stretch that contains the 2008 crisis.

**RESOLVED (decision C3, owner 2026-10-10):** the panel starts at the first
trading day with at least 50 eligible names (about 2008-01, `sp_ratio` NaN
for the first 2-3 years), and training and calibration use only rows that
are point-in-time eligible at `t` ("train on what you would trade",
`SKILL.md` §2). Keeping 2008-2011 keeps more history and the crisis for the
EVT tails; its cost is a missingness pattern that changes over the sample
(`sp_ratio` absent early, present later), which trees can use as a spurious
time signal, so the panel build report prints the per-year missing share of
every feature among eligible rows. The global date split (`TARGETS.md` §4)
is computed on the panel dates. If the iShares history starts later than
2008 (probe P7, `01-data-ingestion/BACKFILL.md` §2), the first date with 50
eligible names, and so the panel, starts later.

---

## 7. What Is Not a Feature

- **No lagged returns beyond the momentum windows.** Adding `ret_1d`,
  `ret_5d`, etc. would duplicate `mom_20d` information.
- **No technical indicators.** RSI, MACD, Bollinger Bands are not in the
  GSPQR feature set. Adding them requires a skill amendment and a backtest
  proving incremental value.
- **No fundamental ratios beyond `sp_ratio`.** P/E, P/B, EV/EBITDA are used
  in the conviction overlay, not in GSPQR. Do not add them here.
- **No macro features in the panel.** Macro enters via the regime module and
  the HANET-style attention if implemented. The panel is cross-sectional
  only. (The risk-free rate enters the *targets* only.)
- **No adjusted price levels.** Only ratios of adjusted prices (§0).

If you believe a feature is missing, propose it as a skill amendment with a
backtest. Do not add it silently.

---

## 8. Feature Ordering

The 12 features are stored in this exact order in `panel`:

    mom_20d, mom_60d, mom_12m_1m, vol_60d_res, sp_ratio, turn_20d,
    r_mom_20d, r_mom_60d, r_mom_12m_1m, r_vol_60d, r_sp, r_turn

The GSPQR feature matrix uses this order. Reordering breaks the model's
feature-name binding. The linter enforces this order via the schema hash. The
three appended `label_flag_*` columns are not features.

---

## 9. Verification Summary

| Feature | Test |
|---|---|
| `tri` | Hand-computed total return across a split and a dividend; PIT test: appending later events does not change `tri[t]` |
| `market_cap` | Equals vendor-adjusted close x reported shares x split factor, with a split between cover date and `t` |
| `mom_20d` | Recompute from known `tri`, compare |
| `mom_60d` | Same |
| `mom_12m_1m` | Same, with 1-month skip |
| `vol_60d_res` | Synthetic regression with known beta and sigma; changing membership; stock excluded from its own market |
| `sp_ratio` | Hand-compute from a real filing |
| `turn_20d` | Hand-compute from a real 20-day window including a split |
| Ranks | `CROSS-SECTIONAL.md` §7 |

Every feature has a test. The test file is `tests/features.jl`. No feature is
considered implemented until its test passes.
