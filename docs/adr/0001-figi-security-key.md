# 0001: Key history by the US composite FIGI, with CIK as the issuer code

**Status:** Accepted by the owner, 2026-10-11 (option B: free sources only; DECISIONS D17). An
earlier draft proposed an internal permanent id; the owner preferred existing identifiers.

## Context

Since D12 the backfill covers every NYSE, NYSE American and Nasdaq common stock from 2005, about
6,059 active and 16,588 delisted EODHD codes (probe P2). The lake keys history by **ticker**
(24 of the 35 primary keys), and over this universe a ticker does not identify a security:

- **Reuse.** 1,553 in-scope delisted codes are earlier holders of a ticker given to someone else
  later (`MON_old` = Monsanto 2000-2018; `MON` = Monument Circle Acquisition 2021-2022).
- **Renames.** Facebook's history from 2012 is under `META`; `FB` now belongs to another
  security, while a 2015 IWF holdings file says `FB`.
- **The vendor's own codes move.** EODHD renames the current holder to `{CODE}_old` the next time
  the ticker is reused.

The current spec's canonical ticker (latest symbol, `SYMBOL~CIK10` for an earlier holder,
`UNIVERSE.md` §3-§4) is a key that changes whenever a security is renamed or its ticker reused,
and needs a resolved CIK before a reused ticker can be stored. The first W5 run stores Monsanto and
Monument Circle under one ticker `MON`.

## What is available free (measured 2026-10-11)

| Identifier | Level | Free source | Coverage | Permanent |
|---|---|---|---|---|
| Composite FIGI (US) and share-class FIGI | Security | OpenFIGI API, free key | Dead securities are mapped from an ISIN or a CUSIP (Monsanto `BBG000BFWKC0`, Lehman `BBG000BKRK35`, Sears `BBG000PQ6YJ3`); a ticker lookup returns only the current holder | Yes; survives ticker and name changes |
| CIK | Issuer | SEC submissions and ticker files | Every SEC filer, live or dead | Yes, never reused |
| ISIN (US = CUSIP + check) | Issue | EODHD symbol list `Isin` | 94% of active, 32% of delisted, 31% of reused (`_old`) codes | No (changes with the CUSIP) |
| CUSIP | Issue | SEC Official List of Section 13(f) Securities, quarterly 1996-2026 (PDF; text from 2020): CUSIP, issuer name (28 characters), issue description; no ticker | Exchange-listed equities | No |
| EODHD search | Lookup | In the plan | Active tickers only: no help for delisted names | n/a |

Not free: gvkey, gvkey_iid, Capital IQ ids, PERMNO/PERMCO, EODHD Fundamentals (`General` has CIK,
CUSIP, ISIN and FIGI per code; HTTP 403 on the current plan; option A, kept for later).

## Decision

1. **The security key is the US composite FIGI** (`figi`, `BBG` + 9 characters): one per security
   on US venues, assigned by the FIGI standard, never reused, unchanged by ticker or name changes. A
   reorganization that issues new shares gets a new FIGI and is a new security here too.
2. **The issuer code is the CIK**, carried beside the FIGI (fundamentals, filings and the B1 lock
   groups join on it). ISIN, CUSIP, share-class FIGI, EODHD code and ticker are attributes.
3. **Resolution of an EODHD code to its FIGI** (identity job J4a, before prices), first that applies:
   1. a manual row in `config/identifier_overrides.csv` (gains a `figi` column);
   2. the code's ISIN from the EODHD symbol list -> OpenFIGI `ID_ISIN` with
      `includeUnlistedEquities`, keeping the result with `exchCode = US` and `securityType =
      Common Stock`;
   3. no ISIN: a CUSIP from the 13(f) lists whose normalized issuer name equals the code's
      normalized EODHD name (the `UNIVERSE.md` §4 name rule) and whose list quarters overlap the
      code's trading span -> OpenFIGI `ID_CUSIP`, accepted only if OpenFIGI's name also matches.
      More than one candidate is unresolved, never a guess;
   4. otherwise **unresolved**: the code's raw responses are kept and its rows are staged, but it is
      not compacted into the lake; it is counted (gate G5, `identity_unresolved`) and listed in the
      survivorship report, so a later override or source adds it without a refetch.
4. **CIK** comes from the SEC: the current ticker files for active codes, and the `UNIVERSE.md` §4
   name-and-date match against submissions `name` / `formerNames` for the rest.
5. **`security_master`** (not yet written) becomes one row per `figi`: CIK, share-class FIGI,
   current ticker, name, share class, exchange, status, first and last trading day, SIC fields,
   `resolution` (`override` / `isin` / `cusip_13f`) and its evidence.
6. **`security_spells`** (new dataset #36, single file, `overwrite`): one row per `(figi, id_type,
   value, valid_from)` with `valid_to` and `source`, for `id_type` in `ticker`, `eodhd_code`,
   `isin`, `cusip`. It answers "which security was `FB` on 2015-06-30" (holdings files) and "which
   EODHD code serves this FIGI" (prices). It is a lookup of vendor identifiers, not a new
   identifier.
7. **Keys.** History and analytics are keyed by `figi`; live trading stays on the tradable ticker:

   | Datasets | Key | Change |
   |---|---|---|
   | `prices`, `splits`, `dividends` | `(figi, date)` / `(figi, ex_date)` | `figi` added first; `ticker` becomes the ticker in force on that date |
   | `fundamentals`, `filing_events`, `delistings`, `price_coverage` | `figi` replaces `ticker` in the key | `ticker` kept as of the row's date |
   | `etf_holdings`, `listings` | unchanged | `figi` appended (null if unresolved) |
   | `universe`, `panel`, `gspqr_predictions`, `tcp_calibration`, `evt_residuals`, `evt_tail_bounds`, `conviction_scores`, `veto_flags` | `figi` replaces `ticker` | `ticker` kept for display |
   | `portfolio_targets` | `(rebalance_date, figi)` | both columns: the bridge from model to order |
   | execution datasets (`orders`, `fills`, `positions`, `position_locks`, `pending_orders`, `tax_lots`, `order_history`, `execution_costs`, `quote_spread_1m`, `trade_summary`) | unchanged (tradable ticker) | `figi` appended where history is joined (`trade_summary`) |

   Two EODHD codes that resolve to one FIGI (a vendor rename) are one security: their bars are
   combined, and a conflicting bar on one date is a resolution error, reported.
8. **The `SYMBOL~CIK10` form is retired.** `symbols::is_tradable` keeps rejecting `~`; order
   entry, the lock, the restricted list and the B1 lock groups are unchanged.

## Consequences

- **Corrections before first write.** Every dataset in the table is unwritten except `prices`,
  `splits`, `dividends` and `data_quality`, which hold only the eight-code W5 test run: it is
  deleted and rebuilt from the landing zone (raw files are kept). Recorded in
  `migrations/2026-10-10-initial.md`.
- **Job order (`BACKFILL.md` §8):** J4a identity (EODHD symbol lists, OpenFIGI, 13(f) lists, SEC
  CIK) runs before J5 prices; W5 stages by FIGI and compacts only resolved codes.
- **New environment variable** `OPENFIGI_API_KEY` (free; without it the job runs at the keyless
  rate, about 90 minutes instead of a few minutes for 22,600 codes).
- **New raw source** `sec_13f_list` (about 120 quarterly files). Text extraction of the pre-2020
  PDFs needs a tool without Python: the `pdftotext` CLI (poppler-utils, not installed) or a Rust
  PDF crate, chosen in the implementation.
- **Expected coverage**, to be measured by J4a: about 94% of active codes through ISIN; delisted
  codes through ISIN (32%) plus whatever the 13(f) name match adds. The unresolved remainder is a
  known, reported gap, like the survivorship gap, and a reason to reconsider option A later.
- **Skills to change in the same commit as the implementation:** `13-parquet-lake` (SKILL §3,
  SCHEMAS #1-#3, #5, #7-#13, #24, #28-#36, registry), `01-data-ingestion` (UNIVERSE §3-§4,
  BACKFILL §5.1, §5.5, §8, SOURCES identifiers, QUALITY-GATES G5), the key columns in `02`-`07`,
  `16` and `18`.

## Amendment 2026-10-12: training-only keys (owner)

The audit of 2026-10-12 found that most of the survivorship gap was this decision's, not the
vendor's: 6,124 landed EODHD common-stock codes (9.5M bars, 5,287 delisted) had no FIGI, and
against EDGAR the lake held only 41-55% of the operating companies that delisted in 2006-2017.
OpenFIGI has no FIGI for many retired identifiers (Bear Stearns, Wachovia, National City,
Ambac), and the dropped names delisted almost four times as often as the kept ones.

So a delisted code that no FIGI route reaches is keyed `EOD:<EODHD code>` when one EDGAR filer
with its name (`cik-lookup-data.txt`) filed a 10-K or 10-Q (the landed submissions zip) while
the code traded (`cik_confirmed`); of several such filers, the one that filed between its first
and last bar (Bear Stearns Companies Inc, not the LLC that filed after the merger). The same
choice gives a FIGI security whose name several filers share its CIK: 589 had none, Lehman
Brothers Holdings among them, so no filings, fundamentals or delisting record. The key enters `prices`, the universe
and training like a FIGI (`resolution = eod_code`); it is never bought, since it is delisted and
in no IWF/IWP snapshot. Left out and counted as before: an active code, a filer that a FIGI
security already has (a twin or another class), and codes of one filer whose prices overlap.
Two FIGI routes were also fixed: a when-issued line (`RBC-W`, `NEOGEN CORP-WI`) no longer makes
an ISIN ambiguous (Regal Rexnord had no FIGI), and an OpenFIGI name cut at 28 characters
matches a code whose name starts with it (never a name with an instrument word).

## Alternatives considered

- **An internal permanent id with a ledger** (the earlier draft): complete coverage, but its own
  identifier infrastructure; declined by the owner.
- **EODHD Fundamentals** (option A): CIK, CUSIP, ISIN and FIGI per code without matching; paid
  (about $60/month) and its delisted coverage is unverified. **Declined by the owner
  (2026-10-11): no further EODHD spend.** The unresolved remainder is measured against IWF/IWP
  membership (gate G5) and closed with overrides.
- **Ticker with `~CIK10`** (the current spec): a key that changes.
- **CIK + share class as the key:** issuer-level; a new class or a reorganization breaks it.

## Open questions

1. Probe P7: do iShares holdings files carry ISIN or CUSIP? If so, holdings resolve to FIGI
   directly, without the ticker spells.
2. OpenFIGI terms of service for storing mappings locally.
