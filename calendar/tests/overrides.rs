//! Loading the overrides files (strict, all-or-nothing, add-only), the environment, and the
//! atomic append of `add_closure`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "integration tests: any failure must fail the test"
)]

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use calendar::{
    AddOutcome, Calendar, CalendarError, ClosureOrigin, ClosureSource, EntryLocation, NewClosure,
    OverrideErrorKind, OverridePaths, RefusalReason, parse_overrides,
};
use chrono::{DateTime, NaiveDate, Utc};

fn d(y: i32, m: u32, dd: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, dd).unwrap()
}

fn kind_of(text: &str) -> OverrideErrorKind {
    match parse_overrides(text, Path::new("x.toml"), ClosureOrigin::Runtime) {
        Err(CalendarError::InvalidOverride { kind, .. }) => kind,
        other => panic!("expected InvalidOverride, got {other:?}"),
    }
}

const SEED: &str = "[[closure]]\ndate = \"2025-01-09\"\nname = \"Carter\"\n";

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let p = dir.join(name);
    fs::write(&p, text).unwrap();
    p
}

fn new_closure(date: NaiveDate, name: &str) -> NewClosure {
    let t: DateTime<Utc> = "2026-11-26T14:00:00Z".parse().unwrap();
    NewClosure {
        date,
        name: name.to_owned(),
        added_on: t,
        first_seen: t,
        source: ClosureSource::SchwabMarketdata,
    }
}

#[test]
fn parses_seed_and_runtime_entries() {
    let text = r#"
# comment
[[closure]]
date = "2026-11-27"
name = "Test closure"
added_on = "2026-11-26T14:00:00Z"
first_seen = 2026-11-26T13:00:00Z
source = "same_day_signal"

[[closure]]
date = 2026-12-30
name = "TOML local date"
"#;
    let got = parse_overrides(text, Path::new("x.toml"), ClosureOrigin::Runtime).unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].date, d(2026, 11, 27));
    assert_eq!(got[0].source, Some(ClosureSource::SameDaySignal));
    assert_eq!(got[0].added_on.as_deref(), Some("2026-11-26T14:00:00Z"));
    assert!(got[0].first_seen.is_some());
    assert_eq!(got[1].date, d(2026, 12, 30));
    assert_eq!(got[1].origin, ClosureOrigin::Runtime);
    // An empty file has no closures.
    assert!(
        parse_overrides("", Path::new("x"), ClosureOrigin::Runtime)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn checked_in_seed_loads() {
    let seed = Path::new(env!("CARGO_MANIFEST_DIR")).join("../config/nyse_calendar_overrides.toml");
    let got = calendar::load_overrides_file(&seed, ClosureOrigin::Seed).unwrap();
    assert_eq!(got.len(), 10);
    assert!(got.iter().all(|c| c.origin == ClosureOrigin::Seed));
}

#[test]
fn malformed_entries_are_rejected() {
    assert!(matches!(
        parse_overrides("[[closure]\n", Path::new("x"), ClosureOrigin::Seed),
        Err(CalendarError::Toml { .. })
    ));
    assert_eq!(
        kind_of("[[closure]]\nname = \"x\"\n"),
        OverrideErrorKind::MissingKey("date")
    );
    assert_eq!(
        kind_of("[[closure]]\ndate = \"2026-11-27\"\n"),
        OverrideErrorKind::MissingKey("name")
    );
    assert_eq!(
        kind_of("[[closure]]\ndate = \"2026-11-27\"\nname = \"  \"\n"),
        OverrideErrorKind::EmptyName
    );
    for bad in ["2026-13-01", "2026-2-30", "2026-1-5", "27/11/2026", ""] {
        assert_eq!(
            kind_of(&format!("[[closure]]\ndate = \"{bad}\"\nname = \"x\"\n")),
            OverrideErrorKind::BadDate(bad.to_owned()),
            "{bad}"
        );
    }
    assert!(matches!(
        kind_of("[[closure]]\ndate = 2026-11-27T10:00:00\nname = \"x\"\n"),
        OverrideErrorKind::BadDate(_)
    ));
    assert!(matches!(
        kind_of("[[closure]]\ndate = 20261127\nname = \"x\"\n"),
        OverrideErrorKind::WrongType { key: "date", .. }
    ));
    assert_eq!(
        kind_of("[[closure]]\ndate = \"2026-11-27\"\nname = \"x\"\nsource = \"rumour\"\n"),
        OverrideErrorKind::UnknownSource("rumour".into())
    );
    assert_eq!(
        kind_of("other = 1\n"),
        OverrideErrorKind::UnknownTopLevelKey("other".into())
    );
    assert_eq!(
        kind_of("closure = 1\n"),
        OverrideErrorKind::NotAnArrayOfTables
    );
    assert_eq!(
        kind_of("closure = [1]\n"),
        OverrideErrorKind::NotAnArrayOfTables
    );
}

#[test]
fn an_entry_cannot_open_a_day() {
    // The format has no way to open a day: any extra key is an error, never ignored.
    assert_eq!(
        kind_of("[[closure]]\ndate = \"2026-11-26\"\nname = \"x\"\nopen = true\n"),
        OverrideErrorKind::UnknownKey("open".into())
    );
    // A weekend date closes nothing and is a load error (a typo in the date).
    assert_eq!(
        kind_of("[[closure]]\ndate = \"2026-10-10\"\nname = \"x\"\n"),
        OverrideErrorKind::Weekend(d(2026, 10, 10))
    );
}

#[test]
fn error_location_names_the_entry() {
    let text = format!("{SEED}[[closure]]\ndate = \"2026-11-28\"\nname = \"sat\"\n");
    match parse_overrides(&text, Path::new("x.toml"), ClosureOrigin::Seed) {
        Err(CalendarError::InvalidOverride { location, .. }) => {
            assert_eq!(location, EntryLocation::Closure(2));
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn a_rule_holiday_override_is_redundant_but_accepted() {
    // Thanksgiving 2026 is already a rule holiday: the entry only repeats a closure.
    let got = parse_overrides(
        "[[closure]]\ndate = \"2026-11-26\"\nname = \"dup\"\n",
        Path::new("x"),
        ClosureOrigin::Runtime,
    )
    .unwrap();
    let cal = Calendar::from_closures(got).unwrap();
    assert_eq!(cal.holidays(2026), Calendar::rules_only().holidays(2026));
}

#[test]
fn load_unions_seed_and_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let seed = write(dir.path(), "seed.toml", SEED);
    let runtime = write(
        dir.path(),
        "runtime.toml",
        "[[closure]]\ndate = \"2026-10-14\"\nname = \"unplanned\"\nsource = \"manual\"\n",
    );
    let cal = Calendar::load(&seed, &runtime).unwrap();
    assert!(!cal.is_trading_day(d(2025, 1, 9)));
    assert!(!cal.is_trading_day(d(2026, 10, 14)));
    assert_eq!(cal.overrides().len(), 2);
    assert_eq!(cal.overrides()[1].origin, ClosureOrigin::Runtime);
}

#[test]
fn missing_runtime_is_fine_missing_seed_is_not() {
    let dir = tempfile::tempdir().unwrap();
    let seed = write(dir.path(), "seed.toml", SEED);
    let absent = dir.path().join("absent.toml");
    assert_eq!(Calendar::load(&seed, &absent).unwrap().overrides().len(), 1);
    assert!(matches!(
        Calendar::load(&absent, &seed),
        Err(CalendarError::Read { .. })
    ));
}

#[test]
fn a_malformed_runtime_file_fails_the_whole_load() {
    let dir = tempfile::tempdir().unwrap();
    let seed = write(dir.path(), "seed.toml", SEED);
    let runtime = write(
        dir.path(),
        "runtime.toml",
        "[[closure]]\ndate = \"2026-10-14\"\n",
    );
    assert!(matches!(
        Calendar::load(&seed, &runtime),
        Err(CalendarError::InvalidOverride { .. })
    ));
}

#[test]
fn env_vars_are_required() {
    let none = |_: &str| None;
    match OverridePaths::from_env_lookup(none) {
        Err(CalendarError::MissingEnvVar { var }) => assert_eq!(var, "QUANT_CONFIG"),
        other => panic!("unexpected {other:?}"),
    }
    let only_config = |k: &str| (k == "QUANT_CONFIG").then(|| OsString::from("/cfg"));
    match OverridePaths::from_env_lookup(only_config) {
        Err(CalendarError::MissingEnvVar { var }) => assert_eq!(var, "QUANT_STATE"),
        other => panic!("unexpected {other:?}"),
    }
    let empty_state = |k: &str| Some(OsString::from(if k == "QUANT_STATE" { "" } else { "/cfg" }));
    assert!(matches!(
        OverridePaths::from_env_lookup(empty_state),
        Err(CalendarError::MissingEnvVar { var: "QUANT_STATE" })
    ));
    let both = |k: &str| {
        Some(OsString::from(if k == "QUANT_STATE" {
            "/state"
        } else {
            "/cfg"
        }))
    };
    let paths = OverridePaths::from_env_lookup(both).unwrap();
    assert_eq!(paths.seed, Path::new("/cfg/nyse_calendar_overrides.toml"));
    assert_eq!(
        paths.runtime,
        Path::new("/state/nyse_calendar_overrides.toml")
    );
}

#[test]
fn add_closure_appends_atomically_and_only_adds() {
    let dir = tempfile::tempdir().unwrap();
    let seed = write(dir.path(), "seed.toml", SEED);
    let runtime = dir.path().join("nyse_calendar_overrides.toml");
    let mut cal = Calendar::load(&seed, &runtime).unwrap();
    let monday = d(2026, 10, 12);
    assert_eq!(cal.add_trading_days(monday, 5).unwrap(), d(2026, 10, 19));

    // First write creates the file.
    let out = cal
        .add_closure(
            &runtime,
            &new_closure(d(2026, 10, 14), "Unplanned \"closure\""),
        )
        .unwrap();
    assert_eq!(out, AddOutcome::Added);
    assert!(!cal.is_trading_day(d(2026, 10, 14)));
    assert_eq!(cal.add_trading_days(monday, 5).unwrap(), d(2026, 10, 20));
    let first = fs::read_to_string(&runtime).unwrap();

    // A hand-written comment survives; the second write appends after it.
    fs::write(&runtime, format!("{first}# owner note")).unwrap();
    let before = fs::read_to_string(&runtime).unwrap();
    let out = cal
        .add_closure(&runtime, &new_closure(d(2026, 10, 15), "second"))
        .unwrap();
    assert_eq!(out, AddOutcome::Added);
    let after = fs::read_to_string(&runtime).unwrap();
    assert!(
        after.starts_with(&before),
        "existing content must be kept byte for byte"
    );

    // A reload sees exactly what the in-memory calendar has.
    let reloaded = Calendar::load(&seed, &runtime).unwrap();
    assert_eq!(reloaded.holidays(2026), cal.holidays(2026));
    let r = &reloaded.overrides()[1];
    assert_eq!(r.name, "Unplanned \"closure\"");
    assert_eq!(r.source, Some(ClosureSource::SchwabMarketdata));
    assert_eq!(r.added_on.as_deref(), Some("2026-11-26T14:00:00Z"));

    // Already closed (rules, seed, runtime): nothing written.
    for date in [d(2026, 11, 26), d(2025, 1, 9), d(2026, 10, 14)] {
        assert_eq!(
            cal.add_closure(&runtime, &new_closure(date, "dup"))
                .unwrap(),
            AddOutcome::AlreadyClosed
        );
    }
    assert_eq!(fs::read_to_string(&runtime).unwrap(), after);

    // Refusals.
    assert!(matches!(
        cal.add_closure(&runtime, &new_closure(d(2026, 10, 17), "sat")),
        Err(CalendarError::Refused {
            reason: RefusalReason::Weekend,
            ..
        })
    ));
    assert!(matches!(
        cal.add_closure(&runtime, &new_closure(d(2026, 10, 16), " ")),
        Err(CalendarError::Refused {
            reason: RefusalReason::EmptyName,
            ..
        })
    ));

    // No temporary files left behind; the lock file lives under locks/.
    let names: Vec<String> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().all(|n| !n.contains(".tmp.")), "{names:?}");
    assert!(
        dir.path()
            .join("locks/nyse_calendar_overrides.lock")
            .exists()
    );
}

#[test]
fn add_closure_picks_up_entries_written_by_another_process() {
    let dir = tempfile::tempdir().unwrap();
    let seed = write(dir.path(), "seed.toml", SEED);
    let runtime = dir.path().join("nyse_calendar_overrides.toml");
    let mut cal = Calendar::load(&seed, &runtime).unwrap();
    fs::write(
        &runtime,
        "[[closure]]\ndate = \"2026-10-14\"\nname = \"other\"\n",
    )
    .unwrap();
    assert_eq!(
        cal.add_closure(&runtime, &new_closure(d(2026, 10, 14), "same"))
            .unwrap(),
        AddOutcome::AlreadyClosed
    );
    assert!(!cal.is_trading_day(d(2026, 10, 14)));
}

#[test]
fn add_closure_never_appends_to_a_malformed_file() {
    let dir = tempfile::tempdir().unwrap();
    let seed = write(dir.path(), "seed.toml", SEED);
    let runtime = dir.path().join("nyse_calendar_overrides.toml");
    let mut cal = Calendar::load(&seed, &runtime).unwrap();
    fs::write(
        &runtime,
        "[[closure]]\ndate = \"not a date\"\nname = \"x\"\n",
    )
    .unwrap();
    let before = fs::read_to_string(&runtime).unwrap();
    assert!(
        cal.add_closure(&runtime, &new_closure(d(2026, 10, 14), "x"))
            .is_err()
    );
    assert_eq!(fs::read_to_string(&runtime).unwrap(), before);
}

#[test]
fn add_closure_refuses_while_another_writer_holds_the_lock() {
    let dir = tempfile::tempdir().unwrap();
    let seed = write(dir.path(), "seed.toml", SEED);
    let runtime = dir.path().join("nyse_calendar_overrides.toml");
    let mut cal = Calendar::load(&seed, &runtime).unwrap();
    fs::create_dir_all(dir.path().join("locks")).unwrap();
    let held = fs::File::create(dir.path().join("locks/nyse_calendar_overrides.lock")).unwrap();
    held.lock().unwrap();
    assert!(matches!(
        cal.add_closure(&runtime, &new_closure(d(2026, 10, 14), "x")),
        Err(CalendarError::Locked { .. })
    ));
    assert!(!runtime.exists());
    held.unlock().unwrap();
    assert_eq!(
        cal.add_closure(&runtime, &new_closure(d(2026, 10, 14), "x"))
            .unwrap(),
        AddOutcome::Added
    );
}
