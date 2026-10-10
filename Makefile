# Single entry point for every quality gate (docs/INFRASTRUCTURE-PLAN.md §2).
# `make check` is the local gate and what CI runs. Each target fails loudly if its tool is missing.

JULIA ?= julia --startup-file=no --project=tools
SKILLS := .claude/skills

.PHONY: check rust julia rust-fmt rust-clippy rust-test rust-doc julia-instantiate julia-parse fixtures julia-test md-check hook-test hook-install fmt schemas doc help

help:
	@echo "make check        all gates (rust + julia + docs + constitution hook)"
	@echo "make rust         rustfmt check, clippy -D warnings, tests, rustdoc"
	@echo "make julia        parse check, golden lock/calendar fixtures, Julia package tests"
	@echo "make md-check     markdown fences and tables"
	@echo "make hook-test    constitution pre-commit hook test suite"
	@echo "make hook-install install the constitution hook into .git/hooks"
	@echo "make fmt          format Rust code"
	@echo "make schemas      verify lake schemas against schemas/registry.json (needs duckdb)"

check: rust julia md-check hook-test
	@echo "ALL CHECKS PASSED"

rust: rust-fmt rust-clippy rust-test rust-doc
julia: julia-instantiate julia-parse fixtures julia-test

rust-fmt:
	@cargo fmt --version >/dev/null 2>&1 || { echo "rustfmt missing: sudo dnf install rustfmt" >&2; exit 1; }
	cargo fmt --all --check

rust-clippy:
	@cargo clippy --version >/dev/null 2>&1 || { echo "clippy missing: sudo dnf install clippy" >&2; exit 1; }
	cargo clippy --workspace --all-targets -- -D warnings

rust-test:
	cargo test --workspace

rust-doc:
	RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps

julia-instantiate:
	@command -v julia >/dev/null 2>&1 || { echo "julia missing (1.12 required)" >&2; exit 1; }
	$(JULIA) -e "using Pkg; Pkg.instantiate()"

julia-parse:
	$(JULIA) tools/parsecheck.jl

fixtures:
	$(JULIA) $(SKILLS)/09-lock-state-machine/scripts/run_fixtures.jl

# Pkg.test for every shared Julia package under julia/.
JULIA_PACKAGES := $(wildcard julia/*/Project.toml)
julia-test:
	@for p in $(dir $(JULIA_PACKAGES)); do [ -f $${p}test/runtests.jl ] || { echo "SKIP $$p (no test/runtests.jl)"; continue; }; echo "Pkg.test $$p"; julia --startup-file=no --project=$$p -e "using Pkg; Pkg.test()" || exit 1; done

md-check:
	$(JULIA) tools/mdcheck.jl
	$(JULIA) tools/linkcheck.jl

hook-test:
	bash $(SKILLS)/00-construction/scripts/test-pre-commit-constitution.sh

hook-install:
	ln -sf ../../$(SKILLS)/00-construction/scripts/pre-commit-constitution.sh .git/hooks/pre-commit
	@echo "constitution pre-commit hook installed"

fmt:
	cargo fmt --all

schemas:
	@command -v duckdb >/dev/null 2>&1 || { echo "duckdb CLI missing: install it (docs/development.md)" >&2; exit 1; }
	bash $(SKILLS)/13-parquet-lake/scripts/verify-schemas.sh

doc:
	cargo doc --workspace --no-deps
