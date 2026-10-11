//! Fetch one chunk (`SKILL.md` §4 steps 1-4, `RETRY-POLICY.md`): skip it if the manifest says
//! it is done, pace it through the source's bucket, retry by the policy, land every answer
//! that is data (or a permanent "no data"), record the outcome in the manifest and the
//! quality log.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;

use super::SourceSpec;
use super::bucket::Bucket;
use super::landing::{self, KEEP_HEADERS, Landed, Meta};
use super::manifest::{Manifest, Status};
use super::quality::{Level, QualityLog, Row};

/// The longest retry wait (`RETRY-POLICY.md`).
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// One chunk to fetch.
#[derive(Debug, Clone)]
pub struct Request<'a> {
    /// Manifest entity (vendor code, CIK, series id, ...).
    pub entity: &'a str,
    /// Manifest chunk id.
    pub chunk: &'a str,
    /// Full URL, secrets included; only its redacted form is ever stored.
    pub url: &'a str,
    /// Landing extension (`json`, `csv`, `zip`).
    pub ext: &'a str,
    /// API calls the request costs (EODHD bulk: 100).
    pub cost: u64,
    /// Dataset the chunk feeds, for `data_quality`.
    pub dataset: &'a str,
    /// An empty JSON array means "no data for this entity" (EODHD prices), not "no events"
    /// (EODHD splits and dividends, where `[]` is an answer).
    pub empty_is_gone: bool,
}

/// What happened to a chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Already done in an earlier run: no request made.
    Skipped,
    /// Landed (or an identical body was already landed: `Landed::new` is false).
    Fetched(Landed),
    /// The source has no data (`reason`); the answer is landed and the chunk never retried.
    Gone(String),
    /// Failed; retried on the next run.
    Failed(String),
}

/// An error that stops the run for this source.
#[derive(Debug, thiserror::Error)]
pub enum Fatal {
    /// The manifest cannot be read or written.
    #[error("manifest: {0}")]
    Manifest(#[from] rusqlite::Error),
    /// A body cannot be landed (the chunk is left `fetching`, reset on resume).
    #[error("landing: {0}")]
    Landing(#[from] std::io::Error),
    /// The day's call budget is spent.
    #[error("{id}: daily call cap reached ({used} calls)")]
    DailyCap {
        /// Source id.
        id: &'static str,
        /// Calls counted today.
        used: u64,
    },
    /// Key, plan or IP refused (401/403, or a SEC 429).
    #[error("{id}: refused with HTTP {status}; stopping the source for this run")]
    Refused {
        /// Source id.
        id: &'static str,
        /// HTTP status.
        status: u16,
    },
    /// The HTTP client could not be built.
    #[error("client: {0}")]
    Client(#[from] reqwest::Error),
    /// The bucket's semaphore was closed (never happens).
    #[error("bucket closed")]
    Bucket(#[from] tokio::sync::AcquireError),
}

/// How a response is handled (`RETRY-POLICY.md` algorithm).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Data: land it.
    Data,
    /// A permanent "no data" answer.
    Gone(&'static str),
    /// Stop the source for the run.
    Refused,
    /// Fail the chunk without retrying.
    NoRetry,
    /// Back off for the rate limit, then retry.
    RateLimited,
    /// Back off exponentially, then retry.
    Retry,
}

/// Classify a response.
#[must_use]
pub fn classify(source: &str, status: u16, body: &[u8], empty_is_gone: bool) -> Verdict {
    match status {
        200..=299 if body.trim_ascii().is_empty() => Verdict::NoRetry,
        200..=299 if empty_is_gone && body.trim_ascii() == b"[]" => Verdict::Gone("vendor_no_data"),
        200..=299 => Verdict::Data,
        404 if matches!(source, "eodhd" | "yahoo_chart") => Verdict::Gone("vendor_no_data"),
        401 | 403 => Verdict::Refused,
        429 if source.starts_with("sec") => Verdict::Refused,
        429 => Verdict::RateLimited,
        500..=599 => Verdict::Retry,
        _ => Verdict::NoRetry,
    }
}

/// Replace the values of the `api_key=` (FRED) and `api_token=` (EODHD) query parameters.
#[must_use]
pub fn redact(url: &str) -> String {
    let mut out = url.to_owned();
    for param in ["api_key=", "api_token="] {
        if let Some((head, rest)) = out.split_once(param) {
            let tail = rest.find('&').and_then(|j| rest.get(j..)).unwrap_or("");
            out = format!("{head}{param}REDACTED{tail}");
        }
    }
    out
}

/// One source's fetcher: client, bucket, manifest, landing zone and quality log.
pub struct Fetcher {
    /// The source's constants.
    pub spec: SourceSpec,
    client: reqwest::Client,
    bucket: Bucket,
    manifest: Arc<Manifest>,
    raw: PathBuf,
    quality: QualityLog,
}

impl Fetcher {
    /// A fetcher for `spec`, sending `user_agent` on every request.
    ///
    /// # Errors
    ///
    /// [`Fatal::Client`] if the HTTP client cannot be built.
    pub fn new(
        spec: SourceSpec,
        user_agent: &str,
        manifest: Arc<Manifest>,
        raw: PathBuf,
        quality: QualityLog,
    ) -> Result<Self, Fatal> {
        let client = reqwest::Client::builder()
            .user_agent(user_agent)
            .timeout(spec.timeout)
            .build()?;
        Ok(Self {
            bucket: Bucket::new(&spec),
            spec,
            client,
            manifest,
            raw,
            quality,
        })
    }

    /// Fetch `r` unless the manifest says it is done.
    ///
    /// Cancellation: dropping the future mid-request leaves the chunk `fetching`; the next
    /// run resets it (`Manifest::reset_stale`) and fetches it again. A landed body is never
    /// torn (temp + rename).
    ///
    /// # Errors
    ///
    /// [`Fatal`] for anything that must stop the source for the run; an ordinary failure is
    /// [`Outcome::Failed`].
    pub async fn fetch(&self, r: &Request<'_>) -> Result<Outcome, Fatal> {
        let id = self.spec.id;
        let key = (id, r.entity, r.chunk);
        if self.manifest.status(key)?.is_some_and(Status::is_done) {
            return Ok(Outcome::Skipped);
        }
        self.manifest.begin_fetch(key)?;
        let mut backoff = self.spec.base_backoff;
        let mut last_error = String::new();
        let mut last_status = None;
        for attempt in 1..=self.spec.max_attempts {
            if let Some(cap) = self.spec.daily_cap {
                // Counted before the call: a call that fails still costs.
                let used = self.manifest.add_calls(id, r.cost)?;
                if used > cap {
                    self.manifest.failed(key, None, "daily call cap")?;
                    return Err(Fatal::DailyCap { id, used });
                }
            }
            let sent = {
                let _permit = self.bucket.acquire().await?;
                match self.client.get(r.url).send().await {
                    // without_url: a reqwest error displays the URL, and ours carry API keys.
                    Ok(resp) => {
                        let status = resp.status().as_u16();
                        let headers: Vec<(String, String)> = resp
                            .headers()
                            .iter()
                            .filter(|(k, _)| KEEP_HEADERS.contains(&k.as_str()))
                            .map(|(k, v)| {
                                (
                                    k.as_str().to_owned(),
                                    v.to_str().unwrap_or("<binary>").to_owned(),
                                )
                            })
                            .collect();
                        resp.bytes()
                            .await
                            .map(|b| (status, headers, b))
                            .map_err(reqwest::Error::without_url)
                    }
                    Err(e) => Err(e.without_url()),
                }
            };
            let (status, headers, body) = match sent {
                Ok(x) => x,
                Err(e) => {
                    last_error = e.to_string();
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                    continue;
                }
            };
            last_status = Some(status);
            let meta = || Meta {
                url: redact(r.url),
                status,
                headers: headers.clone(),
                fetched_at: Utc::now(),
                attempt,
            };
            match classify(id, status, &body, r.empty_is_gone) {
                Verdict::Data => {
                    let m = meta();
                    let landed = landing::land(&self.raw, key, r.ext, &body, &m)?;
                    self.manifest.fetched(key, status, m.fetched_at, &landed)?;
                    self.log(r, Level::Ok, attempt, None);
                    return Ok(Outcome::Fetched(landed));
                }
                Verdict::Gone(reason) => {
                    landing::land(&self.raw, key, r.ext, &body, &meta())?;
                    self.manifest.gone(key, status, reason)?;
                    self.log(r, Level::Warn, attempt, Some(reason.to_owned()));
                    return Ok(Outcome::Gone(reason.to_owned()));
                }
                Verdict::Refused => {
                    self.manifest
                        .failed(key, Some(status), &format!("refused: HTTP {status}"))?;
                    self.log(
                        r,
                        Level::Fail,
                        attempt,
                        Some(format!("refused: HTTP {status}")),
                    );
                    return Err(Fatal::Refused { id, status });
                }
                Verdict::NoRetry => {
                    let e = format!("HTTP {status}, {} bytes; not retried", body.len());
                    self.manifest.failed(key, Some(status), &e)?;
                    self.log(r, Level::Fail, attempt, Some(e.clone()));
                    return Ok(Outcome::Failed(e));
                }
                Verdict::RateLimited => {
                    last_error = "HTTP 429".into();
                    self.bucket.throttled().await;
                    tokio::time::sleep(self.spec.rate_limit_backoff).await;
                }
                Verdict::Retry => {
                    last_error = format!("HTTP {status}");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                }
            }
        }
        let e = format!(
            "{} attempts exhausted; last: {last_error}",
            self.spec.max_attempts
        );
        self.manifest.failed(key, last_status, &e)?;
        self.log(r, Level::Fail, self.spec.max_attempts, Some(e.clone()));
        Ok(Outcome::Failed(e))
    }

    fn log(&self, r: &Request<'_>, level: Level, attempts: u32, notes: Option<String>) {
        self.quality.push(Row {
            source: self.spec.label,
            dataset: r.dataset.to_owned(),
            entity: r.entity.to_owned(),
            chunk: r.chunk.to_owned(),
            level,
            attempts,
            notes,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_follows_the_retry_policy() {
        assert_eq!(
            classify("eodhd", 200, b"[{\"date\":\"x\"}]", true),
            Verdict::Data
        );
        assert_eq!(
            classify("eodhd", 200, b" [] \n", true),
            Verdict::Gone("vendor_no_data")
        );
        assert_eq!(
            classify("eodhd", 200, b"[]", false),
            Verdict::Data,
            "no splits is an answer, not missing data"
        );
        assert_eq!(classify("eodhd", 200, b"", true), Verdict::NoRetry);
        assert_eq!(
            classify("eodhd", 404, b"", true),
            Verdict::Gone("vendor_no_data")
        );
        assert_eq!(classify("sec_api", 404, b"", true), Verdict::NoRetry);
        assert_eq!(classify("eodhd", 401, b"", true), Verdict::Refused);
        assert_eq!(
            classify("sec_api", 429, b"", true),
            Verdict::Refused,
            "SEC 429 may be an IP block"
        );
        assert_eq!(classify("eodhd", 429, b"", true), Verdict::RateLimited);
        assert_eq!(classify("eodhd", 503, b"", true), Verdict::Retry);
        assert_eq!(classify("eodhd", 400, b"", true), Verdict::NoRetry);
    }

    #[test]
    fn api_keys_are_redacted_anywhere_in_the_query() {
        assert_eq!(
            redact("https://x/s?api_key=SECRET&a=1"),
            "https://x/s?api_key=REDACTED&a=1"
        );
        assert_eq!(
            redact("https://x/s?a=1&api_token=SECRET"),
            "https://x/s?a=1&api_token=REDACTED"
        );
        assert_eq!(redact("https://x/s?a=1"), "https://x/s?a=1");
    }
}
