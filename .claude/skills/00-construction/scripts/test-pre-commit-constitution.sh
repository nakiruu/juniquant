#!/usr/bin/env bash
# Runs pre-commit-constitution.sh against a throwaway git repository (52 cases). Exit code = number of failures.
HOOK="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/pre-commit-constitution.sh"
W=$(mktemp -d); cd "$W"; git init -q . ; git config user.email t@t; git config user.name t
pass=0; failn=0
# expect <pass|fail> <description> <setup commands...>  (stages files then runs hook)
t() {
  local want="$1" desc="$2"; shift 2
  git reset -q --hard 2>/dev/null; git clean -fdq
  git rm -rq --cached . 2>/dev/null || true
  rm -rf ./* 2>/dev/null
  eval "$@"
  git add -A >/dev/null 2>&1
  if out=$(bash "$HOOK" 2>&1); then got=pass; else got=fail; fi
  if [[ "$got" == "$want" ]]; then pass=$((pass+1)); echo "ok   [$want] $desc"; else failn=$((failn+1)); echo "BAD  [want $want got $got] $desc"; echo "$out" | sed 's/^/       /'; fi
}
mkf() { mkdir -p "$(dirname "$1")"; printf '%s\n' "${2:-x}" > "$1"; }
# --- initial commit so deletions can be staged
mkf README.md hi; git add -A; git commit -qm init

t pass "nothing relevant staged (root file)"           'mkf NOTES.md hi'
t pass "julia src + Project.toml + Manifest + README"  'mkf models/src/QuantModels.jl "module M end"; mkf models/Project.toml a; mkf models/Manifest.toml a; mkf models/README.md a'
t pass "julia test fixture json"                       'mkf features/test/fixtures/a.json "{}"'
t pass "all seven julia dirs accept .jl"               'for d in models portfolio regime conviction features ledger backtest; do mkf $d/src/x.jl "1"; done'
t pass "rust src + Cargo.toml + Cargo.lock + fixture"  'mkf execution/src/lib.rs "fn main(){}"; mkf execution/Cargo.toml a; mkf execution/Cargo.lock a; mkf execution/tests/fixtures/x.json "{}"'
t pass "ingest rust crate"                             'mkf ingest/Cargo.toml a; mkf ingest/src/lib.rs "a"'
t pass "symbols rust crate"                            'mkf symbols/Cargo.toml a; mkf symbols/src/lib.rs "a"; mkf symbols/tests/fixtures/v.csv a'
t fail ".jl in symbols (rust dir)"                      'mkf symbols/src/x.jl a'
t pass "frontend ts + manifests"                       'mkf frontend/src/index.ts a; mkf frontend/package.json "{}"; mkf frontend/bun.lockb a; mkf frontend/tsconfig.json "{}"; mkf frontend/README.md a; mkf frontend/src/app.tsx a'
t pass "config/schemas/scripts/docs unconstrained"     'mkf config/restricted.toml a; mkf schemas/registry.json "{}"; mkf scripts/x.sh a; mkf docker/compose.yaml a; mkf .claude/skills/09/x.jl a; mkf .claude/skills/09/fixtures/a.json "{}"'

t fail "python file in scripts/"                       'mkf scripts/helper.py "print(1)"'
t fail "python file in models/"                        'mkf models/x.py "print(1)"'
t fail "python file in skill scripts"                  'mkf .claude/skills/09/scripts/ref.py "print(1)"'
t fail "notebook"                                      'mkf research/a.ipynb "{}"'
t pass "python file staged for DELETION passes"        'mkf scripts/old.py a; git add -A; git commit -qm addpy --no-verify; git rm -q scripts/old.py'

t fail ".rs in models/"                                'mkf models/src/x.rs a'
t fail ".rs in features/"                              'mkf features/x.rs a'
t fail ".jl in execution/"                             'mkf execution/src/x.jl a'
t fail ".txt in models/"                               'mkf models/notes.txt a'
t fail ".json (non-fixture) in portfolio/"             'mkf portfolio/out.json "{}"'
t fail "Cargo.toml in a julia dir"                     'mkf ledger/Cargo.toml a'
t fail "Project.toml in a rust dir"                    'mkf execution/Project.toml a'
t fail ".js in frontend"                               'mkf frontend/src/x.js a'
t fail ".jl in frontend"                               'mkf frontend/src/x.jl a'
t fail ".ts in ingest (rust dir)"                      'mkf ingest/x.ts a'
t fail ".rs in ledger (julia dir)"                     'mkf ledger/src/x.rs a'

t fail "train_test_split in a julia file"              'mkf features/src/other.jl "x = train_test_split(a)"'
t fail "KFold( in backtest/src/other.jl"               'mkf backtest/src/other.jl "KFold(5)"'
t fail "KFold( in rust"                                'mkf ingest/src/a.rs "KFold(5)"'
t pass "train_test_split allowed in features/src/splits.jl"  'mkf features/src/splits.jl "# train_test_split is forbidden elsewhere"'
t pass "KFold( allowed in backtest/src/cpcv.jl"        'mkf backtest/src/cpcv.jl "KFold(5)"'
t pass "pattern in this hook file itself does not self-match"  'mkf scripts/pre-commit-constitution.sh "grep -E train_test_split|KFold\\("; cp '"$HOOK"' scripts/hook-copy.sh'
t pass "pattern in ENFORCEMENT.md does not self-match" 'mkf .claude/skills/00-construction/ENFORCEMENT.md "grep train_test_split KFold( outside the approved module"'
t pass "real hook staged as scripts/ file"             'mkdir -p scripts; cp '"$HOOK"' scripts/pre-commit-constitution.sh'

t fail "period_end in features"                        'mkf features/src/x.jl "col = :period_end"'
t fail "period_end in conviction"                      'mkf conviction/src/x.jl "period_end"'
t fail "period_end in backtest"                        'mkf backtest/src/x.jl "period_end"'
t pass "period_end in ingest rust is fine"             'mkf ingest/src/edgar.rs "period_end"'
t pass "period_end in a test file is not scanned"      'mkf features/test/test_x.jl "period_end"'

t fail "MarketOrder outside allowed module"            'mkf execution/src/other.rs "let o = MarketOrder::new();"'
t fail "OrderType::Market in locks"                    'mkf execution/src/locks/x.rs "OrderType::Market"'
t pass "MarketOrder in schwab/orders.rs"               'mkf execution/src/schwab/orders.rs "MarketOrder MarketPermit"'
t pass "MarketPermit in risk/pre_trade.rs"             'mkf execution/src/risk/pre_trade.rs "MarketPermit"'
t pass "Market in execution/tests"                     'mkf execution/tests/guards.rs "MarketOrder"'
t fail "BracketOrder referenced in execution src"      'mkf execution/src/schwab/orders.rs "BracketOrder::builder()"'
t fail "OrderType::StopLimit referenced"               'mkf execution/src/schwab/client.rs "OrderType::StopLimit"'
t fail "OTO referenced"                                'mkf execution/src/other.rs "OTO"'
t pass "reserved types may be named in execution tests (negative tests)"  'mkf execution/tests/reserved.rs "OrderType::Bracket"'

t fail "julia references schwabapi"                    'mkf portfolio/src/x.jl "HTTP.get(\"https://api.schwabapi.com/x\")"'
t fail "julia touches TRADING_HALTED"                  'mkf ledger/src/x.jl "write(\"TRADING_HALTED\", \"x\")"'
t pass "rust halt module may name TRADING_HALTED"      'mkf execution/src/locks/halt.rs "const F = \"TRADING_HALTED\";"'
t pass "filenames with spaces (non-code doc)"          'mkf "docs/my file.md" a'
echo; echo "hook test: $pass passed, $failn failed"; exit $failn
