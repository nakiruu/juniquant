//! Property tests for normalization, tradability and rendering (UNIVERSE.md §3).

use std::str::FromStr;

use proptest::prelude::*;
use symbols::{Canon, Cik, SupersededTicker, Vendor, is_tradable, normalize, render};

/// Strings close to real vendor spellings: separators, `$`, `~`, case and spaces.
fn symbolish() -> impl Strategy<Value = String> {
    "[ $a-zA-Z0-9./_~-]{0,10}"
}

/// Strictly canonical symbols.
fn canonical() -> impl Strategy<Value = String> {
    "[A-Z0-9]{1,6}(\\.[A-Z])?"
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2048))]

    #[test]
    fn normalize_is_idempotent_on_outputs(raw in prop_oneof![symbolish(), any::<String>()]) {
        if let Ok(c) = normalize(&raw) {
            let again = normalize(c.as_str());
            prop_assert_eq!(again.as_ref(), Ok(&c));
        }
    }

    #[test]
    fn every_ok_output_is_tradable(raw in prop_oneof![symbolish(), any::<String>()]) {
        if let Ok(c) = normalize(&raw) {
            prop_assert!(is_tradable(c.as_str()), "{:?} -> {:?}", raw, c);
        }
    }

    #[test]
    fn tradable_implies_normalize_identity(raw in prop_oneof![symbolish(), canonical(), any::<String>()]) {
        if is_tradable(&raw) {
            let c = normalize(&raw);
            prop_assert_eq!(c.map(|c| c.to_string()), Ok(raw.clone()));
        }
    }

    #[test]
    fn canonical_strings_are_tradable_and_parse(raw in canonical()) {
        prop_assert!(is_tradable(&raw));
        let c = Canon::from_str(&raw).unwrap();
        prop_assert_eq!(c.as_str(), raw.as_str());
    }

    #[test]
    fn yahoo_render_round_trips(raw in canonical()) {
        let c = Canon::from_str(&raw).unwrap();
        let yahoo = render(&c, Vendor::Yahoo).unwrap();
        prop_assert_eq!(normalize(&yahoo), Ok(c.clone()));
        let sec = render(&c, Vendor::Sec).unwrap();
        prop_assert_eq!(normalize(&sec), Ok(c));
    }

    #[test]
    fn superseded_form_round_trips_and_is_never_tradable(raw in canonical(), cik in 1u64..=9_999_999_999) {
        let s = SupersededTicker::new(&Canon::from_str(&raw).unwrap(), Cik::new(cik).unwrap());
        let text = s.to_string();
        prop_assert!(!is_tradable(&text));
        prop_assert_eq!(normalize(&text).map_err(|e| e.kind()), Err("superseded"));
        prop_assert!(Canon::from_str(&text).is_err());
        prop_assert_eq!(SupersededTicker::from_str(&text), Ok(s));
    }
}
