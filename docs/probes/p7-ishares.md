# Probe P7: iShares holdings (2026-10-11)

`.claude/skills/01-data-ingestion/BACKFILL.md` §2 and §7 (decision D1: P7 is the first step of the
holdings backfill). Requests made by hand with a browser user agent, 5 s apart.

## Findings

- **Product ids.** IWF = `239706` (`/us/products/239706/ishares-russell-1000-growth-etf`, page
  title "iShares Russell 1000 Growth ETF | IWF"). `239718/ishares-russell-midcap-growth-etf`
  serves a page titled "iShares Russell Mid-Cap ETF | IWR", so IWP's id is still to be confirmed.
- **Current holdings.** The page links `.../{id}/{slug}/latest-holdings.csv` (no `.ajax` link is
  left in the page). It returns the latest file: header block (fund name, `Fund Holdings as
  of,"Oct 09, 2026"`, inception date, shares outstanding, asset mix), a blank line, then the
  table `Ticker, Name, Sector, Asset Class, Market Value, Weight (%), Notional Value, Quantity,
  Price, Location, Exchange, Currency, FX Rate, Market Currency, Accrual Date`. IWF: 377 rows,
  futures (`RGYZ6`) and a when-issued right (`UWMC-R-W`) inline with the equities.
- **No identifiers.** The file has no ISIN, CUSIP or SEDOL column: holdings resolve to a FIGI by
  ticker and date only (`security_spells`).
- **Sectors** are GICS sector names in a short form (`Communication`, `Information Technology`).
- **No history over plain HTTP.** `latest-holdings.csv?asOfDate=20200131` ignores the date (the
  Oct 2026 file); `holdings.csv` and `latest-holdings.json` are 404; the old
  `1467271812596.ajax?...&asOfDate=` and a second `.ajax` id return the HTML page (with a
  `text/csv` or `application/json` content type). The page's "As of Dates" selector loads its
  data by script; no URL template is in the HTML.

## Decision needed (`BACKFILL.md` §7 options)

Option A (iShares history to 2008) is not available. The choice is between B (the universe
starts when an archive of iShares files starts), C (the funds' SEC filings: N-PORT from 2019,
N-Q / N-CSR before), and archiving the latest file daily from now on in any case.
