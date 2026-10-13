# Migration: `price_coverage` covers every security and carries the label inputs

- Date: 2026-10-12
- Author: nakiruu
- Type: additive (two appended nullable columns) and two columns made nullable
- Affected dataset: #34 `price_coverage`

## Change

Appends `listing_exchange` (utf8, nullable) and `later_price` (bool, nullable), the two
inputs `02-panel-construction/TARGETS.md` §3 reads besides `terminal_class`. Makes
`first_member_date` and `last_member_date` nullable: the dataset now has a row for every
security with prices, and one that was never an IWF/IWP member has no member dates
(`n_member_days = 0`). `terminal_class` gains the value `performance_delist`.

## Rationale

D20 trains on the broad universe, so the labels need a terminal record for every security,
not only for the members (the audit of 2026-10-12, P0 item 4). Until now the dataset held
members only, and the first builds wrote it empty (the only snapshot postdates the data end).

## Backfill

None needed: the dataset is derived and rewritten in full by every J9 build; it held 0 rows
before this change.

## Downstream impact

The survivorship query (`QUERY-PATTERNS.md`) sums member days; the new rows have none, so
its result does not change. No reader of the two member dates exists yet.
