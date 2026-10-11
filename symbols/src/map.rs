//! The canonical symbol map [`SymbolMap`] over `config/symbol_map.toml`
//! (09-lock-state-machine SKILL.md §3.8).
//!
//! Every load failure is a typed [`MapError`]: a map that does not load denies every lock
//! check (fail closed). An unmapped spelling resolves to `None`, never to a guess.
//!
//! A ticker string can belong to different securities at different times (a rename followed
//! by reuse of the old ticker, DECISIONS B5). Each string therefore has dated **holdings**:
//! an entry holds its canonical ticker from its optional `from` date onward, and holds each
//! alias from the alias's optional `from` through its `until`. Holdings of one string must not
//! overlap; an undated holding is unbounded on that side, so undated reuse still fails to load.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use chrono::{Days, NaiveDate};
use serde::Deserialize;
use thiserror::Error;

use crate::canon::{Canon, SymbolError, normalize};
use crate::cik::Cik;
use crate::vendor::Vendor;

/// The on-disk file: a list of `[[symbol]]` tables. Unknown keys are rejected.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    #[serde(default)]
    symbol: Vec<RawEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEntry {
    canonical: String,
    from: Option<String>,
    yahoo: Option<String>,
    eodhd: Option<String>,
    ishares: Option<String>,
    schwab: Option<String>,
    lock_group: String,
    #[serde(default)]
    aliases: Vec<RawAlias>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAlias {
    symbol: String,
    from: Option<String>,
    until: String,
}

/// An old ticker (after a rename) that maps to the entry's canonical ticker from `from`
/// (inclusive; unbounded when absent) through `until`, inclusive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alias {
    symbol: Canon,
    from: Option<NaiveDate>,
    until: NaiveDate,
}

impl Alias {
    /// The old ticker, normalized.
    #[must_use]
    pub fn symbol(&self) -> &Canon {
        &self.symbol
    }

    /// First date (inclusive) on which the old ticker maps to the new canonical ticker;
    /// `None` means unbounded. Needed only when another security also held the string.
    #[must_use]
    pub fn from(&self) -> Option<NaiveDate> {
        self.from
    }

    /// Last date (inclusive) on which the old ticker maps to the new canonical ticker.
    #[must_use]
    pub fn until(&self) -> NaiveDate {
        self.until
    }
}

/// One validated `[[symbol]]` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolEntry {
    canonical: Canon,
    from: Option<NaiveDate>,
    yahoo: Option<String>,
    eodhd: Option<String>,
    ishares: Option<String>,
    schwab: Option<String>,
    lock_group: String,
    aliases: Vec<Alias>,
}

impl SymbolEntry {
    /// The canonical ticker.
    #[must_use]
    pub fn canonical(&self) -> &Canon {
        &self.canonical
    }

    /// First date (inclusive) on which this security holds its canonical ticker; `None`
    /// means unbounded. Required in practice only when the string was another security's
    /// ticker earlier (reuse, DECISIONS B5).
    #[must_use]
    pub fn from(&self) -> Option<NaiveDate> {
        self.from
    }

    /// The vendor spelling exactly as written in the map (`None` if absent, and always
    /// `None` for [`Vendor::Sec`], which has no column).
    #[must_use]
    pub fn spelling(&self, vendor: Vendor) -> Option<&str> {
        match vendor {
            Vendor::Yahoo => self.yahoo.as_deref(),
            Vendor::Eodhd => self.eodhd.as_deref(),
            Vendor::IShares => self.ishares.as_deref(),
            Vendor::Schwab => self.schwab.as_deref(),
            Vendor::Sec => None,
        }
    }

    /// The lock group (one per issuer, DECISIONS B1), trimmed.
    #[must_use]
    pub fn lock_group(&self) -> &str {
        &self.lock_group
    }

    /// Old tickers that map to this one.
    #[must_use]
    pub fn aliases(&self) -> &[Alias] {
        &self.aliases
    }
}

/// One earlier ticker of a security as `security_master.prior_tickers` records it
/// (`{ticker, from, to}`), the independent evidence for [`SymbolMap::validate_prior_tickers`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriorTicker {
    /// The earlier ticker.
    pub ticker: Canon,
    /// First date it was used, if known.
    pub from: Option<NaiveDate>,
    /// Last date it was used.
    pub to: NaiveDate,
}

/// Why the map failed to load or validate. [`MapError::kind`] gives a stable string.
#[derive(Debug, Error)]
pub enum MapError {
    /// The file could not be read.
    #[error("cannot read symbol map {path}: {source}")]
    Io {
        /// The path that was read.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
    /// Not valid TOML, a missing required key, an unknown key or a wrong type.
    #[error("invalid symbol map TOML: {0}")]
    Toml(#[from] Box<toml::de::Error>),
    /// A `canonical` value is not exactly canonical (no normalization is applied to it).
    #[error("canonical {value:?} is not a canonical symbol")]
    NotCanonical {
        /// The value as written.
        value: String,
    },
    /// A vendor spelling or alias does not normalize.
    #[error("{field} spelling of {canonical} does not normalize: {source}")]
    InvalidSpelling {
        /// The entry it belongs to.
        canonical: Canon,
        /// `eodhd`, `yahoo`, `ishares`, `schwab` or `alias`.
        field: &'static str,
        /// The normalization failure.
        source: SymbolError,
    },
    /// A `from` or `until` value is not a `YYYY-MM-DD` date.
    #[error("{symbol} of {canonical}: invalid {field} date {value:?}")]
    InvalidDate {
        /// The entry it belongs to.
        canonical: Canon,
        /// The ticker string the date belongs to (the canonical itself or an alias).
        symbol: Canon,
        /// `from` or `until`.
        field: &'static str,
        /// The value as written.
        value: String,
    },
    /// An alias whose `from` is after its `until`.
    #[error("alias {alias} of {canonical}: from {from} is after until {until}")]
    EmptyPeriod {
        /// The entry it belongs to.
        canonical: Canon,
        /// The alias symbol.
        alias: Canon,
        /// The alias `from`.
        from: NaiveDate,
        /// The alias `until`.
        until: NaiveDate,
    },
    /// A `lock_group` is empty or blank.
    #[error("{canonical} has an empty lock_group")]
    EmptyLockGroup {
        /// The entry.
        canonical: Canon,
    },
    /// Two entries have the same canonical ticker.
    #[error("canonical {canonical} appears twice")]
    DuplicateCanonical {
        /// The repeated canonical ticker.
        canonical: Canon,
    },
    /// Two entries claim the same (normalized) spelling for one vendor.
    #[error("{vendor} spelling {spelling} is claimed by both {first} and {second}")]
    DuplicateSpelling {
        /// The vendor column.
        vendor: Vendor,
        /// The normalized spelling.
        spelling: Canon,
        /// The entry that claimed it first.
        first: Canon,
        /// The entry that claimed it again.
        second: Canon,
    },
    /// An alias's holding period overlaps another holding of the same string: a canonical
    /// ticker or another alias (an undated side is unbounded, DECISIONS B5).
    #[error("alias {alias} of {owner} overlaps the {conflict} {alias} of {other}")]
    AliasCollision {
        /// The alias symbol (normalized).
        alias: Canon,
        /// The entry the alias belongs to.
        owner: Canon,
        /// `canonical` or `alias`: what it collides with.
        conflict: &'static str,
        /// The entry it collides with.
        other: Canon,
    },
    /// Two tickers of one issuer (same CIK) are in different lock groups (DECISIONS B1).
    #[error(
        "issuer {cik}: {first} is in lock group {first_group:?} but {second} is in {second_group:?}"
    )]
    IssuerGroupConflict {
        /// The shared issuer CIK.
        cik: Cik,
        /// One ticker of the issuer.
        first: Canon,
        /// Its lock group.
        first_group: String,
        /// Another ticker of the same issuer.
        second: Canon,
        /// Its (different) lock group.
        second_group: String,
    },
    /// An alias's dates are not corroborated by the security's `prior_tickers`
    /// (DECISIONS B5).
    #[error("alias {alias} of {owner} ({from:?} through {until}) is not in its prior_tickers")]
    PriorTickerMismatch {
        /// The entry the alias belongs to.
        owner: Canon,
        /// The alias symbol.
        alias: Canon,
        /// The alias `from`.
        from: Option<NaiveDate>,
        /// The alias `until`.
        until: NaiveDate,
    },
    /// A prior ticker in `security_master.prior_tickers` has no matching alias in the map
    /// (DECISIONS B5): history under it would miss the renamed security.
    #[error("prior ticker {ticker} of {owner} ({from:?} through {to}) has no matching alias")]
    MissingAlias {
        /// The security it belongs to.
        owner: Canon,
        /// The prior ticker.
        ticker: Canon,
        /// Its first date, if known.
        from: Option<NaiveDate>,
        /// Its last date.
        to: NaiveDate,
    },
    /// A vendor spelling normalizes to a ticker string held by a different entry: one
    /// string would have two owners (DECISIONS B5).
    #[error("{vendor} spelling {spelling} of {owner} is a ticker held by {other}")]
    SpellingCollision {
        /// The vendor column.
        vendor: Vendor,
        /// The normalized spelling.
        spelling: Canon,
        /// The entry claiming the spelling.
        owner: Canon,
        /// The entry holding the ticker string.
        other: Canon,
    },
}

impl MapError {
    /// Stable machine-readable kind: `io`, `toml`, `not_canonical`, `invalid_spelling`,
    /// `invalid_date`, `empty_period`, `empty_lock_group`, `duplicate_canonical`,
    /// `duplicate_spelling`, `alias_collision`, `spelling_collision`,
    /// `issuer_group_conflict`, `prior_ticker_mismatch` or `missing_alias`.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Io { .. } => "io",
            Self::Toml(_) => "toml",
            Self::NotCanonical { .. } => "not_canonical",
            Self::InvalidSpelling { .. } => "invalid_spelling",
            Self::InvalidDate { .. } => "invalid_date",
            Self::EmptyPeriod { .. } => "empty_period",
            Self::EmptyLockGroup { .. } => "empty_lock_group",
            Self::DuplicateCanonical { .. } => "duplicate_canonical",
            Self::DuplicateSpelling { .. } => "duplicate_spelling",
            Self::AliasCollision { .. } => "alias_collision",
            Self::IssuerGroupConflict { .. } => "issuer_group_conflict",
            Self::PriorTickerMismatch { .. } => "prior_ticker_mismatch",
            Self::MissingAlias { .. } => "missing_alias",
            Self::SpellingCollision { .. } => "spelling_collision",
        }
    }
}

/// One dated holding of a ticker string: `owner` maps from `from` through `until`
/// (inclusive; `None` is unbounded on that side).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Holding {
    owner: Canon,
    from: Option<NaiveDate>,
    until: Option<NaiveDate>,
    /// `canonical` or `alias`, for error messages.
    kind: &'static str,
}

impl Holding {
    fn covers(&self, on: NaiveDate) -> bool {
        self.from.is_none_or(|f| f <= on) && self.until.is_none_or(|u| on <= u)
    }

    /// [`Holding::covers`] with both bounded sides widened by `margin`.
    fn covers_widened(&self, on: NaiveDate, margin: Days) -> bool {
        let from_ok = self
            .from
            .is_none_or(|f| f.checked_sub_days(margin).is_none_or(|f| f <= on));
        let until_ok = self
            .until
            .is_none_or(|u| u.checked_add_days(margin).is_none_or(|u| on <= u));
        from_ok && until_ok
    }

    fn overlaps(&self, other: &Self) -> bool {
        let starts_before_other_ends = match (self.from, other.until) {
            (Some(f), Some(u)) => f <= u,
            _ => true,
        };
        let other_starts_before_self_ends = match (other.from, self.until) {
            (Some(f), Some(u)) => f <= u,
            _ => true,
        };
        starts_before_other_ends && other_starts_before_self_ends
    }
}

/// The validated canonical symbol map.
///
/// Lookups:
/// - [`SymbolMap::resolve`]: a vendor spelling to its canonical ticker today (normalized
///   first);
/// - [`SymbolMap::resolve_alias`] / [`SymbolMap::resolve_on`]: a ticker on a date, through
///   the dated holdings;
/// - [`SymbolMap::resolve_history`]: every security a history record may belong to (lock
///   history, fail closed around a handover);
/// - [`SymbolMap::lock_group`], [`SymbolMap::group_members`]: the B1 issuer groups.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SymbolMap {
    entries: BTreeMap<Canon, SymbolEntry>,
    spellings: BTreeMap<(Vendor, Canon), Canon>,
    holders: BTreeMap<Canon, Vec<Holding>>,
    groups: BTreeMap<String, Vec<Canon>>,
}

/// True if the prior ticker `p` and `alias` describe the same holding: same ticker, `until`
/// equal to `to`, and equal `from` when the alias has one.
fn corroborates(p: &PriorTicker, alias: &Alias) -> bool {
    p.ticker == alias.symbol && p.to == alias.until && alias.from.is_none_or(|f| p.from == Some(f))
}

fn parse_date(
    value: &str,
    canonical: &Canon,
    symbol: &Canon,
    field: &'static str,
) -> Result<NaiveDate, MapError> {
    // Round-trip so only zero-padded `YYYY-MM-DD` loads, as in the calendar overrides.
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .ok()
        .filter(|d| d.format("%Y-%m-%d").to_string() == value)
        .ok_or_else(|| MapError::InvalidDate {
            canonical: canonical.clone(),
            symbol: symbol.clone(),
            field,
            value: value.to_owned(),
        })
}

impl SymbolMap {
    /// Reads and validates a map file of the `config/symbol_map.toml` shape.
    ///
    /// # Errors
    ///
    /// [`MapError::Io`] if the file cannot be read, otherwise as [`SymbolMap::from_toml_str`].
    pub fn load(path: impl AsRef<Path>) -> Result<Self, MapError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|source| MapError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_toml_str(&text)
    }

    /// Parses and validates a map from TOML text. A file with no `[[symbol]]` table (for
    /// example the committed, fully commented-out config) is an empty map.
    ///
    /// # Errors
    ///
    /// The first [`MapError`] found: malformed TOML or unknown keys, a non-canonical
    /// `canonical`, a spelling or alias that does not normalize, a bad date, an alias whose
    /// `from` is after its `until`, an empty lock group, a duplicate canonical, two entries
    /// claiming one vendor spelling, an alias whose holding period overlaps another
    /// holding of the same string, or a vendor spelling that is a ticker string held by
    /// another entry.
    pub fn from_toml_str(text: &str) -> Result<Self, MapError> {
        let raw: RawFile = toml::from_str(text).map_err(Box::new)?;
        let mut map = Self::default();

        for raw_entry in raw.symbol {
            let canonical = Canon::from_str(&raw_entry.canonical)
                .map_err(|e| MapError::NotCanonical { value: e.input })?;
            if map.entries.contains_key(&canonical) {
                return Err(MapError::DuplicateCanonical { canonical });
            }
            let from = raw_entry
                .from
                .as_deref()
                .map(|v| parse_date(v, &canonical, &canonical, "from"))
                .transpose()?;
            let lock_group = raw_entry.lock_group.trim().to_owned();
            if lock_group.is_empty() {
                return Err(MapError::EmptyLockGroup { canonical });
            }
            for (vendor, spelling) in [
                (Vendor::Eodhd, &raw_entry.eodhd),
                (Vendor::Yahoo, &raw_entry.yahoo),
                (Vendor::IShares, &raw_entry.ishares),
                (Vendor::Schwab, &raw_entry.schwab),
            ] {
                if let Some(spelling) = spelling {
                    map.claim_spelling(&canonical, vendor, spelling)?;
                }
            }
            let mut aliases = Vec::with_capacity(raw_entry.aliases.len());
            for raw_alias in raw_entry.aliases {
                let symbol =
                    normalize(&raw_alias.symbol).map_err(|source| MapError::InvalidSpelling {
                        canonical: canonical.clone(),
                        field: "alias",
                        source,
                    })?;
                let alias_from = raw_alias
                    .from
                    .as_deref()
                    .map(|v| parse_date(v, &canonical, &symbol, "from"))
                    .transpose()?;
                let until = parse_date(&raw_alias.until, &canonical, &symbol, "until")?;
                if let Some(alias_from) = alias_from
                    && alias_from > until
                {
                    return Err(MapError::EmptyPeriod {
                        canonical: canonical.clone(),
                        alias: symbol,
                        from: alias_from,
                        until,
                    });
                }
                aliases.push(Alias {
                    symbol,
                    from: alias_from,
                    until,
                });
            }
            map.holders
                .entry(canonical.clone())
                .or_default()
                .push(Holding {
                    owner: canonical.clone(),
                    from,
                    until: None,
                    kind: "canonical",
                });
            for alias in &aliases {
                map.holders
                    .entry(alias.symbol.clone())
                    .or_default()
                    .push(Holding {
                        owner: canonical.clone(),
                        from: alias.from,
                        until: Some(alias.until),
                        kind: "alias",
                    });
            }
            map.groups
                .entry(lock_group.clone())
                .or_default()
                .push(canonical.clone());
            map.entries.insert(
                canonical.clone(),
                SymbolEntry {
                    canonical,
                    from,
                    eodhd: raw_entry.eodhd,
                    yahoo: raw_entry.yahoo,
                    ishares: raw_entry.ishares,
                    schwab: raw_entry.schwab,
                    lock_group,
                    aliases,
                },
            );
        }

        // Holdings are checked once every entry is known. Two canonical holdings of one
        // string cannot exist (DuplicateCanonical), so every overlap involves an alias.
        for (symbol, holdings) in &map.holders {
            for (i, a) in holdings.iter().enumerate() {
                for b in holdings.iter().skip(i + 1) {
                    if a.overlaps(b) {
                        let (alias, other) = if b.kind == "alias" { (b, a) } else { (a, b) };
                        return Err(MapError::AliasCollision {
                            alias: symbol.clone(),
                            owner: alias.owner.clone(),
                            conflict: other.kind,
                            other: other.owner.clone(),
                        });
                    }
                }
            }
        }

        // A vendor spelling must not reach a ticker string held only by other entries. A
        // spelling of a string the entry itself holds is governed by the dated holdings.
        for ((vendor, spelling), owner) in &map.spellings {
            let holdings = map.holders.get(spelling).map_or(&[][..], Vec::as_slice);
            if holdings.iter().all(|h| h.owner != *owner)
                && let Some(other) = holdings.first()
            {
                return Err(MapError::SpellingCollision {
                    vendor: *vendor,
                    spelling: spelling.clone(),
                    owner: owner.clone(),
                    other: other.owner.clone(),
                });
            }
        }

        for members in map.groups.values_mut() {
            members.sort();
        }
        Ok(map)
    }

    /// Records `spelling` (normalized) as `canonical`'s spelling for `vendor`.
    fn claim_spelling(
        &mut self,
        canonical: &Canon,
        vendor: Vendor,
        spelling: &str,
    ) -> Result<(), MapError> {
        let normalized = normalize(spelling).map_err(|source| MapError::InvalidSpelling {
            canonical: canonical.clone(),
            field: vendor.as_str(),
            source,
        })?;
        if let Some(first) = self.spellings.get(&(vendor, normalized.clone())) {
            return Err(MapError::DuplicateSpelling {
                vendor,
                spelling: normalized,
                first: first.clone(),
                second: canonical.clone(),
            });
        }
        self.spellings
            .insert((vendor, normalized), canonical.clone());
        Ok(())
    }

    /// Checks DECISIONS B1 against the issuer key of `security_master`: every two mapped
    /// tickers with the same CIK must share one lock group. Tickers absent from
    /// `issuer_of`, and keys of `issuer_of` absent from the map, are ignored.
    ///
    /// # Errors
    ///
    /// [`MapError::IssuerGroupConflict`] for the first issuer split across groups (in
    /// canonical-ticker order). A caller must treat this like a failed load.
    pub fn validate_issuers(&self, issuer_of: &HashMap<Canon, Cik>) -> Result<(), MapError> {
        let mut seen: HashMap<Cik, &SymbolEntry> = HashMap::new();
        for entry in self.entries.values() {
            let Some(cik) = issuer_of.get(&entry.canonical) else {
                continue;
            };
            match seen.get(cik) {
                Some(first) if first.lock_group != entry.lock_group => {
                    return Err(MapError::IssuerGroupConflict {
                        cik: *cik,
                        first: first.canonical.clone(),
                        first_group: first.lock_group.clone(),
                        second: entry.canonical.clone(),
                        second_group: entry.lock_group.clone(),
                    });
                }
                Some(_) => {}
                None => {
                    seen.insert(*cik, entry);
                }
            }
        }
        Ok(())
    }

    /// Checks DECISIONS B5 against `security_master.prior_tickers`, in both directions, for
    /// every mapped security that appears in `prior_of`: each alias must match one of its
    /// prior tickers exactly (same ticker, `until` equal to `to`, and `from` equal when the
    /// alias has one), and each prior ticker must have such an alias. A typo in an alias date
    /// or a forgotten alias then fails the load instead of attributing an order to the wrong
    /// security, or to none. Securities absent from `prior_of` are not checked.
    ///
    /// # Errors
    ///
    /// [`MapError::PriorTickerMismatch`] for the first uncorroborated alias, else
    /// [`MapError::MissingAlias`] for the first prior ticker without an alias (in
    /// canonical-ticker order). A caller must treat either like a failed load.
    pub fn validate_prior_tickers(
        &self,
        prior_of: &HashMap<Canon, Vec<PriorTicker>>,
    ) -> Result<(), MapError> {
        for entry in self.entries.values() {
            let Some(prior) = prior_of.get(&entry.canonical) else {
                continue;
            };
            for alias in &entry.aliases {
                if !prior.iter().any(|p| corroborates(p, alias)) {
                    return Err(MapError::PriorTickerMismatch {
                        owner: entry.canonical.clone(),
                        alias: alias.symbol.clone(),
                        from: alias.from,
                        until: alias.until,
                    });
                }
            }
            for p in prior {
                if !entry.aliases.iter().any(|a| corroborates(p, a)) {
                    return Err(MapError::MissingAlias {
                        owner: entry.canonical.clone(),
                        ticker: p.ticker.clone(),
                        from: p.from,
                        to: p.to,
                    });
                }
            }
        }
        Ok(())
    }

    /// Number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True if the map has no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Entries in canonical-ticker order.
    pub fn entries(&self) -> impl Iterator<Item = &SymbolEntry> {
        self.entries.values()
    }

    /// The entry of a canonical ticker; `None` if unmapped.
    #[must_use]
    pub fn get(&self, canonical: &Canon) -> Option<&SymbolEntry> {
        self.entries.get(canonical)
    }

    /// A vendor spelling as written in the map; `None` if unmapped or absent.
    #[must_use]
    pub fn spelling(&self, canonical: &Canon, vendor: Vendor) -> Option<&str> {
        self.get(canonical).and_then(|e| e.spelling(vendor))
    }

    /// Resolves a vendor spelling to its canonical ticker **today** (for order entry and
    /// current data; history goes through [`SymbolMap::resolve_on`] or
    /// [`SymbolMap::resolve_history`]). The spelling is normalized first, then looked up in
    /// that vendor's column only; an entry without a spelling for `vendor` is not reachable
    /// from it. [`Vendor::Sec`] has no column: its spelling resolves when its normalization
    /// is a mapped canonical ticker. Unmapped, unparseable and superseded spellings give
    /// `None` (fail closed).
    #[must_use]
    pub fn resolve(&self, spelling: &str, vendor: Vendor) -> Option<&Canon> {
        let normalized = normalize(spelling).ok()?;
        match vendor {
            Vendor::Sec => self.entries.get(&normalized).map(|e| &e.canonical),
            Vendor::Eodhd | Vendor::Yahoo | Vendor::IShares | Vendor::Schwab => {
                self.spellings.get(&(vendor, normalized))
            }
        }
    }

    /// Resolves an old ticker through the `aliases` lists: `Some` only if `on` lies within
    /// the alias's `from` (if any) through `until`.
    #[must_use]
    pub fn resolve_alias(&self, spelling: &str, on: NaiveDate) -> Option<&Canon> {
        let normalized = normalize(spelling).ok()?;
        self.holders
            .get(&normalized)?
            .iter()
            .find(|h| h.kind == "alias" && h.covers(on))
            .map(|h| &h.owner)
    }

    /// Resolves a spelling to the one security that held it on `on`. A string with dated
    /// holdings (a canonical ticker or an alias) resolves through those holdings only, and
    /// to `None` on a date no holding covers; any other spelling resolves through `vendor`'s
    /// column, provided the entry's `from` is not after `on`.
    #[must_use]
    pub fn resolve_on(&self, spelling: &str, vendor: Vendor, on: NaiveDate) -> Option<&Canon> {
        let normalized = normalize(spelling).ok()?;
        if let Some(holdings) = self.holders.get(&normalized) {
            return holdings.iter().find(|h| h.covers(on)).map(|h| &h.owner);
        }
        self.vendor_column(normalized, vendor)
            .filter(|c| self.held_from(c, on, Days::new(0)))
    }

    /// Every security a history record under `spelling` dated `on` may belong to, sorted
    /// (DECISIONS B5). As [`SymbolMap::resolve_on`], but each bounded side of every holding
    /// is widened by `margin`: near a handover the record counts against both securities,
    /// and a stale old ticker typed shortly after a rename still reaches the renamed
    /// security. Over-blocking is the fail-closed direction. The lock history loader passes
    /// its entered-time lookback (09 §3.8). An empty result means unmapped: the record is
    /// carried under its normalized raw symbol, never dropped.
    #[must_use]
    pub fn resolve_history(
        &self,
        spelling: &str,
        vendor: Vendor,
        on: NaiveDate,
        margin: Days,
    ) -> Vec<&Canon> {
        let Ok(normalized) = normalize(spelling) else {
            return Vec::new();
        };
        let mut owners: Vec<&Canon> = self.holders.get(&normalized).map_or_else(
            || {
                self.vendor_column(normalized, vendor)
                    .filter(|c| self.held_from(c, on, margin))
                    .into_iter()
                    .collect()
            },
            |holdings| {
                holdings
                    .iter()
                    .filter(|h| h.covers_widened(on, margin))
                    .map(|h| &h.owner)
                    .collect()
            },
        );
        owners.sort();
        owners.dedup();
        owners
    }

    /// True if `canonical` is mapped and its `from` (moved earlier by `margin`) is not after
    /// `on`.
    fn held_from(&self, canonical: &Canon, on: NaiveDate, margin: Days) -> bool {
        self.entries.get(canonical).is_some_and(|e| {
            e.from
                .is_none_or(|f| f.checked_sub_days(margin).is_none_or(|f| f <= on))
        })
    }

    /// The canonical ticker claiming the normalized `spelling` in `vendor`'s column.
    fn vendor_column(&self, normalized: Canon, vendor: Vendor) -> Option<&Canon> {
        match vendor {
            Vendor::Sec => None,
            Vendor::Eodhd | Vendor::Yahoo | Vendor::IShares | Vendor::Schwab => {
                self.spellings.get(&(vendor, normalized))
            }
        }
    }

    /// The lock group of a canonical ticker; `None` if unmapped.
    #[must_use]
    pub fn lock_group(&self, canonical: &Canon) -> Option<&str> {
        self.get(canonical).map(SymbolEntry::lock_group)
    }

    /// Every canonical ticker in a lock group, sorted; `None` for an unknown group.
    #[must_use]
    pub fn group_members(&self, lock_group: &str) -> Option<&[Canon]> {
        self.groups.get(lock_group).map(Vec::as_slice)
    }

    /// Every canonical ticker in the lock group of `canonical` (itself included), sorted;
    /// `None` if `canonical` is unmapped.
    #[must_use]
    pub fn group_members_of(&self, canonical: &Canon) -> Option<&[Canon]> {
        self.lock_group(canonical)
            .and_then(|g| self.group_members(g))
    }
}
