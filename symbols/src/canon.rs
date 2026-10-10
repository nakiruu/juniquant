//! The canonical ticker [`Canon`], [`normalize`] and [`is_tradable`] (UNIVERSE.md §3).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

/// Longest base (the part before the class dot) the grammar allows.
const MAX_BASE_LEN: usize = 6;

/// Separators that may stand between a base and a one-letter share class in raw input.
const CLASS_SEPARATORS: &[u8] = b".-/_ ";

/// A canonical ticker: `CANON := [A-Z0-9]{1,6} ( "." [A-Z] )?`.
///
/// The only way to obtain one is through validation ([`normalize`], [`Canon::from_str`] or
/// deserialization), so a `Canon` is always tradable by grammar. A superseded
/// `SYMBOL~CIK10` ticker can never be a `Canon` (see [`crate::SupersededTicker`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Canon(String);

impl Canon {
    /// The canonical string, for example `BRK.B`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The base before the class dot: `BRK` for `BRK.B`, `GOOGL` for `GOOGL`.
    #[must_use]
    pub fn base(&self) -> &str {
        self.0
            .split_once('.')
            .map_or(self.0.as_str(), |(base, _)| base)
    }

    /// The share-class letter after the dot, if any: `Some('B')` for `BRK.B`.
    #[must_use]
    pub fn class(&self) -> Option<char> {
        self.0
            .split_once('.')
            .and_then(|(_, class)| class.chars().next())
    }

    /// Wraps a string already checked by [`is_canonical`].
    fn from_checked(s: String) -> Self {
        Self(s)
    }
}

impl fmt::Display for Canon {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for Canon {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// The input to a strict parse was not exactly canonical (stable kind `not_canonical`).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("not a canonical symbol: {input:?}")]
pub struct NotCanonical {
    /// The rejected input, verbatim.
    pub input: String,
}

impl NotCanonical {
    /// Stable machine-readable kind: `not_canonical`.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        "not_canonical"
    }
}

impl FromStr for Canon {
    type Err = NotCanonical;

    /// Strict parse: accepts only input that is already exactly canonical (no trimming, no
    /// case folding, no separator rewriting). Use [`normalize`] for raw vendor input.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if is_canonical(s) {
            Ok(Self::from_checked(s.to_owned()))
        } else {
            Err(NotCanonical {
                input: s.to_owned(),
            })
        }
    }
}

impl Serialize for Canon {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Canon {
    /// Deserializes from a string with the strict [`Canon::from_str`] check.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::from_str(&s).map_err(serde::de::Error::custom)
    }
}

/// Why [`normalize`] rejected a raw symbol. [`SymbolError::kind`] gives the stable strings
/// used in `schemas/fixtures/symbol_normalization.csv`.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SymbolError {
    /// The input contains a non-ASCII character (checked first, before trimming).
    #[error("non-ASCII symbol {0:?}")]
    NonAscii(String),
    /// Nothing is left after trimming and stripping one leading `$`.
    #[error("empty symbol {0:?}")]
    Empty(String),
    /// The input contains `~`: a superseded `SYMBOL~CIK10` ticker, produced only by the
    /// identifier master and never normalized or traded.
    #[error("superseded symbol {0:?} is never normalized")]
    Superseded(String),
    /// The input does not match the grammar (preferred series, too long, two separators, ...).
    #[error("unparseable symbol {0:?}")]
    Unparseable(String),
}

impl SymbolError {
    /// Stable machine-readable kind: `non_ascii`, `empty`, `superseded` or `unparseable`.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::NonAscii(_) => "non_ascii",
            Self::Empty(_) => "empty",
            Self::Superseded(_) => "superseded",
            Self::Unparseable(_) => "unparseable",
        }
    }

    /// The raw input that was rejected, verbatim.
    #[must_use]
    pub fn input(&self) -> &str {
        match self {
            Self::NonAscii(s) | Self::Empty(s) | Self::Superseded(s) | Self::Unparseable(s) => s,
        }
    }
}

/// `[A-Z0-9]{1,6}`.
fn is_base(s: &[u8]) -> bool {
    (1..=MAX_BASE_LEN).contains(&s.len())
        && s.iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

/// Exactly `CANON := [A-Z0-9]{1,6} ( "." [A-Z] )?`.
fn is_canonical(s: &str) -> bool {
    match s.split_once('.') {
        None => is_base(s.as_bytes()),
        Some((base, class)) => {
            is_base(base.as_bytes())
                && matches!(class.as_bytes(), [letter] if letter.is_ascii_uppercase())
        }
    }
}

/// True iff `s` is already exactly canonical (the strict grammar, no normalization).
///
/// Execution and lock code use this to reject anything else at order entry, in particular
/// superseded `SYMBOL~CIK10` tickers.
#[must_use]
pub fn is_tradable(s: &str) -> bool {
    is_canonical(s)
}

/// Normalizes a raw vendor symbol to its canonical form.
///
/// Checks apply in this order (the fixture-format paragraph of UNIVERSE.md §3):
///
/// 1. any non-ASCII character: [`SymbolError::NonAscii`];
/// 2. trim, strip **one** leading `$`; nothing left: [`SymbolError::Empty`];
/// 3. any `~` (a superseded `SYMBOL~CIK10` ticker): [`SymbolError::Superseded`];
/// 4. upper-case, then the grammar: `[A-Z0-9]{1,6}` is unchanged; `BASE` + one separator
///    from `. - / _ space` + one letter becomes `BASE.LETTER`; anything else is
///    [`SymbolError::Unparseable`].
///
/// A class letter fused to the base (`BRKB`) is left unchanged: only an alias can map it.
///
/// # Errors
///
/// A [`SymbolError`] as listed above.
pub fn normalize(raw: &str) -> Result<Canon, SymbolError> {
    if !raw.is_ascii() {
        return Err(SymbolError::NonAscii(raw.to_owned()));
    }
    let trimmed = raw.trim();
    let body = trimmed.strip_prefix('$').unwrap_or(trimmed);
    if body.is_empty() {
        return Err(SymbolError::Empty(raw.to_owned()));
    }
    if body.contains('~') {
        return Err(SymbolError::Superseded(raw.to_owned()));
    }
    let upper = body.to_ascii_uppercase();
    let bytes = upper.as_bytes();
    if is_base(bytes) {
        return Ok(Canon::from_checked(upper));
    }
    if let [base @ .., sep, class] = bytes
        && CLASS_SEPARATORS.contains(sep)
        && class.is_ascii_uppercase()
        && is_base(base)
    {
        let mut out: String = base.iter().copied().map(char::from).collect();
        out.push('.');
        out.push(char::from(*class));
        return Ok(Canon::from_checked(out));
    }
    Err(SymbolError::Unparseable(raw.to_owned()))
}
