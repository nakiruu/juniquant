//! The [`Calendar`]: rules unioned with the seed and runtime overrides.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use chrono::{Datelike, NaiveDate};

use crate::error::{CalendarError, RefusalReason};
use crate::overrides::{Closure, ClosureOrigin, NewClosure, load_overrides_file, parse_overrides};
use crate::rules::{is_rule_holiday, is_weekend, rule_holidays};
use crate::write::{RuntimeLock, read_optional, write_atomic};

/// File name of both overrides files (seed under `$QUANT_CONFIG`, runtime under
/// `$QUANT_STATE`), IO-CONTRACT §1.
pub const OVERRIDES_FILE_NAME: &str = "nyse_calendar_overrides.toml";

/// Environment variable naming the repository's `config/` directory.
pub const ENV_QUANT_CONFIG: &str = "QUANT_CONFIG";

/// Environment variable naming the mutable service-state directory.
pub const ENV_QUANT_STATE: &str = "QUANT_STATE";

/// The locations of the two overrides files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverridePaths {
    /// The checked-in seed, `$QUANT_CONFIG/nyse_calendar_overrides.toml` (must exist).
    pub seed: PathBuf,
    /// The runtime file, `$QUANT_STATE/nyse_calendar_overrides.toml` (may be absent).
    pub runtime: PathBuf,
}

impl OverridePaths {
    /// Derives both paths from `QUANT_CONFIG` and `QUANT_STATE`.
    ///
    /// # Errors
    ///
    /// [`CalendarError::MissingEnvVar`] naming the first variable that is unset or empty.
    pub fn from_env() -> Result<Self, CalendarError> {
        Self::from_env_lookup(|name| std::env::var_os(name))
    }

    /// As [`OverridePaths::from_env`], reading variables through `lookup` (tests, embedding).
    ///
    /// # Errors
    ///
    /// [`CalendarError::MissingEnvVar`] naming the first variable that is unset or empty.
    pub fn from_env_lookup(
        lookup: impl Fn(&str) -> Option<OsString>,
    ) -> Result<Self, CalendarError> {
        let dir = |var: &'static str| match lookup(var) {
            Some(v) if !v.is_empty() => Ok(PathBuf::from(v)),
            _ => Err(CalendarError::MissingEnvVar { var }),
        };
        let config = dir(ENV_QUANT_CONFIG)?;
        let state = dir(ENV_QUANT_STATE)?;
        Ok(Self {
            seed: config.join(OVERRIDES_FILE_NAME),
            runtime: state.join(OVERRIDES_FILE_NAME),
        })
    }
}

/// Result of [`Calendar::add_closure`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddOutcome {
    /// The closure was appended to the runtime file and is now in the calendar.
    Added,
    /// The date was already closed (rule holiday, seed or runtime override); nothing was
    /// written. The calendar is unchanged except that runtime entries written by another
    /// process since load are now included.
    AlreadyClosed,
}

/// The NYSE trading calendar: rule holidays (any year) unioned with the override closures.
///
/// This is the single calendar of the system (09 SKILL.md §4). A trading day ("session") is
/// a weekday that is neither a rule holiday nor an override closure. Half days are sessions;
/// there is no early-close data here (session times come from the market-hours data).
///
/// Overrides can only add closures. Treating an extra day as closed can only make
/// `add_trading_days(d, n)` later, never earlier, which is the safe direction for the
/// 5-trading-day lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Calendar {
    closures: Vec<Closure>,
    closed: BTreeSet<NaiveDate>,
}

impl Calendar {
    /// The rules alone, with no overrides. Not a production calendar (it lacks the seed
    /// closures); meant for tests and comparisons.
    #[must_use]
    pub fn rules_only() -> Self {
        Self {
            closures: Vec::new(),
            closed: BTreeSet::new(),
        }
    }

    /// The rules plus the given closures.
    ///
    /// # Errors
    ///
    /// [`CalendarError::InvalidClosure`] for a weekend date or an empty name.
    pub fn from_closures(
        closures: impl IntoIterator<Item = Closure>,
    ) -> Result<Self, CalendarError> {
        let mut cal = Self::rules_only();
        for c in closures {
            c.validate()
                .map_err(|kind| CalendarError::InvalidClosure { date: c.date, kind })?;
            cal.insert(c);
        }
        Ok(cal)
    }

    /// Loads the production calendar from explicit paths: the rules, the seed file (which
    /// must exist) and the runtime file (absent means no runtime closures).
    ///
    /// # Errors
    ///
    /// [`CalendarError::Read`] if the seed is missing or either file is unreadable;
    /// [`CalendarError::Toml`] / [`CalendarError::InvalidOverride`] for a malformed file.
    pub fn load(seed: &Path, runtime: &Path) -> Result<Self, CalendarError> {
        let mut cal = Self::rules_only();
        for c in load_overrides_file(seed, ClosureOrigin::Seed)? {
            cal.insert(c);
        }
        if let Some(text) = read_optional(runtime)? {
            for c in parse_overrides(&text, runtime, ClosureOrigin::Runtime)? {
                cal.insert(c);
            }
        }
        Ok(cal)
    }

    /// Loads the production calendar from [`OverridePaths`].
    ///
    /// # Errors
    ///
    /// As [`Calendar::load`].
    pub fn load_paths(paths: &OverridePaths) -> Result<Self, CalendarError> {
        Self::load(&paths.seed, &paths.runtime)
    }

    /// Loads the production calendar from `$QUANT_CONFIG` and `$QUANT_STATE`.
    ///
    /// # Errors
    ///
    /// [`CalendarError::MissingEnvVar`] if either variable is unset or empty, otherwise as
    /// [`Calendar::load`].
    pub fn from_env() -> Result<Self, CalendarError> {
        Self::load_paths(&OverridePaths::from_env()?)
    }

    fn insert(&mut self, closure: Closure) {
        self.closed.insert(closure.date);
        self.closures.push(closure);
    }

    /// Every override closure, seed first, then runtime, in file order (a date listed in
    /// both files appears twice).
    #[must_use]
    pub fn overrides(&self) -> &[Closure] {
        &self.closures
    }

    /// `true` when `date` is a weekday on which the exchange is closed (rule holiday or
    /// override). Weekends are not "closures" in this sense.
    #[must_use]
    pub fn is_closure(&self, date: NaiveDate) -> bool {
        !is_weekend(date) && (self.closed.contains(&date) || is_rule_holiday(date))
    }

    /// `true` when `date` is a session: a weekday that is not a rule holiday and not an
    /// override closure. Half days are sessions.
    #[must_use]
    pub fn is_trading_day(&self, date: NaiveDate) -> bool {
        !is_weekend(date) && !self.closed.contains(&date) && !is_rule_holiday(date)
    }

    /// The full-day closures of `year` on weekdays: rule holidays plus override closures,
    /// sorted and without duplicates. This is the list the golden fixture records per year.
    #[must_use]
    pub fn holidays(&self, year: i32) -> Vec<NaiveDate> {
        let mut out: BTreeSet<NaiveDate> =
            rule_holidays(year).into_iter().map(|h| h.date).collect();
        out.extend(self.closed.iter().copied().filter(|d| d.year() == year));
        out.into_iter().collect()
    }

    /// The fixture's `add_trading_days`: roll `date` forward to the first session on or
    /// after it, then advance `n` sessions.
    ///
    /// Consequences: for a session `d`, `add_trading_days(d, 0) == d`; for a non-session
    /// `d`, `add_trading_days(d, 0)` is the next session and `add_trading_days(d, n)` is the
    /// `n`-th session after that one. For the lock, `add_trading_days(D, 5)` is the first
    /// permitted day of a block placed on session `D`.
    ///
    /// # Errors
    ///
    /// [`CalendarError::OutOfRange`] only if the walk leaves `chrono`'s date range.
    pub fn add_trading_days(&self, date: NaiveDate, n: u32) -> Result<NaiveDate, CalendarError> {
        let mut d = date;
        while !self.is_trading_day(d) {
            d = d
                .succ_opt()
                .ok_or(CalendarError::OutOfRange { from: date })?;
        }
        for _ in 0..n {
            d = self
                .next_trading_day(d)
                .map_err(|_| CalendarError::OutOfRange { from: date })?;
        }
        Ok(d)
    }

    /// The first session strictly after `date`.
    ///
    /// # Errors
    ///
    /// [`CalendarError::OutOfRange`] only if the walk leaves `chrono`'s date range.
    pub fn next_trading_day(&self, date: NaiveDate) -> Result<NaiveDate, CalendarError> {
        let mut d = date;
        loop {
            d = d
                .succ_opt()
                .ok_or(CalendarError::OutOfRange { from: date })?;
            if self.is_trading_day(d) {
                return Ok(d);
            }
        }
    }

    /// The last session strictly before `date`.
    ///
    /// # Errors
    ///
    /// [`CalendarError::OutOfRange`] only if the walk leaves `chrono`'s date range.
    pub fn prev_trading_day(&self, date: NaiveDate) -> Result<NaiveDate, CalendarError> {
        let mut d = date;
        loop {
            d = d
                .pred_opt()
                .ok_or(CalendarError::OutOfRange { from: date })?;
            if self.is_trading_day(d) {
                return Ok(d);
            }
        }
    }

    /// The number of sessions in the half-open interval `(from, to]`: `from` excluded, `to`
    /// included. Negative when `to < from` (then it is minus the count in `(to, from]`), zero
    /// when they are equal.
    ///
    /// Consistent with [`Calendar::add_trading_days`]: for a session `d`,
    /// `trading_days_between(d, add_trading_days(d, n)) == n`.
    #[must_use]
    pub fn trading_days_between(&self, from: NaiveDate, to: NaiveDate) -> i64 {
        let (lo, hi, sign) = if from <= to {
            (from, to, 1)
        } else {
            (to, from, -1)
        };
        let count = lo
            .iter_days()
            .skip(1)
            .take_while(|d| *d <= hi)
            .filter(|d| self.is_trading_day(*d))
            .count();
        sign * i64::try_from(count).unwrap_or(i64::MAX)
    }

    /// Appends a closure to the runtime overrides file (`calendar-sync`, the owner's
    /// `calendar add`), then adds it to this calendar. Add-only:
    ///
    /// - a weekend date or an empty name is refused ([`CalendarError::Refused`]);
    /// - a date that is already closed (rules, seed, this calendar or the current runtime
    ///   file) writes nothing and returns [`AddOutcome::AlreadyClosed`];
    /// - otherwise the existing file is kept byte for byte and one `[[closure]]` block is
    ///   appended: the new content goes to a temporary dot-file in the same directory
    ///   (`.nyse_calendar_overrides.toml.tmp.<pid>.<unix_ns>`), is fsynced, renamed over the
    ///   file, and the directory is fsynced (IO-CONTRACT §2-§3). No line is ever removed.
    ///
    /// The existing runtime file is re-read and validated under the writer lock
    /// `<runtime dir>/locks/<file stem>.lock` (for `$QUANT_STATE/...` that is
    /// `$QUANT_STATE/locks/nyse_calendar_overrides.lock`); a malformed file is never
    /// appended to. A concurrent writer holding the lock gives [`CalendarError::Locked`].
    ///
    /// Audit rows (`calendar_override_added`, `manual_override`) are the caller's job.
    ///
    /// # Errors
    ///
    /// [`CalendarError::Refused`], [`CalendarError::Locked`], [`CalendarError::Read`],
    /// [`CalendarError::Toml`] / [`CalendarError::InvalidOverride`] for an existing malformed
    /// runtime file, [`CalendarError::Write`] if the write fails (the old file is untouched).
    pub fn add_closure(
        &mut self,
        runtime: &Path,
        closure: &NewClosure,
    ) -> Result<AddOutcome, CalendarError> {
        let refuse = |reason| CalendarError::Refused {
            date: closure.date,
            reason,
        };
        if is_weekend(closure.date) {
            return Err(refuse(RefusalReason::Weekend));
        }
        if closure.name.trim().is_empty() {
            return Err(refuse(RefusalReason::EmptyName));
        }

        let _lock = RuntimeLock::acquire(runtime)?;
        let existing = read_optional(runtime)?.unwrap_or_default();
        let current = parse_overrides(&existing, runtime, ClosureOrigin::Runtime)?;
        for c in current {
            if !self.closures.contains(&c) {
                self.insert(c);
            }
        }
        if self.is_closure(closure.date) {
            return Ok(AddOutcome::AlreadyClosed);
        }

        let mut content = existing;
        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(&closure.to_toml_block());

        // Paranoia before replacing the file: the new content must parse and contain the date.
        let reparsed = parse_overrides(&content, runtime, ClosureOrigin::Runtime)?;
        if !reparsed.iter().any(|c| c.date == closure.date) {
            return Err(CalendarError::Write {
                path: runtime.to_path_buf(),
                source: std::io::Error::other("rendered entry did not read back"),
            });
        }
        // Inserted before the write: a write that fails after the rename (directory fsync)
        // has still changed the file, and an extra in-memory closure can only lengthen a block.
        self.insert(closure.to_closure());
        write_atomic(runtime, content.as_bytes())?;
        Ok(AddOutcome::Added)
    }
}
