//! `quant-ingest`: data ingestion and the 15+ year backfill.
//!
//! Specification: `.claude/skills/01-data-ingestion/` (SKILL.md, BACKFILL.md, SOURCES.md).
//!
//! ```text
//! quant-ingest probe <p1..p6|y1..y4>                      BACKFILL.md §2
//! quant-ingest eodhd backfill [--codes A,B] [--limit N]   W5, BACKFILL.md §5.1
//! ```
//!
//! Paths from `QUANT_ROOT` / `QUANT_STATE`, the registry from `QUANT_REGISTRY_PATH`, the key
//! from `EODHD_API_KEY`, the start from `BACKFILL_START` (default 2005-01-01, BACKFILL.md §1).

mod probe;

use std::process::ExitCode;
use std::sync::Arc;

use chrono::{NaiveDate, Utc};
use ingest::eodhd::{self, Code, run};
use ingest::infra::fetch::Fetcher;
use ingest::infra::manifest::Manifest;
use ingest::infra::quality::QualityLog;
use ingest::infra::{Paths, SourceSpec};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["probe", name] => probe::run(name)
            .map(|p| println!("{name}: evidence in {}", p.display()))
            .map_err(|e| e.to_string()),
        ["eodhd", "backfill", rest @ ..] => backfill(rest),
        _ => {
            eprintln!(
                "usage: quant-ingest probe <p1..p6|y1..y4>\n       quant-ingest eodhd backfill [--codes A,B] [--limit N]"
            );
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("quant-ingest: {e}");
            ExitCode::FAILURE
        }
    }
}

fn backfill(rest: &[&str]) -> Result<(), String> {
    let (mut codes, mut limit) = (None, None);
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match (*a, it.next()) {
            ("--codes", Some(v)) => {
                codes = Some(v.split(',').map(str::to_owned).collect::<Vec<_>>())
            }
            ("--limit", Some(v)) => {
                limit = Some(v.parse::<usize>().map_err(|e| format!("--limit: {e}"))?)
            }
            (other, _) => return Err(format!("unknown argument {other}")),
        }
    }
    let paths = Paths::from_env().map_err(|v| format!("{v} is not set"))?;
    let key = std::env::var("EODHD_API_KEY")
        .ok()
        .filter(|k| !k.is_empty())
        .ok_or("EODHD_API_KEY is not set")?;
    let from = match std::env::var("BACKFILL_START") {
        Ok(v) => {
            NaiveDate::parse_from_str(&v, "%Y-%m-%d").map_err(|e| format!("BACKFILL_START: {e}"))?
        }
        Err(_) => NaiveDate::from_ymd_opt(2005, 1, 1).ok_or("bad default start")?,
    };
    let lake = lake::Lake::from_env().map_err(|e| e.to_string())?;
    let manifest = Arc::new(Manifest::open(&paths.manifest).map_err(|e| e.to_string())?);
    manifest
        .reset_stale(chrono::Duration::minutes(15))
        .map_err(|e| e.to_string())?;
    let today = eodhd::eastern(Utc::now()).date();
    let quality = QualityLog::new("backfill", today);
    let fetcher = Fetcher::new(
        SourceSpec::EODHD,
        "juniquant quant-ingest",
        manifest.clone(),
        paths.raw.clone(),
        quality.clone(),
    )
    .map_err(|e| e.to_string())?;
    let ctx = Arc::new(run::Ctx {
        fetcher,
        manifest,
        stage: paths.stage,
        key,
        from,
        quality: quality.clone(),
        base: run::BASE.into(),
    });
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let outcome = rt.block_on(async {
        let mut list: Vec<Code> = match codes {
            Some(cs) => cs
                .iter()
                .filter_map(|c| {
                    eodhd::code_ticker(c).map(|t| Code {
                        code: c.clone(),
                        ticker: t,
                        delisted: false,
                    })
                })
                .collect(),
            None => run::symbols(&ctx, today).await.map_err(|e| e.to_string())?,
        };
        for r in run::REFERENCE {
            if !list.iter().any(|c| c.code == *r) {
                list.push(Code {
                    code: (*r).into(),
                    ticker: (*r).into(),
                    delisted: false,
                });
            }
        }
        if let Some(n) = limit {
            list.truncate(n);
        }
        println!("eodhd: {} codes from {from}", list.len());
        let mut set = tokio::task::JoinSet::new();
        let mut spans = Vec::new();
        let (mut done, mut fatal) = (0usize, None);
        let mut todo = list.into_iter();
        loop {
            // Up to 16 codes in flight; the bucket paces the requests themselves.
            while fatal.is_none() && set.len() < 16 {
                let Some(c) = todo.next() else { break };
                let ctx = ctx.clone();
                set.spawn(async move {
                    let r = run::code(&ctx, &c).await;
                    (c, r)
                });
            }
            let Some(joined) = set.join_next().await else {
                break;
            };
            let (c, r) = joined.map_err(|e| e.to_string())?;
            match r {
                Ok(res) => {
                    if let Some(s) = res.span {
                        spans.push((c, s));
                    }
                }
                Err(e) => fatal = Some(e.to_string()),
            }
            done += 1;
            if done % 500 == 0 {
                println!("eodhd: {done} codes done");
            }
        }
        for c in run::collisions(&spans) {
            run::exclude(&ctx, &c).map_err(|e| e.to_string())?;
        }
        Ok::<_, String>(fatal)
    });
    // Whatever happened, keep what was staged and the quality rows.
    let compacted = run::compact(&ctx, &lake).map_err(|e| e.to_string());
    quality.write(&lake).map_err(|e| e.to_string())?;
    let rows = quality.rows();
    let count = |l| rows.iter().filter(|r| r.level == l).count();
    use ingest::infra::quality::Level;
    println!(
        "eodhd: quality OK {} WARN {} FAIL {}; compaction {:?}",
        count(Level::Ok),
        count(Level::Warn),
        count(Level::Fail),
        compacted
    );
    match outcome? {
        Some(fatal) => Err(format!("stopped early: {fatal} (re-run resumes)")),
        None => compacted.map(|_| ()),
    }
}
