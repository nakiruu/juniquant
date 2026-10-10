#!/usr/bin/env bash
# Run at the start of every session from the repository root. Prints the state of the
# environment and exits non-zero if anything required for the current phase is missing.
# Optional tools for later phases only warn.
set -uo pipefail

failures=0
ok()   { echo "  [ok]   $1"; }
fail() { echo "  [FAIL] $1" >&2; failures=$((failures + 1)); }
warn() { echo "  [warn] $1"; }

echo "Session start checks (phase: see docs/INFRASTRUCTURE-PLAN.md and outline.md):"

# 1. Julia 1.12 and the workspace environment
if command -v julia >/dev/null 2>&1; then
  v=$(julia --startup-file=no -e 'print(VERSION >= v"1.12" ? "ok" : string(VERSION))' 2>/dev/null)
  if [[ "$v" == "ok" ]]; then ok "julia >= 1.12"; else fail "julia $v found, 1.12 required"; fi
  if julia --startup-file=no --project=tools -e 'using Pkg; Pkg.instantiate(); using JSON3' >/dev/null 2>&1; then
    ok "Julia workspace (tools) instantiates"
  else
    fail "Julia workspace does not instantiate: run make julia-instantiate"
  fi
else
  fail "julia not on PATH"
fi

# 2. Rust toolchain, rustfmt, clippy
command -v cargo >/dev/null 2>&1 && ok "cargo present" || fail "cargo not on PATH"
cargo fmt --version >/dev/null 2>&1 && ok "rustfmt present" || fail "rustfmt missing: sudo dnf install rustfmt"
cargo clippy --version >/dev/null 2>&1 && ok "clippy present" || fail "clippy missing: sudo dnf install clippy"

# 3. make (single entry point for the gates)
command -v make >/dev/null 2>&1 && ok "make present" || fail "make not on PATH"

# 4. Tools needed by later work (warn only)
command -v duckdb >/dev/null 2>&1 && ok "duckdb CLI present" || warn "duckdb CLI missing (needed from phase I1 W4: schema registry)"
command -v bun >/dev/null 2>&1 && ok "bun present" || warn "bun missing (frontend phase only)"

# 5. No Python in the repository (constitution section 4)
if [[ -n "$(git ls-files '*.py' '*.pyi' '*.ipynb' 2>/dev/null)" ]]; then
  fail "Python files are tracked in the repository"
else
  ok "no Python files tracked"
fi

# 6. Constitution pre-commit hook installed (symlink into the skill directory)
hook=".git/hooks/pre-commit"
if [[ -L "$hook" && "$(readlink "$hook")" == *pre-commit-constitution.sh ]]; then
  ok "constitution pre-commit hook installed"
else
  fail "constitution pre-commit hook not installed: run make hook-install"
fi

# 7. Last commit message does not mention skipping checks
if git log -1 --pretty=%B 2>/dev/null | grep -qiE 'fix.*later|skip.*check|disable.*lint|no-verify'; then
  warn "last commit message mentions skipping a check"
fi

# 8. Schema registry (created in phase I1 W4)
[[ -f schemas/registry.json ]] && ok "schema registry present" || warn "schemas/registry.json not created yet (phase I1 W4)"

# 9. Working tree
if [[ -z "$(git status --porcelain 2>/dev/null)" ]]; then
  ok "working tree clean"
else
  echo "  [note] working tree has uncommitted changes:"
  git status --porcelain | head -5 | sed 's/^/         /'
fi

echo
echo "Next: read .claude/skills/00-construction/SKILL.md, then the skill for your layer;"
echo "run 'make check' before claiming anything is done."
exit $(( failures > 0 ))
