use chrono::{
    DateTime, Datelike, FixedOffset, LocalResult, NaiveDate, TimeDelta, TimeZone, Timelike, Utc,
};
use chrono_tz::Europe::Berlin;

use crate::{
    error::{GeneratorError, Result},
    model::EditionName,
};

const MORNING_HOUR: u32 = 6;
const MORNING_MINUTE: u32 = 0;
const EVENING_HOUR: u32 = 17;
const EVENING_MINUTE: u32 = 0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BerlinDay {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PublicationWindow {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

pub fn berlin_day(now: DateTime<Utc>) -> Result<BerlinDay> {
    let date = now.with_timezone(&Berlin).date_naive();
    let tomorrow = date
        .succ_opt()
        .ok_or_else(|| GeneratorError::Config("cannot calculate berlin day boundary".into()))?;
    Ok(BerlinDay {
        start: local_time(date, 0, 0)?.with_timezone(&Utc),
        end: local_time(tomorrow, 0, 0)?.with_timezone(&Utc),
    })
}

pub fn publication_window(
    now: DateTime<Utc>,
    previous_evening_at: Option<DateTime<Utc>>,
) -> Result<PublicationWindow> {
    let start = match edition_name(now) {
        EditionName::Morning => {
            let previous_date = now
                .with_timezone(&Berlin)
                .date_naive()
                .pred_opt()
                .ok_or_else(|| {
                    GeneratorError::Config("cannot calculate previous berlin date".into())
                })?;
            let nominal_start =
                local_time(previous_date, EVENING_HOUR, EVENING_MINUTE)?.with_timezone(&Utc);

            previous_evening_at
                .filter(|generated_at| {
                    generated_at.with_timezone(&Berlin).date_naive() == previous_date
                        && *generated_at >= nominal_start
                        && *generated_at < now
                })
                .unwrap_or(nominal_start)
        }
        EditionName::Evening => berlin_day(now)?.start,
    };
    let end = now
        .checked_add_signed(TimeDelta::minutes(30))
        .ok_or_else(|| GeneratorError::Config("cannot calculate publication window end".into()))?;

    Ok(PublicationWindow { start, end })
}

pub fn prior_day_window(primary: PublicationWindow) -> Result<PublicationWindow> {
    let preceding_instant = primary
        .start
        .checked_sub_signed(TimeDelta::milliseconds(1))
        .ok_or_else(|| GeneratorError::Config("cannot calculate prior-day window".into()))?;

    Ok(PublicationWindow {
        start: berlin_day(preceding_instant)?.start,
        end: primary.start,
    })
}

pub fn edition_name(now: DateTime<Utc>) -> EditionName {
    let local = now.with_timezone(&Berlin);
    if (local.hour(), local.minute()) < (EVENING_HOUR, EVENING_MINUTE) {
        EditionName::Morning
    } else {
        EditionName::Evening
    }
}

pub fn display_date(now: DateTime<Utc>) -> String {
    now.with_timezone(&Berlin)
        .format("%A, %-d %B %Y")
        .to_string()
}

pub fn next_scheduled_at(now: DateTime<Utc>) -> Result<DateTime<FixedOffset>> {
    let local = now.with_timezone(&Berlin);
    let date = local.date_naive();
    let morning = local_time(date, MORNING_HOUR, MORNING_MINUTE)?;
    let evening = local_time(date, EVENING_HOUR, EVENING_MINUTE)?;

    let next = if local < morning {
        morning
    } else if local < evening {
        evening
    } else {
        let tomorrow = date
            .succ_opt()
            .ok_or_else(|| GeneratorError::Config("cannot calculate next schedule date".into()))?;
        local_time(tomorrow, MORNING_HOUR, MORNING_MINUTE)?
    };

    Ok(next.fixed_offset())
}

fn local_time(date: NaiveDate, hour: u32, minute: u32) -> Result<DateTime<chrono_tz::Tz>> {
    match Berlin.with_ymd_and_hms(date.year(), date.month(), date.day(), hour, minute, 0) {
        LocalResult::Single(value) => Ok(value),
        LocalResult::Ambiguous(first, _) => Ok(first),
        LocalResult::None => Err(GeneratorError::Config(
            "scheduled local time does not exist".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, TimeZone, Utc};

    use super::{
        PublicationWindow, edition_name, next_scheduled_at, prior_day_window, publication_window,
    };
    use crate::model::EditionName;

    fn utc(value: &str) -> DateTime<Utc> {
        value.parse().ok().unwrap_or(DateTime::<Utc>::MIN_UTC)
    }

    #[test]
    fn schedules_across_summer_time() {
        let before_morning = utc("2026-08-05T03:59:59Z");
        let at_morning = utc("2026-08-05T04:00:00Z");
        assert_eq!(
            next_scheduled_at(before_morning)
                .ok()
                .map(|value| value.to_rfc3339()),
            Some("2026-08-05T06:00:00+02:00".into())
        );
        assert_eq!(
            next_scheduled_at(at_morning)
                .ok()
                .map(|value| value.to_rfc3339()),
            Some("2026-08-05T17:00:00+02:00".into())
        );
    }

    #[test]
    fn schedules_across_winter_time() {
        let now = utc("2026-12-05T18:00:00Z");
        assert_eq!(
            next_scheduled_at(now).ok().map(|value| value.to_rfc3339()),
            Some("2026-12-06T06:00:00+01:00".into())
        );
    }

    #[test]
    fn builds_morning_window_from_valid_previous_evening() {
        for (now, previous, expected_start) in [
            (
                "2026-08-05T04:00:00Z",
                "2026-08-04T15:12:00Z",
                "2026-08-04T15:12:00Z",
            ),
            (
                "2026-12-05T05:00:00Z",
                "2026-12-04T16:12:00Z",
                "2026-12-04T16:12:00Z",
            ),
        ] {
            let window = publication_window(utc(now), Some(utc(previous))).ok();
            assert_eq!(
                window,
                Some(PublicationWindow {
                    start: utc(expected_start),
                    end: utc(now) + chrono::TimeDelta::minutes(30),
                })
            );
        }
    }

    #[test]
    fn morning_window_rejects_invalid_previous_metadata() {
        let now = utc("2026-08-05T04:00:00Z");
        for previous in [
            None,
            Some(utc("2026-08-03T16:00:00Z")),
            Some(utc("2026-08-04T14:59:59Z")),
            Some(now),
        ] {
            assert_eq!(
                publication_window(now, previous)
                    .ok()
                    .map(|value| value.start),
                Some(utc("2026-08-04T15:00:00Z"))
            );
        }
    }

    #[test]
    fn evening_window_starts_at_berlin_midnight() {
        assert_eq!(
            publication_window(utc("2026-08-05T15:00:00Z"), None)
                .ok()
                .map(|value| value.start),
            Some(utc("2026-08-04T22:00:00Z"))
        );
    }

    #[test]
    fn prior_day_windows_are_non_empty_adjacent_and_non_overlapping() {
        for (now, expected_start) in [
            ("2026-08-05T04:00:00Z", "2026-08-03T22:00:00Z"),
            ("2026-08-05T15:00:00Z", "2026-08-03T22:00:00Z"),
            ("2026-12-05T05:00:00Z", "2026-12-03T23:00:00Z"),
            ("2026-12-05T16:00:00Z", "2026-12-03T23:00:00Z"),
        ] {
            let primary = publication_window(utc(now), None);
            let prior = primary.ok().and_then(|value| prior_day_window(value).ok());
            assert_eq!(prior.map(|value| value.start), Some(utc(expected_start)));
            assert!(prior.is_some_and(|value| value.start < value.end));
            assert_eq!(
                prior.map(|value| value.end),
                publication_window(utc(now), None)
                    .ok()
                    .map(|value| value.start)
            );
        }
    }

    #[test]
    fn names_local_editions() {
        let morning = Utc.with_ymd_and_hms(2026, 8, 5, 4, 15, 0).single();
        let before_evening = Utc.with_ymd_and_hms(2026, 8, 5, 14, 59, 0).single();
        let evening = Utc.with_ymd_and_hms(2026, 8, 5, 15, 0, 0).single();
        assert_eq!(morning.map(edition_name), Some(EditionName::Morning));
        assert_eq!(before_evening.map(edition_name), Some(EditionName::Morning));
        assert_eq!(evening.map(edition_name), Some(EditionName::Evening));
    }
}
