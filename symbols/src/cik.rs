//! SEC Central Index Key ([`Cik`]) and the superseded ticker form ([`SupersededTicker`]).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

use crate::canon::Canon;

/// Number of digits in the zero-padded rendering.
const CIK_DIGITS: usize = 10;

/// Largest CIK that fits in ten digits.
const CIK_MAX: u64 = 9_999_999_999;

/// An SEC Central Index Key, the issuer key of `security_master`.
///
/// Always in `1..=9_999_999_999`; displayed and serialized as a ten-digit zero-padded
/// string (`0001067983`). Zero is rejected: it is not an issued CIK and usually signals a
/// defaulted field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Cik(u64);

/// A CIK could not be parsed or is out of range.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("invalid CIK {input:?}: expected 1 to 10 ASCII digits, not all zero")]
pub struct CikError {
    /// The rejected input, verbatim.
    pub input: String,
}

impl Cik {
    /// Builds a CIK from its numeric value.
    ///
    /// # Errors
    ///
    /// [`CikError`] if `value` is zero or has more than ten digits.
    pub fn new(value: u64) -> Result<Self, CikError> {
        if (1..=CIK_MAX).contains(&value) {
            Ok(Self(value))
        } else {
            Err(CikError {
                input: value.to_string(),
            })
        }
    }

    /// The numeric value.
    #[must_use]
    pub fn as_u64(self) -> u64 {
        self.0
    }

    /// Strict parse of the exactly-ten-digit padded form used in `SYMBOL~CIK10`.
    ///
    /// # Errors
    ///
    /// [`CikError`] unless `s` is exactly ten ASCII digits and not all zero.
    pub fn parse_padded(s: &str) -> Result<Self, CikError> {
        if s.len() == CIK_DIGITS {
            Self::from_str(s)
        } else {
            Err(CikError {
                input: s.to_owned(),
            })
        }
    }
}

impl FromStr for Cik {
    type Err = CikError;

    /// Parses 1 to 10 ASCII digits (padded or not) and pads to ten. No sign, no spaces.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || CikError {
            input: s.to_owned(),
        };
        if s.is_empty() || s.len() > CIK_DIGITS || !s.bytes().all(|b| b.is_ascii_digit()) {
            return Err(err());
        }
        let value: u64 = s.parse().map_err(|_| err())?;
        Self::new(value).map_err(|_| err())
    }
}

impl fmt::Display for Cik {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:0width$}", self.0, width = CIK_DIGITS)
    }
}

impl Serialize for Cik {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Cik {
    /// Deserializes from a string of 1 to 10 digits.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::from_str(&s).map_err(serde::de::Error::custom)
    }
}

/// A delisted security whose symbol was later reused by another issuer, stored in the
/// lake's `ticker` columns as `SYMBOL~CIK10` (for example `ABC~0000012345`).
///
/// It deliberately violates the tradable grammar: there is no conversion to [`Canon`], the
/// symbol is only available as a string, and [`crate::normalize`] / [`crate::is_tradable`]
/// reject its rendering. It is carried in history, never traded.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SupersededTicker {
    symbol: String,
    cik: Cik,
}

/// A string is not a valid `SYMBOL~CIK10` superseded ticker.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("not a superseded ticker {input:?}: expected CANON~CIK10")]
pub struct SupersededError {
    /// The rejected input, verbatim.
    pub input: String,
}

impl SupersededTicker {
    /// The superseded form of `symbol` as held by the older issuer `cik`.
    #[must_use]
    pub fn new(symbol: &Canon, cik: Cik) -> Self {
        Self {
            symbol: symbol.as_str().to_owned(),
            cik,
        }
    }

    /// The reused symbol as a plain string (deliberately not a [`Canon`]).
    #[must_use]
    pub fn symbol(&self) -> &str {
        &self.symbol
    }

    /// The CIK of the older (delisted) issuer.
    #[must_use]
    pub fn cik(&self) -> Cik {
        self.cik
    }
}

impl fmt::Display for SupersededTicker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}~{}", self.symbol, self.cik)
    }
}

impl FromStr for SupersededTicker {
    type Err = SupersededError;

    /// Strict parse of `CANON~CIK10`: a canonical symbol, one `~`, exactly ten digits.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || SupersededError {
            input: s.to_owned(),
        };
        let (symbol, cik) = s.split_once('~').ok_or_else(err)?;
        let symbol = Canon::from_str(symbol).map_err(|_| err())?;
        let cik = Cik::parse_padded(cik).map_err(|_| err())?;
        Ok(Self::new(&symbol, cik))
    }
}

impl Serialize for SupersededTicker {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for SupersededTicker {
    /// Deserializes from the strict `CANON~CIK10` string.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::from_str(&s).map_err(serde::de::Error::custom)
    }
}
