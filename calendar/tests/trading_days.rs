//! Table-driven trading-day cases (09 SKILL.md §9 "Trading days").

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "integration tests: any failure must fail the test"
)]

use calendar::{Calendar, Closure};
use chrono::NaiveDate;

fn d(y: i32, m: u32, dd: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, dd).unwrap()
}

fn seed_calendar() -> Calendar {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    Calendar::load(
        &root.join("config/nyse_calendar_overrides.toml"),
        &root.join("calendar/tests/fixtures/does-not-exist.toml"),
    )
    .unwrap()
}

/// NYSE's published full-day closures, as listed in the Julia oracle's `PUBLISHED` table.
#[test]
fn published_closures_2022_2027() {
    let cal = seed_calendar();
    let published: [(i32, &[(u32, u32)]); 6] = [
        (
            2022,
            &[
                (1, 17),
                (2, 21),
                (4, 15),
                (5, 30),
                (6, 20),
                (7, 4),
                (9, 5),
                (11, 24),
                (12, 26),
            ],
        ),
        (
            2023,
            &[
                (1, 2),
                (1, 16),
                (2, 20),
                (4, 7),
                (5, 29),
                (6, 19),
                (7, 4),
                (9, 4),
                (11, 23),
                (12, 25),
            ],
        ),
        (
            2024,
            &[
                (1, 1),
                (1, 15),
                (2, 19),
                (3, 29),
                (5, 27),
                (6, 19),
                (7, 4),
                (9, 2),
                (11, 28),
                (12, 25),
            ],
        ),
        (
            2025,
            &[
                (1, 1),
                (1, 9),
                (1, 20),
                (2, 17),
                (4, 18),
                (5, 26),
                (6, 19),
                (7, 4),
                (9, 1),
                (11, 27),
                (12, 25),
            ],
        ),
        (
            2026,
            &[
                (1, 1),
                (1, 19),
                (2, 16),
                (4, 3),
                (5, 25),
                (6, 19),
                (7, 3),
                (9, 7),
                (11, 26),
                (12, 25),
            ],
        ),
        (
            2027,
            &[
                (1, 1),
                (1, 18),
                (2, 15),
                (3, 26),
                (5, 31),
                (6, 18),
                (7, 5),
                (9, 6),
                (11, 25),
                (12, 24),
            ],
        ),
    ];
    for (y, days) in published {
        let want: Vec<NaiveDate> = days.iter().map(|&(m, dd)| d(y, m, dd)).collect();
        assert_eq!(cal.holidays(y), want, "{y}");
        for day in want {
            assert!(!cal.is_trading_day(day));
            assert!(cal.is_closure(day));
        }
    }
}

#[test]
fn weekends_are_not_sessions_nor_closures() {
    let cal = Calendar::rules_only();
    assert!(!cal.is_trading_day(d(2026, 10, 10))); // Saturday
    assert!(!cal.is_trading_day(d(2026, 10, 11))); // Sunday
    assert!(!cal.is_closure(d(2026, 10, 10)));
    assert!(cal.is_trading_day(d(2026, 10, 12))); // Monday (Columbus Day: NYSE open)
    assert!(cal.is_trading_day(d(2026, 11, 11))); // Veterans Day: NYSE open
}

#[test]
fn add_trading_days_semantics() {
    let cal = Calendar::rules_only();
    // n = 0 on a session is the identity; on a non-session it rolls forward.
    assert_eq!(
        cal.add_trading_days(d(2026, 10, 12), 0).unwrap(),
        d(2026, 10, 12)
    );
    assert_eq!(
        cal.add_trading_days(d(2026, 10, 10), 0).unwrap(),
        d(2026, 10, 12)
    );
    // A non-session start rolls forward first, then advances.
    assert_eq!(
        cal.add_trading_days(d(2026, 10, 10), 1).unwrap(),
        d(2026, 10, 13)
    );
    assert_eq!(
        cal.add_trading_days(d(2026, 11, 26), 1).unwrap(),
        d(2026, 11, 30)
    );
}

#[test]
fn thanksgiving_week_weekday_count_is_too_early() {
    let cal = Calendar::rules_only();
    // Buy Mon 23 Nov 2026: a weekday count gives Mon 30 Nov, the trading-day count Tue 1 Dec.
    assert_eq!(
        cal.add_trading_days(d(2026, 11, 23), 5).unwrap(),
        d(2026, 12, 1)
    );
}

#[test]
fn christmas_week_and_year_end() {
    let cal = Calendar::rules_only();
    // Fri 25 Dec 2026 and Fri 1 Jan 2027 are closed.
    assert_eq!(
        cal.add_trading_days(d(2026, 12, 21), 5).unwrap(),
        d(2026, 12, 29)
    );
    assert_eq!(
        cal.add_trading_days(d(2026, 12, 28), 5).unwrap(),
        d(2027, 1, 5)
    );
    // Saturday 1 Jan 2022: Fri 31 Dec 2021 is a session.
    assert!(cal.is_trading_day(d(2021, 12, 31)));
    // Saturday 25 Dec 2027 -> Fri 24 Dec closed; Sat 1 Jan 2028 -> Fri 31 Dec open.
    assert!(!cal.is_trading_day(d(2027, 12, 24)));
    assert!(cal.is_trading_day(d(2027, 12, 31)));
}

#[test]
fn override_seeds_2018_and_2025() {
    let cal = seed_calendar();
    assert_eq!(
        cal.add_trading_days(d(2018, 12, 3), 5).unwrap(),
        d(2018, 12, 11)
    );
    assert_eq!(
        cal.add_trading_days(d(2025, 1, 6), 5).unwrap(),
        d(2025, 1, 14)
    );
    // The rules alone would be one session earlier: a missing override shortens a block.
    let rules = Calendar::rules_only();
    assert_eq!(
        rules.add_trading_days(d(2025, 1, 6), 5).unwrap(),
        d(2025, 1, 13)
    );
}

#[test]
fn far_future_2040() {
    let cal = Calendar::rules_only();
    let h = cal.holidays(2040);
    assert_eq!(h.len(), 10);
    assert!(h.contains(&d(2040, 3, 30))); // Good Friday 2040 (Easter 1 April)
    assert!(h.contains(&d(2040, 6, 19)));
}

#[test]
fn unplanned_closure_added_lengthens_block() {
    let rules = Calendar::rules_only();
    let with = Calendar::from_closures([Closure::new(d(2026, 10, 14), "unplanned")]).unwrap();
    assert_eq!(
        rules.add_trading_days(d(2026, 10, 12), 5).unwrap(),
        d(2026, 10, 19)
    );
    assert_eq!(
        with.add_trading_days(d(2026, 10, 12), 5).unwrap(),
        d(2026, 10, 20)
    );
    assert_eq!(with.holidays(2026).len(), 11);
}

#[test]
fn next_prev_and_between() {
    let cal = Calendar::rules_only();
    assert_eq!(
        cal.next_trading_day(d(2026, 11, 25)).unwrap(),
        d(2026, 11, 27)
    );
    assert_eq!(
        cal.prev_trading_day(d(2026, 11, 27)).unwrap(),
        d(2026, 11, 25)
    );
    assert_eq!(
        cal.next_trading_day(d(2026, 10, 9)).unwrap(),
        d(2026, 10, 12)
    );
    assert_eq!(
        cal.prev_trading_day(d(2026, 10, 12)).unwrap(),
        d(2026, 10, 9)
    );
    // (from, to]: Mon 23 Nov -> Tue 1 Dec 2026 is 5 sessions.
    assert_eq!(cal.trading_days_between(d(2026, 11, 23), d(2026, 12, 1)), 5);
    assert_eq!(
        cal.trading_days_between(d(2026, 12, 1), d(2026, 11, 23)),
        -5
    );
    assert_eq!(
        cal.trading_days_between(d(2026, 11, 23), d(2026, 11, 23)),
        0
    );
    // A non-session `from` is excluded, a weekend `to` adds nothing.
    assert_eq!(
        cal.trading_days_between(d(2026, 10, 10), d(2026, 10, 11)),
        0
    );
}

#[test]
fn date_range_end_is_an_error_not_a_panic() {
    let cal = Calendar::rules_only();
    assert!(cal.add_trading_days(NaiveDate::MAX, 1).is_err());
    assert!(cal.prev_trading_day(NaiveDate::MIN).is_err());
}

/// `trading_days_between` counts by weeks and closures; it must equal a day-by-day count, with
/// an override on an ordinary day, an override on a rule holiday (Good Friday, counted once),
/// ranges across years, reversed ranges and empty ones.
#[test]
fn trading_days_between_matches_a_day_by_day_count() {
    let cal = Calendar::from_closures([
        Closure::new(d(2024, 3, 20), "test closure"),
        Closure::new(d(2024, 3, 29), "also a rule holiday"),
    ])
    .unwrap();
    let ranges = [
        (d(2024, 3, 1), d(2024, 4, 30)),
        (d(2023, 12, 29), d(2025, 1, 3)),
        (d(2024, 3, 20), d(2024, 3, 20)),
        (d(2024, 5, 1), d(2024, 3, 1)),
        (d(2024, 3, 16), d(2024, 3, 17)),
        (d(2019, 6, 3), d(2031, 6, 9)),
    ];
    for (from, to) in ranges {
        let (lo, hi, sign) = if from <= to {
            (from, to, 1)
        } else {
            (to, from, -1)
        };
        let brute = lo
            .iter_days()
            .skip(1)
            .take_while(|x| *x <= hi)
            .filter(|x| cal.is_trading_day(*x))
            .count();
        assert_eq!(
            cal.trading_days_between(from, to),
            sign * i64::try_from(brute).unwrap(),
            "{from} to {to}"
        );
    }
}
