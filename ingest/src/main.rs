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
    use arrow::array::{Array, AsArray};
    use ingest::edgar::{self, Security};
    use ingest::infra::quality::{Level, Row};
    let s = setup("fundamentals")?;
    let mut by_cik: std::collections::BTreeMap<String, Vec<Security>> = Default::default();
    let b = s
        .lake
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
    for i in (0..b.num_rows()).filter(|&i| c.is_valid(i)) {
        by_cik
            .entry(c.value(i).to_owned())
            .or_default()
            .push(Security {
                figi: f.value(i).to_owned(),
                ticker: t.value(i).to_owned(),
            });
    }
    // The newest landed chunk (a fetch date) of a zip.
    let latest = |entity: &str| -> Result<(String, std::path::PathBuf), String> {
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
    };
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
    let compacted = ingest::infra::stage::compact(
        &s.lake,
        &s.manifest,
        &s.paths.stage,
        "sec_bulk",
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
