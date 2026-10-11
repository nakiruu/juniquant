# Tool Routing

Given a task, which tool. The rule is one tool per job. Do not chain tools
when one is sufficient.

## Julia MCP

**Use for:**
- Running Julia code to test a hypothesis ("does this formula produce NaN?")
- Loading a package to check its API ("what does `Distributions.GeneralizedPareto` take?")
- Executing a test file after editing it
- Exploring a new dataset loaded via DataFrames
- Checking type inference on a suspicious function

**Do not use for:**
- Editing files (use the editor)
- Running long training jobs (use a background bash job)
- Anything that writes to `.parquet` outside the pipeline (would bypass schema checks)

**Example invocation:** when testing a new feature formula in `features/`,
run it in the MCP against a sample of the panel to confirm the output shape
and value range before committing.

## DuckDB MCP

**Use for:**
- Inspecting a Parquet schema (`DESCRIBE SELECT * FROM 'file.parquet'`)
- Sampling rows to check data quality
- Row counts and date ranges
- Verifying a join produces expected cardinality
- Ad-hoc queries during debugging

**Do not use for:**
- Production queries (those live in Julia or Rust code)
- Modifying data (DuckDB reads Parquet; it does not write to our lake)
- Anything that would be better as a test in `tests/`

**Example invocation:** after writing a new ingestion script, use DuckDB to
confirm the output Parquet has the expected columns and date coverage before
running the full pipeline.

## Bash

**Use for:**
- `cargo build`, `cargo test`, `cargo clippy`
- `julia --project -e 'using Pkg; Pkg.test()'`
- `bun test`, `bun run build`
- `git` operations
- `pre-commit-constitution.sh`

**Do not use for:**
- `curl`-ing APIs manually (that is what ingestion code is for)
- Ad-hoc data manipulation (write a script)
- Anything destructive on `.parquet` files

## Editor (Read/Edit)

**Use for:**
- All file reading
- All file editing
- Reading skills before acting

**Rule:** before editing any file, read it in full. Do not edit based on
memory of what it "probably" contains.

## Web Fetch

**Use for:**
- Checking current API docs (Schwab, SEC EDGAR, EODHD, Yahoo)
- Looking up a library version
- Reading arXiv for a method referenced in a skill

**Do not use for:**
- Anything covered by a skill (skills are the source of truth)
- Financial data (goes through ingestion, not web fetch)

## Decision Tree

    Is it a file edit?           → Editor
    Is it running Julia code?    → Julia MCP
    Is it inspecting Parquet?    → DuckDB MCP
    Is it running tests/builds?  → Bash
    Is it looking up docs?       → Web Fetch
    Is it a real trade?          → STOP. That goes through the execution
                                    system, not through Claude directly.

## Forbidden Actions

- Never place an order via any tool. Orders go through the Rust execution
  service with all its guards. Claude does not touch Schwab.
- Never modify `.parquet` files directly. All writes go through the pipeline.
- Never `git push --force` on a shared branch.
- Never edit `.claude/skills/00-construction/SKILL.md` without following the
  amendment process (§10 of the constitution).