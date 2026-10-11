# Invariant Enforcement

Not every rule can be mechanically enforced. This file states, for each
invariant, whether it is caught by tooling, by tests, or only by review.
If an invariant is "review only," a human must check it before merge.

Code blocks here are sketches (not compiled, not normative); the crate files
named in `scaffold.md` are the real homes. There is no Python in this
repository: every check named here is a shell script, a Julia or Rust test, or
a CI step.

## Enforcement Classes

- **MECHANICAL** — a compiler, linter, CI check, or DB constraint rejects the
  violation. It cannot be merged.
- **TESTED** — a unit or integration test fails. Merge is blocked by CI.
- **REVIEW** — no automated check exists. Requires human sign-off.

---

## §1 Five-Day Lock — MECHANICAL + TESTED

The lock is a contractual obligation, so enforcement favours blocking over
anything else. The lock is a pure function of order history (amendment 1.4);
there is no stored lock state to enforce against.

1. **Rust permit gate (MECHANICAL).** `submit_buy` takes a `BuyPermit` and
   `submit_sell` takes a `SellPermit`, each with a private field, no
   `Clone`/`Copy`, bound to ticker and date, issued only by
   `lock::authorize_and_reserve` (which calls the pure verdict `lock::authorize`).
   `authorize` recomputes the block **from order history at submission time**
   (open orders block with no end date; a filled order blocks through 4 trading
   days after its last fill date; `REPLACED`/`CANCELED`/`REJECTED`/`EXPIRED` with
   zero fills are not counted) over Schwab orders for every linked account, the
   `transactions` fill net, the audit journal (merged with Schwab as in
   `09-lock-state-machine` §3.7) and the owner-maintained `lock_external.csv`.
   **A REST read of the accounts precedes every `authorize` (freshness bound
   `N = 0`)**; the stream is only a dirty trigger. It never reads a cached lock
   row. Mechanical: no submission path without a permit, no permit outside
   `authorize_and_reserve` (compile-fail test), and **fail closed**: if any
   history source is unreadable, stale, unparseable or short of the 21-day
   lookback, `authorize` denies. (One order with an unknown enum value whose
   symbol parses degrades to a block on that ticker only, `09` §3.6.)

   ```rust
   // SKETCH
   pub fn authorize(
       ticker: &Ticker, side: Side, today: NaiveDate, history: &OrderHistory,
       restricted: &RestrictedList, halt: &HaltState, shares_held: Decimal,
       cal: &TradingCalendar,
   ) -> Result<AuthorizedOrder, Denied>;           // pure verdict; no I/O
   // Denied: Halted, Restricted, Unmapped, NonTradingDay, NothingToSell,
   //         BlockedUntil, BlockedOpenOrder, HistoryIncomplete
   ```

   There is no override path to add a test for: none exists, and a grep
   test asserts no `force`/`override`/`bypass`/`skip` parameter on `submit_*`,
   `authorize` or `authorize_and_reserve`.

2. **The per-ticker gate and the halt flag (MECHANICAL + TESTED).** Check and
   send are atomic per ticker: one async mutex per canonical ticker is held
   across `halt check -> REST read -> authorize -> intent append -> send ->
   outcome append`; the generator, every ladder step (including each
   cancel/replace), and the pending-order release all enter through it, and the
   permit owns the guard and is written to the journal as an intent when it is
   issued (`09` §6a). `$QUANT_STATE/TRADING_HALTED` is read by `authorize`
   (`Denied::Halted`), by every `submit_*`, by the pending timer and by the ladder;
   an unreadable or unparseable flag halts. Only the owner clears a `VIOLATION`
   halt; a clean automatic re-run may clear an `INCOMPLETE`-only halt (`09` §8b).

   What types do not guarantee: that the history is complete (layers 3 and 4
   cover that), or that the owner kept `lock_external.csv` current (a human
   control; stated in every compliance report).

3. **Fail-closed history loading (TESTED).** Tests inject every failure
   (API error, timeout, truncated page, missing account, journal unreadable,
   external file missing or malformed, transactions read failing, halt flag
   unparseable) and assert both sides are denied; and that a single unknown enum
   value with a parseable symbol denies only its ticker.

4. **Daily post-close compliance audit (MECHANICAL + TESTED).** A Rust job at
   the 16:40 ET slot rescans all order history, including orders placed outside
   the system (manual trades in the Schwab app, external accounts), for
   violations of the rule. A violation halts trading (halt kind `VIOLATION`),
   writes a CRITICAL `lock_violation_detected` audit row, and a dated compliance
   report is written every trading day. **An outage is not a violation**: an
   unreadable history is retried for a bounded period, then reported as
   `INCOMPLETE` with the distinct event `lock_audit_incomplete` and an
   `INCOMPLETE` halt. Daily position reconciliation (Rust, 16:15) halts on a
   mismatch that survives its tolerance and retry.

**Tested.** The golden fixtures `09-lock-state-machine/fixtures/lock_cases.json`
and `nyse_calendar_cases.json` are the language-neutral contract: the Julia
oracles (`scripts/lock_rule_oracle.jl`, `scripts/nyse_calendar_oracle.jl`, run by
`scripts/run_fixtures.jl`) and the Rust tests `tests/lock_fixtures.rs` read the
same files and must both reproduce them. `tests/lock_rule.rs` adds property
tests of `authorize` against the oracle's verdict on random multi-account
histories, plus the table-driven worked examples, trading-day cases
(Thanksgiving week, holidays, an unplanned closure), timezone and DST cases,
cancelled, replaced and partially filled orders, late (GTC) fills and open
orders with no end date, `NothingToSell`, the journal merge, the symbol map, the
halt flag, the gate (concurrent opposite-side submissions) and the permit's
ticker/date binding. `tests/compliance_audit.rs` covers the audit and the report.

**Review only:** the owner's upkeep of `lock_external.csv` and the WARN
rows from `calendar-sync`. A CI test checks the computed NYSE calendar
against the published 2022-2027 lists.

## §2 Point-in-Time Integrity — MECHANICAL + REVIEW

Mechanical portion:

- A Julia helper `as_of_join(panel, fundamentals)` is the only sanctioned way
  to join price and fundamental data. It uses `filing_date <= date`.
- A linter rule (the pre-commit hook, `scripts/pre-commit-constitution.sh`, and
  a CI step) bans the identifier `period_end` in Julia decision code
  (`features/`, `models/`, `backtest/`, `portfolio/`, `conviction/`, `regime/`,
  `ledger/`; test files are not scanned). Ingestion code is Rust and may use it.
- **Exception scope** (`01-data-ingestion/POINT-IN-TIME.md`, "The `period_end`
  Exception"). The hook scans only the Julia decision directories above, so no
  exception list is needed today. If the hook is ever widened to "anywhere
  except ingestion", the allowed set is exactly `ingest/**`, `schemas/**`
  (registry tooling), `migrations/**` and
  `.claude/skills/01-data-ingestion/scripts/validate-filings.jl`; nothing under
  `features/`, `models/`, `backtest/`, `portfolio/`, `conviction/`, `regime/`,
  `ledger/`.
- The backfill and daily ingestion (`01-data-ingestion/BACKFILL.md`) keep every
  raw response with its fetch timestamp, and the quality gates
  (`01-data-ingestion/QUALITY-GATES.md`) must pass before the panel is built;
  the point-in-time universe (`01-data-ingestion/UNIVERSE.md`) is built from
  holdings snapshots by as-of date, never from current holdings. These are
  TESTED in `ingest/` and the Julia universe builder.

Review portion:

Whether a new data source provides point-in-time or restated data cannot
be checked mechanically. A human must confirm the vendor's semantics before
the source is added.

## §3 No Lookahead — MECHANICAL + TESTED

The train/test split function asserts chronological ordering at runtime and
raises if it detects overlap.

Backtests use a purged, embargoed K-fold implementation. Writing your own
split without using the approved modules fails the pre-commit hook and a CI
check that greps *code files* (`*.jl`, `*.rs`) for `train_test_split` and
`KFold(` outside the approved modules `features/src/splits.jl` (chronological
splits) and `backtest/src/cpcv.jl` (purged, embargoed CPCV). Documentation and
the hook itself are not scanned, so the pattern no longer matches them.

Target construction is isolated in `features/src/targets.jl`. No other file may
reference `shift(-N)` or forward indexing.

Survivorship (`01-data-ingestion/SURVIVORSHIP.md`): the price source
(EODHD, `DECISIONS.md` D15) does not cover every delisted name. The gap is measured per ticker (`price_coverage`), rows
flagged `survivorship_gap` are excluded from training and calibration, and
every model artifact and backtest report carries the survivorship caveat
(TESTED: a report without the caveat header fails, `14-backtest-protocol`).

Review portion:

Whether a feature encodes information that would not have been available
at decision time. Example: a "revenue surprise" feature computed from
revised data. No linter catches this.

## §4 Language Boundaries — MECHANICAL

The pre-commit hook and a CI job inspect staged paths against **per-directory
allow-lists** that include the manifests, config and fixtures each toolchain
needs (a rule that allowed only source extensions would reject `Project.toml`
or `Cargo.lock`):

| Path prefix | Allowed |
|---|---|
| `models/`, `portfolio/`, `regime/`, `conviction/`, `features/`, `ledger/`, `backtest/`, `tools/`, `julia/` (Julia; `tools/` holds repository checks, never decision code; `julia/` holds shared packages such as `NYSECalendar` and `LakeIO`) | `*.jl`; `Project.toml`, `Manifest.toml`, `JuliaManifest.toml`; `README.md`; `test/fixtures/*.{json,csv,toml,arrow}`; `.gitkeep` |
| `execution/` (incl. `execution/src/risk/`), `ingest/`, `symbols/`, `calendar/`, `lake/` (Rust; shared crates: `symbols/` canonical tickers, `01-data-ingestion/UNIVERSE.md` §3; `calendar/` NYSE calendar, `09-lock-state-machine` §4; `lake/` I/O contract, `13-parquet-lake/IO-CONTRACT.md`) | `*.rs`; `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`; `README.md`; `tests/fixtures/*.{json,csv,toml,arrow}`; `.gitkeep` |
| `frontend/`, `api/` (Bun/TypeScript) | `*.ts`, `*.tsx`; `package.json`, `bun.lockb`, `tsconfig.json`; `README.md`; `index.html`, `*.css`; `tests/fixtures/*.{json,csv}`; `.gitkeep` |
| anywhere | **no `*.py`, `*.pyi` or `*.ipynb`** (the repository has no Python) |

A `.rs` file in `models/` is a CI failure, and so is a `.jl` file in
`execution/`. No exceptions. Two content checks back the boundary: Julia code
(`.jl`, outside `test/`) may not reference `schwabapi.com`, a Schwab token file
or `TRADING_HALTED` (Julia never places orders, holds tokens or halts trading),
and Rust execution sources reference only the v1 order types (§6). The hook is
tested in a scratch repository (54 cases: allow-lists, the Python ban, the
approved split modules, market-order and reserved-type references, and that
the hook's own text and this file do not self-match).

## §5 Parquet Schema Immutability — MECHANICAL

The repository file `schemas/registry.json` stores a SHA-256 hash of each
dataset's schema, keyed by dataset name (35 datasets, `13-parquet-lake/SKILL.md`
§3; a dataset is a single file or a hive directory of `data.parquet` leaves,
`13-parquet-lake/IO-CONTRACT.md`). A CI job reads one leaf of every dataset,
hashes its schema in the canonical form, and compares to the registry. Mismatch without a corresponding migration file in
`migrations/` is a CI failure.

Adding a column: append to the schema, update the registry hash, add a
migration file. The migration must be additive-only, verified by a test that
asserts no column was removed or retyped.

## §6 Execution Constraints — MECHANICAL

Before any order is submitted, a Rust guard checks:

- The order type is in the **v1 allowlist `{Limit, Market}`**, and `Market` only
  with a `MarketPermit`. `StopLimit`, `TrailingStop`, `OCO` (same-side legs only),
  `LOC` are **reserved and disabled** until a lock analysis exists (a resting sell
  blocks buys; nothing produces them; they are not in the `OrderType` enum).
  `Bracket`, `OTO`, plain `Stop` and `MOC` are not supported and never built.
  A MarketOrder cannot be built without a MarketPermit (private field), and only
  `Tier::market_permit` issues one: never in the Standard tier, and in the
  Transition tier only for a fractional remainder. The tier is read from the
  trailing 5-day mean NAV at submission time.
- Resulting position weight ≤ 0.05, **for orders that increase the position**. A
  reduce-only sell (it does not take the holding below zero) is exempt from the
  position and sector caps: otherwise a name above the cap, for example one that
  drifted above 5% while a buy blocked its sale, could not be sold down.
- Resulting sector weight ≤ 0.25 (same reduce-only exemption).
- Resulting cash ≥ 0.02 of portfolio value (a sell only raises cash).
- Side is Buy or Sell. Short is not representable.

**Restricted securities (constitution §6, amendment 1.5) — MECHANICAL + TESTED.**
`config/restricted.toml` lists tickers the system never orders (initial
entry `ICE`, `rule = "never_trade"`). Enforced at: (a) the portfolio
eligibility filter, which gives a listed ticker weight 0 (Julia,
`08-portfolio-optimization`; the model panel and ranks are untouched). The
Julia loader **errors** if the `restricted` key is absent or the list is empty
and asserts that `ICE` is present with `rule = "never_trade"`; tickers are
normalized through the canonical symbol map (`09` §3.8); (b) the Rust pre-trade
check in `risk/pre_trade.rs` and again inside `lock::authorize`, which reject
the order and write a CRITICAL `restricted_security_blocked` audit row; an
unreadable or unparseable list rejects every order (fail closed); (c)
`tests/restricted.rs`, which fails if `ICE` is missing from the file or its rule
is not `never_trade`; (d) the daily audit, which flags **any** order (every
status, zero-fill included), fill or position in a restricted ticker in a Schwab
account or the journal (not the `External` file: Fidelity ICE activity is
irrelevant): halt, CRITICAL, **no auto-sell**. A restricted name found in the
account is never a forced-exit trigger. The Schwab account never holds ICE (the
owner's Fidelity account is the only holder), so NAV, tier and reconciliation
need no ICE-specific handling. Review only: adding or removing a list entry is
a commit approved by the account owner.

All of these are pre-submit runtime checks, each witnessed by a permit type
(`MarketPermit`, and `BuyPermit`/`SellPermit` for the lock, the halt flag and
the restricted list, §1), so no submission path exists without the check having
run. No order reaches Schwab without passing them.

## §7 Veto vs. Conviction — TESTED + REVIEW

Tested:

`tests/veto_conviction_separation.rs` (Julia: `portfolio/test/`) asserts that a
vetoed ticker is **not an optimizer variable** and receives no buy, and that its
final target is **0 unless lock-bound**: for a held vetoed name whose sale the
lock blocks today, the target is clamped to the current weight and the sell is
queued for the first permitted day (a veto never cancels a queued sell); a vetoed
name that is not held has no row. This holds regardless of conviction value.

`tests/conviction_bounds.rs` asserts the multiplier stays in [0.50, 1.25]
across 10,000 random composite z-scores.

Review portion:

Whether a new conviction component duplicates an existing GSPQR feature.
This requires judgment. The reviewer checks the component against the
12-feature list in skill 03-gspqr-model.

## §8 Audit Log — MECHANICAL

The system of record is a dedicated SQLite journal, `audit.db`
(`$QUANT_STATE/audit.db`, `13-parquet-lake/IO-CONTRACT.md` §1), with
`journal_mode=WAL` and `synchronous=FULL`. Parquet cannot be appended row by
row, so `audit_log` (hive-partitioned `date=YYYY-MM-DD/data.parquet`) is a
nightly export of closed days, not the write path.

Mechanical portion:

- Append-only is enforced by SQL triggers on `audit_events` and
  `audit_export` that `RAISE(ABORT)` on every UPDATE and DELETE. The
  writer checks at startup that the triggers exist and that WAL and
  FULL are in effect, and refuses to start otherwise. Only the writer
  opens the file read-write; all other readers use `mode=ro`.
- The Rust audit writer is the only component with write access. It
  exposes `append_durable(event)` and no update or delete. Julia-originated
  events (`veto_triggered`, `regime_flip`, `model_retrain_aborted`) go
  through an atomic-file spool directory that the Rust writer ingests; Julia
  never opens `audit.db`.
- `append_durable` returns only after the row is committed (one insert,
  group-committed behind one fsync, acked after `COMMIT`). If the append
  fails, the calling code returns an error and does not proceed. There is
  no fallback path.
- The audit record for an action is written before the action (intent) and
  again after it (outcome); see `11-rust-event-bus` §6. Startup scans the
  journal for intents with no outcome and blocks trading until each is
  resolved.
- The nightly export writes to a temp path, verifies row count and a
  content hash against the journal, then renames atomically. A day is
  marked in `audit_export` only after verification.

Tested portion: `tests/audit_journal.rs` (no truncation on restart, trigger
aborts, group commit, survives `kill -9`, spool idempotence, export
verification).

Not enforced by the triggers: DROP TABLE / DROP TRIGGER and replacing the
file. Mitigations are read-only mounts for everything but `quant-core`,
the startup trigger check, and the nightly `.backup` copy to the array.

## §9 Model Output Contracts — MECHANICAL

Julia is the sole writer of `gspqr_predictions` (training and the daily
inference both run in Julia, `03-gspqr-model/EXPORT.md` §5); `tcp_calibration`
and `evt_tail_bounds` are likewise Julia-written. Rust never runs the daily
inference and never calls Julia for it: it only **reads** these datasets
(in-process FFI, `12-julia-rust-bridge` RUSTCALL.md / JLRS.md, is a
later-phase option). Their schemas are registered and hash-checked (§5).

Rust reads prediction outputs into a struct with named fields, checked against
the registered schema at startup; renaming a field breaks compilation
downstream and a mismatch is a hard failure. No string-keyed maps. A grep test
asserts that only Julia code writes `gspqr_predictions`.

## §10 Amendment Process — REVIEW

Process-only. Enforced by convention:

Branch name must match `constitution/amendment-*`. A CI job checks this on
any PR touching `SKILL.md`.

CODEOWNERS requires two approvals for changes to `00-construction/` (the
owner has also merged amendments 1.3 to 1.6 on their own instruction, which
`AMENDMENTS.md` records).

## Summary Table

| § | Invariant | Class |
|---|---|---|
| 1 | Five-day lock (order-history rule, fail closed, gate, halt, daily audit) | MECHANICAL + TESTED |
| 2 | Point-in-time | MECHANICAL + REVIEW |
| 3 | No lookahead | MECHANICAL + TESTED |
| 4 | Language boundaries (per-directory allow-lists, no Python) | MECHANICAL |
| 5 | Schema immutability | MECHANICAL |
| 6 | Execution constraints (incl. restricted securities, order-type allowlist) | MECHANICAL |
| 7 | Veto vs. conviction | TESTED + REVIEW |
| 8 | Audit log | MECHANICAL |
| 9 | Output contracts | MECHANICAL |
| 10 | Amendment process | REVIEW |

Three invariants have a review component. Those are the ones to be careful
about in code review, because no tool will catch a mistake.
