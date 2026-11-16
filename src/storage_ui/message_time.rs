use chrono::{DateTime, Datelike, FixedOffset, Local, NaiveDate};

const CALENDAR_LABEL: &str = "%-d %B %Y";

pub(super) fn local_separator_days() -> (String, String) {
    separator_days(Local::now().date_naive())
}

fn separator_days(today: NaiveDate) -> (String, String) {
    (
        calendar_label(today),
        today.pred_opt().map(calendar_label).unwrap_or_default(),
    )
}

fn calendar_label(day: NaiveDate) -> String {
    day.format(CALENDAR_LABEL).to_string()
}

pub(super) struct MessageTime(DateTime<FixedOffset>);

impl MessageTime {
    pub(super) fn local(timestamp_ms: i64) -> Option<Self> {
        let utc = DateTime::from_timestamp_millis(timestamp_ms)?;
        // Keep corrupt or out-of-range values away from platform timezone rules.
        if !(1..=9999).contains(&utc.year()) {
            return None;
        }
        Some(Self(utc.with_timezone(&Local).fixed_offset()))
    }

    pub(super) fn day(&self) -> NaiveDate {
        self.0.date_naive()
    }

    pub(super) fn calendar_label(&self) -> String {
        calendar_label(self.day())
    }

    pub(super) fn clock_label(&self) -> String {
        self.0.format("%H:%M").to_string()
    }

    pub(super) fn exact_label(&self) -> String {
        format!(
            "{}, {}",
            self.calendar_label(),
            self.0.format("%H:%M:%S (UTC%:z)")
        )
    }

    pub(super) fn activity_label(&self, today: NaiveDate) -> String {
        if self.day() == today {
            self.clock_label()
        } else if self.day().year() == today.year() {
            self.0.format("%-d %b").to_string()
        } else {
            self.0.format("%-d %b %Y").to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_midnight_same_minute_and_new_year_use_consistent_labels() {
        let before = MessageTime("2026-12-31T23:59:40+08:00".parse().unwrap());
        let after = MessageTime("2027-01-01T00:00:02+08:00".parse().unwrap());
        let later = MessageTime("2027-01-01T00:00:55+08:00".parse().unwrap());
        assert_ne!(before.day(), after.day());
        assert_eq!(before.calendar_label(), "31 December 2026");
        assert_eq!(before.activity_label(after.day()), "31 Dec 2026");
        assert_eq!(after.calendar_label(), "1 January 2027");
        assert_eq!(
            separator_days(after.day()),
            (after.calendar_label(), before.calendar_label())
        );
        assert_eq!(after.activity_label(after.day()), "00:00");
        assert_eq!(after.clock_label(), later.clock_label());
        assert_eq!(after.exact_label(), "1 January 2027, 00:00:02 (UTC+08:00)");
        assert_ne!(after.exact_label(), later.exact_label());
        assert!(MessageTime::local(i64::MAX).is_none());
        assert!(MessageTime::local(i64::MIN).is_none());
    }

    #[test]
    fn repeated_daylight_saving_hour_has_distinct_offsets() {
        let daylight = MessageTime("2026-11-01T01:30:20-04:00".parse().unwrap());
        let standard = MessageTime("2026-11-01T01:30:20-05:00".parse().unwrap());
        assert_eq!(daylight.day(), standard.day());
        assert_eq!(daylight.clock_label(), standard.clock_label());
        assert!(daylight.exact_label().ends_with("(UTC-04:00)"));
        assert!(standard.exact_label().ends_with("(UTC-05:00)"));
    }
}
