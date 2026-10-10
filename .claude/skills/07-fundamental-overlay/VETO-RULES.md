# Veto Rules

Seven triggers. Any one is sufficient. The veto is binary and absolute.

## 1. The Seven Triggers

| # | Trigger | Threshold | Source | Rationale |
|---|---|---|---|---|
| 1 | Altman Z-Score | < 1.8 | 10-Q / 10-K | Distress zone  |
| 2 | Going-concern qualification | Any | 10-K audit opinion | Auditor doubts viability |
| 3 | Negative book + negative OCF | Both | 10-Q / 10-K | No equity, no cash generation |
| 4 | Pending delisting notice | Any | Exchange filing | Imminent removal |
| 5 | Material restatement | Trailing 12 months | 8-K | Accounting integrity |
| 6 | Auditor resignation | Trailing 12 months | 8-K | Red flag |
| 7 | Debt covenant breach | Any | 10-Q / 10-K | Creditor action likely |

## 1a. Trigger Data Sources and Status (owner decision D5; constitution §7, amendment 1.8)

A trigger with no point-in-time source is **inactive**: it is reported as
inactive in every run and never simulated.

| # | Trigger | Status | Source / rule |
|---|---|---|---|
| 1 | Altman Z < 1.8 | Active (non-financial issuers only: `universe.sector` at `t` is not `Financials` or `Real Estate`; when `sector` is null, `security_master.sic` outside 6000-6799) | `fundamentals` (as-first-reported, `filing_date`) |
| 2 | Going-concern qualification | **Inactive** | No free point-in-time source for the audit-opinion text |
| 3 | Negative book + negative OCF | Active | `fundamentals` |
| 4 | Pending delisting notice | Active | `filing_events`: 8-K Item 3.01 (verify) |
| 5 | Material restatement | Active | `filing_events`: 8-K Item 4.02 (verify) |
| 6 | Auditor resignation | Active (text rule) | 8-K Item 4.01 **only when the filing text states the auditor resigned or declined to stand for re-election**; dismissals do not trigger. Documented phrase rule with test vectors in `conviction/test/` |
| 7 | Debt covenant breach | Active as a **labelled proxy** | 8-K Item 2.04 (triggering events that accelerate a direct financial obligation; verify); every veto it causes is labelled `proxy` |

Conviction inputs with no point-in-time source (analyst revision breadth,
WACC inputs) are likewise inactive; the composite uses the remaining
components and reports which were missing.

## 2. Altman Z-Score

The public company model:

    Z = 1.2·(WC/TA) + 1.4·(RE/TA) + 3.3·(EBIT/TA) + 0.6·(MVE/TL) + 1.0·(Sales/TA)

Where:
- WC = Working Capital (current assets − current liabilities)
- TA = Total Assets
- RE = Retained Earnings
- EBIT = Earnings Before Interest and Taxes
- MVE = Market Value of Equity
- TL = Total Liabilities

**Zones:**
- Z ≥ 3.0: Safe
- 1.8 ≤ Z < 3.0: Grey
- Z < 1.8: Distress → **VETO** 

The grey zone (1.8–3.0) does not trigger a veto, but it does widen the
EVT lower bound by 1.5× (see `05-evt-tails`).

**Non-manufacturing adjustment:** For service companies, the original
model recommends omitting the sales/total assets variable and using
different cutoffs (Z < 1.1 = distress). The pipeline uses
the standard public company model for all tickers for simplicity. If
false positives are observed in service sectors, add a sector-conditional
model.

## 3. Going-Concern

**Inactive** (§1a): no free point-in-time source for the audit-opinion text.
Reported as inactive in every run; never simulated.

From the 10-K audit opinion. The auditor states substantial doubt about
the company's ability to continue as a going concern. This is a rare
but definitive signal. Any going-concern qualification triggers an
immediate veto.

## 4. Negative Book + Negative OCF

Two conditions, both required:

- Book value of equity < 0
- Operating cash flow (trailing 12 months) < 0

A company with negative book value but positive cash flow may be
restructuring. A company with negative book and negative cash flow
is burning equity with no internal funding source.

## 5. Pending Delisting

From NYSE or Nasdaq filings. If the exchange has issued a delisting
notice, the stock is removed from the universe immediately. The veto
persists until the notice is withdrawn or the company appeals successfully.

## 6. Material Restatement

From 8-K filings. A restatement of previously issued financial
statements within the trailing 12 months. This signals either accounting
error or fraud. The veto persists until 12 months have passed since
the restatement filing.

## 7. Auditor Resignation

From 8-K Item 4.01 filings, active only when the text states that the
auditor resigned or declined to stand for re-election (§1a). A dismissal by
the company does not trigger. The auditor resigning mid-engagement is a red
flag.
The veto persists until a new auditor is engaged and files an opinion.

## 8. Debt Covenant Breach

Active as a labelled proxy (§1a): 8-K Item 2.04, a triggering event that
accelerates or increases a direct financial obligation. Covenant-breach text
in 10-Q/10-K notes has no structured free source and is not parsed. If the
company discloses a breach of a debt covenant, creditors may accelerate
repayment. The veto persists until the breach
is waived or cured.

## 9. Veto Persistence

A veto, once triggered, persists until a subsequent filing explicitly
clears it.

| Trigger | Cleared by |
|---|---|
| Altman Z < 1.8 | Next 10-Q/10-K with Z ≥ 1.8 |
| Going-concern | Next 10-K without qualification |
| Negative book + OCF | Next 10-Q/10-K with positive book OR positive OCF |
| Delisting | Notice withdrawn |
| Restatement | 12 months since filing |
| Auditor resignation | New auditor opinion filed |
| Covenant breach | Waiver or cure disclosed |

## 10. What Not to Do

- Do not use a single trigger as a soft signal. Veto is binary.
- Do not ignore a grey-zone Z-Score. Widen the EVT bound (see `05-evt-tails`).
- Do not clear a veto without a filing. The persistence rule is strict.
- Do not apply the manufacturing model to all sectors without
  acknowledging the limitation.