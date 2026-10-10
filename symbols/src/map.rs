//! The canonical symbol map [`SymbolMap`] over `config/symbol_map.toml`
//! (09-lock-state-machine SKILL.md §3.8).
//!
//! Every load failure is a typed [`MapError`]: a map that does not load denies every lock
//! check (fail closed). An unmapped spelling resolves to `None`, never to a guess.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use chrono::NaiveDate;
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
    yahoo: Option<String>,
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
    until: String,
}

/// An old ticker (after a rename) that maps to the entry's canonical ticker through
/// `until`, inclusive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alias {
    symbol: Canon,
    until: NaiveDate,
}

impl Alias {
    /// The old ticker, normalized.
    #[must_use]
    pub fn symbol(&self) -> &Canon {
        &self.symbol
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
    yahoo: Option<String>,
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

    /// The vendor spelling exactly as written in the map (`None` if absent, and always
    /// `None` for [`Vendor::Sec`], which has no column).
    #[must_use]
    pub fn spelling(&self, vendor: Vendor) -> Option<&str> {
        match vendor {
            Vendor::Yahoo => self.yahoo.as_deref(),
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
        /// `yahoo`, `ishares`, `schwab` or `alias`.
        field: &'static str,
        /// The normalization failure.
        source: SymbolError,
    },
    /// An alias `until` is not a `YYYY-MM-DD` date.
    #[error("alias {alias} of {canonical}: invalid until date {value:?}")]
    InvalidDate {
        /// The entry it belongs to.
        canonical: Canon,
        /// The alias symbol.
        alias: Canon,
        /// The value as written.
        value: String,
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
    /// An alias equals a canonical ticker or another alias.
    #[error("alias {alias} of {owner} collides with {conflict} of {other}")]
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
}

impl MapError {
    /// Stable machine-readable kind: `io`, `toml`, `not_canonical`, `invalid_spelling`,
    /// `invalid_date`, `empty_lock_group`, `duplicate_canonical`, `duplicate_spelling`,
    /// `alias_collision` or `issuer_group_conflict`.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Io { .. } => "io",
            Self::Toml(_) => "toml",
            Self::NotCanonical { .. } => "not_canonical",
            Self::InvalidSpelling { .. } => "invalid_spelling",
            Self::InvalidDate { .. } => "invalid_date",
            Self::EmptyLockGroup { .. } => "empty_lock_group",
            Self::DuplicateCanonical { .. } => "duplicate_canonical",
            Self::DuplicateSpelling { .. } => "duplicate_spelling",
            Self::AliasCollision { .. } => "alias_collision",
            Self::IssuerGroupConflict { .. } => "issuer_group_conflict",
        }
    }
}

/// The validated canonical symbol map.
///
/// Lookups:
/// - [`SymbolMap::resolve`]: a vendor spelling to its canonical ticker (normalized first);
/// - [`SymbolMap::resolve_alias`] / [`SymbolMap::resolve_on`]: old tickers with dates;
/// - [`SymbolMap::lock_group`], [`SymbolMap::group_members`]: the B1 issuer groups.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SymbolMap {
    entries: BTreeMap<Canon, SymbolEntry>,
    spellings: BTreeMap<(Vendor, Canon), Canon>,
    aliases: BTreeMap<Canon, (Canon, NaiveDate)>,
    groups: BTreeMap<String, Vec<Canon>>,
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
    /// `canonical`, a spelling or alias that does not normalize, a bad date, an empty
    /// lock group, a duplicate canonical, two entries claiming one vendor spelling, or an
    /// alias colliding with a canonical ticker or another alias.
    pub fn from_toml_str(text: &str) -> Result<Self, MapError> {
        let raw: RawFile = toml::from_str(text).map_err(Box::new)?;
        let mut map = Self::default();
        let mut pending_aliases = Vec::new();

        for raw_entry in raw.symbol {
            let canonical = Canon::from_str(&raw_entry.canonical)
                .map_err(|e| MapError::NotCanonical { value: e.input })?;
            if map.entries.contains_key(&canonical) {
                return Err(MapError::DuplicateCanonical { canonical });
            }
            let lock_group = raw_entry.lock_group.trim().to_owned();
            if lock_group.is_empty() {
                return Err(MapError::EmptyLockGroup { canonical });
            }
            for (vendor, spelling) in [
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
                let until =
                    NaiveDate::parse_from_str(&raw_alias.until, "%Y-%m-%d").map_err(|_| {
                        MapError::InvalidDate {
                            canonical: canonical.clone(),
                            alias: symbol.clone(),
                            value: raw_alias.until.clone(),
                        }
                    })?;
                aliases.push(Alias { symbol, until });
            }
            pending_aliases.extend(aliases.iter().map(|a| (a.clone(), canonical.clone())));
            map.groups
                .entry(lock_group.clone())
                .or_default()
                .push(canonical.clone());
            map.entries.insert(
                canonical.clone(),
                SymbolEntry {
                    canonical,
                    yahoo: raw_entry.yahoo,
                    ishares: raw_entry.ishares,
                    schwab: raw_entry.schwab,
                    lock_group,
                    aliases,
                },
            );
        }

        // Aliases are checked once every canonical ticker is known.
        for (alias, owner) in pending_aliases {
            if map.entries.contains_key(&alias.symbol) {
                return Err(MapError::AliasCollision {
                    other: alias.symbol.clone(),
                    alias: alias.symbol,
                    owner,
                    conflict: "canonical",
                });
            }
            if let Some((other, _)) = map.aliases.get(&alias.symbol) {
                return Err(MapError::AliasCollision {
                    other: other.clone(),
                    alias: alias.symbol,
                    owner,
                    conflict: "alias",
                });
            }
            map.aliases.insert(alias.symbol, (owner, alias.until));
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

    /// Resolves a vendor spelling to its canonical ticker. The spelling is normalized
    /// first, then looked up in that vendor's column only; an entry without a spelling for
    /// `vendor` is not reachable from it. [`Vendor::Sec`] has no column: its spelling
    /// resolves when its normalization is a mapped canonical ticker. Unmapped,
    /// unparseable and superseded spellings give `None` (fail closed).
    #[must_use]
    pub fn resolve(&self, spelling: &str, vendor: Vendor) -> Option<&Canon> {
        let normalized = normalize(spelling).ok()?;
        match vendor {
            Vendor::Sec => self.entries.get(&normalized).map(|e| &e.canonical),
            Vendor::Yahoo | Vendor::IShares | Vendor::Schwab => {
                self.spellings.get(&(vendor, normalized))
            }
        }
    }

    /// Resolves an old ticker through the `aliases` lists: `Some` only if `on` is on or
    /// before the alias's `until` date.
    #[must_use]
    pub fn resolve_alias(&self, spelling: &str, on: NaiveDate) -> Option<&Canon> {
        let normalized = normalize(spelling).ok()?;
        self.aliases
            .get(&normalized)
            .and_then(|(canonical, until)| (on <= *until).then_some(canonical))
    }

    /// [`SymbolMap::resolve`], falling back to [`SymbolMap::resolve_alias`] at date `on`.
    #[must_use]
    pub fn resolve_on(&self, spelling: &str, vendor: Vendor, on: NaiveDate) -> Option<&Canon> {
        self.resolve(spelling, vendor)
            .or_else(|| self.resolve_alias(spelling, on))
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
