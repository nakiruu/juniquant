//! `SymbolMap` load validation and lookups (09-lock-state-machine SKILL.md §3.8).

use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;

use chrono::NaiveDate;
use symbols::{Canon, Cik, MapError, SymbolMap, Vendor};

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
