//! `quant-ingest`: data ingestion and the 15+ year backfill.
//!
//! Specification: `.claude/skills/01-data-ingestion/` (SKILL.md, BACKFILL.md, SOURCES.md).
//!
//! ```text
//! quant-ingest probe <p1..p6|y1..y4>                      BACKFILL.md §2
//! quant-ingest eodhd backfill [--codes A,B] [--limit N]   W5, BACKFILL.md §5.1
//! quant-ingest identity [--codes A,B] [--quarters 2008q1-2008q4]   J4a, ADR 0001
//! quant-ingest ishares snapshot                            W8, today's IWF/IWP holdings
//! quant-ingest edgar fetch                              J2, BACKFILL.md §5.2
//! quant-ingest edgar fundamentals                       J7, from the landed zips
//! quant-ingest edgar events                             J6, filing_events + delistings from the zip
//! quant-ingest edgar daily                              J7 + J6 daily: filers in the day's form index
//! quant-ingest macro backfill|daily                      J8, BACKFILL.md §5.3 (same run; daily refetches what changed)
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
        ["ishares", "snapshot"] => ishares_snapshot(),
        ["edgar", "fetch"] => edgar_fetch(),
        ["edgar", "fundamentals"] => edgar_fundamentals(),
        ["edgar", "events"] => edgar_events(),
        ["edgar", "daily"] => edgar_daily(),
        ["macro", "backfill" | "daily"] => macro_backfill(),
        _ => {
            eprintln!(
                "usage: quant-ingest probe <p1..p6|y1..y4>\n       quant-ingest eodhd backfill [--codes A,B] [--limit N]\n       quant-ingest identity [--codes A,B] [--quarters 2008q1-2008q4]\n       quant-ingest ishares snapshot\n       quant-ingest macro backfill|daily\n       quant-ingest edgar fetch|fundamentals|events|daily"
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
        "identity: {} codes -> {} securities (ISIN {}, 13(f) CUSIP {}, rename {}); out of scope {}; unresolved {} (no match {}, ambiguous {})",
        s.codes,
        s.securities,
        s.by_isin,
        s.by_cusip,
        s.by_rename,
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

/// W8: land today's IWF and IWP holdings files, stage them, compact `etf_holdings`.
fn ishares_snapshot() -> Result<(), String> {
    use ingest::ishares;
    let Setup {
        paths,
        lake,
        manifest,
        today,
        quality,
        ..
    } = setup("ishares")?;
    let cal = calendar::Calendar::from_env().map_err(|e| e.to_string())?;
    // Ticker -> FIGI of the security that holds it today (not delisted).
    let figis: std::collections::BTreeMap<String, String> = {
        use arrow::array::AsArray;
        let b = lake
            .read_dataset(
                "security_master",
                Some(&["figi", "ticker", "delisted"]),
                &[],
                None,
            )
            .map_err(|e| e.to_string())?;
        let (f, t, d) = (
            b.column(0).as_string::<i32>(),
            b.column(1).as_string::<i32>(),
            b.column(2).as_boolean(),
        );
        (0..b.num_rows())
            .filter(|&i| !d.value(i))
            .map(|i| (t.value(i).to_owned(), f.value(i).to_owned()))
            .collect()
    };
    let fetcher = Fetcher::new(SourceSpec::ISHARES, "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36", manifest.clone(), paths.raw.clone(), quality.clone())
        .map_err(|e| e.to_string())?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let chunk = format!("latest-{today}");
    for (etf, id, slug) in ishares::FUNDS {
        let url = ishares::url(*id, slug);
        let r = ingest::infra::fetch::Request {
            entity: etf,
            chunk: &chunk,
            url: &url,
            ext: "csv",
            cost: 1,
            dataset: "etf_holdings",
            empty_is_gone: false,
        };
        rt.block_on(fetcher.fetch(&r)).map_err(|e| e.to_string())?;
        let key = ("ishares", *etf, chunk.as_str());
        let Some((path, sha, at)) = manifest.landed(key).map_err(|e| e.to_string())? else {
            println!("ishares: {etf}: not fetched (see data_quality)");
            continue;
        };
        let body =
            ingest::infra::landing::read(std::path::Path::new(&path)).map_err(|e| e.to_string())?;
        let snap = ishares::parse(&body).map_err(|e| format!("{etf}: {e}"))?;
        let available_date = cal
            .next_trading_day(snap.snapshot_date)
            .map_err(|e| e.to_string())?;
        let cx = ishares::Context {
            etf,
            available_date,
            fetched_at: at,
            raw_sha256: &sha,
            figis: &figis,
        };
        let batch = ishares::batch(&snap, &cx).map_err(|e| e.to_string())?;
        let unresolved = batch.column(16).null_count();
        ingest::infra::stage::write_stage(
            &paths.stage,
            &manifest,
            "etf_holdings",
            key,
            ishares::PARSE_VERSION,
            &batch,
        )
        .map_err(|e| e.to_string())?;
        println!(
            "ishares: {etf} as of {} (usable {available_date}): {} holdings, {unresolved} without FIGI, dropped {:?}",
            snap.snapshot_date,
            batch.num_rows(),
            snap.dropped
        );
    }
    let c =
        ingest::infra::stage::compact(&lake, &manifest, &paths.stage, "ishares", "etf_holdings")
            .map_err(|e| e.to_string())?;
    manifest.compacted("ishares").map_err(|e| e.to_string())?;
    quality.write(&lake).map_err(|e| e.to_string())?;
    println!("ishares: compaction {c:?}");
    Ok(())
}

/// The SEC fetcher: `User-Agent` with the contact email (`SOURCES.md` SEC EDGAR).
fn sec_fetcher(s: &Setup, spec: SourceSpec) -> Result<Fetcher, String> {
    let email = std::env::var("SEC_EDGAR_EMAIL")
        .ok()
        .filter(|e| !e.is_empty())
        .ok_or("SEC_EDGAR_EMAIL is not set")?;
    Fetcher::new(
        spec,
        &format!("juniquant research {email}"),
        s.manifest.clone(),
        s.paths.raw.clone(),
        s.quality.clone(),
    )
    .map_err(|e| e.to_string())
}

/// J2: land `submissions.zip`, `companyfacts.zip` and the quarterly `form.idx` files.
fn edgar_fetch() -> Result<(), String> {
    let s = setup("edgar")?;
    let sec = sec_fetcher(&s, SourceSpec::SEC_BULK)?;
    let first = (s.ctx.from.year(), (s.ctx.from.month() - 1) / 3 + 1);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let result = rt.block_on(ingest::edgar::fetch(&sec, s.today, first));
    s.quality.write(&s.lake).map_err(|e| e.to_string())?;
    let out = result.map_err(|e| e.to_string())?;
    let failed: Vec<_> = out
        .iter()
        .filter(|(_, o)| matches!(o, ingest::infra::fetch::Outcome::Failed(_)))
        .collect();
    println!(
        "edgar: {} chunks ({} skipped as already landed), {} failed {failed:?}",
        out.len(),
        out.values()
            .filter(|o| matches!(o, ingest::infra::fetch::Outcome::Skipped))
            .count(),
        failed.len()
    );
    if failed.is_empty() {
        Ok(())
    } else {
        Err("some chunks failed (re-run resumes)".into())
    }
}

/// J7: `fundamentals` from the latest landed zips, for the CIKs of `security_master`.
fn edgar_fundamentals() -> Result<(), String> {
    use ingest::edgar;
    use ingest::infra::quality::{Level, Row};
    let s = setup("fundamentals")?;
    let by_cik = filer_securities(&s.lake)?;
    let latest = |entity: &str| latest_zip(&s, entity);
    let (chunk, facts_path) = latest("companyfacts_zip")?;
    let (subs_chunk, subs_path) = latest("submissions_zip")?;
    println!(
        "fundamentals: {} CIKs, companyfacts {chunk}, submissions {subs_chunk}",
        by_cik.len()
    );
    let open = |p: &std::path::Path| edgar::open(p).map_err(|e| format!("{}: {e}", p.display()));
    let built = edgar::build(
        &mut open(&subs_path)?,
        &mut open(&facts_path)?,
        &by_cik,
        s.today,
    )
    .map_err(|e| e.to_string())?;
    let key = ("sec_bulk", "companyfacts_zip", chunk.as_str());
    ingest::infra::stage::write_stage(
        &s.paths.stage,
        &s.manifest,
        "fundamentals",
        key,
        edgar::facts::PARSE_VERSION,
        &built.batch,
    )
    .map_err(|e| e.to_string())?;
    let compacted = ingest::infra::stage::compact_sources(
        &s.lake,
        &s.manifest,
        &s.paths.stage,
        EDGAR_SOURCES,
        "fundamentals",
    )
    .map_err(|e| e.to_string())?;
    s.manifest
        .compacted("sec_bulk")
        .map_err(|e| e.to_string())?;
    let notes = format!(
        "{} rows; {} of {} CIKs with rows; dropped: {:?}",
        built.batch.num_rows(),
        built.with_rows,
        by_cik.len(),
        built.dropped
    );
    let row = |entity: String, level, notes: String| Row {
        source: "SEC",
        dataset: "fundamentals".into(),
        entity,
        chunk: chunk.clone(),
        level,
        attempts: 1,
        notes: Some(notes),
    };
    s.quality
        .push(row("companyfacts_zip".into(), Level::Ok, notes.clone()));
    for cik in &built.no_submissions {
        s.quality.push(row(
            cik.clone(),
            Level::Warn,
            "CIK not in submissions.zip".into(),
        ));
    }
    for cik in &built.no_rows {
        s.quality.push(row(
            cik.clone(),
            Level::Warn,
            "10-K/10-Q filed since 2011-07-01 but no fundamentals row (no XBRL facts)".into(),
        ));
    }
    s.quality.write(&s.lake).map_err(|e| e.to_string())?;
    println!(
        "fundamentals: {notes}; {} CIKs not in submissions, {} without rows since 2011-07; compaction {compacted:?}",
        built.no_submissions.len(),
        built.no_rows.len()
    );
    Ok(())
}

/// J6: `filing_events` from the latest submissions zip for the CIKs of `security_master`,
/// then `delistings` rebuilt from them.
fn edgar_events() -> Result<(), String> {
    use ingest::edgar::{self, events};
    use ingest::infra::quality::{Level, Row};
    let s = setup("filing-events")?;
    let by_cik = filer_securities(&s.lake)?;
    let (chunk, subs_path) = latest_zip(&s, "submissions_zip")?;
    let mut subs = edgar::open(&subs_path).map_err(|e| format!("{}: {e}", subs_path.display()))?;
    let (mut per_cik, mut missing, mut future) = (Vec::new(), 0usize, 0usize);
    for cik in by_cik.keys() {
        let Some(filings) = edgar::filer_filings(&mut subs, cik).map_err(|e| e.to_string())? else {
            missing += 1;
            continue;
        };
        let (ev, f) = events::events(&filings, s.today);
        future += f;
        per_cik.push((cik.as_str(), ev));
    }
    let rows: Vec<_> = per_cik
        .iter()
        .flat_map(|(cik, ev)| {
            let secs = by_cik.get(*cik).map(Vec::as_slice).unwrap_or_default();
            secs.iter()
                .flat_map(move |sec| ev.iter().map(move |e| (*cik, sec, e)))
        })
        .collect();
    let batch = events::batch(&rows).map_err(|e| e.to_string())?;
    ingest::infra::stage::write_stage(
        &s.paths.stage,
        &s.manifest,
        "filing_events",
        ("sec_bulk", "submissions_zip", &chunk),
        events::PARSE_VERSION,
        &batch,
    )
    .map_err(|e| e.to_string())?;
    let compacted = ingest::infra::stage::compact_sources(
        &s.lake,
        &s.manifest,
        &s.paths.stage,
        EDGAR_SOURCES,
        "filing_events",
    )
    .map_err(|e| e.to_string())?;
    s.manifest
        .compacted("sec_bulk")
        .map_err(|e| e.to_string())?;
    // Gate G8: an 8-K since the start with no items.
    let (eight_ks, no_items) = per_cik
        .iter()
        .flat_map(|(_, ev)| ev)
        .filter(|e| e.form.starts_with("8-K") && e.filing_date >= s.ctx.from)
        .fold(
            (Default::default(), 0usize),
            |(mut acc, n): (std::collections::BTreeSet<&str>, usize), e| {
                let new = acc.insert(e.accession.as_str());
                (acc, n + usize::from(new && e.item == "-"))
            },
        );
    let share = no_items as f64 / eight_ks.len().max(1) as f64;
    let notes = format!(
        "{} rows; {} of {} CIKs in the zip; {no_items} of {} 8-Ks since {} without items ({:.2}%); {future} future filings dropped",
        batch.num_rows(),
        per_cik.len(),
        by_cik.len(),
        eight_ks.len(),
        s.ctx.from,
        share * 100.0
    );
    s.quality.push(Row {
        source: "SEC",
        dataset: "filing_events".into(),
        entity: "submissions_zip".into(),
        chunk: chunk.clone(),
        level: if share > 0.01 { Level::Warn } else { Level::Ok },
        attempts: 1,
        notes: Some(notes.clone()),
    });
    let delisted = write_delistings(&s);
    s.quality.write(&s.lake).map_err(|e| e.to_string())?;
    println!("filing_events: {notes}; {missing} CIKs not in the zip; compaction {compacted:?}");
    println!("delistings: {}", delisted?);
    Ok(())
}

/// `delistings` rebuilt in full from the lake's `filing_events` (the delisting forms and
/// items only) and each security's last price date (`SURVIVORSHIP.md` §3).
fn write_delistings(s: &Setup) -> Result<String, String> {
    use arrow::array::AsArray;
    use arrow::datatypes::Date32Type;
    use ingest::edgar::{Security, events};
    use ingest::infra::quality::{Level, Row};
    let from_days = |v: i32| NaiveDate::default() + chrono::Duration::days(i64::from(v));
    let b = s
        .lake
        .read_dataset(
            "filing_events",
            Some(&[
                "figi",
                "ticker",
                "cik",
                "accession",
                "item",
                "form_type",
                "filing_date",
            ]),
            &[],
            None,
        )
        .map_err(|e| e.to_string())?;
    let col = |i: usize| b.column(i).as_string::<i32>();
    let (figi, ticker, cik, acc, item, form) = (col(0), col(1), col(2), col(3), col(4), col(5));
    let fd = b.column(6).as_primitive::<Date32Type>();
    let wanted = [
        "delisting_filing",
        "deregistration",
        "bankruptcy",
        "acquisition_completed",
        "change_in_control",
        "delisting_notice",
    ];
    let mut by_figi: std::collections::BTreeMap<&str, (&str, Security, Vec<events::Event>)> =
        Default::default();
    for i in 0..b.num_rows() {
        let event_type = events::event_type(form.value(i), item.value(i));
        if !wanted.contains(&event_type) {
            continue;
        }
        by_figi
            .entry(figi.value(i))
            .or_insert_with(|| {
                (
                    cik.value(i),
                    Security {
                        figi: figi.value(i).to_owned(),
                        ticker: ticker.value(i).to_owned(),
                    },
                    Vec::new(),
                )
            })
            .2
            .push(events::Event {
                accession: acc.value(i).to_owned(),
                item: item.value(i).to_owned(),
                form: form.value(i).to_owned(),
                filing_date: from_days(fd.value(i)),
                acceptance: None,
                event_type,
            });
    }
    // Last price date per FIGI, one year of `prices` at a time.
    let mut last: std::collections::HashMap<String, NaiveDate> = Default::default();
    for y in s.ctx.from.year()..=s.today.year() {
        let (Some(a), Some(z)) = (
            NaiveDate::from_ymd_opt(y, 1, 1),
            NaiveDate::from_ymd_opt(y, 12, 31),
        ) else {
            continue;
        };
        let p = s
            .lake
            .read_dataset("prices", Some(&["figi", "date"]), &[], Some((a, z)))
            .map_err(|e| e.to_string())?;
        let (f, d) = (
            p.column(0).as_string::<i32>(),
            p.column(1).as_primitive::<Date32Type>(),
        );
        for i in 0..p.num_rows() {
            let d = from_days(d.value(i));
            last.entry(f.value(i).to_owned())
                .and_modify(|x| *x = (*x).max(d))
                .or_insert(d);
        }
    }
    let rows: Vec<_> = by_figi
        .iter()
        .filter_map(|(f, (cik, sec, ev))| {
            events::classify(ev, last.get(*f).copied()).map(|d| (*cik, sec, d))
        })
        .collect();
    let batch = events::delisting_batch(&rows, Utc::now()).map_err(|e| e.to_string())?;
    s.lake
        .write_dataset("delistings", &batch, lake::Mode::Replace)
        .map_err(|e| e.to_string())?;
    let mut by_rule: std::collections::BTreeMap<&str, usize> = Default::default();
    for (_, _, d) in &rows {
        *by_rule.entry(d.rule).or_default() += 1;
    }
    let notes = format!(
        "{} rows from {} securities with delisting evidence; by rule {by_rule:?}",
        rows.len(),
        by_figi.len()
    );
    s.quality.push(Row {
        source: "SEC",
        dataset: "delistings".into(),
        entity: "all".into(),
        chunk: s.today.to_string(),
        level: Level::Ok,
        attempts: 1,
        notes: Some(notes.clone()),
    });
    Ok(notes)
}

/// J8: every vintage of the 9 macro series (BACKFILL.md §5.3): vintage dates, windows of at
/// most 2,000 vintages, pages of 100,000 rows; stitched and staged per series, then compacted.
fn macro_backfill() -> Result<(), String> {
    use ingest::alfred;
    use ingest::infra::fetch::Request;
    use ingest::infra::quality::{Level, Row};
    let Setup {
        paths,
        lake,
        manifest,
        quality,
        today,
        ..
    } = setup("macro")?;
    let key = std::env::var("FRED_API_KEY")
        .ok()
        .filter(|k| !k.is_empty())
        .ok_or("FRED_API_KEY is not set")?;
    let src = SourceSpec::FRED.id;
    let fetcher = Fetcher::new(
        SourceSpec::FRED,
        "juniquant quant-ingest",
        manifest.clone(),
        paths.raw.clone(),
        quality.clone(),
    )
    .map_err(|e| e.to_string())?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    // The landed body of a chunk (fetched now or in an earlier run); None if it failed (the
    // fetcher logged it). Err stops the run (key refused, manifest or disk).
    type Body = Option<(Vec<u8>, chrono::DateTime<Utc>)>;
    let get = |entity: &str, chunk: &str, url: &str| -> Result<Body, String> {
        let r = Request {
            entity,
            chunk,
            url,
            ext: "json",
            cost: 1,
            dataset: "macro",
            empty_is_gone: false,
        };
        rt.block_on(fetcher.fetch(&r)).map_err(|e| e.to_string())?;
        match manifest
            .landed((src, entity, chunk))
            .map_err(|e| e.to_string())?
        {
            Some((p, _, at)) => ingest::infra::landing::read(std::path::Path::new(&p))
                .map(|b| Some((b, at)))
                .map_err(|e| e.to_string()),
            None => Ok(None),
        }
    };
    let log = |entity: &str, level: Level, notes: String| {
        quality.push(Row {
            source: SourceSpec::FRED.label,
            dataset: "macro".into(),
            entity: entity.to_owned(),
            chunk: "all_vintages".into(),
            level,
            attempts: 1,
            notes: Some(notes),
        });
    };
    let start = NaiveDate::parse_from_str(alfred::OBSERVATION_START, "%Y-%m-%d")
        .map_err(|e| e.to_string())?;
    let (mut incomplete, mut fatal) = (0usize, None);
    'series: for s in alfred::SERIES {
        // Refetched daily: a new vintage date renames the open window, which is then refetched.
        let body = match get(
            s.id,
            &format!("vintagedates-{today}"),
            &alfred::vintage_dates_url(s.id, &key),
        ) {
            Ok(Some((b, _))) => b,
            Ok(None) => {
                incomplete += 1;
                continue;
            }
            Err(e) => {
                fatal = Some(e);
                break;
            }
        };
        let dates = match alfred::parse_vintage_dates(&body) {
            Ok(d) => d,
            Err(e) => {
                log(s.id, Level::Fail, format!("vintagedates: {e}"));
                incomplete += 1;
                continue;
            }
        };
        let ws = alfred::windows(&dates);
        let (mut rows, mut future) = (Vec::new(), 0);
        for w in &ws {
            let mut offset = 0;
            loop {
                let chunk = w.chunk(offset);
                let url = alfred::observations_url(s.id, &key, w, offset);
                let (body, at) = match get(s.id, &chunk, &url) {
                    Ok(Some(b)) => b,
                    Ok(None) => {
                        incomplete += 1;
                        continue 'series;
                    }
                    Err(e) => {
                        fatal = Some(e);
                        break 'series;
                    }
                };
                match alfred::parse_observations(&body, at) {
                    Ok(p) => {
                        future += p.future;
                        rows.extend(p.rows);
                        if p.count <= offset + alfred::PAGE {
                            break;
                        }
                        offset += alfred::PAGE;
                    }
                    Err(e) => {
                        log(s.id, Level::Fail, format!("{chunk}: {e}"));
                        incomplete += 1;
                        continue 'series;
                    }
                }
            }
        }
        let (rows, joined) = alfred::stitch(rows, &alfred::boundaries(&ws));
        let Some(earliest) = rows.first().map(|r| r.obs_date) else {
            log(s.id, Level::Fail, "no observations".into());
            incomplete += 1;
            continue;
        };
        let batch = alfred::batch(s, &rows).map_err(|e| e.to_string())?;
        let lag_rule = alfred::known_from(s, &rows)
            .iter()
            .filter(|(_, b)| *b == alfred::Basis::LagRule)
            .count();
        let observations = rows
            .iter()
            .map(|r| r.obs_date)
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        let deferred = (earliest - start).num_days() > alfred::DEFERRED_AFTER_DAYS;
        let skey = (src, s.id, "all_vintages");
        manifest.ensure(skey).map_err(|e| e.to_string())?;
        ingest::infra::stage::write_stage(
            &paths.stage,
            &manifest,
            "macro",
            skey,
            alfred::PARSE_VERSION,
            &batch,
        )
        .map_err(|e| e.to_string())?;
        let notes = format!(
            "{} rows, {observations} observations from {earliest}; {} vintage dates in {} windows, {joined} intervals stitched; {lag_rule} lag_rule; {future} future-dated rows dropped{}",
            rows.len(),
            dates.len(),
            ws.len(),
            if deferred {
                format!("; deferred: does not reach {}", alfred::OBSERVATION_START)
            } else {
                String::new()
            }
        );
        println!("macro: {}: {notes}", s.id);
        let level = if deferred || future > 0 {
            Level::Warn
        } else {
            Level::Ok
        };
        log(s.id, level, notes);
    }
    // Whatever happened, keep what was staged and the quality rows.
    let compacted = ingest::infra::stage::compact(&lake, &manifest, &paths.stage, src, "macro")
        .map_err(|e| e.to_string())
        .and_then(|c| {
            manifest
                .compacted(src)
                .map(|_| c)
                .map_err(|e| e.to_string())
        });
    println!("macro: compaction {compacted:?}");
    let written = quality
        .write(&lake)
        .map_err(|e| format!("data_quality: {e}"));
    let rows = quality.rows();
    let count = |l| rows.iter().filter(|r| r.level == l).count();
    println!(
        "macro: quality OK {} WARN {} FAIL {}",
        count(Level::Ok),
        count(Level::Warn),
        count(Level::Fail)
    );
    written?;
    compacted?;
    match (fatal, incomplete) {
        (Some(e), _) => Err(format!("stopped early: {e} (re-run resumes)")),
        (None, 0) => Ok(()),
        (None, n) => Err(format!(
            "{n} series incomplete (see data_quality; re-run resumes)"
        )),
    }
}

/// The stage sources of `fundamentals`, in precedence order: the bulk zip, then the daily API.
const EDGAR_SOURCES: &[&str] = &["sec_bulk", "sec_api"];

/// 10-digit CIK -> its securities, from `security_master`.
fn filer_securities(
    lake: &lake::Lake,
) -> Result<std::collections::BTreeMap<String, Vec<ingest::edgar::Security>>, String> {
    use arrow::array::{Array, AsArray};
    let b = lake
        .read_dataset(
            "security_master",
            Some(&["figi", "ticker", "cik"]),
            &[],
            None,
        )
        .map_err(|e| e.to_string())?;
    let (f, t, c) = (
        b.column(0).as_string::<i32>(),
        b.column(1).as_string::<i32>(),
        b.column(2).as_string::<i32>(),
    );
    let mut by_cik: std::collections::BTreeMap<String, Vec<ingest::edgar::Security>> =
        Default::default();
    for i in (0..b.num_rows()).filter(|&i| c.is_valid(i)) {
        by_cik
            .entry(c.value(i).to_owned())
            .or_default()
            .push(ingest::edgar::Security {
                figi: f.value(i).to_owned(),
                ticker: t.value(i).to_owned(),
            });
    }
    Ok(by_cik)
}

/// The newest landed chunk (a fetch date) of a zip, and its path.
fn latest_zip(s: &Setup, entity: &str) -> Result<(String, std::path::PathBuf), String> {
    let dir = s.paths.raw.join("sec_bulk").join(entity);
    let mut chunks: Vec<String> = std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e} (run `quant-ingest edgar fetch`)", dir.display()))?
        .filter_map(|d| d.ok()?.file_name().into_string().ok())
        .collect();
    chunks.sort();
    for chunk in chunks.into_iter().rev() {
        if let Some((p, _, _)) = s
            .manifest
            .landed(("sec_bulk", entity, &chunk))
            .map_err(|e| e.to_string())?
        {
            return Ok((chunk, p.into()));
        }
    }
    Err(format!(
        "{entity}: nothing landed (run `quant-ingest edgar fetch`)"
    ))
}

/// J7 daily: read the EDGAR daily form index of every weekday since the zips; for each of our
/// filers listed with a 10-K/10-Q (or amendment) that day, fetch its submissions and
/// companyfacts JSON once (chunk = that day), rebuild its rows (older filings from the zip
/// pages) and stage those filed from three days before; then compact zip + daily rows.
/// J6 daily: the same for filers listed with an event form (8-K, 25, 15, NT), from the
/// submissions JSON alone, then `delistings` rebuilt.
fn edgar_daily() -> Result<(), String> {
    use ingest::edgar::{self, events, facts};
    use ingest::infra::fetch::Request;
    use ingest::infra::manifest::Status;
    use ingest::infra::quality::{Level, Row};
    let s = setup("edgar-daily")?;
    let by_cik = filer_securities(&s.lake)?;
    let (zip_chunk, _) = latest_zip(&s, "companyfacts_zip")?;
    let (_, subs_path) = latest_zip(&s, "submissions_zip")?;
    let zip_date = NaiveDate::parse_from_str(&zip_chunk, "%Y-%m-%d")
        .map_err(|e| format!("{zip_chunk}: {e}"))?;
    let (bulk, api) = (
        sec_fetcher(&s, SourceSpec::SEC_BULK)?,
        sec_fetcher(&s, SourceSpec::SEC_API)?,
    );
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    // The landed body of a chunk, fetched now or earlier; None if it failed or is gone.
    let get =
        |f: &Fetcher, entity: &str, chunk: &str, url: &str| -> Result<Option<Vec<u8>>, String> {
            let r = Request {
                entity,
                chunk,
                url,
                ext: if f.spec.id == "sec_bulk" {
                    "txt"
                } else {
                    "json"
                },
                cost: 1,
                dataset: "fundamentals",
                empty_is_gone: false,
            };
            rt.block_on(f.fetch(&r)).map_err(|e| e.to_string())?;
            match s
                .manifest
                .landed((f.spec.id, entity, chunk))
                .map_err(|e| e.to_string())?
            {
                Some((p, _, _)) => ingest::infra::landing::read(std::path::Path::new(&p))
                    .map(Some)
                    .map_err(|e| e.to_string()),
                None => Ok(None),
            }
        };
    let days = edgar::daily_days(zip_date, s.today);
    let (mut todo, mut ev_todo) = (Vec::new(), Vec::new());
    for d in &days {
        if let Some(body) = get(
            &bulk,
            "daily_idx",
            &d.to_string(),
            &edgar::daily_index_url(*d),
        )? {
            let ours = |forms: &[&str]| -> Vec<(NaiveDate, String)> {
                edgar::form_ciks(&body, forms)
                    .into_iter()
                    .filter(|c| by_cik.contains_key(c))
                    .map(|c| (*d, c))
                    .collect()
            };
            todo.extend(ours(&edgar::PERIODIC));
            ev_todo.extend(ours(&events::FORMS));
        }
    }
    let mut subs = edgar::open(&subs_path).map_err(|e| format!("{}: {e}", subs_path.display()))?;
    let (mut staged, mut rows_staged, mut failed) = (0usize, 0usize, Vec::new());
    for (d, cik) in &todo {
        let entity = format!("CIK{cik}");
        let fchunk = format!("facts-{d}");
        if s.manifest
            .status(("sec_api", &entity, &fchunk))
            .map_err(|e| e.to_string())?
            .is_some_and(|st| matches!(st, Status::Parsed | Status::Compacted))
        {
            continue;
        }
        let main = get(
            &api,
            &entity,
            &format!("submissions-{d}"),
            &format!("https://data.sec.gov/submissions/{entity}.json"),
        )?;
        let body = get(
            &api,
            &entity,
            &fchunk,
            &format!("https://data.sec.gov/api/xbrl/companyfacts/{entity}.json"),
        )?;
        let (Some(main), Some(body)) = (main, body) else {
            failed.push(cik.clone());
            continue;
        };
        let parse = |b: &[u8]| {
            serde_json::from_slice::<serde_json::Value>(b).map_err(|e| format!("{entity}: {e}"))
        };
        let (main, body) = (parse(&main)?, parse(&body)?);
        // Recent filings from the API, older ones from the zip (they do not change).
        let mut filings = main
            .pointer("/filings/recent")
            .map(facts::filings)
            .unwrap_or_default();
        if let Some(z) =
            edgar::entry(&mut subs, &format!("{entity}.json")).map_err(|e| e.to_string())?
        {
            let z: serde_json::Value = parse(&z)?;
            let seen: std::collections::BTreeSet<String> =
                filings.iter().map(|f| f.accession.clone()).collect();
            filings.extend(
                edgar::all_filings(&mut subs, &z)
                    .map_err(|e| e.to_string())?
                    .into_iter()
                    .filter(|f| !seen.contains(&f.accession)),
            );
        }
        let from = *d - chrono::Duration::days(3);
        let (rows, _) = facts::rows(&filings, &facts::parse_facts(&body), s.today);
        let rows: Vec<_> = rows.into_iter().filter(|r| r.filing_date >= from).collect();
        let secs = by_cik.get(cik).map(Vec::as_slice).unwrap_or_default();
        let batch = edgar::filer_batch(cik, secs, &rows).map_err(|e| e.to_string())?;
        ingest::infra::stage::write_stage(
            &s.paths.stage,
            &s.manifest,
            "fundamentals",
            ("sec_api", &entity, &fchunk),
            facts::PARSE_VERSION,
            &batch,
        )
        .map_err(|e| e.to_string())?;
        staged += 1;
        rows_staged += batch.num_rows();
    }
    // J6: the event rows of the day from the submissions JSON (the same chunk as above when the
    // filer also filed a 10-K/10-Q, so no second request), staged on that chunk's key.
    let (mut ev_staged, mut ev_rows) = (0usize, 0usize);
    for (d, cik) in &ev_todo {
        let entity = format!("CIK{cik}");
        let chunk = format!("submissions-{d}");
        if s.manifest
            .status(("sec_api", &entity, &chunk))
            .map_err(|e| e.to_string())?
            .is_some_and(|st| matches!(st, Status::Parsed | Status::Compacted))
        {
            continue;
        }
        let Some(main) = get(
            &api,
            &entity,
            &chunk,
            &format!("https://data.sec.gov/submissions/{entity}.json"),
        )?
        else {
            failed.push(cik.clone());
            continue;
        };
        let main: serde_json::Value =
            serde_json::from_slice(&main).map_err(|e| format!("{entity}: {e}"))?;
        let filings = main
            .pointer("/filings/recent")
            .map(facts::filings)
            .unwrap_or_default();
        let from = *d - chrono::Duration::days(3);
        let (ev, _) = events::events(&filings, s.today);
        let secs = by_cik.get(cik).map(Vec::as_slice).unwrap_or_default();
        let rows: Vec<_> = secs
            .iter()
            .flat_map(|sec| {
                ev.iter()
                    .filter(|e| e.filing_date >= from)
                    .map(move |e| (cik.as_str(), sec, e))
            })
            .collect();
        let batch = events::batch(&rows).map_err(|e| e.to_string())?;
        ingest::infra::stage::write_stage(
            &s.paths.stage,
            &s.manifest,
            "filing_events",
            ("sec_api", &entity, &chunk),
            events::PARSE_VERSION,
            &batch,
        )
        .map_err(|e| e.to_string())?;
        ev_staged += 1;
        ev_rows += batch.num_rows();
    }
    let compact = |dataset| {
        ingest::infra::stage::compact_sources(
            &s.lake,
            &s.manifest,
            &s.paths.stage,
            EDGAR_SOURCES,
            dataset,
        )
        .map_err(|e| e.to_string())
    };
    let compacted = compact("fundamentals")?;
    let ev_compacted = compact("filing_events")?;
    s.manifest.compacted("sec_api").map_err(|e| e.to_string())?;
    let delisted = write_delistings(&s)?;
    let notes = format!(
        "{} days since the {zip_chunk} zips; {} filer-days with a periodic filing; {staged} staged ({rows_staged} rows); {} with an event form, {ev_staged} staged ({ev_rows} event rows); {} failed (retried next run)",
        days.len(),
        todo.len(),
        ev_todo.len(),
        failed.len()
    );
    println!("filing_events: compaction {ev_compacted:?}; delistings: {delisted}");
    s.quality.push(Row {
        source: "SEC",
        dataset: "fundamentals".into(),
        entity: "daily_idx".into(),
        chunk: s.today.to_string(),
        level: if failed.is_empty() {
            Level::Ok
        } else {
            Level::Warn
        },
        attempts: 1,
        notes: Some(notes.clone()),
    });
    s.quality.write(&s.lake).map_err(|e| e.to_string())?;
    println!("edgar daily: {notes}; compaction {compacted:?}");
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!("{} filers failed: {failed:?}", failed.len()))
    }
}
