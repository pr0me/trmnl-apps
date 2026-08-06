use chrono::{DateTime, Datelike, FixedOffset, LocalResult, NaiveDate, TimeZone, Timelike, Utc};
use chrono_tz::Europe::Berlin;

use crate::{
    error::{GeneratorError, Result},
    model::EditionName,
};

const MORNING_HOUR: u32 = 6;
const MORNING_MINUTE: u32 = 30;
const EVENING_HOUR: u32 = 17;
const EVENING_MINUTE: u32 = 0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BerlinDay {
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
    use chrono::{TimeZone, Utc};

    use super::{edition_name, next_scheduled_at};
    use crate::model::EditionName;

    #[test]
    fn schedules_across_summer_time() {
        let now = Utc.with_ymd_and_hms(2026, 8, 5, 5, 0, 0).single();
        let next = now.and_then(|value| next_scheduled_at(value).ok());
        assert_eq!(
            next.map(|value| value.to_rfc3339()),
            Some("2026-08-05T17:00:00+02:00".into())
        );
    }

    #[test]
    fn schedules_across_winter_time() {
        let now = Utc.with_ymd_and_hms(2026, 12, 5, 18, 0, 0).single();
        let next = now.and_then(|value| next_scheduled_at(value).ok());
        assert_eq!(
            next.map(|value| value.to_rfc3339()),
            Some("2026-12-06T06:30:00+01:00".into())
        );
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
