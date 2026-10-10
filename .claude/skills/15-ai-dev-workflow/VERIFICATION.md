# Verification Protocol

Every task ends with this checklist. It is not a suggestion.

## Universal Checks (every change)

- [ ] I read the relevant layer skill this session
- [ ] I read the constitution this session
- [ ] I can name the layer this change belongs to
- [ ] The change does not violate any of the ten invariants
- [ ] If the change touches a Parquet schema, the schema registry is updated
- [ ] If the change touches an invariant, `AMENDMENTS.md` is updated
- [ ] A test exists and passes for the new behavior
- [ ] I ran `pre-commit-constitution.sh` and it passed (it also rejects any `*.py`)
- [ ] The commit message follows the commit conventions (`SKILL.md` §8)

## Layer-Specific Checks

### Ingestion (01)
- [ ] All fundamental data is stamped with `filing_date`, not `period_end`
- [ ] Rate limits are respected per source
- [ ] Failures write to `data_quality.parquet`
- [ ] Retry logic is bounded (no infinite loops)

### Panel (02)
- [ ] Cross-sectional ranks use only that date's data
- [ ] Rolling windows do not extend past the current date
- [ ] Forward returns are NaN where the horizon exceeds available data
- [ ] Z-score uses the 60-day trailing window, not expanding

### GSPQR (03)
- [ ] Training split is chronological
- [ ] Hyperparameters match the current skill spec
- [ ] Model is exported to `.txt`, not serialized in a language-specific format
- [ ] Feature matrix has exactly 12 columns in the documented order

### TCP (04)
- [ ] Conformity scores are zero-truncated
- [ ] Window parameters (`w`, `m`, `α`) match the skill spec
- [ ] Robbins-Monro offset is updated with decaying step size
- [ ] Calibration window is strictly prior to the forecast date

### EVT (05)
- [ ] GPD is fit to calibrated residuals, not raw returns
- [ ] Threshold is at the 95th percentile of residuals
- [ ] At least 50 exceedances are used per fit
- [ ] Separate fits for lower and upper tails

### Regime (06)
- [ ] HMM has exactly 3 states
- [ ] Confirmation rule (2 consecutive days) is applied before acting
- [ ] Regime factor maps to {1.0, 0.5, 0.0}

### Conviction (07)
- [ ] Veto is binary, conviction is bounded
- [ ] No valuation component (would double-count GSPQR's `sp_ratio`)
- [ ] Median/MAD standardization, not mean/std
- [ ] Multiplier clipped to [0.50, 1.25]

### Portfolio (08)
- [ ] Conviction applied after optimization, not before
- [ ] Weights renormalized with iterative cap redistribution
- [ ] Constraints: 5% single-name, 25% sector, 2% cash

### Lock and compliance (09)
- [ ] The lock is a pure function of order history, not a stored state; no
      code path answers it from a cached row or `position_locks`
- [ ] `authorize` agrees with the oracle verdict (property test),
      `tests/lock_fixtures.rs` passes on `09-lock-state-machine/fixtures/*.json`,
      and `julia --project scripts/run_fixtures.jl` passes in that skill
- [ ] 5 trading days from the computed NYSE calendar, not calendar days, not
      weekdays, not `prices.parquet` dates
- [ ] `D` is the America/New_York date of the order's last fill (its placement
      date if it filled the same day), rolled forward off non-trading days; an
      open order blocks with no end date
- [ ] Every account is in the history (all Schwab accounts, journal, and
      `lock_external.csv`); an unreadable, stale or malformed source blocks
- [ ] Cancelled/rejected zero-fill orders do not count; open and partially
      filled-then-cancelled orders do
- [ ] No exceptions: no override, bypass or veto-forced sale exists
- [ ] Pending orders re-authorize and re-validate on the first permitted day
      (`today >= first_permitted`); a veto does not cancel a queued sell
- [ ] The daily compliance audit and dated report are produced; a violation
      halts trading and writes CRITICAL
- [ ] Restricted list: `config/restricted.toml` contains `ICE`; the portfolio
      filter, the pre-trade check and the audit all enforce it

### Execution (10)

Later phase: these checks apply only when execution work is requested; the
current phase is data collection and GSPQR + TCP + EVT training
(`outline.md`, "Project phase and build order").
- [ ] No market orders outside the tier rules (`10-execution-schwab` §2a)
- [ ] Every submission takes a permit from `lock::authorize` and the restricted
      check ran first
- [ ] Reconciliation runs daily; a restricted position in a Schwab account
      halts trading and is not auto-sold
- [ ] Audit log written for every state change
- [ ] Every Schwab API fact used is labelled in `SCHWAB-API-REFERENCE.md`; no
      UNVERIFIED fact is treated as certain; the live tests before any live
      order (a later phase; `10-execution-schwab` §8) are recorded as passed
- [ ] Order history is REST-anchored (a REST read before every authorize, no
      cache; REST failure => deny); the stream never authorizes
- [ ] `TRADING_HALTED` is honoured by authorize, submit, the pending release and
      the ladder; only the owner clears a `VIOLATION` halt
- [ ] Fractional orders: market only, regular hours only, eligible names only,
      quantity floored to 2 decimals

### Event Bus (11)
- [ ] No polling where events belong
- [ ] Channels are bounded with explicit backpressure policy
- [ ] Handlers are idempotent

### FFI (12)
- [ ] Arrow IPC for array exchange, not JSON
- [ ] Memory ownership documented
- [ ] LightGBM handoff uses `.txt` format

### Parquet (13)
- [ ] Schema change is additive only
- [ ] Schema registry hash updated
- [ ] Migration file added if needed

### Backtest (14)
- [ ] Walk-forward with purged, embargoed folds
- [ ] HAC standard errors for overlapping horizons
- [ ] Coverage reported per regime, not just overall

## When a Check Fails

1. Do not weaken the check. Fix the code.
2. If the check is genuinely wrong, propose an amendment to the skill that
   defines it. Do not edit the check inline.
3. If you cannot fix the code, stop and report. Do not proceed with a
   known-failing check.

## What "Passes Locally" Means

The tests you ran actually exercised the changed code path. A test suite that
passes because it does not cover the change is not a pass. Confirm coverage
before claiming the task is done.