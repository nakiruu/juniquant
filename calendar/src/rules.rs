//! The NYSE full-day holiday rules, computed for any year (09 SKILL.md §4 "Rules").
//!
//! The rules are pure functions of the year: there is no table that can run out.
//! Ad-hoc closures (national days of mourning, disasters) are not predictable by rule
//! and come from the overrides files instead ([`crate::Calendar`]).

use chrono::{Datelike, Days, NaiveDate, Weekday};

/// Which rule produced a holiday.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HolidayRule {
    /// New Year's Day, 1 January. A Saturday 1 January closes nothing (NYSE does not close
    /// on the preceding 31 December); a Sunday 1 January moves to Monday 2 January.
    NewYearsDay,
    /// Martin Luther King Jr. Day: third Monday of January.
    MartinLutherKingJrDay,
    /// Washington's Birthday (Presidents' Day): third Monday of February.
    WashingtonsBirthday,
    /// Good Friday: two days before Gregorian Easter Sunday.
    GoodFriday,
    /// Memorial Day: last Monday of May.
    MemorialDay,
    /// Juneteenth National Independence Day, 19 June, observed by NYSE from 2022.
    Juneteenth,
    /// Independence Day, 4 July.
    IndependenceDay,
    /// Labor Day: first Monday of September.
    LaborDay,
    /// Thanksgiving Day: fourth Thursday of November.
    Thanksgiving,
    /// Christmas Day, 25 December.
    Christmas,
}

impl HolidayRule {
    /// Human-readable holiday name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::NewYearsDay => "New Year's Day",
            Self::MartinLutherKingJrDay => "Martin Luther King Jr. Day",
            Self::WashingtonsBirthday => "Washington's Birthday",
            Self::GoodFriday => "Good Friday",
            Self::MemorialDay => "Memorial Day",
            Self::Juneteenth => "Juneteenth National Independence Day",
            Self::IndependenceDay => "Independence Day",
            Self::LaborDay => "Labor Day",
            Self::Thanksgiving => "Thanksgiving Day",
            Self::Christmas => "Christmas Day",
        }
    }
}

/// One rule-computed full-day closure: the observed date and the rule behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RuleHoliday {
    /// The date the exchange is closed (after the weekend observance rule).
    pub date: NaiveDate,
    /// The rule that produced it.
    pub rule: HolidayRule,
}

/// First year in which NYSE closes for Juneteenth.
pub const JUNETEENTH_FIRST_YEAR: i32 = 2022;

/// `true` for Saturday and Sunday.
#[must_use]
pub fn is_weekend(date: NaiveDate) -> bool {
    matches!(date.weekday(), Weekday::Sat | Weekday::Sun)
}

/// The rule-computed NYSE full-day holidays of `year`, sorted by date.
///
/// Every observed date falls inside `year` (the only rule that could cross a year boundary,
/// a Saturday New Year's Day, closes nothing), so the holidays of a date's own year decide
/// whether a weekday is a rule holiday. Returns an empty list only for years outside
/// `chrono`'s representable range.
///
/// Historical note: the rules are applied uniformly to every year. Before the rules took
/// their current form (for example MLK Day before 1998) this yields extra closures, which
/// is the safe direction for the lock (09 SKILL.md §4 "Errors err toward closed").
#[must_use]
pub fn rule_holidays(year: i32) -> Vec<RuleHoliday> {
    let mut out: Vec<RuleHoliday> = candidates(year)
        .into_iter()
        .filter_map(|(date, rule)| date.map(|date| RuleHoliday { date, rule }))
        .collect();
    out.sort();
    out
}

/// The ten rules applied to `year`, unsorted; allocation-free so `is_rule_holiday` can sit in
/// the per-day loops of the trading-day walks.
fn candidates(year: i32) -> [(Option<NaiveDate>, HolidayRule); 10] {
    let fixed = |month: u32, day: u32| NaiveDate::from_ymd_opt(year, month, day);
    [
        (
            fixed(1, 1).and_then(observed_new_years),
            HolidayRule::NewYearsDay,
        ),
        (
            nth_weekday(year, 1, Weekday::Mon, 3),
            HolidayRule::MartinLutherKingJrDay,
        ),
        (
            nth_weekday(year, 2, Weekday::Mon, 3),
            HolidayRule::WashingtonsBirthday,
        ),
        (
            easter_sunday(year).and_then(|e| e.checked_sub_days(Days::new(2))),
            HolidayRule::GoodFriday,
        ),
        (
            last_weekday(year, 5, Weekday::Mon),
            HolidayRule::MemorialDay,
        ),
        (
            if year >= JUNETEENTH_FIRST_YEAR {
                fixed(6, 19).and_then(observed)
            } else {
                None
            },
            HolidayRule::Juneteenth,
        ),
        (fixed(7, 4).and_then(observed), HolidayRule::IndependenceDay),
        (nth_weekday(year, 9, Weekday::Mon, 1), HolidayRule::LaborDay),
        (
            nth_weekday(year, 11, Weekday::Thu, 4),
            HolidayRule::Thanksgiving,
        ),
        (fixed(12, 25).and_then(observed), HolidayRule::Christmas),
    ]
}

/// `true` when `date` is a rule-computed holiday (overrides not included).
#[must_use]
pub fn is_rule_holiday(date: NaiveDate) -> bool {
    candidates(date.year())
        .iter()
        .any(|(d, _)| *d == Some(date))
}

/// Gregorian Easter Sunday of `year` (anonymous Gregorian algorithm, Meeus/Jones/Butcher).
///
/// Returns `None` only for years outside `chrono`'s representable range.
#[must_use]
pub fn easter_sunday(year: i32) -> Option<NaiveDate> {
    let y = i64::from(year);
    let a = y.rem_euclid(19);
    let b = y.div_euclid(100);
    let c = y.rem_euclid(100);
    let d = b.div_euclid(4);
    let e = b.rem_euclid(4);
    let f = (b + 8).div_euclid(25);
    let g = (b - f + 1).div_euclid(3);
    let h = (19 * a + b - d - g + 15).rem_euclid(30);
    let i = c.div_euclid(4);
    let k = c.rem_euclid(4);
    let l = (32 + 2 * e + 2 * i - h - k).rem_euclid(7);
    let m = (a + 11 * h + 22 * l).div_euclid(451);
    let month = (h + l - 7 * m + 114).div_euclid(31);
    let day = (h + l - 7 * m + 114).rem_euclid(31) + 1;
    NaiveDate::from_ymd_opt(year, u32::try_from(month).ok()?, u32::try_from(day).ok()?)
}

/// Weekend observance for Juneteenth, Independence Day and Christmas: a Saturday moves to
/// the preceding Friday, a Sunday to the following Monday.
fn observed(date: NaiveDate) -> Option<NaiveDate> {
    match date.weekday() {
        Weekday::Sat => date.pred_opt(),
        Weekday::Sun => date.succ_opt(),
        _ => Some(date),
    }
}

/// New Year's Day observance: a Saturday closes nothing, a Sunday moves to Monday.
fn observed_new_years(date: NaiveDate) -> Option<NaiveDate> {
    match date.weekday() {
        Weekday::Sat => None,
        Weekday::Sun => date.succ_opt(),
        _ => Some(date),
    }
}

/// The `n`-th (1-based) `weekday` of `month` in `year`.
fn nth_weekday(year: i32, month: u32, weekday: Weekday, n: u8) -> Option<NaiveDate> {
    NaiveDate::from_weekday_of_month_opt(year, month, weekday, n)
}

/// The last `weekday` of `month` in `year`.
fn last_weekday(year: i32, month: u32, weekday: Weekday) -> Option<NaiveDate> {
    let first_of_next = if month == 12 {
        NaiveDate::from_ymd_opt(year.checked_add(1)?, 1, 1)?
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)?
    };
    let last = first_of_next.pred_opt()?;
    let back = (7 + last.weekday().num_days_from_monday() - weekday.num_days_from_monday()) % 7;
    last.checked_sub_days(Days::new(u64::from(back)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn easter_known_years() {
        assert_eq!(easter_sunday(2024), Some(ymd(2024, 3, 31)));
        assert_eq!(easter_sunday(2026), Some(ymd(2026, 4, 5)));
        assert_eq!(easter_sunday(2038), Some(ymd(2038, 4, 25)));
        assert_eq!(easter_sunday(2285), Some(ymd(2285, 3, 22)));
    }

    #[test]
    fn saturday_new_year_closes_nothing() {
        // 1 Jan 2022 and 1 Jan 2028 are Saturdays.
        assert!(
            !rule_holidays(2021)
                .iter()
                .any(|h| h.date == ymd(2021, 12, 31))
        );
        assert!(
            !rule_holidays(2022)
                .iter()
                .any(|h| h.rule == HolidayRule::NewYearsDay)
        );
        assert!(!is_rule_holiday(ymd(2027, 12, 31)));
    }

    #[test]
    fn sunday_new_year_moves_to_monday() {
        assert!(is_rule_holiday(ymd(2023, 1, 2)));
    }

    #[test]
    fn juneteenth_only_from_2022() {
        assert!(
            !rule_holidays(2021)
                .iter()
                .any(|h| h.rule == HolidayRule::Juneteenth)
        );
        assert!(is_rule_holiday(ymd(2022, 6, 20))); // Sunday 19 June -> Monday
        assert!(is_rule_holiday(ymd(2027, 6, 18))); // Saturday 19 June -> Friday
    }

    #[test]
    fn far_future_year_has_ten() {
        assert_eq!(rule_holidays(2040).len(), 10);
    }

    #[test]
    fn last_monday_of_may() {
        assert_eq!(last_weekday(2026, 5, Weekday::Mon), Some(ymd(2026, 5, 25)));
        assert_eq!(last_weekday(2027, 5, Weekday::Mon), Some(ymd(2027, 5, 31)));
        assert_eq!(
            last_weekday(2026, 12, Weekday::Thu),
            Some(ymd(2026, 12, 31))
        );
    }
}
