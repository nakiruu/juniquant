# symbols

The one definition of a canonical ticker, shared by `ingest/` and `execution/`.
Specification: [UNIVERSE.md §3](../.claude/skills/01-data-ingestion/UNIVERSE.md) (grammar,
normalization, vendor rendering, the superseded `SYMBOL~CIK10` form) and
[09-lock-state-machine §3.8](../.claude/skills/09-lock-state-machine/SKILL.md) (the canonical
symbol map, one lock group per issuer, fail closed). Julia does not reimplement any of this;
it reads canonical tickers from the lake.

## API

| Item | Purpose |
|---|---|
| `Canon` | Validated canonical ticker `[A-Z0-9]{1,6} ( "." [A-Z] )?`; `base()`, `class()`, strict `FromStr`, serde as a validated string |
| `normalize(raw)` | Raw vendor input to `Canon`; errors `non_ascii`, `empty`, `superseded`, `unparseable` (checked in that order) |
| `is_tradable(s)` | True iff `s` is already exactly canonical; rejects `SYMBOL~CIK10` at order entry |
| `Cik` | SEC CIK, 1 to 10 digits in, ten-digit zero-padded out |
| `SupersededTicker` | `SYMBOL~CIK10`; renders and parses, never converts to `Canon` |
| `Vendor`, `render`, `render_str` | Canonical to vendor spelling; Yahoo/SEC hyphenate the class, Schwab refuses class shares (`unverified_vendor`), iShares is `input_only`, non-canonical input is `not_canonical` |
| `SymbolMap` | `config/symbol_map.toml`, validated at load: duplicate canonical, duplicate vendor spelling, alias collisions, bad dates, empty lock groups are typed `MapError`s; `validate_issuers` enforces one lock group per CIK (B1); `resolve`, `resolve_alias`, `resolve_on`, `lock_group`, `group_members` |

An unmapped spelling resolves to `None`, never to a guess.

## Tests

`cargo test -p symbols` reproduces every row of `schemas/fixtures/symbol_normalization.csv`
and `schemas/fixtures/symbol_render.csv` (the fixtures are the authority), runs property
tests (`proptest`) on normalization and rendering, and covers each `SymbolMap` load failure.
