# P5: EDGAR bulk files

Spec: `.claude/skills/01-data-ingestion/BACKFILL.md` §2 (P5), `SOURCES.md` "SEC EDGAR".
Run: 2026-10-11 about 14:05 to 14:15 UTC, by hand with `curl`, `User-Agent: juniquant-probe
<SEC_EDGAR_EMAIL>` (the address is not recorded here), pauses of 0.3 s between requests (under
the 5 requests/s bucket). The two large files were not downloaded: their sizes come from `HEAD`
and their contents from HTTP range requests. The Rust probe `quant-ingest probe p5` repeats
this and writes the evidence JSON.

## Result

| Question | Answer |
|---|---|
| Bulk URLs | Both documented URLs answer `HTTP/2 200`: `.../daily-index/bulkdata/submissions.zip` and `.../daily-index/xbrl/companyfacts.zip`. Also 200: `files/company_tickers.json`, `Archives/edgar/cik-lookup-data.txt`. |
| Size | `submissions.zip` **1,568,623,659 bytes** (1.57 GB); `companyfacts.zip` **1,410,325,968 bytes** (1.41 GB). |
| `Last-Modified` | `submissions.zip` Sat 2026-10-10 04:33:26 GMT; `companyfacts.zip` Sat 2026-10-10 04:26:00 GMT; `cik-lookup-data.txt` 2026-10-10 03:10:27 GMT; `company_tickers.json` Wed 2026-10-07 20:38:50 GMT. The probe ran on 2026-10-11 14:05 UTC, so the zips were about 33 hours old. The "regenerated nightly" claim is **not confirmed by one observation**; repeat the `HEAD` on two consecutive days before relying on it. |
| Range requests | `accept-ranges: bytes`; `206` answers worked (used below). The zip is S3-multipart (`etag` ends in `-187` and `-169`). |
| Are `filings.files[]` pages in the submissions zip? | **Yes.** The zip64 central directory (993,915 entries, 87.5 MB, read with one range request) lists 988,520 main files `CIK##########.json` and **5,394 page files** `CIK##########-submissions-NNN.json` for 4,169 filers, up to page 071. Both GOOGL (`-001`) and BRK (`-001`) have theirs. |
| Multi-class company: dei shares facts | **They are not in `companyfacts` for the multi-class names.** GOOGL (CIK 1652044): the `dei` section holds only `EntityPublicFloat`; there is **no** `EntityCommonStockSharesOutstanding`. BRK (CIK 1067983): `EntityCommonStockSharesOutstanding` has 7 facts, the last filed 2011-05-06 (`val` 941,481 on 2011-04-29), none after. Cover-page counts per class carry a class dimension, and `companyfacts` drops dimensioned facts (`SOURCES.md` already says so). |
| What does carry shares for GOOGL | `us-gaap:CommonStockSharesOutstanding` (all classes together): 12,230,000,000 at 2026-06-30 (10-Q filed 2026-07-23, frame `CY2026Q2I`). Weighted-average basic and diluted shares are also present. |
| What carries them for BRK | Nothing usable: no `CommonStockSharesOutstanding`; only `WeightedAverageNumberOfSharesOutstandingBasic` and preferred/treasury tags. |
| `fy` / `fp` quirk | **Confirmed: they describe the filing, not the fact.** A fact with `end = 2025-12-31` in the 10-Q filed 2026-04-30 (accn `0001652044-26-000048`) carries `fy: 2026, fp: "Q1"`; the same value reappears in the 10-Q filed 2026-07-23 with `fy: 2026, fp: "Q2"`. The probe's finding: that one `end` (2025-12-31) is reported under three pairs, `(2025, FY)`, `(2026, Q1)` and `(2026, Q2)`. Never date a fact from `fy`/`fp`. |
| `sic`, `sicDescription` | Top level of the submissions JSON, strings: GOOGL `"7370"` / `Services-Computer Programming, Data Processing, Etc.`; BRK `"6331"` / `Fire, Marine & Casualty Insurance`. |

Also seen, relevant to the symbol map:

- `tickers[]` of GOOGL is `["GOOGL","GOOG","GOOGM","GOOGN"]` with a parallel `exchanges[]` of
  four `Nasdaq`. `GOOGM` and `GOOGN` are not the tickers we trade. Only a ticker the symbol
  map lists may be taken from `tickers[]`; the rest are noise.
- BRK is `["BRK-B","BRK-A"]` with a hyphen, which `symbols::normalize` turns into `BRK.B` /
  `BRK.A`.
- `filings.recent` holds 1,000 filings for both; the older ones are in
  `CIK0001652044-submissions-001.json` (1,713 filings, 2015-10-02 to 2023-07-13) and
  `CIK0001067983-submissions-001.json` (1,404 filings, 1998-08-10 to 2017-02-07). Fields per
  filing include `acceptanceDateTime`, `accessionNumber`, `filingDate`, `form`, `items`,
  `reportDate`, `isXBRL`, `isInlineXBRL`.
- Single-company calls are 150 to 160 KB (submissions) and 2.8 to 3.2 MB (facts).

## Consequences

1. **Backfill from the zips is possible and complete** for filing lists: the page files are
   inside, so no per-filer follow-up request is needed for the 4,169 filers with older
   filings.
2. **Shares outstanding cannot come from `dei` for multi-class issuers.** Use
   `us-gaap:CommonStockSharesOutstanding` where it exists (GOOGL). For BRK-style issuers
   with no tag, the shares figure has to come from another source (the filing's cover page or
   the XBRL instance) or the name is excluded from turnover and market-cap features. That is a
   design decision for the owner; the fundamentals builder must not read `dei` shares for
   these names and treat a missing value as zero.
3. **Do not date facts from `fy`/`fp`.** The spec already says so; the probe confirms it.
4. **Zip age.** Record `Last-Modified` on every download and keep it in the sidecar, so a
   backfill run states which snapshot it used.

## Not checked

- Contents of `companyfacts.zip` (only its size and `Last-Modified`).
- That the zips regenerate every night (one observation only).
- How many multi-class issuers lack a usable shares tag. Count them once the facts are parsed.
- `Last-Modified` of the per-company `data.sec.gov` endpoints: they sent no `last-modified`
  among the headers kept.
