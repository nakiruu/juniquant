# Schema Migrations

The lake's schemas are frozen once written. This file describes how to change
them without breaking downstream consumers.

## The Rule

Additive only. New columns append to the right. No renames. No retypes.
No reorders. If you need a different shape, write a new dataset.

## When You Need a Migration File

| Change | Migration file? | Registry change | Notes |
|---|---|---|---|
| Add a nullable column (appended) | **Yes** | hash changes | The common case |
| Deprecate / remove a column | **Yes** (two phases) | hash changes | Rare |
| Retype a column | **Yes** (four steps) | hash changes | Rare, avoid |
| Add a **new dataset** | **No** | new entry | `SCHEMAS.md` section + `SKILL.md` §3 row + registry entry, one commit |
| Change a writer's behaviour, not its schema | No | none | Document in the writer's skill |
| Change compression | No | none | |
| Change partitioning / leaf name / location | **Yes**, plus a rewrite of the data | layout fields change | Breaks hard-coded paths; the I/O contract forbids those |

So "adding a file needs a migration?" has one answer: no migration file,
but a registry entry and a `SCHEMAS.md` section. Earlier notes in
`SCHEMAS.md` that said "Migration required (new file; registry hash)" were
corrected to this.

**Before the first write of a dataset** its `SCHEMAS.md` entry may be
corrected in place (nothing exists yet that could break). Such corrections
are listed in `migrations/2026-10-10-initial.md`, which is the only
migration file that may describe several datasets. The 2026-10-10 rewrite of
#1-#5 and #20 and the appended datasets #28-#35 are of this kind.

## Adding a Column

One commit containing all four items (writer, migration file, `SCHEMAS.md`,
registry):

**1. Update the writer.**

```julia
# Before
df = select(df, :ticker, :date, :q_50)

# After
df = select(df, :ticker, :date, :q_50, :q_60)  # new column appended
```

**2. Add a migration file** `migrations/2026-11-02-add-q60-to-gspqr-predictions.md`:

    # Migration: add q_60 to gspqr_predictions

    - Date: 2026-11-02
    - Author: <name>
    - Type: additive
    - Affected dataset: gspqr_predictions

    ## Change
    Appends column `q_60` (float64, nullable) to gspqr_predictions.
    Contains the 60th-percentile prediction. Written by the same module.

    ## Rationale
    The optimizer wants a slightly-above-median quantile for a variant
    sizing rule. Existing median (`q_50`) is unchanged.

    ## Backfill
    Not backfilled. `q_60` is null for dates before 2026-11-02. Consumers
    must handle null.

    ## Downstream Impact
    None. Adding a nullable column does not break readers that ignore it.

**3. Update `SCHEMAS.md`.** Add the column to the table for that dataset,
marked "Appended".

**4. Regenerate the registry** (repository copy, `schemas/registry.json`):

```bash
julia --project=schemas --startup-file=no \
  schemas/generate-registry.jl --migration 2026-11-02-add-q60-to-gspqr-predictions
```

The generator takes the columns from `SCHEMAS.md` and writes a probe leaf
through the Rust writer (`SKILL.md` §6), so the new column must already be in
`SCHEMAS.md` (step 3). It sets `last_migration` only for datasets whose hash
changed.

## Adding a Dataset

One commit with four items (the same count as a column change, no migration
file):

1. `SCHEMAS.md`: the new section with columns, primary key, writer, layout.
2. `SKILL.md` §3: the new row (number appended, never renumbered), and §4 if
   partitioned.
3. `schemas/registry.json`: regenerated (the new entry).
4. The writer's skill, referring to the dataset.

## Deprecating a Column

Rare. Requires a two-phase rollout.

**Phase 1 (soft deprecate).** Mark the column as deprecated in `SCHEMAS.md`.
Stop writing new values (write null). Keep the column in the schema for 90
days so readers do not break.

**Phase 2 (remove).** Only after 90 days with no reads. This requires a
migration file and a PR that updates every reader.

Do not skip phase 1. A column that disappears overnight breaks every
downstream consumer.

## Retyping a Column

Not supported in place. The procedure:

1. Add a new column with the new type.
2. Backfill from the old column.
3. Update readers to use the new column.
4. Deprecate the old column (90 days).
5. Remove the old column.

Four migrations, spread over a quarter. If the change is urgent, the
problem is upstream: the original type was wrong. Consider whether the fix is
worth the disruption.

## What Breaks Consumers

| Change | Breaks readers? |
|---|---|
| Add nullable column | No |
| Add non-nullable column | Yes: readers get null errors |
| Remove column | Yes |
| Rename column | Yes |
| Retype column | Yes, if narrowing |
| Reorder columns | Yes, if position-based access |
| Change partitioning | Yes, if the path is hard-coded |
| Change compression | No |
| Change file location | Yes |

The safe zone is: add nullable columns and change compression. Everything
else requires consumer updates in the same commit.

## Verification

After any migration:

- [ ] `schemas/verify-schemas.sh` passes
- [ ] `schemas/registry.json` is regenerated and in the same commit
- [ ] The migration file exists and is complete
- [ ] `SCHEMAS.md` matches the actual files
- [ ] A test reads back the dataset and asserts the new schema
- [ ] No downstream consumer crashes on the new data
