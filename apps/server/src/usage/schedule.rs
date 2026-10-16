//! Reset schedules people set by hand (U1), for accounts whose harness
//! reports no windows: "resets Mondays at 09:00 Europe/Berlin".

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Cadence {
    Daily,
    /// 1 is Monday, 7 is Sunday.
    Weekly {
        weekday: u8,
    },
    /// 1..=28, so every month has the day.
    Monthly {
        day: u8,
    },
}

impl Cadence {
    pub fn from_parts(cadence: &str, weekday: Option<i16>, month_day: Option<i16>) -> Option<Self> {
        match (cadence, weekday, month_day) {
            ("daily", None, None) => Some(Self::Daily),
            ("weekly", Some(day), None) if (1..=7).contains(&day) => Some(Self::Weekly {
                weekday: u8::try_from(day).ok()?,
            }),
            ("monthly", None, Some(day)) if (1..=28).contains(&day) => Some(Self::Monthly {
                day: u8::try_from(day).ok()?,
            }),
            _ => None,
        }
    }
}

pub fn parse_time_zone(name: &str) -> Option<Tz> {
    name.parse::<Tz>().ok()
}

/// The next reset strictly after `now`, and the one before it, so callers
/// know the window's length (months and DST changes vary it).
pub fn resets_around(
    cadence: Cadence,
    minute_of_day: u16,
    zone: Tz,
    now: DateTime<Utc>,
) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let local_today = now.with_timezone(&zone).date_naive();
    // Walk candidate dates from a little before today forward; the first
    // reset after `now` and the one before it are among them.
    let mut previous = None;
    let start = local_today.checked_sub_signed(Duration::days(40))?;
    let mut date = start;
    for _ in 0..80 {
        if matches(cadence, date)
            && let Some(at) = local_reset(zone, date, minute_of_day)
        {
            if at > now {
                return previous.map(|previous| (at, previous));
            }
            previous = Some(at);
        }
        date = date.succ_opt()?;
    }
    None
}

fn matches(cadence: Cadence, date: NaiveDate) -> bool {
    match cadence {
        Cadence::Daily => true,
        Cadence::Weekly { weekday } => date.weekday().number_from_monday() == u32::from(weekday),
        Cadence::Monthly { day } => date.day() == u32::from(day),
    }
}

/// The reset's instant on `date`. A time the clocks skip (spring forward)
/// moves to the first minute that exists after it.
fn local_reset(zone: Tz, date: NaiveDate, minute_of_day: u16) -> Option<DateTime<Utc>> {
    let naive: NaiveDateTime = date.and_hms_opt(
        u32::from(minute_of_day / 60),
        u32::from(minute_of_day % 60),
        0,
    )?;
    for shift in 0..=180 {
        if let Some(at) = zone
            .from_local_datetime(&(naive + Duration::minutes(shift)))
            .earliest()
        {
            return Some(at.with_timezone(&Utc));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::{Cadence, parse_time_zone, resets_around};

    #[test]
    fn weekly_schedules_reset_on_their_weekday_in_their_zone() {
        let zone = parse_time_zone("Europe/Berlin").expect("zone");
        // Wednesday 2026-10-14 12:00 UTC.
        let now = Utc.with_ymd_and_hms(2026, 10, 14, 12, 0, 0).unwrap();
        let (next, previous) =
            resets_around(Cadence::Weekly { weekday: 1 }, 9 * 60, zone, now).expect("resets");
        // Monday 2026-10-19 09:00 CEST is 07:00 UTC.
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 10, 19, 7, 0, 0).unwrap());
        assert_eq!(
            previous,
            Utc.with_ymd_and_hms(2026, 10, 12, 7, 0, 0).unwrap()
        );
    }

    #[test]
    fn daily_and_monthly_schedules_roll_over() {
        let zone = parse_time_zone("UTC").expect("zone");
        let now = Utc.with_ymd_and_hms(2026, 10, 31, 23, 30, 0).unwrap();
        let (next, previous) = resets_around(Cadence::Daily, 23 * 60, zone, now).expect("daily");
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 11, 1, 23, 0, 0).unwrap());
        assert_eq!(
            previous,
            Utc.with_ymd_and_hms(2026, 10, 31, 23, 0, 0).unwrap()
        );
        let (next, _) = resets_around(Cadence::Monthly { day: 1 }, 0, zone, now).expect("monthly");
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap());
    }

    #[test]
    fn a_skipped_local_time_moves_forward() {
        let zone = parse_time_zone("Europe/Berlin").expect("zone");
        // Clocks jump from 02:00 to 03:00 on 2027-03-28.
        let now = Utc.with_ymd_and_hms(2027, 3, 27, 12, 0, 0).unwrap();
        let (next, _) = resets_around(Cadence::Daily, 2 * 60 + 30, zone, now).expect("resets");
        assert_eq!(next, Utc.with_ymd_and_hms(2027, 3, 28, 1, 0, 0).unwrap());
    }

    #[test]
    fn bad_shapes_are_refused() {
        assert!(Cadence::from_parts("weekly", None, None).is_none());
        assert!(Cadence::from_parts("monthly", None, Some(31)).is_none());
        assert!(Cadence::from_parts("daily", Some(1), None).is_none());
        assert!(parse_time_zone("Mars/Olympus").is_none());
    }
}
