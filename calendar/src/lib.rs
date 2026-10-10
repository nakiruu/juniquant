//! NYSE trading calendar: holiday rules computed for any year, unioned with the overrides
//! seed (`$QUANT_CONFIG/nyse_calendar_overrides.toml`) and the runtime overrides
//! (`$QUANT_STATE/nyse_calendar_overrides.toml`).
//!
//! Specification: `.claude/skills/09-lock-state-machine/SKILL.md` §4 and constitution §1
//! rule 6 (amendment 1.12). This crate is the single calendar of the system: the 5-trading-day
//! opposite-side lock, the compliance audit and the panel windows all count sessions with it.
//! It reproduces `.claude/skills/09-lock-state-machine/fixtures/nyse_calendar_cases.json`
//! exactly, like the Julia oracle and the Julia package `NYSECalendar`.
//!
//! **Fail closed.** Overrides can only add closures, and every load error is an error (never
//! a silent fallback to the rules alone): a missed closure would make a lock block one session
//! too short, while an extra closure only makes it longer.
//!
//! ```
//! use calendar::Calendar;
//! use chrono::NaiveDate;
//!
//! # fn main() -> Result<(), calendar::CalendarError> {
//! let cal = Calendar::rules_only(); // production code uses Calendar::from_env()
//! let monday = NaiveDate::from_ymd_opt(2026, 11, 23).ok_or(calendar::CalendarError::OutOfRange {
//!     from: NaiveDate::MIN,
//! })?;
//! // Thanksgiving week: five sessions after Mon 23 Nov 2026 is Tue 1 Dec, not Mon 30 Nov.
//! assert_eq!(cal.add_trading_days(monday, 5)?.to_string(), "2026-12-01");
//! # Ok(())
//! # }
//! ```

mod calendar;
mod error;
mod overrides;
mod rules;
mod write;

pub use crate::calendar::{
    AddOutcome, Calendar, ENV_QUANT_CONFIG, ENV_QUANT_STATE, OVERRIDES_FILE_NAME, OverridePaths,
};
pub use crate::error::{CalendarError, EntryLocation, OverrideErrorKind, RefusalReason};
pub use crate::overrides::{
    Closure, ClosureOrigin, ClosureSource, NewClosure, load_overrides_file, parse_overrides,
};
pub use crate::rules::{
    HolidayRule, JUNETEENTH_FIRST_YEAR, RuleHoliday, easter_sunday, is_rule_holiday, is_weekend,
    rule_holidays,
};
