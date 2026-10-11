//! Lake I/O contract: `write_partition` / `write_dataset` / `read_dataset`, atomic temp-file +
//! rename per partition, merge on the primary key, idempotent re-runs.
//!
//! Specification: `.claude/skills/13-parquet-lake/IO-CONTRACT.md` (the registry fields used here
//! are `SKILL.md` §6). Every dataset's columns, keys, layout and merge rule come from
//! `schemas/registry.json`; nothing is hard-coded per dataset.
//!
//! The library takes paths as arguments ([`Lake::new`]); only [`Lake::from_env`] and the
//! binaries read `QUANT_ROOT` / `QUANT_STATE` / `QUANT_REGISTRY_PATH`.

mod error;
mod read;
mod registry;
mod write;

pub use error::LakeError;
pub use registry::{DatasetSpec, FORMAT_VERSION, FieldSpec, Layout, Merge, Registry};
pub use write::{Mode, ROW_GROUP_ROWS};

use std::path::{Path, PathBuf};

/// A lake rooted at one directory, with its registry and its lock directory.
#[derive(Debug, Clone)]
pub struct Lake {
    root: PathBuf,
    locks: PathBuf,
    registry: Registry,
}

impl Lake {
    /// A lake at `root` (the `lake/` directory itself), taking single-writer locks in
    /// `locks` (`$QUANT_STATE/locks` in production).
    pub fn new(root: impl Into<PathBuf>, locks: impl Into<PathBuf>, registry: Registry) -> Self {
        Self {
            root: root.into(),
            locks: locks.into(),
            registry,
        }
    }

    /// The production lake: `$QUANT_ROOT/lake`, locks in `$QUANT_STATE/locks`, registry at
    /// `$QUANT_REGISTRY_PATH` (default `schemas/registry.json`), `IO-CONTRACT.md` §1.
    ///
    /// # Errors
    ///
    /// [`LakeError::EnvUnset`] if `QUANT_ROOT` or `QUANT_STATE` is unset or empty (there is no
    /// silent default to a production path); a registry load error.
    pub fn from_env() -> Result<Self, LakeError> {
        let var = |name: &'static str| match std::env::var_os(name) {
            Some(v) if !v.is_empty() => Ok(PathBuf::from(v)),
            _ => Err(LakeError::EnvUnset(name)),
        };
        let registry_path = std::env::var_os("QUANT_REGISTRY_PATH")
            .filter(|v| !v.is_empty())
            .map_or_else(|| PathBuf::from("schemas/registry.json"), PathBuf::from);
        let registry = Registry::load(&registry_path)?;
        Ok(Self::new(
            var("QUANT_ROOT")?.join("lake"),
            var("QUANT_STATE")?.join("locks"),
            registry,
        ))
    }

    /// The lake directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The registry this lake validates against.
    pub fn registry(&self) -> &Registry {
        &self.registry
    }
}
