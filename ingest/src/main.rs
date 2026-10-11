//! `quant-ingest`: data ingestion and the 15+ year backfill.
//!
//! Specification: `.claude/skills/01-data-ingestion/` (SKILL.md, BACKFILL.md, SOURCES.md).
//!
//! ```text
//! quant-ingest probe <p1..p6|y1..y4>                      BACKFILL.md §2
//! quant-ingest eodhd backfill [--codes A,B] [--limit N]   W5, BACKFILL.md §5.1
//! quant-ingest identity [--codes A,B] [--quarters 2008q1-2008q4]   J4a, ADR 0001
//! ```
//!
//! Paths from `QUANT_ROOT` / `QUANT_STATE`, the registry from `QUANT_REGISTRY_PATH`, the key
//! from `EODHD_API_KEY`, the start from `BACKFILL_START` (default 2005-01-01, BACKFILL.md §1).

mod probe;

use std::process::ExitCode;
use std::sync::Arc;

use chrono::{Datelike, NaiveDate, Utc};
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
        ["identity", rest @ ..] => identity(rest),
        _ => {
            eprintln!(
                "usage: quant-ingest probe <p1..p6|y1..y4>\n       quant-ingest eodhd backfill [--codes A,B] [--limit N]\n       quant-ingest identity [--codes A,B] [--quarters 2008q1-2008q4]"
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
    let Setup {
        lake,
        today,
        quality,
        mut ctx,
        ..
    } = setup("backfill")?;
    ctx.figis = load_figis(&lake)?;
    if ctx.figis.is_empty() {
        return Err(
            "security_spells is empty: run `quant-ingest identity` first (J4a, ADR 0001)".into(),
        );
    }
    let ctx = Arc::new(ctx);
    let from = ctx.from;
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
                        ..Code::default()
                    })
                })
                .collect(),
            None => run::symbols(&ctx, today).await.map_err(|e| e.to_string())?,
        };
        for (r, _) in run::REFERENCE {
            if !list.iter().any(|c| c.code == *r) {
                list.push(Code {
                    code: (*r).into(),
                    ticker: (*r).into(),
                    delisted: false,
                    ..Code::default()
                });
            }
        }
        if let Some(n) = limit {
            list.truncate(n);
        }
        println!("eodhd: {} codes from {from}", list.len());
        let mut set = tokio::task::JoinSet::new();
        let mut spans = Vec::new();
        let mut unresolved_with_prices = 0usize;
        let mut spac_with_prices = 0usize;
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
                    if !res.resolved && res.bars > 0 {
                        // A SPAC shell is out of scope, not an identity gap (D19).
                        let spac = ingest::identity::is_spac_name(&c.name);
                        if spac {
                            spac_with_prices += 1;
                        } else {
                            unresolved_with_prices += 1;
                        }
                        let (level, what) = if spac {
                            (ingest::infra::quality::Level::Ok, "out_of_scope_with_prices: SPAC shell (name, D19)")
                        } else {
                            (ingest::infra::quality::Level::Warn, "identity_unresolved_with_prices")
                        };
                        ctx.quality.push(ingest::infra::quality::Row {
                            source: "EODHD",
                            dataset: "prices".into(),
                            entity: format!("{}.US", c.code),
                            chunk: "eod-full/identity".into(),
                            level,
                            attempts: 1,
                            notes: Some(format!("{what}: {} bars, {:?}; landed, not compacted", res.bars, res.span)),
                        });
                    }
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
        for c in run::collisions(&spans, &ctx.figis) {
            run::exclude(&ctx, &c).map_err(|e| e.to_string())?;
        }
        println!("eodhd: {unresolved_with_prices} unresolved codes have prices since {from}, plus {spac_with_prices} SPAC shells (out of scope); landed, not compacted");
        Ok::<_, String>(fatal)
    });
    // Whatever happened, keep what was staged and the quality rows.
    let compacted = run::compact(&ctx, &lake).map_err(|e| e.to_string());
    println!("eodhd: compaction {compacted:?}");
    // Reported after compaction, so one failure never hides the other.
    let written = quality
        .write(&lake)
        .map_err(|e| format!("data_quality: {e}"));
    let rows = quality.rows();
    let count = |l| rows.iter().filter(|r| r.level == l).count();
    use ingest::infra::quality::Level;
    println!(
        "eodhd: quality OK {} WARN {} FAIL {}",
        count(Level::Ok),
        count(Level::Warn),
        count(Level::Fail)
    );
    written?;
    match outcome? {
        Some(fatal) => Err(format!("stopped early: {fatal} (re-run resumes)")),
        None => compacted.map(|_| ()),
    }
}

/// What every run needs, from the environment.
struct Setup {
    paths: Paths,
    lake: lake::Lake,
    manifest: Arc<Manifest>,
    today: NaiveDate,
    quality: QualityLog,
    ctx: run::Ctx,
}

fn setup(mode: &str) -> Result<Setup, String> {
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
    let quality = QualityLog::new(mode, today);
    let fetcher = Fetcher::new(
        SourceSpec::EODHD,
        "juniquant quant-ingest",
        manifest.clone(),
        paths.raw.clone(),
        quality.clone(),
    )
    .map_err(|e| e.to_string())?;
    let ctx = run::Ctx {
        fetcher,
        manifest: manifest.clone(),
        stage: paths.stage.clone(),
        key,
        from,
        quality: quality.clone(),
        base: run::BASE.into(),
        figis: std::collections::BTreeMap::new(),
    };
    Ok(Setup {
        paths,
        lake,
        manifest,
        today,
        quality,
        ctx,
    })
}

fn parse_quarter(s: &str) -> Result<(i32, u32), String> {
    let (y, q) = s
        .split_once('q')
        .ok_or_else(|| format!("quarter {s:?}: expected YYYYqN"))?;
    let (y, q) = (
        y.parse::<i32>().map_err(|e| e.to_string())?,
        q.parse::<u32>().map_err(|e| e.to_string())?,
    );
    if (1..=4).contains(&q) {
        Ok((y, q))
    } else {
        Err(format!("quarter {s:?}: N must be 1-4"))
    }
}

fn identity(rest: &[&str]) -> Result<(), String> {
    use ingest::identity::run::{Job, run as run_identity};
    let (mut codes, mut quarters) = (None::<Vec<String>>, None);
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match (*a, it.next()) {
            ("--codes", Some(v)) => codes = Some(v.split(',').map(str::to_owned).collect()),
            ("--quarters", Some(v)) => {
                let (a, b) = v
                    .split_once('-')
                    .ok_or("--quarters FROM-TO, e.g. 2008q1-2008q4")?;
                quarters = Some((parse_quarter(a)?, parse_quarter(b)?));
            }
            (other, _) => return Err(format!("unknown argument {other}")),
        }
    }
    let Setup {
        paths,
        lake,
        manifest,
        today,
        quality,
        ctx,
    } = setup("identity")?;
    let email = std::env::var("SEC_EDGAR_EMAIL")
        .ok()
        .filter(|e| !e.is_empty())
        .ok_or("SEC_EDGAR_EMAIL is not set")?;
    let sec = Fetcher::new(
        SourceSpec::SEC_13F,
        &format!("juniquant research {email}"),
        manifest.clone(),
        paths.raw.clone(),
        quality.clone(),
    )
    .map_err(|e| e.to_string())?;
    let figi_key = std::env::var("OPENFIGI_API_KEY")
        .ok()
        .filter(|k| !k.is_empty());
    let spec = if figi_key.is_some() {
        SourceSpec::OPENFIGI
    } else {
        SourceSpec::OPENFIGI_KEYLESS
    };
    let mut figi = Fetcher::new(
        spec,
        "juniquant quant-ingest",
        manifest.clone(),
        paths.raw.clone(),
        quality.clone(),
    )
    .map_err(|e| e.to_string())?;
    if let Some(k) = &figi_key {
        figi = figi
            .with_header("X-OPENFIGI-APIKEY", k)
            .map_err(|e| e.to_string())?;
    }
    let start = ctx.from;
    // The quarter before BACKFILL_START: a CUSIP listed then is still trading at the start.
    let first = if start.month() <= 3 {
        (start.year() - 1, 4)
    } else {
        (start.year(), (start.month() - 1) / 3)
    };
    let (first_quarter, last_quarter) = match quarters {
        Some((a, b)) => (a, Some(b)),
        None => (first, None),
    };
    let job = Job {
        eodhd: ctx,
        sec,
        figi,
        batch: if figi_key.is_some() { 100 } else { 10 },
        manifest,
        quality: quality.clone(),
        first_quarter,
        last_quarter,
        keyless_limit: if figi_key.is_some() {
            None
        } else {
            Some(2_000)
        },
    };
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let result = rt.block_on(async {
        let all = run::symbols(&job.eodhd, today)
            .await
            .map_err(|e| e.to_string())?;
        let list = match &codes {
            Some(want) => all.into_iter().filter(|c| want.contains(&c.code)).collect(),
            None => all,
        };
        run_identity(&job, &lake, today, Some(list))
            .await
            .map_err(|e| e.to_string())
    });
    quality.write(&lake).map_err(|e| e.to_string())?;
    let s = result?;
    println!(
        "identity: {} codes -> {} securities (ISIN {}, 13(f) CUSIP {}); out of scope {}; unresolved {} (no match {}, ambiguous {})",
        s.codes,
        s.securities,
        s.by_isin,
        s.by_cusip,
        s.out_of_scope,
        s.no_match + s.ambiguous,
        s.no_match,
        s.ambiguous
    );
    Ok(())
}

/// EODHD code -> FIGI from `security_spells` (written by J4a).
fn load_figis(lake: &lake::Lake) -> Result<std::collections::BTreeMap<String, String>, String> {
    use arrow::array::AsArray;
    let b = lake
        .read_dataset(
            "security_spells",
            Some(&["figi", "id_type", "value"]),
            &[],
            None,
        )
        .map_err(|e| e.to_string())?;
    let (f, t, v) = (
        b.column(0).as_string::<i32>(),
        b.column(1).as_string::<i32>(),
        b.column(2).as_string::<i32>(),
    );
    Ok((0..b.num_rows())
        .filter(|&i| t.value(i) == "eodhd_code")
        .map(|i| (v.value(i).to_owned(), f.value(i).to_owned()))
        .collect())
}
