#!/usr/bin/env bash
# Fast checks that catch the most common constitutional violations before
# a commit lands. Slow checks (schema hashes, full test suite) run in CI.
# Install: ln -sf ../../.claude/skills/00-construction/scripts/pre-commit-constitution.sh .git/hooks/pre-commit
#
# Checks (ENFORCEMENT.md section 4 has the same table):
#   0. no staged *.py file anywhere (the repository has no Python)
#   1. language boundaries: per-directory extension allow-lists (sources + manifests,
#      config and fixtures); Julia dirs, Rust dirs, TypeScript dirs
#   2. section 2: `period_end` only in ingestion code (never in Julia decision code)
#   3. section 3: no random splits outside the approved modules
#   4. section 6: market-order identifiers only in the tier-gated Rust modules; reserved order
#      types (stop, trailing, OCO, bracket, OTO, LOC) never referenced in execution sources
#   5. section 4: Julia never holds a Schwab token or touches the halt flag
set -euo pipefail

fail() { echo "CONSTITUTION VIOLATION: $1" >&2; exit 1; }

# Staged files that are added, copied, modified or renamed (a deletion never fails the hook).
mapfile -d '' STAGED < <(git diff --cached --name-only -z --diff-filter=ACMR)
(( ${#STAGED[@]} )) || { echo "Constitution pre-commit checks passed (nothing staged)."; exit 0; }

JULIA_DIRS='models|portfolio|regime|conviction|features|ledger|backtest|tools'
RUST_DIRS='execution|ingest|symbols|calendar|lake'   # shared crates: symbols/ (01 UNIVERSE.md §3), calendar/ (09 §4), lake/ (13 IO-CONTRACT); risk/ lives under execution/src/
TS_DIRS='frontend|api'

# --- 0. no Python anywhere ---------------------------------------------------
for f in "${STAGED[@]}"; do
  case "$f" in
    *.py|*.pyi|*.ipynb) fail "$f: Python is not part of this repository. Use Julia (analytics) or Rust (execution).";;
  esac
done

# --- 1. language boundaries: per-directory allow-lists -------------------------
# Each allow-list = sources + the manifests, config and fixtures that belong to the toolchain.
julia_ok() {   # $1 = path relative to repo root
  case "$1" in
    *.jl) return 0;;
    */Project.toml|*/Manifest.toml|*/JuliaManifest.toml|*/README.md|*/.gitkeep) return 0;;
    */test/fixtures/*.json|*/test/fixtures/*.csv|*/test/fixtures/*.toml|*/test/fixtures/*.arrow) return 0;;
  esac
  return 1
}
rust_ok() {
  case "$1" in
    *.rs) return 0;;
    */Cargo.toml|*/Cargo.lock|*/rust-toolchain.toml|*/README.md|*/.gitkeep) return 0;;
    */tests/fixtures/*.json|*/tests/fixtures/*.csv|*/tests/fixtures/*.toml|*/tests/fixtures/*.arrow) return 0;;
  esac
  return 1
}
ts_ok() {
  case "$1" in
    *.ts|*.tsx) return 0;;
    */package.json|*/bun.lockb|*/bun.lock|*/tsconfig.json|*/README.md|*/.gitkeep) return 0;;
    */tests/fixtures/*.json|*/tests/fixtures/*.csv) return 0;;
    */index.html|*.css) return 0;;   # a browser dashboard needs its entry page and styles
  esac
  return 1
}
for f in "${STAGED[@]}"; do
  top="${f%%/*}"
  [[ "$f" == */* ]] || continue            # a file at the repository root is not in a language dir
  if   [[ "$top" =~ ^($JULIA_DIRS)$ ]]; then julia_ok "$f" || fail "$f: not allowed in a Julia directory ($top/): expected .jl, Project.toml/Manifest.toml, README.md or test fixtures (section 4)"
  elif [[ "$top" =~ ^($RUST_DIRS)$  ]]; then rust_ok  "$f" || fail "$f: not allowed in a Rust directory ($top/): expected .rs, Cargo.toml/Cargo.lock, README.md or tests/fixtures (section 4)"
  elif [[ "$top" =~ ^($TS_DIRS)$    ]]; then ts_ok    "$f" || fail "$f: not allowed in a TypeScript directory ($top/): expected .ts/.tsx, package.json, bun.lockb, tsconfig.json, README.md, index.html, .css (section 4)"
  fi
done

# Helpers: staged source files by language (content checks scan code, never docs or this hook).
julia_src=(); rust_src=()
for f in "${STAGED[@]}"; do
  top="${f%%/*}"
  [[ "$f" == */* ]] || continue
  if [[ "$top" =~ ^($JULIA_DIRS)$ && "$f" == *.jl && "$f" != */test/* ]]; then julia_src+=("$f"); fi
  if [[ "$top" =~ ^($RUST_DIRS)$  && "$f" == *.rs ]]; then rust_src+=("$f"); fi
done
code_src=("${julia_src[@]}" "${rust_src[@]}")

# --- 2. section 2: period_end must not appear in Julia decision code -----------
if (( ${#julia_src[@]} )); then
  hits=$(grep -l 'period_end' "${julia_src[@]}" 2>/dev/null || true)
  [[ -z "$hits" ]] || { echo "$hits" >&2; fail "period_end used in decision code (section 2). Use filing_date."; }
fi

# --- 3. section 3: no random splits outside the approved modules ----------------
# Approved: features/src/splits.jl (chronological splits) and backtest/src/cpcv.jl (purged CV).
# Only code files are scanned, so documentation and this hook may name the identifiers.
if (( ${#code_src[@]} )); then
  split_bad=$(grep -lE 'train_test_split|KFold\(' "${code_src[@]}" 2>/dev/null \
              | grep -Ev '^(features/src/splits\.jl|backtest/src/cpcv\.jl)$' || true)
  [[ -z "$split_bad" ]] || { echo "$split_bad" >&2; fail "random split outside features/src/splits.jl or backtest/src/cpcv.jl (section 3)."; }
fi

# --- 4. section 6: market orders (amendment 1.2) and reserved order types -------
# The Market variant may be named only in the order builder, the tier/pre-trade check, the
# shadow engine, and tests. Everywhere else in execution/ a market-order reference is a violation.
MARKET_ALLOWED='^execution/src/(schwab/(orders|shadow)|risk/pre_trade)\.rs$|^execution/tests/'
exec_src=()
for f in "${rust_src[@]}"; do [[ "$f" == execution/* ]] && exec_src+=("$f"); done
if (( ${#exec_src[@]} )); then
  market_bad=$(printf '%s\n' "${exec_src[@]}" | grep -Ev "$MARKET_ALLOWED" \
    | xargs -r grep -lE 'OrderType::Market|MarketOrder|MarketPermit|"[Mm]arket"' 2>/dev/null || true)
  [[ -z "$market_bad" ]] || { echo "$market_bad" >&2; fail "market order reference outside the tier-gated modules (section 6)."; }
  # v1 order types are Limit and Market only. The reserved/unusable types are not referenced in
  # execution sources (tests excluded) until a written lock analysis enables one.
  reserved_bad=$(printf '%s\n' "${exec_src[@]}" | grep -v '^execution/tests/' \
    | xargs -r grep -lE 'BracketOrder|OtoOrder|OTO\b|StopOrder|StopLimitOrder|TrailingStopOrder|OcoOrder|OrderType::(Stop|StopLimit|TrailingStop|Oco|Bracket|Loc|LimitOnClose)' 2>/dev/null || true)
  [[ -z "$reserved_bad" ]] || { echo "$reserved_bad" >&2; fail "reserved order type referenced (section 6: v1 allows Limit and Market only)."; }
fi

# --- 5. section 4: Julia never holds a Schwab token or touches the halt flag -----
if (( ${#julia_src[@]} )); then
  jl_bad=$(grep -liE 'schwabapi\.com|schwab_refresh_token|schwab_access_token|TRADING_HALTED' "${julia_src[@]}" 2>/dev/null || true)
  [[ -z "$jl_bad" ]] || { echo "$jl_bad" >&2; fail "Julia code references the Schwab API, a token or the halt flag (section 4: Julia never places orders or halts trading)."; }
fi

echo "Constitution pre-commit checks passed."
