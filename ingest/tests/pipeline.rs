//! The common ingest pipeline against a scripted local HTTP server: retries, landing,
//! manifest states, idempotent re-runs (no request for a done chunk), refusals, the daily
//! cap, secret redaction, and stage -> compact into the lake.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "integration tests: any failure must fail the test"
)]

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use arrow::array::{
    ArrayRef, Date32Array, Float64Array, RecordBatch, StringArray, TimestampNanosecondArray,
};
use chrono::NaiveDate;
use ingest::infra::SourceSpec;
use ingest::infra::fetch::{Fatal, Fetcher, Outcome, Request};
use ingest::infra::manifest::{Manifest, Status};
use ingest::infra::quality::{Level, QualityLog};
use ingest::infra::stage::{self, Compacted};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Serve `script` (status, body), one response per connection, in order; count requests.
async fn serve(script: Vec<(u16, &'static str)>) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    tokio::spawn(async move {
        for (status, body) in script {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0; 4096];
            let _ = sock.read(&mut buf).await.unwrap();
            seen.fetch_add(1, Ordering::SeqCst);
            let resp = format!(
                "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
        }
    });
    (base, count)
}

/// EODHD constants with test-sized waits.
fn spec() -> SourceSpec {
    SourceSpec {
        base_backoff: Duration::from_millis(1),
        rate_limit_backoff: Duration::from_millis(1),
        rate: 1000.0,
        ..SourceSpec::EODHD
    }
}

struct Env {
    dir: TempDir,
    manifest: Arc<Manifest>,
    quality: QualityLog,
}

fn env() -> Env {
    let dir = TempDir::new().unwrap();
    let manifest = Arc::new(Manifest::open(&dir.path().join("state/ingest/manifest.db")).unwrap());
    let quality = QualityLog::new("test", NaiveDate::from_ymd_opt(2026, 10, 11).unwrap());
    Env {
        dir,
        manifest,
        quality,
    }
}

impl Env {
    fn fetcher(&self, spec: SourceSpec) -> Fetcher {
        Fetcher::new(
            spec,
            "test",
            self.manifest.clone(),
            self.dir.path().join("raw"),
            self.quality.clone(),
        )
        .unwrap()
    }
}

fn req<'a>(url: &'a str, entity: &'a str) -> Request<'a> {
    Request {
        entity,
        chunk: "eod-full",
        url,
        ext: "json",
        cost: 1,
        dataset: "prices",
    }
}

#[tokio::test]
async fn retries_lands_redacts_and_skips_done_chunks() {
    let (base, count) = serve(vec![
        (503, "busy"),
        (429, "slow"),
        (200, "[{\"date\":\"2024-01-02\"}]"),
    ])
    .await;
    let e = env();
    let url = format!("{base}/api/eod/AAPL.US?api_token=SECRET&fmt=json");
    let f = e.fetcher(spec());
    let Outcome::Fetched(landed) = f.fetch(&req(&url, "AAPL.US")).await.unwrap() else {
        panic!("not fetched")
    };
    assert_eq!(count.load(Ordering::SeqCst), 3);
    assert!(landed.new);
    assert_eq!(
        ingest::infra::landing::read(&landed.path).unwrap(),
        b"[{\"date\":\"2024-01-02\"}]"
    );
    assert_eq!(
        e.manifest.status(("eodhd", "AAPL.US", "eod-full")).unwrap(),
        Some(Status::Fetched)
    );
    // The secret never reaches disk.
    for entry in walk(&e.dir.path().join("raw")) {
        let text = String::from_utf8_lossy(&std::fs::read(&entry).unwrap()).into_owned();
        assert!(!text.contains("SECRET"), "{}", entry.display());
    }
    // A second run makes no request for the done chunk.
    let again = e
        .fetcher(spec())
        .fetch(&req(&url, "AAPL.US"))
        .await
        .unwrap();
    assert_eq!(again, Outcome::Skipped);
    assert_eq!(count.load(Ordering::SeqCst), 3);
    let rows = e.quality.rows();
    assert_eq!(
        (rows.len(), rows[0].level, rows[0].attempts),
        (1, Level::Ok, 3)
    );
}

#[tokio::test]
async fn empty_series_is_gone_and_never_refetched() {
    let (base, count) = serve(vec![(200, "[]")]).await;
    let e = env();
    let url = format!("{base}/api/eod/XX.US");
    assert_eq!(
        e.fetcher(spec()).fetch(&req(&url, "XX.US")).await.unwrap(),
        Outcome::Gone("vendor_no_data".into())
    );
    assert_eq!(
        e.fetcher(spec()).fetch(&req(&url, "XX.US")).await.unwrap(),
        Outcome::Skipped
    );
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(e.quality.rows()[0].level, Level::Warn);
}

#[tokio::test]
async fn refusal_stops_the_source_and_failures_are_retryable() {
    let (base, _) = serve(vec![
        (401, "bad key"),
        (400, "bad request"),
        (500, "a"),
        (500, "b"),
        (500, "c"),
    ])
    .await;
    let e = env();
    let f = e.fetcher(spec());
    let url = format!("{base}/x");
    assert!(matches!(
        f.fetch(&req(&url, "A.US")).await,
        Err(Fatal::Refused { status: 401, .. })
    ));
    assert!(
        matches!(
            f.fetch(&req(&url, "B.US")).await.unwrap(),
            Outcome::Failed(_)
        ),
        "400 is not retried"
    );
    let Outcome::Failed(e3) = f.fetch(&req(&url, "C.US")).await.unwrap() else {
        panic!()
    };
    assert!(e3.contains("3 attempts"), "{e3}");
    let work = e.manifest.work("eodhd").unwrap();
    assert_eq!(
        work.len(),
        3,
        "every failed chunk is on the next run's work list"
    );
    assert!(e.quality.rows().iter().all(|r| r.level == Level::Fail));
}

#[tokio::test]
async fn daily_cap_stops_before_the_call() {
    let (base, count) = serve(vec![(200, "[1]")]).await;
    let e = env();
    let f = e.fetcher(SourceSpec {
        daily_cap: Some(100),
        ..spec()
    });
    let url = format!("{base}/x");
    assert!(matches!(
        f.fetch(&req(&url, "A.US")).await.unwrap(),
        Outcome::Fetched(_)
    ));
    let bulk = Request {
        cost: 100,
        ..req(&url, "US")
    };
    assert!(matches!(
        f.fetch(&bulk).await,
        Err(Fatal::DailyCap { used: 101, .. })
    ));
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

/// A `splits` stage batch for one ticker (the single-file dataset keeps the test small).
fn splits(ticker: &'static str, ex_days: &[i32]) -> RecordBatch {
    let n = ex_days.len();
    let s = |v: &'static str| -> ArrayRef { Arc::new(StringArray::from(vec![v; n])) };
    let f = |v: f64| -> ArrayRef { Arc::new(Float64Array::from(vec![v; n])) };
    let cols: Vec<ArrayRef> = vec![
        s(ticker),
        Arc::new(Date32Array::from(ex_days.to_vec())),
        f(4.0),
        f(1.0),
        f(4.0),
        s("eodhd"),
        Arc::new(TimestampNanosecondArray::from(vec![0; n]).with_timezone("UTC")),
        s("ab"),
    ];
    let names = [
        "ticker",
        "ex_date",
        "numerator",
        "denominator",
        "ratio",
        "source",
        "fetched_at",
        "raw_sha256",
    ];
    let schema = arrow::datatypes::Schema::new(
        names
            .iter()
            .zip(&cols)
            .map(|(n, c)| arrow::datatypes::Field::new(*n, c.data_type().clone(), true))
            .collect::<Vec<_>>(),
    );
    RecordBatch::try_new(Arc::new(schema), cols).unwrap()
}

#[test]
fn stage_then_compact_is_idempotent() {
    let e = env();
    let registry = lake::Registry::load(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../schemas/registry.json"),
    )
    .unwrap();
    let lake = lake::Lake::new(
        e.dir.path().join("lake"),
        e.dir.path().join("locks"),
        registry,
    );
    let st = e.dir.path().join("stage");
    for (t, days) in [("AAPL", &[18_505][..]), ("NVDA", &[19_884][..])] {
        let entity = format!("{t}.US");
        let key = ("eodhd", entity.as_str(), "splits-full");
        e.manifest.ensure(key).unwrap();
        stage::write_stage(&st, &e.manifest, "splits", key, "1", &splits(t, days)).unwrap();
    }
    assert_eq!(
        stage::compact(&lake, &e.manifest, &st, "eodhd", "splits").unwrap(),
        Compacted::Written(2)
    );
    assert_eq!(
        stage::compact(&lake, &e.manifest, &st, "eodhd", "splits").unwrap(),
        Compacted::Unchanged
    );
    assert_eq!(
        lake.read_dataset("splits", None, &[], None)
            .unwrap()
            .num_rows(),
        2
    );
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let p = entry.unwrap().path();
        if p.is_dir() {
            out.extend(walk(&p))
        } else {
            out.push(p)
        }
    }
    out
}
