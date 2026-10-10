# NYSECalendar

Julia twin of the Rust [`calendar`](../../calendar/README.md) crate. It has the same rules,
override files and `add_trading_days` semantics
([09-lock-state-machine §4](../../.claude/skills/09-lock-state-machine/SKILL.md)). Julia uses it
for panel windows, horizons and schedules. The lock itself is decided in Rust.

This package is read-only: only Rust writes the runtime overrides file.

```julia
using NYSECalendar, Dates
cal = calendar_from_env()                       # needs QUANT_CONFIG and QUANT_STATE
add_trading_days(cal, Date(2026, 11, 23), 5)    # 2026-12-01 (Thanksgiving week)
```

Exports: `Calendar`, `Closure`, `CalendarError`, `load_calendar`, `calendar_from_env`,
`rules_only`, `rule_holidays`, `easter_sunday`, `is_trading_day`, `is_closure`, `holidays`,
`add_trading_days`, `next_trading_day`, `prev_trading_day`, `trading_days_between`.

Tests (`make julia-test`) cover:

- the golden fixtures;
- the cross-language holiday list, 1990-2100;
- agreement with the verified oracle;
- override validation and the environment variables.
