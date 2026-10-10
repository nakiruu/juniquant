//! `SymbolMap` load validation and lookups (09-lock-state-machine SKILL.md §3.8).

use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;

use chrono::{Days, NaiveDate};
use symbols::{Canon, Cik, MapError, PriorTicker, SymbolMap, Vendor};

// The helpers below sit outside `#[test]` functions, where clippy.toml's test allowance
// does not reach; a bad literal in a test must fail that test.

#[allow(clippy::unwrap_used, reason = "test helper")]
fn canon(s: &str) -> Canon {
    Canon::from_str(s).unwrap()
}

#[allow(clippy::unwrap_used, reason = "test helper")]
fn date(s: &str) -> NaiveDate {
    NaiveDate::from_str(s).unwrap()
}

#[allow(clippy::unwrap_used, reason = "test helper")]
fn load_err(toml: &str) -> MapError {
    SymbolMap::from_toml_str(toml).unwrap_err()
}

const SAMPLE: &str = r#"
[[symbol]]
canonical  = "BRK.B"
yahoo      = "BRK-B"
ishares    = "BRKB"
schwab     = "BRK/B"
lock_group = "BRK"

[[symbol]]
canonical  = "BRK.A"
yahoo      = "BRK-A"
schwab     = "BRK/A"
lock_group = "BRK"

[[symbol]]
canonical  = "GOOGL"
yahoo      = "GOOGL"
ishares    = "GOOGL"
schwab     = "GOOGL"
lock_group = "ALPHABET"

[[symbol]]
canonical  = "GOOG"
yahoo      = "GOOG"
ishares    = "GOOG"
schwab     = "GOOG"
lock_group = "ALPHABET"

[[symbol]]
canonical  = "META"
yahoo      = "META"
schwab     = "META"
lock_group = "META"
aliases    = [{ symbol = "FB", until = "2022-06-09" }]
"#;

#[test]
fn committed_config_loads_as_empty_map() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../config/symbol_map.toml");
    let map = SymbolMap::load(&path).unwrap();
    assert!(map.is_empty());
    assert_eq!(map.len(), 0);
    assert_eq!(map.resolve("AAPL", Vendor::Yahoo), None);
    assert_eq!(map.lock_group(&canon("AAPL")), None);
}

#[test]
fn empty_string_loads_as_empty_map() {
    assert!(SymbolMap::from_toml_str("").unwrap().is_empty());
}

#[test]
fn missing_file_is_an_io_error() {
    let err = SymbolMap::load(std::path::Path::new("/nonexistent/symbol_map.toml")).unwrap_err();
    assert_eq!(err.kind(), "io");
}

#[test]
fn share_classes_resolve_to_one_group() {
    let map = SymbolMap::from_toml_str(SAMPLE).unwrap();
    assert_eq!(map.len(), 5);
    let googl = map.resolve("googl", Vendor::Schwab).unwrap().clone();
    let goog = map.resolve("GOOG", Vendor::Yahoo).unwrap().clone();
    assert_eq!(map.lock_group(&googl), Some("ALPHABET"));
    assert_eq!(map.lock_group(&goog), map.lock_group(&googl));
    let members = map.group_members("ALPHABET").unwrap();
    assert_eq!(members, &[canon("GOOG"), canon("GOOGL")]);
    assert_eq!(map.group_members_of(&goog), map.group_members_of(&googl));
    assert_eq!(
        map.group_members("BRK").unwrap(),
        &[canon("BRK.A"), canon("BRK.B")]
    );
    assert_eq!(map.group_members("NOPE"), None);
}

#[test]
fn vendor_spellings_resolve_after_normalization() {
    let map = SymbolMap::from_toml_str(SAMPLE).unwrap();
    let brkb = canon("BRK.B");
    assert_eq!(map.resolve("BRK/B", Vendor::Schwab), Some(&brkb));
    assert_eq!(map.resolve(" brk-b ", Vendor::Yahoo), Some(&brkb));
    assert_eq!(map.resolve("BRKB", Vendor::IShares), Some(&brkb));
    // A fused class letter is only an iShares spelling here, not a Yahoo one: never a guess.
    assert_eq!(map.resolve("BRKB", Vendor::Yahoo), None);
    // SEC has no column: the rule-derived spelling must equal a canonical ticker.
    assert_eq!(map.resolve("BRK-B", Vendor::Sec), Some(&brkb));
    // BRK.A has no iShares spelling: not resolvable from iShares.
    assert_eq!(map.resolve("BRK.A", Vendor::IShares), None);
    // Unmapped, unparseable and superseded spellings are None.
    assert_eq!(map.resolve("AAPL", Vendor::Yahoo), None);
    assert_eq!(map.resolve("BAC-PL", Vendor::Yahoo), None);
    assert_eq!(map.resolve("BRK.B~0001067983", Vendor::Yahoo), None);
    assert_eq!(map.spelling(&brkb, Vendor::Schwab), Some("BRK/B"));
    assert_eq!(map.spelling(&canon("BRK.A"), Vendor::IShares), None);
    assert!(map.get(&brkb).is_some());
    assert!(map.get(&canon("AAPL")).is_none());
}

#[test]
fn aliases_resolve_until_their_date() {
    let map = SymbolMap::from_toml_str(SAMPLE).unwrap();
    let meta = canon("META");
    assert_eq!(map.resolve_alias("FB", date("2021-01-04")), Some(&meta));
    assert_eq!(map.resolve_alias("fb", date("2022-06-09")), Some(&meta));
    assert_eq!(map.resolve_alias("FB", date("2022-06-10")), None);
    assert_eq!(map.resolve_alias("META", date("2021-01-04")), None);
    // resolve_on: vendor spelling first, then a dated alias.
    assert_eq!(
        map.resolve_on("FB", Vendor::Schwab, date("2022-01-03")),
        Some(&meta)
    );
    assert_eq!(
        map.resolve_on("FB", Vendor::Schwab, date("2023-01-03")),
        None
    );
    assert_eq!(
        map.resolve_on("META", Vendor::Schwab, date("2021-01-04")),
        Some(&meta)
    );
}

#[test]
fn duplicate_canonical_fails() {
    let err = load_err(
        r#"
[[symbol]]
canonical = "AAPL"
lock_group = "AAPL"
[[symbol]]
canonical = "AAPL"
lock_group = "APPLE"
"#,
    );
    assert_eq!(err.kind(), "duplicate_canonical");
}

#[test]
fn duplicate_schwab_spelling_fails() {
    let err = load_err(
        r#"
[[symbol]]
canonical = "BRK.B"
schwab = "BRK/B"
lock_group = "BRK"
[[symbol]]
canonical = "BRK.A"
schwab = "BRK/B"
lock_group = "BRK"
"#,
    );
    assert_eq!(err.kind(), "duplicate_spelling");
    assert!(err.to_string().contains("schwab"), "{err}");
}

#[test]
fn duplicate_spelling_after_normalization_fails() {
    // "BRK-B" and "brk/b" are one spelling once normalized.
    let err = load_err(
        r#"
[[symbol]]
canonical = "BRK.B"
yahoo = "BRK-B"
lock_group = "BRK"
[[symbol]]
canonical = "BRK.A"
yahoo = "brk/b"
lock_group = "BRK"
"#,
    );
    assert_eq!(err.kind(), "duplicate_spelling");
}

#[test]
fn alias_colliding_with_canonical_fails() {
    let err = load_err(
        r#"
[[symbol]]
canonical = "META"
lock_group = "META"
aliases = [{ symbol = "FB", until = "2022-06-09" }]
[[symbol]]
canonical = "FB"
lock_group = "FB"
"#,
    );
    assert_eq!(err.kind(), "alias_collision");
}

#[test]
fn alias_colliding_with_own_canonical_fails() {
    let err = load_err(
        r#"
[[symbol]]
canonical = "META"
lock_group = "META"
aliases = [{ symbol = "META", until = "2022-06-09" }]
"#,
    );
    assert_eq!(err.kind(), "alias_collision");
}

#[test]
fn alias_colliding_with_another_alias_fails() {
    let err = load_err(
        r#"
[[symbol]]
canonical = "META"
lock_group = "META"
aliases = [{ symbol = "FB", until = "2022-06-09" }]
[[symbol]]
canonical = "XYZ"
lock_group = "XYZ"
aliases = [{ symbol = "fb", until = "2010-01-01" }]
"#,
    );
    assert_eq!(err.kind(), "alias_collision");
}

#[test]
fn malformed_entries_fail() {
    let cases = [
        // canonical must already be canonical
        (
            "[[symbol]]\ncanonical = \"brk-b\"\nlock_group = \"BRK\"\n",
            "not_canonical",
        ),
        (
            "[[symbol]]\ncanonical = \"ABC~0000012345\"\nlock_group = \"ABC\"\n",
            "not_canonical",
        ),
        // vendor spelling that does not normalize
        (
            "[[symbol]]\ncanonical = \"BAC\"\nyahoo = \"BAC-PL\"\nlock_group = \"BAC\"\n",
            "invalid_spelling",
        ),
        // alias that does not normalize
        (
            "[[symbol]]\ncanonical = \"BAC\"\nlock_group = \"BAC\"\naliases = [{ symbol = \"\", until = \"2020-01-01\" }]\n",
            "invalid_spelling",
        ),
        // bad alias date
        (
            "[[symbol]]\ncanonical = \"BAC\"\nlock_group = \"BAC\"\naliases = [{ symbol = \"BOA\", until = \"2020-13-01\" }]\n",
            "invalid_date",
        ),
        // empty lock group
        (
            "[[symbol]]\ncanonical = \"BAC\"\nlock_group = \" \"\n",
            "empty_lock_group",
        ),
        // missing lock group, unknown field, wrong type
        ("[[symbol]]\ncanonical = \"BAC\"\n", "toml"),
        (
            "[[symbol]]\ncanonical = \"BAC\"\nlock_group = \"BAC\"\nsector = \"x\"\n",
            "toml",
        ),
        ("[[symbol]]\ncanonical = 5\nlock_group = \"BAC\"\n", "toml"),
        ("not toml at all [", "toml"),
    ];
    for (toml, kind) in cases {
        let err = load_err(toml);
        assert_eq!(err.kind(), kind, "{toml:?}: {err}");
    }
}

#[test]
fn issuer_check_passes_when_classes_share_a_group() {
    let map = SymbolMap::from_toml_str(SAMPLE).unwrap();
    let alphabet = Cik::from_str("1652044").unwrap();
    let berkshire = Cik::from_str("1067983").unwrap();
    let issuers = HashMap::from([
        (canon("GOOG"), alphabet),
        (canon("GOOGL"), alphabet),
        (canon("BRK.A"), berkshire),
        (canon("BRK.B"), berkshire),
        // Tickers outside the map are ignored.
        (canon("AAPL"), Cik::from_str("320193").unwrap()),
    ]);
    map.validate_issuers(&issuers).unwrap();
}

#[test]
fn issuer_check_fails_when_one_issuer_spans_two_groups() {
    // B1: GOOG and GOOGL share a CIK, so a map that splits them must not load.
    let map = SymbolMap::from_toml_str(
        r#"
[[symbol]]
canonical = "GOOGL"
lock_group = "GOOGL"
[[symbol]]
canonical = "GOOG"
lock_group = "GOOG"
"#,
    )
    .unwrap();
    let alphabet = Cik::from_str("0001652044").unwrap();
    let issuers = HashMap::from([(canon("GOOG"), alphabet), (canon("GOOGL"), alphabet)]);
    let err = map.validate_issuers(&issuers).unwrap_err();
    assert_eq!(err.kind(), "issuer_group_conflict");
    assert!(err.to_string().contains("0001652044"), "{err}");
}

// DECISIONS B5: a ticker string reused by another security after a rename. The reusing
// entry is fictional ("FB" relisted by a new issuer); the dates only exercise the rules.
const REUSE: &str = r#"
[[symbol]]
canonical  = "META"
schwab     = "META"
lock_group = "META"
aliases    = [{ symbol = "FB", until = "2024-02-28" }]

[[symbol]]
canonical  = "FB"
from       = "2024-03-01"
ishares    = "FBNEW"
schwab     = "FB"
lock_group = "NEWFB"
"#;

const LOOKBACK: Days = Days::new(45);

#[test]
fn reused_ticker_loads_and_resolves_by_date() {
    let map = SymbolMap::from_toml_str(REUSE).unwrap();
    let (meta, fb) = (canon("META"), canon("FB"));
    assert_eq!(map.get(&fb).unwrap().from(), Some(date("2024-03-01")));
    // The old holder through `until`, nobody in the gap, the new holder from `from`.
    assert_eq!(
        map.resolve_on("FB", Vendor::Schwab, date("2024-02-28")),
        Some(&meta)
    );
    assert_eq!(
        map.resolve_on("FB", Vendor::Schwab, date("2024-02-29")),
        None
    );
    assert_eq!(
        map.resolve_on("fb", Vendor::Schwab, date("2024-03-01")),
        Some(&fb)
    );
    assert_eq!(map.resolve_alias("FB", date("2024-03-01")), None);
    // Today's lookup sees only the current holder.
    assert_eq!(map.resolve("FB", Vendor::Schwab), Some(&fb));
    assert_eq!(map.resolve("FB", Vendor::Sec), Some(&fb));
}

#[test]
fn history_near_a_handover_counts_against_both_holders() {
    let map = SymbolMap::from_toml_str(REUSE).unwrap();
    let (meta, fb) = (canon("META"), canon("FB"));
    let hist = |on: &str| map.resolve_history("FB", Vendor::Schwab, date(on), LOOKBACK);
    assert_eq!(hist("2023-06-01"), vec![&meta]);
    // Within the lookback of the handover on either side: both (fail closed).
    assert_eq!(hist("2024-02-01"), vec![&fb, &meta]);
    assert_eq!(hist("2024-03-05"), vec![&fb, &meta]);
    assert_eq!(hist("2024-04-13"), vec![&fb, &meta]); // 2024-02-28 + 45 days (leap year)
    assert_eq!(hist("2024-04-14"), vec![&fb]);
    // With no margin it is the dated holder only.
    assert_eq!(
        map.resolve_history("FB", Vendor::Schwab, date("2024-03-05"), Days::new(0)),
        vec![&fb]
    );
}

#[test]
fn stale_old_ticker_after_a_rename_still_reaches_the_renamed_security() {
    let map = SymbolMap::from_toml_str(SAMPLE).unwrap();
    let meta = canon("META");
    // An external-file row typed with the old ticker 10 days after the rename.
    assert_eq!(
        map.resolve_on("FB", Vendor::Schwab, date("2022-06-19")),
        None
    );
    assert_eq!(
        map.resolve_history("FB", Vendor::Schwab, date("2022-06-19"), LOOKBACK),
        vec![&meta]
    );
}

#[test]
fn history_in_a_gap_is_unmapped() {
    let map = SymbolMap::from_toml_str(
        r#"
[[symbol]]
canonical  = "META"
lock_group = "META"
aliases    = [{ symbol = "FB", until = "2022-06-09" }]
[[symbol]]
canonical  = "FB"
from       = "2024-03-01"
lock_group = "NEWFB"
"#,
    )
    .unwrap();
    // Nobody held FB in mid-2023: empty, so the record is carried under its raw symbol.
    assert!(
        map.resolve_history("FB", Vendor::Schwab, date("2023-06-01"), LOOKBACK)
            .is_empty()
    );
}

#[test]
fn vendor_column_respects_the_entry_from_date() {
    let map = SymbolMap::from_toml_str(REUSE).unwrap();
    let fb = canon("FB");
    assert_eq!(
        map.resolve_on("FBNEW", Vendor::IShares, date("2024-01-02")),
        None
    );
    assert_eq!(
        map.resolve_on("FBNEW", Vendor::IShares, date("2024-03-01")),
        Some(&fb)
    );
    assert_eq!(
        map.resolve_history("FBNEW", Vendor::IShares, date("2024-02-01"), LOOKBACK),
        vec![&fb]
    );
    assert!(
        map.resolve_history("FBNEW", Vendor::IShares, date("2023-06-01"), LOOKBACK)
            .is_empty()
    );
}

#[test]
fn two_dated_aliases_of_one_string_load() {
    let map = SymbolMap::from_toml_str(
        r#"
[[symbol]]
canonical = "META"
lock_group = "META"
aliases = [{ symbol = "FB", from = "2010-06-01", until = "2022-06-09" }]
[[symbol]]
canonical = "XYZ"
lock_group = "XYZ"
aliases = [{ symbol = "fb", until = "2010-01-01" }]
"#,
    )
    .unwrap();
    assert_eq!(
        map.resolve_alias("FB", date("2009-12-31")),
        Some(&canon("XYZ"))
    );
    assert_eq!(map.resolve_alias("FB", date("2010-03-01")), None);
    assert_eq!(
        map.resolve_alias("FB", date("2011-01-03")),
        Some(&canon("META"))
    );
}

#[test]
fn overlapping_reuse_fails() {
    for reuser_from in ["2024-02-28", "2024-01-01"] {
        let err = load_err(&REUSE.replace("2024-03-01", reuser_from));
        assert_eq!(err.kind(), "alias_collision", "{reuser_from}");
        assert!(err.to_string().contains("FB"), "{err}");
    }
}

#[test]
fn bad_periods_fail() {
    let err = load_err(
        r#"
[[symbol]]
canonical = "META"
lock_group = "META"
aliases = [{ symbol = "FB", from = "2022-07-01", until = "2022-06-09" }]
"#,
    );
    assert_eq!(err.kind(), "empty_period");
    for toml in [
        "[[symbol]]\ncanonical = \"FB\"\nfrom = \"2024-02-30\"\nlock_group = \"FB\"\n",
        "[[symbol]]\ncanonical = \"META\"\nlock_group = \"META\"\naliases = [{ symbol = \"FB\", from = \"x\", until = \"2022-06-09\" }]\n",
    ] {
        assert_eq!(load_err(toml).kind(), "invalid_date", "{toml}");
    }
}

#[test]
fn prior_tickers_corroborate_alias_dates() {
    let map = SymbolMap::from_toml_str(REUSE).unwrap();
    let fb_prior = |to: &str| {
        HashMap::from([(
            canon("META"),
            vec![PriorTicker {
                ticker: canon("FB"),
                from: Some(date("2012-05-18")),
                to: date(to),
            }],
        )])
    };
    map.validate_prior_tickers(&fb_prior("2024-02-28")).unwrap();
    // Securities absent from security_master are ignored.
    map.validate_prior_tickers(&HashMap::new()).unwrap();
    // An alias `until` one day off fails the load.
    let err = map
        .validate_prior_tickers(&fb_prior("2024-02-27"))
        .unwrap_err();
    assert_eq!(err.kind(), "prior_ticker_mismatch");
    // A security_master row without the alias ticker fails too.
    let err = map
        .validate_prior_tickers(&HashMap::from([(canon("META"), Vec::new())]))
        .unwrap_err();
    assert_eq!(err.kind(), "prior_ticker_mismatch");
}

#[test]
fn alias_from_must_match_prior_from() {
    let map = SymbolMap::from_toml_str(
        r#"
[[symbol]]
canonical = "META"
lock_group = "META"
aliases = [{ symbol = "FB", from = "2012-05-18", until = "2022-06-09" }]
"#,
    )
    .unwrap();
    let prior = |from: Option<&str>| {
        HashMap::from([(
            canon("META"),
            vec![PriorTicker {
                ticker: canon("FB"),
                from: from.map(date),
                to: date("2022-06-09"),
            }],
        )])
    };
    map.validate_prior_tickers(&prior(Some("2012-05-18")))
        .unwrap();
    for bad in [Some("2012-05-17"), None] {
        assert_eq!(
            map.validate_prior_tickers(&prior(bad)).unwrap_err().kind(),
            "prior_ticker_mismatch"
        );
    }
}

#[test]
fn vendor_spelling_reaching_another_entrys_ticker_fails() {
    // Review finding: an iShares spelling "FB" on META while FB is another entry's ticker
    // would let a record under "FB" skip META. The load must refuse it.
    let err = load_err(
        r#"
[[symbol]]
canonical = "META"
ishares   = "FB"
lock_group = "META"
[[symbol]]
canonical = "FB"
lock_group = "NEWFB"
"#,
    );
    assert_eq!(err.kind(), "spelling_collision");
    // A spelling of a string the entry itself holds (the reuser's own ticker) is fine.
    SymbolMap::from_toml_str(REUSE).unwrap();
}

#[test]
fn reuse_boundary_is_strict() {
    // Old alias until 2024-02-28: a reuser from the same day overlaps, from the next day loads.
    let map = |from: &str| SymbolMap::from_toml_str(&REUSE.replace("2024-03-01", from));
    assert_eq!(map("2024-02-28").unwrap_err().kind(), "alias_collision");
    map("2024-02-29").unwrap();
}

#[test]
fn overlap_between_first_and_third_holding_fails() {
    let err = load_err(
        r#"
[[symbol]]
canonical = "AAA"
lock_group = "AAA"
aliases = [{ symbol = "X", until = "2010-12-31" }]
[[symbol]]
canonical = "BBB"
lock_group = "BBB"
aliases = [{ symbol = "X", from = "2012-01-01", until = "2015-12-31" }]
[[symbol]]
canonical = "X"
from = "2010-06-01"
lock_group = "CCC"
"#,
    );
    assert_eq!(err.kind(), "alias_collision");
}

#[test]
fn margin_overflow_widens_without_bound() {
    // Widening past the representable range counts as unbounded (fail closed), not as a
    // miss: the old holder still matches near the end of time.
    let map = SymbolMap::from_toml_str(&format!(
        "[[symbol]]\ncanonical = \"META\"\nlock_group = \"META\"\naliases = [{{ symbol = \"FB\", from = \"{min}\", until = \"{max}\" }}]\n",
        min = NaiveDate::MIN.format("%Y-%m-%d"),
        max = NaiveDate::MAX.format("%Y-%m-%d"),
    ));
    // chrono's extreme dates round-trip through the date format (+262142-12-31), so both
    // widenings overflow here.
    let map = map.unwrap();
    for on in [NaiveDate::MIN, NaiveDate::MAX] {
        assert_eq!(
            map.resolve_history("FB", Vendor::Schwab, on, LOOKBACK),
            vec![&canon("META")]
        );
    }
}

#[test]
fn prior_ticker_without_an_alias_fails() {
    // Review finding: security_master knows META was FB, but the map forgot the alias.
    let map = SymbolMap::from_toml_str("[[symbol]]\ncanonical = \"META\"\nlock_group = \"META\"\n")
        .unwrap();
    let prior = HashMap::from([(
        canon("META"),
        vec![PriorTicker {
            ticker: canon("FB"),
            from: None,
            to: date("2022-06-09"),
        }],
    )]);
    let err = map.validate_prior_tickers(&prior).unwrap_err();
    assert_eq!(err.kind(), "missing_alias");
    assert!(err.to_string().contains("FB"), "{err}");
}
