//! The overrides files: ad-hoc full-day closures unioned with the rules (09 SKILL.md §4).
//!
//! Two files share one format:
//!
//! - the checked-in seed `$QUANT_CONFIG/nyse_calendar_overrides.toml` (required);
//! - the runtime file `$QUANT_STATE/nyse_calendar_overrides.toml`, written only by
//!   `calendar-sync` and the owner's `quant-exec calendar add` (optional: absent means no
//!   runtime closures yet).
//!
//! ```toml
//! [[closure]]
//! date = "2025-01-09"                       # required; "YYYY-MM-DD" string or TOML local date
//! name = "National Day of Mourning"         # required, non-empty
//! added_on = "2025-01-08T14:00:00Z"         # optional (runtime entries), kept verbatim
//! first_seen = "2025-01-08T14:00:00Z"       # optional (runtime entries), kept verbatim
//! source = "schwab_marketdata"              # optional; one of ClosureSource
//! ```
//!
//! Validation is strict and all-or-nothing: one bad entry rejects the whole file, and the
//! caller must not fall back to a calendar without it (fail closed). An entry can only ever
//! *add* a closure: the format has no key that opens a day, and unknown keys are errors.

use std::fmt;
use std::path::Path;

use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use toml::{Table, Value};

use crate::error::{CalendarError, EntryLocation, OverrideErrorKind};
use crate::rules::is_weekend;

/// Who recorded a runtime closure (09 SKILL.md §4, runtime file entries).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ClosureSource {
    /// `calendar-sync` saw the market-hours endpoint report the day closed.
    SchwabMarketdata,
    /// The 10:00 ET same-day signal (quotes not advancing, or status `Closed`).
    SameDaySignal,
    /// The add-only daily-bar backfill (amendment 1.12).
    PriceBarBackfill,
    /// The owner's `quant-exec calendar add`.
    Manual,
}

impl ClosureSource {
    /// Every source, in declaration order.
    pub const ALL: [Self; 4] = [
        Self::SchwabMarketdata,
        Self::SameDaySignal,
        Self::PriceBarBackfill,
        Self::Manual,
    ];

    /// The spelling used in the overrides file.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SchwabMarketdata => "schwab_marketdata",
            Self::SameDaySignal => "same_day_signal",
            Self::PriceBarBackfill => "price_bar_backfill",
            Self::Manual => "manual",
        }
    }

    /// Parses the file spelling; `None` for anything else.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.as_str() == s)
    }
}

impl fmt::Display for ClosureSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which file (or constructor) a closure came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ClosureOrigin {
    /// The checked-in seed file under `$QUANT_CONFIG`.
    Seed,
    /// The runtime file under `$QUANT_STATE`.
    Runtime,
    /// Passed directly to [`crate::Calendar::from_closures`].
    Explicit,
}

/// One ad-hoc full-day closure from an overrides file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Closure {
    /// The closed weekday.
    pub date: NaiveDate,
    /// Why the exchange is closed.
    pub name: String,
    /// When the entry was written (runtime entries), verbatim from the file.
    pub added_on: Option<String>,
    /// When the closure was first observed (runtime entries), verbatim from the file.
    pub first_seen: Option<String>,
    /// Who recorded it (runtime entries).
    pub source: Option<ClosureSource>,
    /// Which file it came from.
    pub origin: ClosureOrigin,
}

impl Closure {
    /// A closure with only a date and a name (for [`crate::Calendar::from_closures`]).
    #[must_use]
    pub fn new(date: NaiveDate, name: impl Into<String>) -> Self {
        Self {
            date,
            name: name.into(),
            added_on: None,
            first_seen: None,
            source: None,
            origin: ClosureOrigin::Explicit,
        }
    }

    /// Checks the invariants every closure satisfies (non-empty name, weekday date).
    pub(crate) fn validate(&self) -> Result<(), OverrideErrorKind> {
        if self.name.trim().is_empty() {
            return Err(OverrideErrorKind::EmptyName);
        }
        if is_weekend(self.date) {
            return Err(OverrideErrorKind::Weekend(self.date));
        }
        Ok(())
    }
}

/// A closure to append to the runtime overrides with [`crate::Calendar::add_closure`].
/// All provenance fields are required for code-written entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewClosure {
    /// The weekday to close.
    pub date: NaiveDate,
    /// Why (for example "National Day of Mourning (President ...)").
    pub name: String,
    /// When this entry is written.
    pub added_on: DateTime<Utc>,
    /// When the closure was first observed.
    pub first_seen: DateTime<Utc>,
    /// Who recorded it.
    pub source: ClosureSource,
}

impl NewClosure {
    /// Renders the entry as one `[[closure]]` TOML block (ends with a newline).
    pub(crate) fn to_toml_block(&self) -> String {
        let mut entry = Table::new();
        entry.insert("date".into(), Value::String(format_date(self.date)));
        entry.insert("name".into(), Value::String(self.name.clone()));
        entry.insert(
            "added_on".into(),
            Value::String(self.added_on.to_rfc3339_opts(SecondsFormat::Secs, true)),
        );
        entry.insert(
            "first_seen".into(),
            Value::String(self.first_seen.to_rfc3339_opts(SecondsFormat::Secs, true)),
        );
        entry.insert(
            "source".into(),
            Value::String(self.source.as_str().to_owned()),
        );
        let mut doc = Table::new();
        doc.insert("closure".into(), Value::Array(vec![Value::Table(entry)]));
        doc.to_string()
    }

    /// The closure as it reads back from the runtime file.
    pub(crate) fn to_closure(&self) -> Closure {
        Closure {
            date: self.date,
            name: self.name.clone(),
            added_on: Some(self.added_on.to_rfc3339_opts(SecondsFormat::Secs, true)),
            first_seen: Some(self.first_seen.to_rfc3339_opts(SecondsFormat::Secs, true)),
            source: Some(self.source),
            origin: ClosureOrigin::Runtime,
        }
    }
}

/// `YYYY-MM-DD`.
pub(crate) fn format_date(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

/// Parses a strict `YYYY-MM-DD` date (zero-padded; no other form is accepted).
pub(crate) fn parse_iso_date(s: &str) -> Option<NaiveDate> {
    let d = NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()?;
    (format_date(d) == s).then_some(d)
}

/// Parses and validates the text of an overrides file. `path` is used in error messages
/// only; `origin` is recorded on every closure.
///
/// # Errors
///
/// [`CalendarError::Toml`] for invalid TOML, [`CalendarError::InvalidOverride`] for any
/// entry that breaks the format (see [`OverrideErrorKind`]).
pub fn parse_overrides(
    text: &str,
    path: &Path,
    origin: ClosureOrigin,
) -> Result<Vec<Closure>, CalendarError> {
    let doc: Table = text.parse().map_err(|e| CalendarError::Toml {
        path: path.to_path_buf(),
        source: Box::new(e),
    })?;
    let invalid = |location, kind| CalendarError::InvalidOverride {
        path: path.to_path_buf(),
        location,
        kind,
    };
    let mut out = Vec::new();
    for (key, value) in &doc {
        if key != "closure" {
            return Err(invalid(
                EntryLocation::TopLevel,
                OverrideErrorKind::UnknownTopLevelKey(key.clone()),
            ));
        }
        let Value::Array(entries) = value else {
            return Err(invalid(
                EntryLocation::TopLevel,
                OverrideErrorKind::NotAnArrayOfTables,
            ));
        };
        for (i, entry) in entries.iter().enumerate() {
            let location = EntryLocation::Closure(i + 1);
            let Value::Table(table) = entry else {
                return Err(invalid(
                    EntryLocation::TopLevel,
                    OverrideErrorKind::NotAnArrayOfTables,
                ));
            };
            let closure = parse_entry(table, origin).map_err(|kind| invalid(location, kind))?;
            out.push(closure);
        }
    }
    Ok(out)
}

/// Reads and validates an overrides file.
///
/// # Errors
///
/// [`CalendarError::Read`] when the file cannot be read (including when it does not exist),
/// otherwise as [`parse_overrides`].
pub fn load_overrides_file(
    path: &Path,
    origin: ClosureOrigin,
) -> Result<Vec<Closure>, CalendarError> {
    let text = std::fs::read_to_string(path).map_err(|source| CalendarError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    parse_overrides(&text, path, origin)
}

fn parse_entry(table: &Table, origin: ClosureOrigin) -> Result<Closure, OverrideErrorKind> {
    for key in table.keys() {
        if !matches!(
            key.as_str(),
            "date" | "name" | "added_on" | "first_seen" | "source"
        ) {
            return Err(OverrideErrorKind::UnknownKey(key.clone()));
        }
    }
    let date = match table.get("date") {
        None => return Err(OverrideErrorKind::MissingKey("date")),
        Some(Value::String(s)) => {
            parse_iso_date(s).ok_or_else(|| OverrideErrorKind::BadDate(s.clone()))?
        }
        Some(Value::Datetime(dt)) => match (dt.date, dt.time, dt.offset) {
            (Some(d), None, None) => {
                NaiveDate::from_ymd_opt(i32::from(d.year), u32::from(d.month), u32::from(d.day))
                    .ok_or_else(|| OverrideErrorKind::BadDate(dt.to_string()))?
            }
            _ => return Err(OverrideErrorKind::BadDate(dt.to_string())),
        },
        Some(_) => {
            return Err(OverrideErrorKind::WrongType {
                key: "date",
                expected: "a \"YYYY-MM-DD\" string or a TOML local date",
            });
        }
    };
    let name = match table.get("name") {
        None => return Err(OverrideErrorKind::MissingKey("name")),
        Some(Value::String(s)) => s.clone(),
        Some(_) => {
            return Err(OverrideErrorKind::WrongType {
                key: "name",
                expected: "a string",
            });
        }
    };
    let added_on = optional_timestamp(table, "added_on")?;
    let first_seen = optional_timestamp(table, "first_seen")?;
    let source = match table.get("source") {
        None => None,
        Some(Value::String(s)) => Some(
            ClosureSource::parse(s).ok_or_else(|| OverrideErrorKind::UnknownSource(s.clone()))?,
        ),
        Some(_) => {
            return Err(OverrideErrorKind::WrongType {
                key: "source",
                expected: "a string",
            });
        }
    };
    let closure = Closure {
        date,
        name,
        added_on,
        first_seen,
        source,
        origin,
    };
    closure.validate()?;
    Ok(closure)
}

fn optional_timestamp(
    table: &Table,
    key: &'static str,
) -> Result<Option<String>, OverrideErrorKind> {
    match table.get(key) {
        None => Ok(None),
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(Some(s.clone())),
        Some(Value::Datetime(dt)) => Ok(Some(dt.to_string())),
        Some(_) => Err(OverrideErrorKind::WrongType {
            key,
            expected: "a non-empty string or a TOML date-time",
        }),
    }
}
