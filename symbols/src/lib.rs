//! Canonical ticker symbols shared by `ingest` and `execution`.
//!
//! Specification: `.claude/skills/01-data-ingestion/UNIVERSE.md` §3 (normalization grammar,
//! test vectors in `schemas/fixtures/symbol_normalization.csv` and
//! `schemas/fixtures/symbol_render.csv`) and `.claude/skills/09-lock-state-machine/SKILL.md`
//! §3.8 (one canonical symbol map; share classes of one issuer form one lock group, owner
//! decision B1; superseded `SYMBOL~CIK10` tickers are rejected; unmapped fails closed).
//!
//! - [`Canon`]: a validated canonical ticker, `[A-Z0-9]{1,6} ( "." [A-Z] )?`.
//! - [`normalize`]: raw vendor input to [`Canon`], with [`SymbolError::kind`] strings.
//! - [`is_tradable`]: true iff a string is already exactly canonical.
//! - [`Cik`], [`SupersededTicker`]: the `SYMBOL~CIK10` form, never convertible to [`Canon`].
//! - [`Vendor`], [`render`], [`render_str`]: canonical to vendor spelling.
//! - [`SymbolMap`]: `config/symbol_map.toml`, validated at load (fail closed).

mod canon;
mod cik;
mod map;
mod vendor;

pub use canon::{Canon, NotCanonical, SymbolError, is_tradable, normalize};
pub use cik::{Cik, CikError, SupersededError, SupersededTicker};
pub use map::{Alias, MapError, SymbolEntry, SymbolMap};
pub use vendor::{RenderError, UnknownVendor, Vendor, render, render_str};
