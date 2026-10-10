# calendar

The NYSE trading calendar shared by every Rust component. The 5-trading-day lock, the
compliance audit and calendar-sync all count sessions with it. Specification:
[09-lock-state-machine §4](../.claude/skills/09-lock-state-machine/SKILL.md) and amendment 1.12.

- **Rules for any year:** New Year (a Saturday closes nothing, a Sunday moves to Monday), MLK,
  Washington's Birthday, Good Friday, Memorial Day, Juneteenth (from 2022), Independence Day,
  Labor Day, Thanksgiving and Christmas.
- **Overrides:** the seed `$QUANT_CONFIG/nyse_calendar_overrides.toml` (required) and the
  runtime `$QUANT_STATE/nyse_calendar_overrides.toml` (optional). They can only **add** closures.
  A malformed file is an error, never a fallback to the rules alone.
- **Writes:** `Calendar::add_closure` appends one entry to the runtime file. It holds a writer
  lock and writes to a temporary file, then renames it. It refuses weekends and empty names.

## API

| Item | Purpose |
|---|---|
| `Calendar::from_env`, `load`, `rules_only`, `from_closures` | Build a calendar |
| `is_trading_day`, `is_closure`, `holidays(year)` | Day queries |
| `add_trading_days(d, n)` | Roll `d` forward to a session, then advance `n` sessions; `add_trading_days(D, 5)` is the first permitted day of a lock placed on `D` |
| `next_trading_day`, `prev_trading_day` | Strictly after / before |
| `trading_days_between(from, to)` | Sessions in `(from, to]` |
| `rule_holidays(year)`, `easter_sunday(year)` | The rules alone |

Half days are sessions; this crate has no early-close times.

## Tests

`cargo test -p calendar` reproduces every case of
`.claude/skills/09-lock-state-machine/fixtures/nyse_calendar_cases.json` and the cross-language
holiday list `tests/fixtures/rule_holidays_1990_2100.json`, which the Julia `NYSECalendar`
package generates and also checks. It also runs property tests and override-file tests.
