# Covariance Estimation

The covariance matrix is used for **diagnostics and risk reporting only**.
It does not enter the optimization problem: the objective is the Sortino
ratio on historical downside scenarios, and the constraints are linear
caps on weights (`PMPT-FORMULATION.md` §3, §8). There are no
risk-contribution or covariance-based constraints. If a volatility cap is
ever added to the problem (for example `√(wᵀΣw) ≤ σ_max`), that is a
change to the formulation and needs this section and
`PMPT-FORMULATION.md` updated together. It is estimated via Ledoit-Wolf
shrinkage.

Uses of the matrix, all after the weights are fixed: reporting the
portfolio's ex-ante volatility and the contribution of each name or sector
to it, and flagging when the final `w_target` carries a volatility far
from the recent realized level.

## 1. Why Shrinkage

The sample covariance matrix is a poor estimator when the number of
assets is large relative to the number of observations. For a
500-asset universe with 252 days of returns, the sample covariance is
singular or near-singular.

Ledoit-Wolf shrinkage addresses this by combining the sample covariance
with a structured target:

    Σ̂ = δ·F̂ + (1 − δ)·Ŝ

Where:
- `Ŝ` = sample covariance matrix
- `F̂` = structured target (constant correlation model by default)
- `δ` = optimal shrinkage intensity (estimated from data)

## 2. The Target Matrix

The default target is the **constant correlation model**: all pairwise
correlations are set to the average sample correlation, while variances
are preserved from the sample.

This is the target used in the original Ledoit-Wolf (2003, 2004) papers
and in the `CovTools` R implementation .

## 3. The Shrinkage Intensity

The optimal `δ` is computed analytically to minimize the expected
Frobenius norm of the estimation error. The formula
depends on the concentration ratio `p/n` (assets over observations) and
the variance of the sample covariance entries.

`PortfolioOptimisers.jl` implements this via its `EmpiricalPrior` with
shrinkage enabled (verify the API in the pinned version).

## 4. Estimation Window

The covariance is estimated on a rolling 252-day window (1 year) of
daily returns.

**Why 252 days:** Research on shrinkage estimators uses rolling windows
of 1 year as a standard in-sample period. Shorter windows
are too noisy; longer windows include stale regime information.

## 5. What Not to Do

- Do not describe the covariance as a constraint or objective input. It is
  diagnostic only unless the formulation is changed.
- Do not use the sample covariance matrix directly. It is ill-conditioned
  for large universes.
- Do not use a diagonal covariance matrix (ignoring correlations). This
  destroys the diversification benefit in the risk report.
- Do not estimate on more than 2 years of data. Older data describes a
  different regime.
- Do not skip the shrinkage intensity calculation. The optimal δ is not
  0.5; it is data-dependent.
