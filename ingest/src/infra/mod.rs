//! The fetch discipline of `SKILL.md` §4, one module per step:
//!
//! 1. [`manifest`]: what is already fetched (`$QUANT_STATE/ingest/manifest.db`).
//! 2. [`bucket`]: the per-source token bucket and concurrency cap (`SKILL.md` §5).
//! 3. [`fetch`]: one chunk, with the retry policy (`RETRY-POLICY.md`).
//! 4. [`landing`]: the raw body and its sidecar (`BACKFILL.md` §3).
//! 5. [`stage`]: stage files and compaction into the lake (`BACKFILL.md` §4).
//! 6. [`quality`]: the `data_quality` rows of a run.

pub mod bucket;
pub mod fetch;
pub mod landing;
pub mod manifest;
pub mod quality;
pub mod stage;

use std::path::PathBuf;
use std::time::Duration;

/// Where ingestion reads and writes (`13-parquet-lake/IO-CONTRACT.md` §1).
#[derive(Debug, Clone)]
pub struct Paths {
    /// `$QUANT_ROOT/raw`: the immutable landing zone.
    pub raw: PathBuf,
    /// `$QUANT_ROOT/stage`: parsed intermediates, rebuildable.
    pub stage: PathBuf,
    /// `$QUANT_STATE/ingest/manifest.db`.
    pub manifest: PathBuf,
}

impl Paths {
    /// Paths from `QUANT_ROOT` and `QUANT_STATE`.
    ///
    /// # Errors
    ///
    /// The name of the first variable that is unset or empty (no default to a production path).
    pub fn from_env() -> Result<Self, &'static str> {
        let var = |name: &'static str| {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .ok_or(name)
        };
        let (root, state) = (var("QUANT_ROOT")?, var("QUANT_STATE")?);
        Ok(Self {
            raw: root.join("raw"),
            stage: root.join("stage"),
            manifest: state.join("ingest").join("manifest.db"),
        })
    }
}

/// Per-source constants: the rate table of `SKILL.md` §5 and the retry table of
/// `RETRY-POLICY.md`, in one place.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceSpec {
    /// Manifest and landing-zone source id (`eodhd`, `sec_api`, ...).
    pub id: &'static str,
    /// `data_quality.source` label (`EODHD`, `SEC`, ...).
    pub label: &'static str,
    /// Bucket size.
    pub burst: f64,
    /// Sustained requests per second.
    pub rate: f64,
    /// Requests in flight at once.
    pub concurrency: usize,
    /// First retry wait; doubles per attempt, capped at 60 s.
    pub base_backoff: Duration,
    /// Attempts per request, the first included.
    pub max_attempts: u32,
    /// Wait after a 429.
    pub rate_limit_backoff: Duration,
    /// Calls per UTC day the client stops at (EODHD: 90,000 of 100,000).
    pub daily_cap: Option<u64>,
    /// Request timeout.
    pub timeout: Duration,
}

impl SourceSpec {
    /// EODHD (DECISIONS D15).
    pub const EODHD: Self = Self {
        id: "eodhd",
        label: "EODHD",
        burst: 20.0,
        rate: 14.0,
        concurrency: 4,
        base_backoff: Duration::from_secs(1),
        max_attempts: 3,
        rate_limit_backoff: Duration::from_secs(60),
        daily_cap: Some(90_000),
        timeout: Duration::from_secs(30),
    };
    /// SEC EDGAR JSON API.
    pub const SEC_API: Self = Self {
        id: "sec_api",
        label: "SEC",
        burst: 10.0,
        rate: 5.0,
        concurrency: 4,
        base_backoff: Duration::from_secs(1),
        max_attempts: 5,
        rate_limit_backoff: Duration::from_secs(60),
        daily_cap: None,
        timeout: Duration::from_secs(30),
    };
    /// SEC 13(f) securities lists (`www.sec.gov/files/investment/...`), SEC fair-access pacing.
    pub const SEC_13F: Self = Self {
        id: "sec_13f_list",
        label: "SEC",
        burst: 10.0,
        rate: 5.0,
        concurrency: 2,
        base_backoff: Duration::from_secs(1),
        max_attempts: 5,
        rate_limit_backoff: Duration::from_secs(60),
        daily_cap: None,
        timeout: Duration::from_secs(120),
    };
    /// OpenFIGI mapping with an API key: 25 requests per 6 s, 100 jobs each.
    pub const OPENFIGI: Self = Self {
        id: "openfigi",
        label: "OpenFIGI",
        burst: 5.0,
        rate: 3.5,
        concurrency: 2,
        base_backoff: Duration::from_secs(2),
        max_attempts: 5,
        rate_limit_backoff: Duration::from_secs(10),
        daily_cap: None,
        timeout: Duration::from_secs(60),
    };
    /// OpenFIGI without a key: 25 requests per minute, 10 jobs each.
    pub const OPENFIGI_KEYLESS: Self = Self {
        rate: 0.38,
        burst: 2.0,
        concurrency: 1,
        rate_limit_backoff: Duration::from_secs(60),
        ..Self::OPENFIGI
    };
    /// FRED / ALFRED.
    pub const FRED: Self = Self {
        id: "fred_alfred",
        label: "FRED",
        burst: 20.0,
        rate: 1.0,
        concurrency: 2,
        base_backoff: Duration::from_secs(1),
        max_attempts: 3,
        rate_limit_backoff: Duration::from_secs(120),
        daily_cap: None,
        timeout: Duration::from_secs(30),
    };
    /// iShares holdings files.
    pub const ISHARES: Self = Self {
        id: "ishares",
        label: "iShares",
        burst: 2.0,
        rate: 0.2,
        concurrency: 1,
        base_backoff: Duration::from_secs(5),
        max_attempts: 2,
        rate_limit_backoff: Duration::from_secs(300),
        daily_cap: None,
        timeout: Duration::from_secs(30),
    };
}
