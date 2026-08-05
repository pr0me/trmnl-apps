use chrono::{DateTime, Datelike, FixedOffset, LocalResult, NaiveDate, TimeZone, Timelike, Utc};
use chrono_tz::Europe::Berlin;

use crate::{
    error::{GeneratorError, Result},
    model::EditionName,
};

const MORNING_HOUR: u32 = 6;
const EVENING_HOUR: u32 = 18;
const EDITION_MINUTE: u32 = 15;

pub fn edition_name(now: DateTime<Utc>) -> EditionName {
    let local = now.with_timezone(&Berlin);
    if local.hour() < 12 {
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
    let morning = local_time(date, MORNING_HOUR)?;
    let evening = local_time(date, EVENING_HOUR)?;

    let next = if local < morning {
        morning
    } else if local < evening {
        evening
    } else {
        let tomorrow = date
            .succ_opt()
            .ok_or_else(|| GeneratorError::Config("cannot calculate next schedule date".into()))?;
        local_time(tomorrow, MORNING_HOUR)?
    };

    Ok(next.fixed_offset())
}

fn local_time(date: NaiveDate, hour: u32) -> Result<DateTime<chrono_tz::Tz>> {
    match Berlin.with_ymd_and_hms(
        date.year(),
        date.month(),
        date.day(),
        hour,
        EDITION_MINUTE,
        0,
    ) {
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
            Some("2026-08-05T18:15:00+02:00".into())
        );
    }

    #[test]
    fn schedules_across_winter_time() {
        let now = Utc.with_ymd_and_hms(2026, 12, 5, 18, 0, 0).single();
        let next = now.and_then(|value| next_scheduled_at(value).ok());
        assert_eq!(
            next.map(|value| value.to_rfc3339()),
            Some("2026-12-06T06:15:00+01:00".into())
        );
    }

    #[test]
    fn names_local_editions() {
        let morning = Utc.with_ymd_and_hms(2026, 8, 5, 4, 15, 0).single();
        let evening = Utc.with_ymd_and_hms(2026, 8, 5, 16, 15, 0).single();
        assert_eq!(morning.map(edition_name), Some(EditionName::Morning));
        assert_eq!(evening.map(edition_name), Some(EditionName::Evening));
    }
}
