//! Typed errors of the calendar crate.
//!
//! Every error is fatal for the caller's purpose: a calendar that cannot be loaded must not
//! be replaced by a rules-only or partial calendar. The lock treats a load error as "deny"
//! (09 SKILL.md §4, fail closed).

use std::fmt;
use std::io;
use std::path::PathBuf;

use chrono::NaiveDate;

/// Where in an overrides file a validation error was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryLocation {
    /// The top level of the document (outside any `[[closure]]` entry).
    TopLevel,
    /// The `[[closure]]` entry with this 1-based position in the file.
    Closure(usize),
}

impl fmt::Display for EntryLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TopLevel => write!(f, "top level"),
            Self::Closure(i) => write!(f, "[[closure]] #{i}"),
        }
    }
}

/// What is wrong with an overrides file that is valid TOML.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OverrideErrorKind {
    /// A top-level key other than `closure`.
    #[error("unknown top-level key `{0}` (only `[[closure]]` entries are allowed)")]
    UnknownTopLevelKey(String),
    /// `closure` is not an array of tables.
    #[error("`closure` must be an array of tables (`[[closure]]`)")]
    NotAnArrayOfTables,
    /// A required key is absent.
    #[error("missing required key `{0}`")]
    MissingKey(&'static str),
    /// A key the format does not define. The format has no way to express an opening: an
    /// override can only add a closure, so any other key (for example `open = true`) is
    /// rejected rather than ignored.
    #[error("unknown key `{0}` (allowed: date, name, added_on, first_seen, source)")]
    UnknownKey(String),
    /// A key has the wrong TOML type.
    #[error("key `{key}` must be {expected}")]
    WrongType {
        /// The offending key.
        key: &'static str,
        /// What the key must be.
        expected: &'static str,
    },
    /// `date` is not a calendar date in `YYYY-MM-DD` form.
    #[error("key `date` must be a date in YYYY-MM-DD form, got `{0}`")]
    BadDate(String),
    /// `name` is empty or only whitespace.
    #[error("key `name` must not be empty")]
    EmptyName,
    /// `source` is not one of the documented values.
    #[error(
        "unknown source `{0}` (allowed: schwab_marketdata, same_day_signal, price_bar_backfill, manual)"
    )]
    UnknownSource(String),
    /// The closure is dated on a Saturday or Sunday, when the exchange never opens: the
    /// entry closes nothing and almost certainly carries a typo in its date.
    #[error("{0} is a weekend day; an override must name a weekday (check the date)")]
    Weekend(NaiveDate),
}

/// Why [`crate::Calendar::add_closure`] refused a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RefusalReason {
    /// The date is a Saturday or Sunday.
    #[error("it is a weekend day")]
    Weekend,
    /// The closure name is empty.
    #[error("the closure name is empty")]
    EmptyName,
}

/// Errors of the calendar crate.
#[derive(Debug, thiserror::Error)]
pub enum CalendarError {
    /// A required environment variable is unset or empty (IO-CONTRACT §1: never a silent
    /// default).
    #[error(
        "environment variable {var} is not set (or is empty); it is required to locate the NYSE calendar overrides"
    )]
    MissingEnvVar {
        /// The variable's name.
        var: &'static str,
    },
    /// A file could not be read.
    #[error("cannot read {}: {source}", path.display())]
    Read {
        /// The file.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: io::Error,
    },
    /// A file is not valid TOML.
    #[error("{} is not valid TOML: {source}", path.display())]
    Toml {
        /// The file.
        path: PathBuf,
        /// The TOML parser's error.
        #[source]
        source: Box<toml::de::Error>,
    },
    /// A file is valid TOML but not a valid overrides file.
    #[error("{}: {location}: {kind}", path.display())]
    InvalidOverride {
        /// The file.
        path: PathBuf,
        /// Where in the file.
        location: EntryLocation,
        /// What is wrong.
        kind: OverrideErrorKind,
    },
    /// A closure passed to a constructor is invalid (same checks as a file entry).
    #[error("invalid closure {date}: {kind}")]
    InvalidClosure {
        /// The closure's date.
        date: NaiveDate,
        /// What is wrong.
        kind: OverrideErrorKind,
    },
    /// Writing the runtime overrides failed; the previous file is left untouched.
    #[error("cannot write {}: {source}", path.display())]
    Write {
        /// The file (or directory) being written.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: io::Error,
    },
    /// Another process holds the runtime overrides writer lock.
    #[error("another writer holds {}; retry later", path.display())]
    Locked {
        /// The lock file.
        path: PathBuf,
    },
    /// [`crate::Calendar::add_closure`] refused the request (it only adds closures).
    #[error("refusing to add a closure on {date}: {reason}")]
    Refused {
        /// The requested date.
        date: NaiveDate,
        /// Why.
        reason: RefusalReason,
    },
    /// Date arithmetic left `chrono`'s representable range (around year 262143).
    #[error("date arithmetic from {from} left the representable date range")]
    OutOfRange {
        /// The date the computation started from.
        from: NaiveDate,
    },
}
