//! Phase 0 probes (`01-data-ingestion/BACKFILL.md` §2): one-off live checks that settle the
//! "verify" items before any bulk fetch. Each probe records its evidence (every request with
//! status, selected headers, size and SHA-256; every raw body; its findings) under
//! `$QUANT_ROOT/reports/probes/`.
//!
//! shortcut: blocking HTTP and a per-source minimum interval instead of the token bucket and
//! manifest of the I2 ingest infrastructure (`SKILL.md` §4-§5); probes run once, by hand.

mod p5_edgar;
mod p6_alfred;
mod yahoo;

use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use chrono::Utc;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// Why a probe could not run or record its evidence. A probe whose *finding* is bad (a 404,
/// a 429) is not an error: the finding is the result.
#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    /// A required environment variable is unset or empty.
    #[error("{0} is not set")]
    EnvUnset(&'static str),
    /// Unknown probe name.
    #[error("unknown probe `{0}` (p1-p6 are implemented)")]
    Unknown(String),
    /// The HTTP client failed to build or the request did not complete (DNS, TLS, timeout).
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    /// Writing evidence failed.
    #[error("{path}: {source}")]
    Io {
        /// File involved.
        path: PathBuf,
        /// Underlying error.
        source: std::io::Error,
    },
    /// A response that should be JSON is not, or lacks a field the probe needs.
    #[error("{0}")]
    Parse(String),
}

/// A data source, with its pacing and headers (`SKILL.md` §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    /// Yahoo chart API.
    Yahoo,
    /// SEC EDGAR (needs `SEC_EDGAR_EMAIL`).
    Edgar,
    /// FRED / ALFRED (key passed as `api_key=` in the URL, redacted in the log).
    Fred,
    /// iShares holdings files.
    #[allow(dead_code, reason = "used by probe P7")]
    IShares,
}

impl Source {
    /// Minimum time between two requests to the source (the sustained rate of `SKILL.md` §5).
    fn min_interval(self) -> Duration {
        Duration::from_millis(match self {
            Self::Yahoo => 2000,
            Self::Edgar => 200,
            Self::Fred => 1000,
            Self::IShares => 5000,
        })
    }
}

/// One HTTP response, body in full.
#[derive(Debug, Clone)]
pub struct Reply {
    /// HTTP status.
    pub status: u16,
    /// Response headers (lower-case names).
    pub headers: Vec<(String, String)>,
    /// Body bytes (decompressed).
    pub body: Vec<u8>,
}

impl Reply {
    /// The body parsed as JSON.
    ///
    /// # Errors
    ///
    /// [`ProbeError::Parse`] if it is not JSON.
    pub fn json(&self) -> Result<Value, ProbeError> {
        serde_json::from_slice(&self.body)
            .map_err(|e| ProbeError::Parse(format!("body is not JSON: {e}")))
    }
}

/// A running probe: paced HTTP plus the evidence it accumulates.
pub struct Probe {
    name: String,
    dir: PathBuf,
    client: reqwest::blocking::Client,
    edgar_ua: Option<String>,
    last: Vec<(Source, Instant)>,
    min_interval: Vec<(Source, Duration)>,
    requests: Vec<Value>,
    findings: serde_json::Map<String, Value>,
}

/// Browser-like agent for Yahoo and iShares (`SOURCES.md`).
const BROWSER_UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36";

/// Response headers worth keeping as evidence.
const KEEP_HEADERS: &[&str] = &[
    "content-type",
    "content-length",
    "last-modified",
    "etag",
    "retry-after",
    "date",
    "x-ratelimit-limit",
    "x-ratelimit-remaining",
    "accept-ranges",
    "content-range",
];

impl Probe {
    /// A probe named `name` writing under `$QUANT_ROOT/reports/probes/`.
    ///
    /// # Errors
    ///
    /// [`ProbeError::EnvUnset`] without `QUANT_ROOT`; client or directory creation errors.
    pub fn new(name: &str) -> Result<Self, ProbeError> {
        let root = std::env::var_os("QUANT_ROOT")
            .filter(|v| !v.is_empty())
            .ok_or(ProbeError::EnvUnset("QUANT_ROOT"))?;
        let dir = PathBuf::from(root)
            .join("reports")
            .join("probes")
            .join(name);
        let raw = dir.join("raw");
        fs::create_dir_all(&raw).map_err(|source| ProbeError::Io { path: raw, source })?;
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()?;
        let edgar_ua = std::env::var("SEC_EDGAR_EMAIL")
            .ok()
            .filter(|e| !e.is_empty())
            .map(|e| format!("juniquant research {e}"));
        Ok(Self {
            name: name.to_owned(),
            dir,
            client,
            edgar_ua,
            last: Vec::new(),
            min_interval: Vec::new(),
            requests: Vec::new(),
            findings: serde_json::Map::new(),
        })
    }

    /// Override a source's pacing for this probe (the rate probe ramps it).
    pub fn set_interval(&mut self, source: Source, interval: Duration) {
        self.min_interval.retain(|(s, _)| *s != source);
        self.min_interval.push((source, interval));
    }

    /// GET `url` from `source`: waits out the source's interval, lands the raw body and logs
    /// the request. A non-2xx status is returned, not an error.
    ///
    /// # Errors
    ///
    /// [`ProbeError::EnvUnset`] for EDGAR without `SEC_EDGAR_EMAIL`; transport and I/O errors.
    pub fn get(&mut self, source: Source, url: &str) -> Result<Reply, ProbeError> {
        self.send(source, reqwest::Method::GET, url, None)
    }

    /// HEAD `url`: paced and logged like [`Probe::get`]; there is no body to land.
    ///
    /// # Errors
    ///
    /// As [`Probe::get`].
    pub fn head(&mut self, source: Source, url: &str) -> Result<Reply, ProbeError> {
        self.send(source, reqwest::Method::HEAD, url, None)
    }

    /// GET bytes `start..=end` of `url` (`Range`, uncompressed so the offsets are the file's);
    /// a 206 is a normal reply. The range body is landed like any other.
    ///
    /// # Errors
    ///
    /// As [`Probe::get`].
    pub fn get_range(
        &mut self,
        source: Source,
        url: &str,
        start: u64,
        end: u64,
    ) -> Result<Reply, ProbeError> {
        self.send(source, reqwest::Method::GET, url, Some((start, end)))
    }

    fn send(
        &mut self,
        source: Source,
        method: reqwest::Method,
        url: &str,
        range: Option<(u64, u64)>,
    ) -> Result<Reply, ProbeError> {
        let interval = self
            .min_interval
            .iter()
            .find(|(s, _)| *s == source)
            .map_or_else(|| source.min_interval(), |(_, d)| *d);
        if let Some((_, at)) = self.last.iter().find(|(s, _)| *s == source) {
            let wait = interval.saturating_sub(at.elapsed());
            thread::sleep(wait);
        }
        let ua = match source {
            Source::Edgar => self
                .edgar_ua
                .clone()
                .ok_or(ProbeError::EnvUnset("SEC_EDGAR_EMAIL"))?,
            _ => BROWSER_UA.to_owned(),
        };
        let fetched_at = Utc::now();
        let started = Instant::now();
        let mut req = self
            .client
            .request(method.clone(), url)
            .header(reqwest::header::USER_AGENT, ua);
        if let Some((start, end)) = range {
            req = req
                .header(reqwest::header::RANGE, format!("bytes={start}-{end}"))
                .header(reqwest::header::ACCEPT_ENCODING, "identity");
        }
        let resp = req.send()?;
        let status = resp.status().as_u16();
        let headers: Vec<(String, String)> = resp
            .headers()
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_owned(),
                    v.to_str().unwrap_or("<binary>").to_owned(),
                )
            })
            .collect();
        let body = resp.bytes()?.to_vec();
        self.last.retain(|(s, _)| *s != source);
        self.last.push((source, Instant::now()));

        let n = self.requests.len() + 1;
        let raw = self.dir.join("raw").join(format!("{n:03}.body"));
        fs::write(&raw, &body).map_err(|e| ProbeError::Io {
            path: raw.clone(),
            source: e,
        })?;
        let kept: serde_json::Map<String, Value> = headers
            .iter()
            .filter(|(k, _)| KEEP_HEADERS.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), Value::from(v.as_str())))
            .collect();
        self.requests.push(json!({
            "n": n,
            "source": format!("{source:?}"),
            "method": method.as_str(),
            "range": range.map(|(s, e)| format!("{s}-{e}")),
            "url": redact(url),
            "status": status,
            "headers": kept,
            "bytes": body.len(),
            "sha256": hex(&Sha256::digest(&body)),
            "fetched_at": fetched_at.to_rfc3339(),
            "elapsed_ms": started.elapsed().as_millis(),
            "raw": raw.strip_prefix(&self.dir).unwrap_or(&raw).display().to_string(),
        }));
        Ok(Reply {
            status,
            headers,
            body,
        })
    }

    /// Record a finding.
    pub fn note(&mut self, key: &str, value: Value) {
        self.findings.insert(key.to_owned(), value);
    }

    /// Write `<dir>.json` (the probe's evidence) and return its path.
    fn finish(self) -> Result<PathBuf, ProbeError> {
        let path = self.dir.with_extension("json");
        let doc = json!({
            "probe": self.name,
            "written_at": Utc::now().to_rfc3339(),
            "findings": self.findings,
            "requests": self.requests,
        });
        let text =
            serde_json::to_string_pretty(&doc).map_err(|e| ProbeError::Parse(e.to_string()))?;
        fs::write(&path, text + "\n").map_err(|source| ProbeError::Io {
            path: path.clone(),
            source,
        })?;
        Ok(path)
    }
}

/// Run probe `name` and write its evidence; returns the evidence path.
///
/// # Errors
///
/// [`ProbeError::Unknown`] for an unimplemented probe, and the probe's own errors. Evidence
/// gathered before an error is still written.
pub fn run(name: &str) -> Result<PathBuf, ProbeError> {
    let f: fn(&mut Probe) -> Result<(), ProbeError> = match name {
        "p1" => yahoo::p1,
        "p2" => yahoo::p2,
        "p3" => yahoo::p3,
        "p4" => yahoo::p4,
        "p5" => p5_edgar::run,
        "p6" => p6_alfred::run,
        other => return Err(ProbeError::Unknown(other.to_owned())),
    };
    let mut probe = Probe::new(name)?;
    let result = f(&mut probe);
    if let Err(e) = &result {
        probe.note("error", Value::from(e.to_string()));
    }
    let path = probe.finish()?;
    result.map(|()| path)
}

/// Replace the value of an `api_key=` query parameter.
fn redact(url: &str) -> String {
    let Some((head, rest)) = url.split_once("api_key=") else {
        return url.to_owned();
    };
    let tail = rest.find('&').and_then(|j| rest.get(j..)).unwrap_or("");
    format!("{head}api_key=REDACTED{tail}")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::redact;

    #[test]
    fn api_key_is_redacted_anywhere_in_the_query() {
        assert_eq!(
            redact("https://x/s?api_key=SECRET&a=1"),
            "https://x/s?api_key=REDACTED&a=1"
        );
        assert_eq!(
            redact("https://x/s?a=1&api_key=SECRET"),
            "https://x/s?a=1&api_key=REDACTED"
        );
        assert_eq!(redact("https://x/s?a=1"), "https://x/s?a=1");
    }
}
