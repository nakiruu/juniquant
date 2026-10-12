//! The raw landing zone (`BACKFILL.md` §3): every response body kept, ZSTD-compressed (a
//! `.zip` is kept as downloaded), with a JSON sidecar, written temp + fsync + rename, sidecar
//! first so a body without a sidecar is the only possible torn state.
//!
//! ```text
//! raw/{source}/{entity}/{chunk}/{fetched_at_utc}__{sha8}.{ext}[.zst]
//! raw/{source}/{entity}/{chunk}/{fetched_at_utc}__{sha8}.{ext}.meta.json
//! ```

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// What the sidecar records about the request (`BACKFILL.md` §3).
#[derive(Debug, Clone)]
pub struct Meta {
    /// Request URL with every secret already redacted.
    pub url: String,
    /// HTTP status.
    pub status: u16,
    /// Selected response headers.
    pub headers: Vec<(String, String)>,
    /// When the response arrived.
    pub fetched_at: DateTime<Utc>,
    /// Attempt number of this response (1 = first try).
    pub attempt: u32,
}

/// A landed body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Landed {
    /// The body file.
    pub path: PathBuf,
    /// SHA-256 of the uncompressed body, hex.
    pub sha256: String,
    /// Uncompressed length.
    pub bytes: u64,
    /// False when an identical body was already landed for this chunk (nothing written).
    pub new: bool,
}

/// Response headers worth keeping.
pub const KEEP_HEADERS: &[&str] = &[
    "date",
    "etag",
    "last-modified",
    "content-type",
    "content-length",
];

/// Reject a path component that could leave the landing zone or hide as a dot-file.
fn component(s: &str) -> io::Result<&str> {
    let ok = !s.is_empty()
        && !s.starts_with('.')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._=-^".contains(&b));
    if ok {
        Ok(s)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("bad landing path component {s:?}"),
        ))
    }
}

/// SHA-256 of `bytes`, hex.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Land `body` for `(source, entity, chunk)`. A body whose SHA-256 is already landed for the
/// chunk writes nothing and returns that file (`new = false`).
///
/// # Errors
///
/// An invalid path component, or an I/O error (nothing is left half-written under the
/// final names).
pub fn land(
    raw: &Path,
    (source, entity, chunk): (&str, &str, &str),
    ext: &str,
    body: &[u8],
    meta: &Meta,
) -> io::Result<Landed> {
    let dir = raw
        .join(component(source)?)
        .join(component(entity)?)
        .join(component(chunk)?);
    component(ext)?;
    let sha256 = sha256_hex(body);
    let bytes = body.len() as u64;
    if let Some(path) = find(&dir, &sha256)? {
        return Ok(Landed {
            path,
            sha256,
            bytes,
            new: false,
        });
    }
    let created: Vec<PathBuf> = dir
        .ancestors()
        .take_while(|a| !a.exists())
        .map(Path::to_path_buf)
        .collect();
    fs::create_dir_all(&dir)?;
    for parent in created.iter().filter_map(|c| c.parent()) {
        File::open(parent)?.sync_all()?;
    }
    let stem = format!(
        "{}__{}",
        meta.fetched_at.format("%Y%m%dT%H%M%SZ"),
        sha256.get(..8).unwrap_or(&sha256)
    );
    let body_name = if ext == "zip" {
        format!("{stem}.zip")
    } else {
        format!("{stem}.{ext}.zst")
    };
    let sidecar = json!({
        "url": meta.url,
        "status": meta.status,
        "headers": meta.headers.iter().map(|(k, v)| (k.clone(), Value::from(v.as_str()))).collect::<serde_json::Map<_, _>>(),
        "fetched_at": meta.fetched_at.to_rfc3339(),
        "client": concat!("quant-ingest ", env!("CARGO_PKG_VERSION")),
        "attempt": meta.attempt,
        "sha256": sha256,
        "bytes": bytes,
        "body": body_name,
    });
    // A zip is already compressed; borrowing it avoids a second copy of a 1.5 GB body.
    let encoded: std::borrow::Cow<'_, [u8]> = if ext == "zip" {
        body.into()
    } else {
        zstd::encode_all(body, 3)?.into()
    };
    write_atomic(
        &dir.join(format!("{stem}.{ext}.meta.json")),
        sidecar.to_string().as_bytes(),
    )?;
    let path = dir.join(&body_name);
    write_atomic(&path, &encoded)?;
    File::open(&dir)?.sync_all()?;
    Ok(Landed {
        path,
        sha256,
        bytes,
        new: true,
    })
}

/// The landed body of `dir` with this SHA-256, if any (sidecars are the index).
fn find(dir: &Path, sha256: &str) -> io::Result<Option<PathBuf>> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let path = entry?.path();
        if !path.to_string_lossy().ends_with(".meta.json") {
            continue;
        }
        let side: Value = serde_json::from_slice(&fs::read(&path)?).unwrap_or(Value::Null);
        if side.get("sha256").and_then(Value::as_str) == Some(sha256)
            && let Some(body) = side.get("body").and_then(Value::as_str)
        {
            return Ok(Some(dir.join(body)));
        }
    }
    Ok(None)
}

/// The uncompressed bytes of a landed body.
///
/// # Errors
///
/// I/O or decompression errors.
pub fn read(path: &Path) -> io::Result<Vec<u8>> {
    let data = fs::read(path)?;
    if path.extension().is_some_and(|e| e == "zst") {
        zstd::decode_all(data.as_slice())
    } else {
        Ok(data)
    }
}

fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("body");
    let tmp = dir.join(format!(".{name}.tmp.{}", std::process::id()));
    let result = (|| {
        let mut f = File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing, reason = "test")]

    use super::*;

    fn meta() -> Meta {
        Meta {
            url: "https://x/y?api_token=REDACTED".into(),
            status: 200,
            headers: vec![("etag".into(), "\"1\"".into())],
            fetched_at: DateTime::from_timestamp(1_760_000_000, 0).unwrap(),
            attempt: 1,
        }
    }

    #[test]
    fn lands_once_per_body_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let key = ("eodhd", "MON_old.US", "eod-full");
        let a = land(dir.path(), key, "json", b"[1,2]", &meta()).unwrap();
        assert!(a.new && a.path.to_string_lossy().ends_with(".json.zst"));
        assert_eq!(read(&a.path).unwrap(), b"[1,2]");
        let again = land(dir.path(), key, "json", b"[1,2]", &meta()).unwrap();
        assert_eq!(
            (again.new, &again.path),
            (false, &a.path),
            "same body: nothing new"
        );
        let changed = land(dir.path(), key, "json", b"[1,2,3]", &meta()).unwrap();
        assert!(
            changed.new && changed.path != a.path,
            "a different body lands next to the old one"
        );
        let files = fs::read_dir(a.path.parent().unwrap()).unwrap().count();
        assert_eq!(files, 4, "two bodies, two sidecars, no temporaries");
    }

    #[test]
    fn scan_reports_landed_bodies_and_quarantines_torn_ones() {
        let dir = tempfile::tempdir().unwrap();
        let a = land(
            dir.path(),
            ("eodhd", "AAPL.US", "eod-full"),
            "json",
            b"[1]",
            &meta(),
        )
        .unwrap();
        let torn = a
            .path
            .parent()
            .unwrap()
            .join("20260101T000000Z__deadbeef.json.zst");
        fs::write(&torn, b"x").unwrap();
        let found = scan(dir.path()).unwrap();
        assert_eq!(found.len(), 2);
        assert!(
            matches!(&found[0], Found::Landed(k, l, 200) if k.1 == "AAPL.US" && l.path == a.path)
        );
        assert!(
            matches!(&found[1], Found::Orphan(p) if p.starts_with(dir.path().join("_orphans")))
        );
        assert!(!torn.exists());
        assert_eq!(
            scan(dir.path()).unwrap().len(),
            1,
            "orphans are not rescanned"
        );
    }

    #[test]
    fn path_components_cannot_escape() {
        let dir = tempfile::tempdir().unwrap();
        for bad in ["..", ".hidden", "a/b", ""] {
            assert!(
                land(dir.path(), ("eodhd", bad, "c"), "json", b"x", &meta()).is_err(),
                "{bad:?}"
            );
        }
    }
}

/// A raw file the manifest did not record, found on resume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    /// A sidecar with its body: `(source, entity, chunk)`, the body and its SHA-256.
    Landed((String, String, String), Landed, u16),
    /// A body without a sidecar (a torn write), moved to `raw/_orphans/`.
    Orphan(PathBuf),
}

/// Resume rule 2 (`BACKFILL.md` §4): every landed body with its sidecar, newest last per
/// chunk; bodies without a sidecar are moved to `raw/_orphans/` and reported.
///
/// # Errors
///
/// I/O errors.
pub fn scan(raw: &Path) -> io::Result<Vec<Found>> {
    let mut out = Vec::new();
    let level = |p: &Path| -> io::Result<Vec<PathBuf>> {
        let mut v: Vec<PathBuf> = match fs::read_dir(p) {
            Ok(e) => e.map(|e| e.map(|e| e.path())).collect::<io::Result<_>>()?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e),
        };
        v.retain(|p| {
            p.file_name()
                .is_some_and(|n| !n.to_string_lossy().starts_with(['.', '_']))
        });
        v.sort();
        Ok(v)
    };
    let name = |p: &Path| {
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    for source in level(raw)? {
        for entity in level(&source)? {
            for chunk in level(&entity)? {
                let files = level(&chunk)?;
                let mut claimed = Vec::new();
                for side in files.iter().filter(|p| name(p).ends_with(".meta.json")) {
                    let v: Value = serde_json::from_slice(&fs::read(side)?).unwrap_or(Value::Null);
                    let (Some(sha), Some(body)) = (
                        v.get("sha256").and_then(Value::as_str),
                        v.get("body").and_then(Value::as_str),
                    ) else {
                        continue;
                    };
                    let path = chunk.join(body);
                    if path.exists() {
                        claimed.push(path.clone());
                        let bytes = v.get("bytes").and_then(Value::as_u64).unwrap_or_default();
                        let status = v
                            .get("status")
                            .and_then(Value::as_u64)
                            .and_then(|s| u16::try_from(s).ok())
                            .unwrap_or_default();
                        let key = (name(&source), name(&entity), name(&chunk));
                        out.push(Found::Landed(
                            key,
                            Landed {
                                path,
                                sha256: sha.to_owned(),
                                bytes,
                                new: false,
                            },
                            status,
                        ));
                    }
                }
                for body in files
                    .iter()
                    .filter(|p| !name(p).ends_with(".meta.json") && !claimed.contains(p))
                {
                    let rel = body.strip_prefix(raw).unwrap_or(body);
                    let dest = raw.join("_orphans").join(rel);
                    if let Some(d) = dest.parent() {
                        fs::create_dir_all(d)?;
                    }
                    fs::rename(body, &dest)?;
                    out.push(Found::Orphan(dest));
                }
            }
        }
    }
    Ok(out)
}
