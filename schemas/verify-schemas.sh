#!/usr/bin/env bash
# Verifies lake files against schemas/registry.json (13-parquet-lake SKILL.md §6).
# No Python: DuckDB CLI + jq + sha256sum + bash.
#
#   verify-schemas.sh              lake mode: $VERIFY_SCHEMAS_LAKE, else $QUANT_ROOT/lake
#   verify-schemas.sh --self-test  CI: probe leaves from the real writer must pass, a swapped
#                                  leaf must fail (needs cargo)
#
# Canonical form (identical to generate-registry.jl): for ONE leaf file
#   DESCRIBE SELECT * FROM read_parquet('<leaf>', hive_partitioning = false)
# rows rendered "column_name:column_type", joined by "|" in column order, SHA-256 of that
# string. For a partitioned dataset the first and last leaf (sorted path order) are checked.
# The hash does not see nullability; the writer enforces it from the registry `fields`.
#
# Environment (IO-CONTRACT.md §1): QUANT_ROOT (lake mode, unless VERIFY_SCHEMAS_LAKE is set),
# QUANT_REGISTRY_PATH (default schemas/registry.json).
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REGISTRY="${QUANT_REGISTRY_PATH:-$REPO/schemas/registry.json}"
SKILL_MD="$REPO/.claude/skills/13-parquet-lake/SKILL.md"

for tool in duckdb jq sha256sum; do
  command -v "$tool" >/dev/null 2>&1 || { echo "FAIL: $tool not found" >&2; exit 1; }
done
[[ -f "$REGISTRY" ]] || { echo "FAIL: registry not found at $REGISTRY" >&2; exit 1; }

sha_of() { printf '%s' "$1" | sha256sum | cut -d' ' -f1; }

describe_leaf() {
  local leaf="$1"
  [[ "$leaf" != *"'"* ]] || { echo "single quote in path: $leaf" >&2; return 1; }
  duckdb -noheader -list -c \
    "SELECT column_name || ':' || column_type FROM (DESCRIBE SELECT * FROM read_parquet('${leaf}', hive_partitioning = false))"
}

# verify_lake <lake-dir>: prints failures, returns 1 if any.
verify_lake() {
  local LAKE="$1" failures=() written=0
  [[ -d "$LAKE" ]] || { echo "FAIL: lake directory not found at $LAKE" >&2; return 1; }
  fail() { failures+=("$1"); }

  mapfile -t names < <(jq -r '.datasets | keys_unsorted[]' "$REGISTRY")

  # 0. Registry self-consistency: hash == sha256(columns joined by '|').
  for name in "${names[@]}"; do
    expect_hash=$(jq -r --arg n "$name" '.datasets[$n].hash' "$REGISTRY")
    joined=$(jq -j --arg n "$name" '.datasets[$n].columns | join("|")' "$REGISTRY")
    [[ "$(sha_of "$joined")" == "$expect_hash" ]] \
      || fail "$name: registry hash does not match its own column list (hand-edited?)"
  done

  # 1. Each written dataset's first and last leaf match the registry (unwritten is fine).
  for name in "${names[@]}"; do
    layout=$(jq -r --arg n "$name" '.datasets[$n].layout' "$REGISTRY")
    expect_hash=$(jq -r --arg n "$name" '.datasets[$n].hash' "$REGISTRY")
    leaves=()
    if [[ "$layout" == "single" ]]; then
      [[ -f "$LAKE/$name.parquet" ]] && leaves+=("$LAKE/$name.parquet")
    elif [[ -d "$LAKE/$name" ]]; then
      mapfile -t all < <(find "$LAKE/$name" -type f -name data.parquet | LC_ALL=C sort)
      if (( ${#all[@]} > 0 )); then
        leaves+=("${all[0]}")
        (( ${#all[@]} > 1 )) && leaves+=("${all[-1]}")
      fi
    fi
    (( ${#leaves[@]} > 0 )) || continue
    written=$((written + 1))
    for leaf in "${leaves[@]}"; do
      if ! actual_lines=$(describe_leaf "$leaf" 2>&1); then
        fail "$name: DESCRIBE failed for $leaf: $actual_lines"
        continue
      fi
      actual_hash=$(sha_of "$(printf '%s\n' "$actual_lines" | paste -sd'|' -)")
      if [[ "$actual_hash" != "$expect_hash" ]]; then
        fail "$name: schema hash mismatch at $leaf (expected ${expect_hash:0:12}, got ${actual_hash:0:12})"
        diff <(jq -r --arg n "$name" '.datasets[$n].columns[]' "$REGISTRY") \
             <(printf '%s\n' "$actual_lines") | sed 's/^/      /' >&2 || true
      fi
    done
  done

  # 2. Partitioned datasets hold only data.parquet leaves.
  for name in "${names[@]}"; do
    layout=$(jq -r --arg n "$name" '.datasets[$n].layout' "$REGISTRY")
    [[ "$layout" == "partitioned" && -d "$LAKE/$name" ]] || continue
    while IFS= read -r stray; do
      fail "$name: stray file $stray (a leaf must be named data.parquet)"
    done < <(find "$LAKE/$name" -type f -name '*.parquet' ! -name data.parquet)
  done

  # 3. Nothing in the lake is unregistered.
  local registered=" ${names[*]} "
  shopt -s nullglob
  for p in "$LAKE"/*; do
    base=$(basename "$p")
    dsname="${base%.parquet}"
    [[ "$registered" == *" $dsname "* ]] || fail "$dsname: present in the lake but not in the registry"
  done
  shopt -u nullglob

  # 4. The registry and the SKILL.md §3 table name the same datasets.
  mapfile -t table_names < <(grep -oP '^\| \d+ \| `\K[a-z0-9_]+' "$SKILL_MD")
  [[ "$(printf '%s\n' "${names[@]}" | LC_ALL=C sort)" == "$(printf '%s\n' "${table_names[@]}" | LC_ALL=C sort)" ]] \
    || fail "registry dataset names differ from the SKILL.md §3 table"

  # 5. Warn about old temporary files (the writer deletes those older than an hour).
  while IFS= read -r t; do
    echo "WARN: stale temporary file $t" >&2
  done < <(find "$LAKE" -type f -name '.*.tmp.*' -mmin +1440 2>/dev/null || true)

  if (( ${#failures[@]} > 0 )); then
    echo "SCHEMA VERIFICATION FAILED ($LAKE):" >&2
    for f in "${failures[@]}"; do echo "  $f" >&2; done
    return 1
  fi
  echo "OK: ${#names[@]} schemas registered, $written written datasets verified in $LAKE"
}

if [[ "${1:-}" == "--self-test" ]]; then
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
  (cd "$REPO" && cargo run --quiet --release -p lake --bin lake-schema-probe -- "$REGISTRY" "$tmp/lake" "$tmp/locks")
  verify_lake "$tmp/lake" || { echo "SELF-TEST FAILED: probe leaves do not verify" >&2; exit 1; }
  cp "$tmp/lake/prices/year=1970/month=01/data.parquet" "$tmp/lake/splits.parquet"
  if verify_lake "$tmp/lake" 2>/dev/null; then
    echo "SELF-TEST FAILED: a swapped leaf was not detected" >&2; exit 1
  fi
  echo "OK: self-test (probe leaves verify, a swapped leaf fails)"
else
  [[ -n "${VERIFY_SCHEMAS_LAKE:-}" ]] || : "${QUANT_ROOT:?QUANT_ROOT is not set (see 13-parquet-lake/IO-CONTRACT.md)}"
  verify_lake "${VERIFY_SCHEMAS_LAKE:-$QUANT_ROOT/lake}"
fi
