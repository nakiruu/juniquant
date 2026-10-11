# Probes P1-P4: EODHD (2026-10-11)

Run with `quant-ingest probe p1` ... `p4` (`.claude/skills/01-data-ingestion/BACKFILL.md` §2)
on 2026-10-11 from 14:25 UTC, plan "EOD Historical Data, All World" (DECISIONS D15). Evidence:
`$QUANT_ROOT/reports/probes/p{1..4}.json` plus the landed bodies under `p{1..4}/raw/` (the key
is redacted; the `/api/user` account record was not landed). 31 requests and 2 manual checks
of `_old` codes; about 130 API calls of the 100,000 daily allowance.

## P1: adjustment (settles the EODHD normalization in `SOURCES.md`)

| Split | Close before -> after | Close ratio | Volume ratio | Reading |
|---|---|---|---|---|
| AAPL 4:1, 2020-08-31 | 499.23 -> 129.04 | 3.87 | 0.83 | raw close (499.23 is the known as-traded close); volume adjusted |
| NVDA 10:1, 2024-06-10 | 1,208.88 -> 121.79 | 9.93 | 1.31 | raw close; volume adjusted |
| GE 1:8 reverse, 2021-08-02 | 12.95 -> 100.60 | 0.129 | 0.30 | raw close; volume adjusted (as traded it would be near 8) |

AAPL dividend, ex-date 2020-08-07: `unadjustedValue` 0.82 (as paid), `value` 0.205 (restated).

**Decision:** `open`-`close` are stored as returned; volume is divided by `F(d)`; `dividends.amount`
= `unadjustedValue`. Matches the EODHD documentation.

## P2: delisted, acquired and renamed codes

| Code | Bars | First -> last | Holder |
|---|---|---|---|
| TWX.US | 5,148 | 1997-12-31 -> 2018-06-15 | Time Warner (acquired) |
| CELG.US | 8,135 | 1987-07-28 -> 2019-11-20 | Celgene (acquired) |
| RHT.US | 5,008 | 1999-08-11 -> 2019-07-08 | Red Hat (acquired) |
| LLTC.US | 4,829 | 1997-12-31 -> 2017-03-10 | Linear Technology (acquired) |
| LEH.US | 2,695 | 1997-12-31 -> 2008-09-17 | Lehman Brothers (bankrupt, last close 0.13) |
| TWTR.US | 2,259 | 2013-11-07 -> 2022-10-27 | Twitter (taken private) |
| MON.US | 454 | 2021-03-16 -> 2022-12-30 | **a later holder of the ticker, not Monsanto** |
| SHLD.US | 772 | 2023-09-13 -> 2026-10-09 | **a later holder (a fund), not Sears** |
| FB.US | 325 | 2025-06-26 -> 2026-10-09 | **a later holder, not Facebook** |
| META.US | 3,619 | 2012-05-18 -> 2026-10-09 | Facebook/Meta, the whole history under the current code |

**Reused tickers.** A code always holds its **current** holder. An earlier holder of a reused
code is renamed `{CODE}_old`, `_old1`, `_old2`, ... in the delisted list (1,712 such codes),
identified by `Name` and `Isin`: `MON_old` = Monsanto (ISIN US61166W1018, 2000-10-18 ->
2018-06-06, 4,435 bars), `SHLD_old` = Sears Holdings (US8123501061, 2005-03-28 -> 2018-10-23,
3,420 bars). A renamed company's history moves with it to the new code (FB -> META).

**Symbol lists** (`type=common_stock`): 17,794 active and 33,173 delisted codes in all;
NYSE + Nasdaq + NYSE American (AMEX and NYSE MKT) = **6,059 active and 16,588 delisted**. OTC
venues are the rest and are out of scope (D12).

## P3: depth, reuse, share class

| Code | First bar | Note |
|---|---|---|
| SPY.US | 1993-01-29 | full history since listing |
| DLB.US | 2005-02-17 | the IPO day |
| GM.US | 2010-11-18 | new GM only: old GM is not mixed in (it is under its own code) |
| BRK-B.US | 1996-05-09 | **hyphen form confirmed** |
| BRK.B.US | 404 | dot form does not exist |

Several delisted series start on 1997-12-31 rather than at their listing (TWX, LLTC, LEH): the
backfill starts in 2005, so this does not matter here; logged as `vendor_truncated` if a name's
first member day is earlier than its first bar.

## P4: plan and costs

- Plan `monthly`, `dailyRateLimit` 100,000.
- Counter deltas: `/api/eod` 1 call, `/api/splits` 1 call, `/api/eod-bulk-last-day/US` **100
  calls** (7.5 MB). `/api/user` itself costs nothing.
- On Sunday 2026-10-11 the bulk endpoint's latest day was Friday 2026-10-09. The publication time
  of a trading day is not measured yet: rerun P4 on a weekday evening (open item).

## Consequences recorded in the skills

- `SOURCES.md` EODHD: normalization confirmed; share-class form `BRK-B.US`; the `_old` rule for
  reused tickers; call costs.
- `BACKFILL.md` §9: about 68,000 calls for 3 x (6,059 + 16,588) codes, under the 90,000 daily
  cap, so the backfill fits in one day.
- Open: where the EODHD code of a historical holder is stored. It is not a canonical ticker
  (`MON_old` fails `symbols::normalize`), so it does not belong in `config/symbol_map.toml`; the
  natural home is the identifier master's per-holding vendor code (I2, W6/identifiers).
