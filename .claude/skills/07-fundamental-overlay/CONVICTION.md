# Conviction Score

Six quality components. Cross-sectionally standardized. Bounded multiplier.

## 1. Component Definitions

### 1.1 Piotroski F-Score

Nine binary tests summed to a score in [0, 9]. The score is always out of 9:
if a test cannot be evaluated (for example "no equity issuance" without share
count history) the component is **missing for that stock**, it is not scored
out of 8; scaling an 8-point score to 9 is not allowed:

| Test | Condition for 1 point |
|---|---|
| ROA > 0 | Net income / total assets positive |
| ΔROA > 0 | ROA higher than 4 quarters ago |
| CFO > 0 | Operating cash flow positive |
| Accruals | CFO / assets > ROA (quality of earnings) |
| ΔLever < 0 | Long-term debt / assets decreased |
| ΔLiquid > 0 | Current ratio improved |
| No equity issuance | No common stock issued in trailing 4 quarters |
| ΔMargin > 0 | Gross margin improved |
| ΔTurn > 0 | Asset turnover improved |

**Direction:** Higher = better. F-Score is a composite quality measure
.

**Note on large caps:** Research suggests F-Score performance fades on
large caps. Since the universe is IWF + IWP (large and
mid-cap growth), the F-Score weight is kept moderate (0.25) rather than
high.

### 1.2 ROIC − WACC Spread

    ROIC = NOPAT / Invested Capital
    WACC = weighted average cost of capital
    spread = ROIC − WACC

**Direction:** Higher = better. A positive spread means the company
creates value above its cost of capital. Research shows ROIC outperforms
ROE as a quality signal on identical firms.

### 1.3 Accruals Ratio

    accruals = (Net Income − Operating Cash Flow) / Total Assets

**Direction:** Lower = better. High accruals predict negative returns
(Sloan 1996). The sign is flipped for the z-score.

### 1.4 Gross Margin Trend

    margin = Gross Profit / Revenue
    trend = slope of margin over trailing 4 quarters

**Direction:** Improving = better. Captures pricing power.

### 1.5 Revenue Growth Persistence

    persistence = fraction of last 8 quarters with positive YoY revenue growth

**Direction:** Higher = better.

### 1.6 Analyst Revision Breadth

    breadth = (up revisions − down revisions) / total revisions (trailing 60 days)

**Direction:** Higher = better.

## 2. Cross-Sectional Standardization

Each component is standardized **per rebalance date** across the eligible
universe:

    z_i,c = (x_i,c − median_c) / (1.4826 × MAD_c)

Where:
- `median_c` = cross-sectional median of component `c` across all
  eligible stocks on the rebalance date
- `MAD_c` = median absolute deviation, **scaled by 1.4826** so that it is a
  consistent estimator of the standard deviation under normality. Without the
  scale a median/MAD z is about 1.48 times too large and the multiplier
  `1.0 + 0.12 z` would be steeper than the stated "12% per standard deviation".
  A component with `MAD_c = 0` has no z for that date (missing).

**Why median/MAD:** Financial fundamentals have fat tails and
occasional data errors. Mean/std is not robust. Median/MAD is.

**Winsorization:** Cap z-scores at ±3 to prevent single outliers from
dominating.

**Missing data:** If a stock lacks a component, reweight the remaining
components proportionally. If more than 2 of 6 components are missing,
set conviction to 1.0 (neutral) and flag for review.

## 3. Composite Score

    composite_z = Σ_c w_c · z_i,c

Weights:

| Component | Weight |
|---|---|
| Piotroski F-Score | 0.25 |
| ROIC − WACC spread | 0.20 |
| Accruals ratio | 0.20 |
| Gross margin trend | 0.15 |
| Revenue growth persistence | 0.10 |
| Analyst revision breadth | 0.10 |

## 4. Conviction Multiplier

    conviction = clip(1.0 + 0.12 · composite_z, 0.50, 1.25)

| composite_z | conviction |
|---|---|
| −4.17 | 0.50 (floor) |
| −2.0 | 0.76 |
| 0.0 | 1.00 |
| +2.0 | 1.24 |
| +2.08 | 1.25 (ceiling) |

**Why 0.12:** A 1-standard-deviation improvement in composite quality
translates to a 12% position size increase. If backtests show excessive
turnover from conviction changes, reduce to 0.08. If conviction has no
measurable effect, increase to 0.15.

**Why clip at [0.50, 1.25]:** The floor prevents quality signals from
eliminating a position the optimizer wants. The ceiling prevents
conviction from overriding position limits.

## 5. Update Cadence

| Trigger | What updates |
|---|---|
| 10-Q / 10-K filing | F-Score, ROIC−WACC, accruals, margin trend, revenue persistence |
| Daily close | Analyst revision breadth; cross-sectional re-standardization |
| Universe change | Cross-sectional medians and MADs; all z-scores |

## 6. Verification

- [ ] A test asserts conviction is in [0.50, 1.25]
- [ ] A test asserts z-scores use median/MAD, not mean/std
- [ ] A test asserts winsorization at ±3
- [ ] A test asserts missing components reweight proportionally
- [ ] A test asserts conviction does not include valuation