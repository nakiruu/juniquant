//! Data and broker vendors ([`Vendor`]) and canonical-to-vendor rendering ([`render`]).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::canon::Canon;

/// A source or destination whose ticker spelling may differ from the canonical one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Vendor {
    /// EODHD, the primary price source (DECISIONS D15): `BRK-B`, sent as `BRK-B.US` (the
    /// client appends the exchange). Class-share form verify (P3).
    Eodhd,
    /// Yahoo Finance chart API (`BRK-B`), optional secondary source.
    Yahoo,
    /// SEC EDGAR `company_tickers` (`BRK-B`).
    Sec,
    /// Charles Schwab trader API. Class-share spelling unverified (`10-execution-schwab`).
    Schwab,
    /// iShares holdings files. Input only: never rendered.
    #[serde(rename = "ishares")]
    IShares,
}

impl Vendor {
    /// All vendors, in declaration order.
    pub const ALL: [Self; 5] = [
        Self::Eodhd,
        Self::Yahoo,
        Self::Sec,
        Self::Schwab,
        Self::IShares,
    ];

    /// The lower-case name used in fixtures and config: `eodhd`, `yahoo`, `sec`, `schwab`, `ishares`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Eodhd => "eodhd",
            Self::Yahoo => "yahoo",
            Self::Sec => "sec",
            Self::Schwab => "schwab",
            Self::IShares => "ishares",
        }
    }
}

impl fmt::Display for Vendor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An unknown vendor name.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("unknown vendor {input:?}: expected eodhd, yahoo, sec, schwab or ishares")]
pub struct UnknownVendor {
    /// The rejected input, verbatim.
    pub input: String,
}

impl FromStr for Vendor {
    type Err = UnknownVendor;

    /// Parses the exact lower-case name returned by [`Vendor::as_str`].
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|v| v.as_str() == s)
            .ok_or_else(|| UnknownVendor {
                input: s.to_owned(),
            })
    }
}

/// Why a symbol cannot be rendered for a vendor. [`RenderError::kind`] gives the stable
/// strings used in `schemas/fixtures/symbol_render.csv`.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RenderError {
    /// The vendor's spelling for this symbol is not verified (Schwab class shares): refuse
    /// rather than guess.
    #[error("{vendor} spelling of {symbol} is unverified; refusing to render")]
    UnverifiedVendor {
        /// The vendor asked for.
        vendor: Vendor,
        /// The symbol that could not be rendered.
        symbol: Canon,
    },
    /// The vendor is an input source only (iShares); nothing is ever sent to it.
    #[error("{vendor} is an input-only source; symbols are never rendered for it")]
    InputOnly {
        /// The vendor asked for.
        vendor: Vendor,
    },
    /// The string entry point [`render_str`] was given a non-canonical symbol.
    #[error("render takes canonical input only, got {input:?}")]
    NotCanonical {
        /// The rejected input, verbatim.
        input: String,
    },
}

impl RenderError {
    /// Stable machine-readable kind: `unverified_vendor`, `input_only` or `not_canonical`.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::UnverifiedVendor { .. } => "unverified_vendor",
            Self::InputOnly { .. } => "input_only",
            Self::NotCanonical { .. } => "not_canonical",
        }
    }
}

/// Renders a canonical symbol in a vendor's spelling (UNIVERSE.md §3, `symbol_render.csv`).
///
/// - EODHD, Yahoo and SEC: the class dot becomes a hyphen (`BRK.B` -> `BRK-B`).
/// - Schwab: class-less symbols are unchanged; class shares are refused until their
///   spelling is verified (`10-execution-schwab`).
/// - iShares: input only, always refused.
///
/// # Errors
///
/// [`RenderError::UnverifiedVendor`] or [`RenderError::InputOnly`] as above.
pub fn render(symbol: &Canon, vendor: Vendor) -> Result<String, RenderError> {
    match (vendor, symbol.class()) {
        (Vendor::IShares, _) => Err(RenderError::InputOnly { vendor }),
        (Vendor::Eodhd | Vendor::Yahoo | Vendor::Sec | Vendor::Schwab, None) => {
            Ok(symbol.as_str().to_owned())
        }
        (Vendor::Eodhd | Vendor::Yahoo | Vendor::Sec, Some(class)) => {
            Ok(format!("{}-{class}", symbol.base()))
        }
        (Vendor::Schwab, Some(_)) => Err(RenderError::UnverifiedVendor {
            vendor,
            symbol: symbol.clone(),
        }),
    }
}

/// [`render`] from a string: the input must already be exactly canonical (checked first).
///
/// # Errors
///
/// [`RenderError::NotCanonical`] if `symbol` is not canonical (including superseded
/// `SYMBOL~CIK10` tickers and lower-case input), otherwise as [`render`].
pub fn render_str(symbol: &str, vendor: Vendor) -> Result<String, RenderError> {
    let canon =
        Canon::from_str(symbol).map_err(|e| RenderError::NotCanonical { input: e.input })?;
    render(&canon, vendor)
}
