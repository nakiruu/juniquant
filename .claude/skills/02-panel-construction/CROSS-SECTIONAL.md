# Cross-Sectional Rank Features

Ranks transform raw features into relative signals. They answer "is this
stock's momentum high compared to its peers today?" rather than "is
momentum high in absolute terms?"

## 1. What Ranks Are (the one definition)

For each of the six **raw** features `f` and each date `t`, the rank of
ticker `i` is its position in the cross-sectional distribution of that raw
feature across the eligible tickers with a non-NaN value on `t`:

    r_f[i, t] = ( rank_avg(f[i, t]) - 1 ) / ( N_f(t) - 1 )

where `rank_avg` is the 1-based rank with ties averaged, and `N_f(t)` is the
number of tickers that are in the eligible set on `t` **and** have a non-NaN
`f` on `t`. The smallest value gets 0, the largest gets 1.

There is **no** time-series step before ranking. An earlier draft also
described a rolling 60-day Z-score of each feature "before the ranks"; that
step is removed (`FEATURES.md` §5): ranking the raw feature is the definition,
and nothing is z-scored in the panel.

## 2. Why Ranks, Not Raw Z-Scores

Cross-sectional Z-scores and ranks differ in how they handle outliers.

- A Z-score of 3 for one stock pulls the mean and standard deviation,
  distorting every other stock's Z.
- A rank of 1 for one stock doesn't affect any other stock's rank.

For financial features with fat tails, ranks are more robust. The GSPQR
paper uses ranks for this reason.

## 3. Tie Handling

Ties are broken by average rank. If three stocks tie for ranks 5, 6, 7,
each gets rank 6.

Julia: `StatsBase.tiedrank(values)` already returns average ranks for ties.
Do **not** use DuckDB's `PERCENT_RANK()` or `CUME_DIST()` for panel columns:
`PERCENT_RANK()` gives tied rows the *minimum* rank, and `CUME_DIST()` is a
different statistic, so neither equals this definition. (They are fine for
eyeballing.) Polars `rank(method='average')` is equivalent but is not used in
the Julia panel.

## 4. Universe Requirement and the Floor

Ranks are computed per date across the **eligible universe**. Eligible
means:

- `universe.eligible = true` on date `t` (`SCHEMAS.md` #1, the point-in-time
  daily table: trailing dollar volume and history at `t`, member of IWF or
  IWP on `t`; not today's constituents and not a flag computed over the
  whole file)
- Has a non-NaN value for the raw feature on date `t`

**`N` differs by feature.** `mom_20d` may exist for 600 names on a date
while `sp_ratio` exists for 480 (no filing yet, or negative revenue). Each
rank column uses its own `N_f(t)` for both the floor and the scaling. A
rank of 0.9 in `r_sp` (among 480) and in `r_mom_20d` (among 600) are both
"90th percentile among names that have the feature", but their resolution
(`1 / (N_f - 1)`) differs. That is accepted; the model sees NaN for names
without the feature.

**Floor.** If `N_f(t) < 50`, the whole column `r_f` is NaN on date `t`
(per feature, not per date): the system refuses to produce ranks on a
too-small cross-section. In the early years `r_sp` is NaN for every date
until at least 50 eligible names have a usable `revenue_ttm`
(`FEATURES.md` §6).

**Why 50:** Below 50, the rank distribution is too coarse to be
informative. A rank of 1 out of 20 is 5% top; out of 500 it's 0.2% top.
These are different signals, and the model was trained on the latter.

**Mag7 note:** With only 7 tickers, ranks are 0, 1/6, 2/6, ..., 1. The
values are still computable, but the model cannot learn rank-based signals
from such a coarse cross-section. The universe is IWF + IWP.

## 5. Per-Feature Details

The six rank columns and their source features:

| Rank column | Source | Direction |
|---|---|---|
| `r_mom_20d` | `mom_20d` | Higher is better |
| `r_mom_60d` | `mom_60d` | Higher is better |
| `r_mom_12m_1m` | `mom_12m_1m` | Higher is better |
| `r_vol_60d` | `vol_60d_res` | Lower is better (sign-flipped) |
| `r_sp` | `sp_ratio` | Higher is better |
| `r_turn` | `turn_20d` | Higher is better |
| `r_size` (candidate, not a feature; `FEATURES.md` §7a) | `market_cap` | None: larger = higher rank |
| `r_amihud` (candidate, not a feature) | `amihud_60d` | None: less liquid = higher rank |

**Sign flip for `r_vol_60d`:** Low volatility is the desirable signal. The
rank is computed on `-vol_60d_res`, so a high rank means low volatility.

**Sign flip for `r_sp`:** Not sign-flipped. High sales-to-price is a value
signal; higher is better.

## 6. Implementation

The canonical implementation is in `features/src/ranks.jl`. It works on row
positions inside each date group (the earlier sketch used a `:row_id` column
that does not exist in the panel):

```julia
using DataFrames
using StatsBase: tiedrank

# One rank column from one feature column. `df` is the panel (all dates).
function cross_sectional_rank(
    df::DataFrame;
    feature_col::Symbol,
    eligible_col::Symbol = :eligible,
    date_col::Symbol = :date,
    flip::Bool = false,               # true for vol_60d_res (low is better)
    min_n::Int = 50,
)::Vector{Union{Float64,Missing}}
    result = Vector{Union{Float64,Missing}}(missing, nrow(df))

    for g in groupby(df, date_col)
        rows = parentindices(g)[1]                       # positions of this date in df
        elig = coalesce.(g[!, eligible_col], false)
        x    = g[!, feature_col]
        ok   = [e && !ismissing(v) && !isnan(v) for (e, v) in zip(elig, x)]
        idx  = findall(ok)
        n    = length(idx)
        n < min_n && continue                            # leave as missing

        vals = Float64.(x[idx])
        flip && (vals = -vals)
        result[rows[idx]] = (tiedrank(vals) .- 1) ./ (n - 1)   # average ranks, scaled to [0, 1]
    end
    return result
end
```

(`parentindices`, `tiedrank` are used from memory of their APIs; verify when
the tests run.) The six calls produce the six rank columns in the order of
`FEATURES.md` §8. The panel also holds ineligible member rows, kept for
diagnostics only (`SKILL.md` §2, decision C3); `eligible_col` (joined from
`universe` on `(date, figi)`) keeps them out of the distribution, and their
rank columns stay NaN.

## 7. Verification

- [ ] A test on synthetic data with known ranks produces the expected
      output
- [ ] A test asserts a feature's rank column is NaN on a date where that
      feature has fewer than 50 eligible non-NaN values, while another
      feature's rank column on the same date is not
- [ ] A test asserts an ineligible member row on a date has NaN rank
      columns and does not change any eligible row's rank
- [ ] A test asserts ties are broken by average rank
- [ ] A test asserts `r_vol_60d` is sign-flipped
- [ ] A test asserts the scaling denominator is `N_f - 1` with `N_f` the
      per-feature count
- [ ] A test asserts no rank uses future data (altering data after `t` does
      not change any rank at `t`)
- [ ] A test asserts there is no use of `:row_id`, Z-scores or `PERCENT_RANK`
      in the panel code

## 8. What Not to Do

- Do not compute ranks on the full universe including ineligible names.
  Only eligible tickers on the date count.
- Do not include tickers with NaN raw features in the rank computation.
  They are excluded from the distribution, and their rank is NaN.
- Do not use `N` = total universe size when computing the scaling
  denominator. Use the number of eligible tickers with a non-NaN value of
  that feature.
- Do not compute ranks with fewer than 50 names. Return NaN.
- Do not use raw Z-score instead of rank, and do not z-score before ranking.
  The GSPQR paper uses ranks for a reason.
