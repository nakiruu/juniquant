# 0001: A permanent security id instead of the ticker as the key of history

**Status:** Proposed (2026-10-11). Becomes DECISIONS D17 when the owner approves it; the skill and
schema changes listed under Consequences land in the same change.

## Context

Since D12 the backfill covers every NYSE, NYSE American and Nasdaq common stock from 2005, about
6,059 active and 16,588 delisted EODHD codes (probe P2). The lake keys history by **ticker**
(`prices (ticker, date)`, `splits (ticker, ex_date)`, `security_master (ticker)`, 24 of the 35
primary keys). Over this universe a ticker does not identify a security:

- **Reuse.** 1,553 in-scope delisted codes are earlier holders of a ticker given to someone else
  later (`MON_old` = Monsanto 2000-2018; `MON` = Monument Circle Acquisition 2021-2022).
- **Renames.** Facebook's history from 2012 is under `META`; `FB` now belongs to another
  security. A 2015 IWF holdings file says `FB`.
- **The vendor's own codes move.** EODHD renames the current holder to `{CODE}_old` the next time
  the ticker is reused, so even the vendor code is not a permanent key.

The current spec handles this with a canonical ticker that is "unique across all time": a renamed
security takes its **latest** symbol, and an earlier holder of a reused symbol becomes
`SYMBOL~CIK10` (`UNIVERSE.md` §3-§4). That key **changes**: the next rename of a security, or the
next reuse of a still-current ticker, re-keys its whole history in every dataset, against the
immutability of written rows (constitution §5, `13-parquet-lake` §2). It also needs a resolved CIK
before a reused ticker can be stored at all. The first W5 run shows the failure mode: `prices`
holds Monsanto and Monument Circle under one ticker `MON` (the spans do not overlap, so the
collision check did not fire).

The established answer is CRSP's PERMNO: a permanent number per security, never reused, with dated
names and tickers in a separate table. CRSP, Compustat (gvkey, gvkey_iid) and Capital IQ ids are
licensed, so the system needs its own.

## Identifiers available free

Measured on 2026-10-11 (EODHD symbol lists from P2; one OpenFIGI request):

| Identifier | Level | Free source | Coverage here | Permanent | Use |
|---|---|---|---|---|---|
| CIK | Issuer | SEC (submissions, `company_tickers_exchange.json`) | Every SEC filer, live or dead; dead names found by name and date matching (`UNIVERSE.md` §4) | Yes, never reused | Issuer attribute; lock groups already use it (B1) |
| Composite FIGI (US) and share-class FIGI | Security | OpenFIGI API, free key (25 requests / 6 s, 100 lookups each) | Dead securities are mapped (Monsanto `BBG000BFWKC0`, Time Warner `BBG000F0PF65`) but **only from an ISIN or CUSIP**: a ticker lookup returns the current holder (`MON` -> Monument Circle) | Yes; survives ticker and name changes | The strongest free identity evidence, where reachable |
| ISIN (US = CUSIP + check) | Issue | EODHD symbol list `Isin` | 5,685 of 6,059 active (94%), 5,240 of 16,588 delisted (32%), 478 of 1,553 reused (31%) | No: a CUSIP can change on a name change or reorganization | Evidence, dated |
| CUSIP | Issue | SEC 13(f) securities lists, N-PORT fund holdings (verify format and depth) | Exchange-listed equities in the 13(f) era | No (as ISIN) | Evidence, dated; a route to FIGI for names without an EODHD ISIN |
| EODHD code | Vendor | EODHD | All of the backfill | No (`_old` renames) | Dated attribute; how prices are fetched |
| EODHD search (`/api/search/{q}`) | Lookup | EODHD (in the plan, 1 call each) | **Active tickers only**: Monsanto by ISIN, by name and Lehman by CUSIP return nothing (tested 2026-10-11) | n/a | A fallback for resolving a current holdings name or ticker to an EODHD code and ISIN; no help for delisted securities |
| Ticker | Exchange | Every source | All | No | Dated attribute; what holdings files and the broker use |

gvkey, gvkey_iid, Capital IQ ids, PERMNO/PERMCO: not free. LEI: entity level and sparse for small
or dead companies.

## Decision

1. **`security_id`.** One per security: one listed share class of one issuer, over its whole life,
   through ticker, name, CUSIP and exchange changes. Format `S` + 7 digits (`S0000001`); it never
   matches the ticker grammar, so the two cannot be confused. Assigned once, never reused, never
   changed, never merged away.
2. **The ledger `security_ids`** (new dataset #36, single file, `append_only`): `security_id`,
   `assigned_at`, the evidence that created it (`composite_figi`, `isin`, `eodhd_code`,
   `first_bar_date`, `cik`, `name`), `status` (`resolved` / `provisional`), and `same_as` (null, or an
   earlier id when a duplicate is found later). Ids are handed out by the next free number in ledger
   order, so a rebuild of every other dataset from raw files reproduces the same ids: the ledger is
   the one thing a rebuild reads instead of recomputing, like `audit_log`.
3. **Dated identifiers `security_spells`** (new dataset #37, single file, `overwrite`): one row per
   `(security_id, id_type, value, valid_from)` with `valid_to`, `source` and `evidence`, for
   `id_type` in `ticker`, `eodhd_code`, `isin`, `cusip`, `cik`, `composite_figi`,
   `share_class_figi`. "Which security was `FB` on 2015-06-30" is one as-of lookup here, and so is
   "which EODHD code serves `S0000123` today".
4. **`security_master`** (not yet written) becomes one row per `security_id`: the current
   ticker, name, share class, CIK, FIGIs, exchange, first and last trade date, status, and the SIC
   fields it already has. `prior_tickers` (JSON) is dropped in favour of the spells.
5. **Resolution** of an observation (an EODHD code with its series, a holdings row, an EDGAR
   record) to a `security_id`, first rule that applies:
   1. a manual row in `config/identifier_overrides.csv` (gains a `security_id` column);
   2. same composite FIGI;
   3. same ISIN, with a consistent name or date span (an ISIN alone is not enough: CUSIPs change);
   4. continuity of the EODHD series: same `first_bar_date` and overlapping bars under the old and
      new code (how `MON` -> `MON_old` is followed);
   5. same CIK and share class with an overlapping trading span;
   6. otherwise a **new** id, `status = provisional`. Provisional ids are counted by the gate that
      replaces G5's ticker count; above its threshold the panel build stops and the list is the work
      queue for the overrides file.

   A duplicate found later (two ids for one security) is recorded as `same_as` in the ledger and is
   never deleted; compaction maps every row of the later id to the earlier one.
6. **Keys.** History and analytics are keyed by `security_id`; live trading stays on the tradable
   ticker:

   | Datasets | Key | Change |
   |---|---|---|
   | `prices`, `splits`, `dividends` | `(security_id, date)` / `(security_id, ex_date)` | `security_id` added first; `ticker` becomes the ticker in force on that date |
   | `fundamentals`, `filing_events`, `delistings`, `price_coverage` | `security_id` replaces `ticker` in the key | `ticker` kept as of the row's date |
   | `etf_holdings`, `listings` | unchanged | `security_id` appended (resolved, nullable if unresolved) |
   | `universe`, `panel`, `gspqr_predictions`, `tcp_calibration`, `evt_residuals`, `evt_tail_bounds`, `conviction_scores`, `veto_flags` | `security_id` replaces `ticker` | `ticker` kept for display |
   | `portfolio_targets` | `(rebalance_date, security_id)` | both columns: the bridge from model to order |
   | execution datasets (`orders`, `fills`, `positions`, `position_locks`, `pending_orders`, `tax_lots`, `order_history`, `execution_costs`, `quote_spread_1m`, `trade_summary`) | unchanged (tradable ticker) | `security_id` appended where a join to history is needed (`trade_summary`) |

   Every dataset above except `prices`, `splits`, `dividends` and `data_quality` is unwritten, so
   these are corrections before first write (`13-parquet-lake` §1). The three written ones hold
   only the eight-code W5 test run, which is deleted and refetched from the landing zone (raw files
   are kept), with the owner's approval of this record.
7. **The `SYMBOL~CIK10` form is retired** as a storage key: a reused ticker no longer needs a
   distinct string. `symbols::is_tradable` keeps rejecting `~`, and order entry is unaffected (the
   lock, the restricted list and B1 lock groups work on tradable tickers and CIKs, as now).

## Consequences

- **Job order (`BACKFILL.md` §8).** A new identity job J4a runs before prices: EODHD symbol lists
  -> OpenFIGI (ISIN -> FIGI, `includeUnlistedEquities`, US composite only) -> SEC CIK matching ->
  ledger, spells, master. J5 (W5) resolves each code to its `security_id` before staging, and the
  ticker-collision exclusion becomes a resolution conflict (two ids claiming one code on one date),
  reported, not silently merged.
- **New environment variable** `OPENFIGI_API_KEY` (free); without it the job runs at the keyless
  rate (25 requests per minute, 10 lookups each, about 90 minutes for 22,600 codes instead of a few
  minutes).
- **Skills to change in the same commit:** `13-parquet-lake` (SKILL §3 table, SCHEMAS #1-#3,
  #5, #7-#13, #24, #28-#36/#37, registry), `01-data-ingestion` (UNIVERSE §3-§4, BACKFILL §5.1, §5.5,
  §8, SOURCES identifiers, QUALITY-GATES G5), `02-panel-construction` and `03`-`07` keys, `16`/`18`
  where they join history, the migration note `migrations/2026-10-10-initial.md`.
- **Unchanged:** constitution, lock rules, `symbols` normalization, the restricted list, the
  broker-facing code.
- **Cost.** One identity job (about the size of W5), schema text across the skills, a regenerated
  registry; no new paid source.

## Alternatives considered

- **Keep the ticker key with the `~CIK10` suffix** (the current spec): re-keys history on every
  later rename or reuse, and cannot store a reused ticker until its CIK is resolved.
- **Composite FIGI as the key:** permanent and free, but unreachable for about 68% of delisted
  codes today (no ISIN), so a second kind of key would be needed anyway.
- **CIK + share class:** issuer-level evidence only; one CIK can list a new class or reorganize into
  a new security, and some issuers are not SEC filers.
- **EODHD code:** not permanent (`_old`).
- **Exclude every `_old` code** until CIK resolution can name them `SYMBOL~CIK10`: smallest change,
  but leaves renames to string matching and keeps a key that changes.

## Open questions

1. iShares holdings files: do they carry ISIN or CUSIP (probe P7)? If so, universe membership
   resolves through FIGI directly, the most important link in the system.
2. SEC 13(f) lists and N-PORT: machine-readable depth and format (a CUSIP route for the 68% of
   delisted codes without an EODHD ISIN).
3. OpenFIGI terms of service for storing FIGI mappings locally (the FIGI standard is open; confirm
   the API terms).
4. The security boundary for reorganizations (a company emerging from bankruptcy with new shares,
   a holding-company reorganization): a new id when the composite FIGI changes, unless an override
   says otherwise. Owner to confirm.
