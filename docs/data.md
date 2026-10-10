# Data

Summary of what the system collects and how. Normative details:
[`01-data-ingestion`](../.claude/skills/01-data-ingestion/SKILL.md) (sources, backfill,
survivorship, universe, quality gates) and [`13-parquet-lake`](../.claude/skills/13-parquet-lake/SKILL.md)
(datasets, schemas, I/O contract).

## Sources (free only)

| Source | What | History |
|---|---|---|
| Yahoo chart API | Raw as-traded daily OHLCV, split and dividend events | From 2005-01-01 |
| SEC EDGAR | XBRL fundamentals (bulk), 8-K events, delistings, company identifiers | XBRL from about 2009 |
| FRED / ALFRED | Treasury yields, credit spreads, CPI, VIX, with publication vintages | From 2004-01-01 |
| iShares | IWF and IWP holdings by as-of date (point-in-time universe) | As far back as iShares serves (probe P7 decides) |

Every value is used only from the date it became public (filing date, release vintage,
holdings as-of date). Requires a FRED API key and a contact email for the SEC User-Agent.

## The backfill

15+ years, fetched once and then kept current daily. Every raw response is kept in an
immutable landing zone with a SHA-256, so parsing can be replayed. A manifest makes the
backfill resumable after any interruption. Probes P1-P8 run first to settle every
assumption about the sources ([`BACKFILL.md`](../.claude/skills/01-data-ingestion/BACKFILL.md)).

## Survivorship bias

Free price sources generally do not serve stocks that were delisted or acquired. Those are
often the biggest losers, so missing them makes results look better than they were.
The system cannot fix this with free data, so it measures it:

- A coverage report lists every stock that was ever in IWF/IWP and whether its prices exist
  through its delisting.
- Delisted stocks get terminal returns where the reason is known (later prices if any;
  otherwise -30% NYSE/AMEX, -55% Nasdaq for performance delistings; last price for cash
  acquisitions). Unexplained disappearances are excluded and counted.
- Every model and evaluation artifact carries a survivorship caveat. Above 2% missing in any
  year it warns; above 10% a model is not promoted without the owner's acknowledgement.

Details: [`SURVIVORSHIP.md`](../.claude/skills/01-data-ingestion/SURVIVORSHIP.md).

## Quality gates

Gates G0-G10 must pass before the panel is built: calendar consistency, price completeness
and sanity, survivorship coverage, universe vs holdings, FRED completeness, filings, events
([`QUALITY-GATES.md`](../.claude/skills/01-data-ingestion/QUALITY-GATES.md)); thresholds in
`config/quality_gates.toml`.

## The lake

34 Parquet datasets under `$QUANT_ROOT/lake`, each with exactly one writer, schemas frozen
once written and verified against `schemas/registry.json`. Large daily datasets are
partitioned by time. The list is in [`13-parquet-lake/SKILL.md`](../.claude/skills/13-parquet-lake/SKILL.md).
