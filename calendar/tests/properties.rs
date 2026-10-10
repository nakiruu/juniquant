//! Property tests: `add_trading_days` lands on sessions, is monotone in `n` and in the set of
//! closures (an extra closure never moves a result earlier), and agrees with
//! `trading_days_between`, `next_trading_day` and `prev_trading_day`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "integration tests: any failure must fail the test"
)]

use calendar::{Calendar, Closure};
use chrono::{Days, NaiveDate};
use proptest::prelude::*;

fn base() -> NaiveDate {
    NaiveDate::from_ymd_opt(1990, 1, 1).unwrap()
}

/// Any date from 1990 to 2100.
fn any_date() -> impl Strategy<Value = NaiveDate> {
    (0u64..40_541).prop_map(|k| base().checked_add_days(Days::new(k)).unwrap())
}

fn seed_calendar() -> Calendar {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    Calendar::load(
        &root.join("config/nyse_calendar_overrides.toml"),
        &root.join("calendar/tests/fixtures/does-not-exist.toml"),
    )
    .unwrap()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn result_is_a_session_and_monotone(d in any_date(), n in 0u32..30) {
        let cal = seed_calendar();
        let a = cal.add_trading_days(d, n).unwrap();
        let b = cal.add_trading_days(d, n + 1).unwrap();
        prop_assert!(cal.is_trading_day(a));
        prop_assert!(cal.is_trading_day(b));
        prop_assert!(a >= d);
        prop_assert!(b > a);
        prop_assert_eq!(cal.next_trading_day(a).unwrap(), b);
        prop_assert_eq!(cal.prev_trading_day(b).unwrap(), a);
        if n >= 1 {
            prop_assert!(a > d);
        }
    }

    #[test]
    fn between_is_consistent_with_add(d in any_date(), n in 0u32..30) {
        let cal = seed_calendar();
        let start = cal.add_trading_days(d, 0).unwrap();
        let end = cal.add_trading_days(start, n).unwrap();
        prop_assert_eq!(cal.trading_days_between(start, end), i64::from(n));
        prop_assert_eq!(cal.trading_days_between(end, start), -i64::from(n));
        // From a non-session start, the roll-forward session is counted.
        prop_assert_eq!(
            cal.trading_days_between(d, end),
            i64::from(n) + i64::from(!cal.is_trading_day(d))
        );
    }

    #[test]
    fn extra_closures_never_shorten_a_block(
        d in any_date(),
        n in 0u32..15,
        extra in prop::collection::vec(0u64..60, 0..8),
    ) {
        let rules = Calendar::rules_only();
        let closures = extra
            .into_iter()
            .filter_map(|k| d.checked_add_days(Days::new(k)))
            .filter(|x| !calendar::is_weekend(*x))
            .map(|x| Closure::new(x, "extra"));
        let more = Calendar::from_closures(closures).unwrap();
        prop_assert!(more.add_trading_days(d, n).unwrap() >= rules.add_trading_days(d, n).unwrap());
    }
}
