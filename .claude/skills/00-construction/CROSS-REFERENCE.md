# Invariant → Skill Cross-Reference

When an invariant changes, these skills must be updated in the same PR.
Every skill (00 to 18) appears in at least one row or in the reverse index; a
skill that appears in neither is unclassified: classify it or delete it.
"Real usage" below was checked against what each skill's text actually cites.

| Invariant (§)              | Implementing skills                                        |
|----------------------------|------------------------------------------------------------|
| §1 Five-day lock           | 09-lock-state-machine, 10-execution-schwab, 11-rust-event-bus, 07-fundamental-overlay, 08-portfolio-optimization, 13-parquet-lake, 14-backtest-protocol, 15-ai-dev-workflow, 16-portfolio-ledger (lock forensics only; never an input), 17-frontend (displays derived labels, never authorizes), 18-trade-summary |
| §2 Point-in-time           | 01-data-ingestion (`POINT-IN-TIME.md` incl. the `period_end` exception scope: `ingest/**`, `schemas/**`, `migrations/**`, `validate-filings.jl` if the hook is widened; `BACKFILL.md` raw landing with fetch timestamps; `UNIVERSE.md` holdings by as-of date), 02-panel-construction, 07-fundamental-overlay (`filing_date` of every fundamental datum), 13-parquet-lake |
| §3 No lookahead            | 01-data-ingestion (snapshots stamped at fetch; trailing-only eligibility, `UNIVERSE.md`; `QUALITY-GATES.md` before the panel; `SURVIVORSHIP.md` gap and caveat), 02-panel-construction, 03-gspqr-model, 04-tcp-calibration, 05-evt-tails, 14-backtest-protocol |
| §4 Language boundaries     | 10-execution-schwab (Rust only; Julia never holds a token, places an order or halts trading), 12-julia-rust-bridge, 15-ai-dev-workflow, 17-frontend (Bun/TS viewer, no tokens) |
| §5 Schema immutability     | 02-panel-construction (panel columns frozen), 10-execution-schwab (`orders` columns), 13-parquet-lake (`SKILL.md` §3 the 34 datasets; `IO-CONTRACT.md` paths, layout and write semantics, the single definition), 16-portfolio-ledger (ledger and tax-lot schemas, the §5/§8 update exception) |
| §6 Execution constraints (incl. restricted securities, `config/restricted.toml`) | 08-portfolio-optimization, 09-lock-state-machine, 10-execution-schwab, 11-rust-event-bus, 13-parquet-lake, 14-backtest-protocol, 15-ai-dev-workflow, 16-portfolio-ledger |
| §6a NAV tiers              | 08-portfolio-optimization, 10-execution-schwab, 14-backtest-protocol, 16-portfolio-ledger, 18-trade-summary (`tier_at_entry`) |
| §7 Veto vs. conviction     | 07-fundamental-overlay, 08-portfolio-optimization (held vetoed names: target 0 unless lock-bound), 18-trade-summary (veto-driven exits) |
| §8 Audit log               | 10-execution-schwab, 11-rust-event-bus, 16-portfolio-ledger (the ledger is not the audit log), 09-lock-state-machine (audit rows, halt flag) |
| §9 Output contracts        | 03-gspqr-model, 04-tcp-calibration, 05-evt-tails, 07-fundamental-overlay (`conviction`), 12-julia-rust-bridge |
| §10 Amendment process      | 15-ai-dev-workflow, 00-construction (AMENDMENTS.md) |

## Per-skill index (every skill, with the sections it cites)

| Skill | Constitution sections |
|---|---|
| 00-construction | all (defines them) |
| 01-data-ingestion | §2, §3 |
| 02-panel-construction | §2, §3, §5 |
| 03-gspqr-model | §2, §3, §9 |
| 04-tcp-calibration | §3, §9 |
| 05-evt-tails | §3, §9 |
| 06-regime-detection | none (see reverse index) |
| 07-fundamental-overlay | §1, §2, §7, §9 |
| 08-portfolio-optimization | §1, §6, §6a, §7 |
| 09-lock-state-machine | §1, §6, §8 |
| 10-execution-schwab | §1, §4, §5, §6, §6a, §8 |
| 11-rust-event-bus | §1, §8 |
| 12-julia-rust-bridge | §4, §9 |
| 13-parquet-lake | §1, §2, §5, §6 |
| 14-backtest-protocol | §1, §3, §6a |
| 15-ai-dev-workflow | §1, §4, §6, §10 (and all, as the routing skill) |
| 16-portfolio-ledger | §5, §6a, §8 (and §1 as non-input) |
| 17-frontend | §4 (viewer only; §1 and §8 as non-actions) |
| 18-trade-summary | §1 (`lock_overridden` reserved false), §6a, §7 |

## Reverse Index

Skills with no constitutional dependency (pure implementation detail):

- 06-regime-detection

Known tension to resolve in the constitution (recorded, not edited here): §4's
table lists "manual override" among the Bun/TS layer's responsibilities, while
`17-frontend` (and §1 rule 8, "no exceptions") forbid any manual override in the
frontend; and §7 says a vetoed stock "is not traded" while the lock-compliant
design sells a held vetoed name on its first permitted day (`08` §5b-2).

If a skill appears in neither table, it is unclassified. Classify it or
delete it.
