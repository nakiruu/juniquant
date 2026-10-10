#!/usr/bin/env bash
# Verifies every registered dataset's schema against schemas/registry.json.
# Runs in CI (against a fixture lake), pre-commit, and daily on the host.
# No Python: DuckDB CLI + jq + sha256sum + bash.
#
# Canonical form (identical to generate-registry.jl): for ONE leaf file
#   DESCRIBE SELECT * FROM read_parquet('<leaf>', hive_partitioning = false)
# rows rendered "column_name:column_type", joined by "|" in column order,
# SHA-256 of that string.  For a partitioned dataset the FIRST and the LAST
# leaf (sorted path order) are checked; the monthly read-through job checks
# every leaf (UNRAID-STORAGE.md).  The "DESCRIBE inside a subquery" form needs
# a recent DuckDB (verify the version).
#
# Environment (IO-CONTRACT.md section 1):
#   QUANT_ROOT           required (lake = $QUANT_ROOT/lake)
#   QUANT_REGISTRY_PATH  default: schemas/registry.json (repository copy)
#   VERIFY_SCHEMAS_LAKE  optional override of the lake directory (tests)
#   VERIFY_SCHEMAS_SKILL optional path of 13-parquet-lake/SKILL.md (dataset-name check)
set -euo pipefail

: "${QUANT_ROOT:?QUANT_ROOT is not set (see 13-parquet-lake/IO-CONTRACT.md)}"
LAKE="${VERIFY_SCHEMAS_LAKE:-$QUANT_ROOT/lake}"
REGISTRY="${QUANT_REGISTRY_PATH:-schemas/registry.json}"
SKILL_MD="${VERIFY_SCHEMAS_SKILL:-.claude/skills/13-parquet-lake/SKILL.md}"

for tool in duckdb jq sha256sum; do
  command -v "$tool" >/dev/null 2>&1 || { echo "FAIL: $tool not found" >&2; exit 1; }
done
[[ -f "$REGISTRY" ]] || { echo "FAIL: registry not found at $REGISTRY" >&2; exit 1; }
[[ -d "$LAKE" ]]     || { echo "FAIL: lake directory not found at $LAKE" >&2; exit 1; }

failures=()
fail() { failures+=("$1"); }

sha_of() { printf '%s' "$1" | sha256sum | cut -d' ' -f1; }

# Canonical string and column lines of one leaf file.
describe_leaf() {
  local leaf="$1"
  [[ "$leaf" != *"'"* ]] || { echo "single quote in path: $leaf" >&2; return 1; }
  duckdb -noheader -list -c \
    "SELECT column_name || ':' || column_type FROM (DESCRIBE SELECT * FROM read_parquet('${leaf}', hive_partitioning = false))"
}

# 0. The registry is internally consistent: hash == sha256(columns joined by '|').
mapfile -t names < <(jq -r '.datasets | keys_unsorted[]' "$REGISTRY")
for name in "${names[@]}"; do
  expect_hash=$(jq -r --arg n "$name" '.datasets[$n].hash' "$REGISTRY")
  joined=$(jq -j --arg n "$name" '.datasets[$n].columns | join("|")' "$REGISTRY")
  [[ "$(sha_of "$joined")" == "$expect_hash" ]] \
    || fail "$name: registry hash does not match its own column list (hand-edited?)"
done

# 1. Every registered dataset exists and each checked leaf matches the registry.
for name in "${names[@]}"; do
  layout=$(jq -r --arg n "$name" '.datasets[$n].layout' "$REGISTRY")
  expect_hash=$(jq -r --arg n "$name" '.datasets[$n].hash' "$REGISTRY")
  leaves=()
  if [[ "$layout" == "single" ]]; then
    [[ -f "$LAKE/$name.parquet" ]] && leaves+=("$LAKE/$name.parquet")
  else
    if [[ -d "$LAKE/$name" ]]; then
      mapfile -t all < <(find "$LAKE/$name" -type f -name data.parquet | LC_ALL=C sort)
      if (( ${#all[@]} > 0 )); then
        leaves+=("${all[0]}")
        (( ${#all[@]} > 1 )) && leaves+=("${all[-1]}")
      fi
    fi
  fi
  if (( ${#leaves[@]} == 0 )); then
    fail "$name: registered but not found in the lake"
    continue
  fi
  for leaf in "${leaves[@]}"; do
    if ! actual_lines=$(describe_leaf "$leaf" 2>&1); then
      fail "$name: DESCRIBE failed for $leaf: $actual_lines"
      continue
    fi
    actual_joined=$(printf '%s\n' "$actual_lines" | paste -sd'|' -)
    actual_hash=$(sha_of "$actual_joined")
    if [[ "$actual_hash" != "$expect_hash" ]]; then
      fail "$name: schema hash mismatch at $leaf (expected ${expect_hash:0:12}, got ${actual_hash:0:12})"
      diff <(jq -r --arg n "$name" '.datasets[$n].columns[]' "$REGISTRY") \
           <(printf '%s\n' "$actual_lines") | sed 's/^/      /' >&2 || true
    fi
  done
done

# 2. Layout hygiene: partitioned datasets contain only data.parquet leaves.
for name in "${names[@]}"; do
  layout=$(jq -r --arg n "$name" '.datasets[$n].layout' "$REGISTRY")
  [[ "$layout" == "partitioned" && -d "$LAKE/$name" ]] || continue
  while IFS= read -r stray; do
    fail "$name: stray file $stray (a leaf must be named data.parquet)"
  done < <(find "$LAKE/$name" -type f -name '*.parquet' ! -name data.parquet)
done

# 3. Nothing in the lake is unregistered (a written dataset must be in the registry).
registered=" ${names[*]} "
shopt -s nullglob
for p in "$LAKE"/*; do
  base=$(basename "$p")
  [[ "$base" == .* ]] && continue
  dsname="${base%.parquet}"
  [[ "$registered" == *" $dsname "* ]] || fail "$dsname: present in the lake but not in the registry"
done
shopt -u nullglob

# 4. Registry names are a subset of the dataset table in SKILL.md section 3.
if [[ -f "$SKILL_MD" ]]; then
  mapfile -t table_names < <(grep -oP '^\| \d+ \| `\K[a-z0-9_]+' "$SKILL_MD")
  table=" ${table_names[*]} "
  for name in "${names[@]}"; do
    [[ "$table" == *" $name "* ]] || fail "$name: in the registry but not in SKILL.md section 3"
  done
fi

# 5. Warn about old temporary files (not a failure).
while IFS= read -r t; do
  echo "WARN: stale temporary file $t" >&2
done < <(find "$LAKE" -type f -name '.*.tmp.*' -mmin +1440 2>/dev/null || true)

if (( ${#failures[@]} > 0 )); then
  echo "SCHEMA VERIFICATION FAILED:" >&2
  for f in "${failures[@]}"; do echo "  $f" >&2; done
  exit 1
fi
echo "OK: ${#names[@]} schemas verified"
