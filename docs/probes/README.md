# Probe Results

Results of the data-source probes P1-P8 (`.claude/skills/01-data-ingestion/BACKFILL.md` §2),
run at the start of phase I2 before any bulk fetch. Each probe gets one file with the date, the
exact requests, the observed behaviour and the decision it settles.

| Probes | File | Status |
|---|---|---|
| P1-P4 (EODHD, D15) | [`p1-p4-eodhd.md`](p1-p4-eodhd.md) | Run 2026-10-11; open item: EODHD publication time (rerun P4 on a weekday evening) |
| P5 (EDGAR bulk files) | [`p5-edgar.md`](p5-edgar.md) | Run 2026-10-11; open: nightly zip regeneration (HEAD on two consecutive days); shares outstanding for BRK-style issuers (owner decision) |
| P6 (ALFRED) | [`p6-alfred.md`](p6-alfred.md) | Run 2026-10-11; ICE BofA credit spreads deferred; BAA10Y is the substitute if ever needed (D16) |
