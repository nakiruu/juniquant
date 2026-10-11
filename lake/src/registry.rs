//! `schemas/registry.json` (format_version 2, `13-parquet-lake/SKILL.md` §6): the part of each
//! entry the writer needs. Other entry fields (`writer`, `schemas_md`, `last_migration`) are
//! ignored here.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use arrow::datatypes::{DataType, Field, Schema, SchemaRef, TimeUnit};
use serde::Deserialize;

use crate::LakeError;

/// The registry format this crate reads.
pub const FORMAT_VERSION: u32 = 2;

/// The parsed registry.
#[derive(Debug, Clone, Deserialize)]
pub struct Registry {
    /// Must equal [`FORMAT_VERSION`].
    pub format_version: u32,
    /// Entries by dataset name.
    pub datasets: BTreeMap<String, DatasetSpec>,
}

/// `single` or `partitioned`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    /// `lake/<name>.parquet`.
    Single,
    /// `lake/<name>/<k>=<v>/.../data.parquet`.
    Partitioned,
}

/// Primary-key collision rule (`IO-CONTRACT.md` §3 step 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Merge {
    /// The new row wins.
    Overwrite,
    /// A differing row is an error; an identical row is dropped.
    AppendOnly,
}

/// One stored column.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FieldSpec {
    /// Column name.
    pub name: String,
    /// Arrow type string as in `SCHEMAS.md`.
    #[serde(rename = "type")]
    pub ty: String,
    /// Whether nulls are allowed.
    pub nullable: bool,
}

/// One registry entry.
#[derive(Debug, Clone, Deserialize)]
pub struct DatasetSpec {
    /// `SKILL.md` §3 number.
    pub number: u32,
    /// Physical layout.
    pub layout: Layout,
    /// Directory keys in order; empty for a single file.
    pub partition_keys: Vec<String>,
    /// Stored column that places a row in a leaf; `None` for a single file.
    pub partition_column: Option<String>,
    /// Stored columns in order.
    pub fields: Vec<FieldSpec>,
    /// Primary key; empty when the dataset has none (`audit_log`).
    pub primary_key: Vec<String>,
    /// Row order of every leaf.
    pub sort_key: Vec<String>,
    /// Collision rule.
    pub merge: Merge,
}

impl Registry {
    /// Read and check a registry file.
    ///
    /// # Errors
    ///
    /// [`LakeError::Registry`] if the file cannot be read or parsed, has another
    /// `format_version`, or an entry is inconsistent ([`DatasetSpec::check`]).
    pub fn load(path: &Path) -> Result<Self, LakeError> {
        let err = |message: String| LakeError::Registry {
            path: path.to_path_buf(),
            message,
        };
        let text = std::fs::read_to_string(path).map_err(|e| err(e.to_string()))?;
        let registry: Self = serde_json::from_str(&text).map_err(|e| err(e.to_string()))?;
        if registry.format_version != FORMAT_VERSION {
            return Err(err(format!(
                "format_version {} (expected {FORMAT_VERSION})",
                registry.format_version
            )));
        }
        for (name, spec) in &registry.datasets {
            spec.check().map_err(|m| err(format!("{name}: {m}")))?;
        }
        Ok(registry)
    }

    /// The entry for `name`.
    ///
    /// # Errors
    ///
    /// [`LakeError::UnknownDataset`] if it is not registered.
    pub fn get(&self, name: &str) -> Result<&DatasetSpec, LakeError> {
        self.datasets
            .get(name)
            .ok_or_else(|| LakeError::UnknownDataset(name.to_owned()))
    }
}

impl DatasetSpec {
    /// The Arrow schema every leaf is written with.
    ///
    /// # Errors
    ///
    /// A message if a field type is outside the `SKILL.md` §6 list.
    pub fn schema(&self) -> Result<SchemaRef, String> {
        let fields = self
            .fields
            .iter()
            .map(|f| Ok(Field::new(&f.name, arrow_type(&f.ty)?, f.nullable)))
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Arc::new(Schema::new(fields)))
    }

    /// Index of a stored column.
    pub(crate) fn column_index(&self, name: &str) -> Option<usize> {
        self.fields.iter().position(|f| f.name == name)
    }

    /// Internal consistency: known types, keys name stored columns, partition keys are not
    /// stored columns, layout and partition fields agree.
    ///
    /// # Errors
    ///
    /// A message naming the first inconsistency.
    pub fn check(&self) -> Result<(), String> {
        self.schema()?;
        for k in self.primary_key.iter().chain(&self.sort_key) {
            if self.column_index(k).is_none() {
                return Err(format!("key column `{k}` is not a stored column"));
            }
        }
        if self.sort_key.is_empty() {
            return Err("empty sort_key".into());
        }
        if !self.primary_key.is_empty() && self.sort_key != self.primary_key {
            return Err("sort_key must equal primary_key when there is one".into());
        }
        for k in &self.partition_keys {
            if !matches!(k.as_str(), "year" | "month" | "date" | "session") {
                return Err(format!("unknown partition key `{k}`"));
            }
            if self.column_index(k).is_some() {
                return Err(format!("partition key `{k}` is also a stored column"));
            }
        }
        match (self.layout, &self.partition_column) {
            (Layout::Single, None) if self.partition_keys.is_empty() => Ok(()),
            (Layout::Partitioned, Some(c)) if !self.partition_keys.is_empty() => {
                match self
                    .column_index(c)
                    .map(|i| self.fields.get(i).map(|f| f.ty.as_str()))
                {
                    Some(Some("date32" | "timestamp[ns, UTC]")) => Ok(()),
                    _ => Err(format!(
                        "partition_column `{c}` must be a date32 or timestamp column"
                    )),
                }
            }
            _ => Err("layout, partition_keys and partition_column disagree".into()),
        }
    }
}

/// Arrow type for a `SCHEMAS.md` type string.
fn arrow_type(ty: &str) -> Result<DataType, String> {
    Ok(match ty {
        "utf8" => DataType::Utf8,
        "bool" => DataType::Boolean,
        "int16" => DataType::Int16,
        "int32" => DataType::Int32,
        "int64" => DataType::Int64,
        "float64" => DataType::Float64,
        "date32" => DataType::Date32,
        "timestamp[ns, UTC]" => DataType::Timestamp(TimeUnit::Nanosecond, Some("UTC".into())),
        other => return Err(format!("unknown type `{other}`")),
    })
}
