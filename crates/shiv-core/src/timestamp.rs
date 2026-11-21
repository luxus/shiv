//! UTC timestamps as text, from the Unix clock alone: the civil-from-days
//! algorithm turns days into calendar dates without a date library, and the
//! fixed-width renderings keep text order the same as instant order.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A UTC instant broken into calendar fields.
struct UtcMoment {
    year: u64,
    month: u64,
    day: u64,
    hour: u64,
    minute: u64,
    second: u64,
    nanosecond: u32,
}

impl UtcMoment {
    /// `2026-11-19T12:25:01Z`.
    fn iso8601(&self) -> String {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }

    /// `2026-11-20T12:34:56.123456789Z`.
    fn rfc3339(&self) -> String {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:09}Z",
            self.year, self.month, self.day, self.hour, self.minute, self.second, self.nanosecond
        )
    }

    /// `20261120T123456123456789Z`.
    fn compact(&self) -> String {
        format!(
            "{:04}{:02}{:02}T{:02}{:02}{:02}{:09}Z",
            self.year, self.month, self.day, self.hour, self.minute, self.second, self.nanosecond
        )
    }
}

/// The current UTC time as RFC-3339 text with nanosecond precision
/// (`2026-11-20T12:34:56.123456789Z`): the rendering is fixed-width, so
/// stored stamps compare and sort as instants without being parsed.
#[must_use]
pub fn rfc3339_utc_now() -> String {
    utc_now().rfc3339()
}

/// The current UTC time as a compact stamp (`20261120T123456123456789Z`)
/// for folder names that sort by recency.
#[must_use]
pub fn compact_utc_now() -> String {
    utc_now().compact()
}

/// `2026-11-19T12:25:01Z` from Unix seconds: the interchange manifest's
/// `createdAt`.
pub(crate) fn iso8601_utc(seconds: u64) -> String {
    utc_fields(seconds, 0).iso8601()
}

/// The current instant; a clock that reads before 1970 reports the epoch.
fn utc_now() -> UtcMoment {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO);
    utc_fields(elapsed.as_secs(), elapsed.subsec_nanos())
}

/// The instant `seconds` and `nanoseconds` after 1970-01-01T00:00:00Z.
fn utc_fields(seconds: u64, nanoseconds: u32) -> UtcMoment {
    let (year, month, day) = civil_from_days(seconds / 86_400);
    let rest = seconds % 86_400;
    UtcMoment {
        year,
        month,
        day,
        hour: rest / 3_600,
        minute: (rest % 3_600) / 60,
        second: rest % 60,
        nanosecond: nanoseconds,
    }
}

/// `(year, month, day)` from days since 1970-01-01 (Howard Hinnant's
/// `civil_from_days`, with the pre-1970 branch removed: the Unix clock
/// counts up from the epoch only).
fn civil_from_days(days_since_epoch: u64) -> (u64, u64, u64) {
    let z = days_since_epoch + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 86_400;
    const Y2K: u64 = 946_684_800;

    #[test]
    fn epoch_renders_as_1970() {
        assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00Z");
        let epoch = utc_fields(0, 0);
        assert_eq!(epoch.rfc3339(), "1970-01-01T00:00:00.000000000Z");
        assert_eq!(epoch.compact(), "19700101T000000000000000Z");
    }

    #[test]
    fn leap_days_render() {
        assert_eq!(iso8601_utc(Y2K + 59 * DAY), "2000-02-29T00:00:00Z");
        assert_eq!(iso8601_utc(Y2K + 60 * DAY), "2000-03-01T00:00:00Z");
        let century = Y2K + (100 * 365 + 25) * DAY;
        assert_eq!(iso8601_utc(century + 58 * DAY), "2100-02-28T00:00:00Z");
        assert_eq!(iso8601_utc(century + 59 * DAY), "2100-03-01T00:00:00Z");
    }

    #[test]
    fn compact_stamps_strip_the_rfc3339_separators() {
        for index in 0..50u64 {
            let moment = utc_fields(
                Y2K + index * 3_533 * DAY,
                ((index * 999_999_937) % 1_000_000_000) as u32,
            );
            assert_eq!(
                moment.compact(),
                moment.rfc3339().replace(['-', ':', '.'], "")
            );
        }
    }

    #[test]
    fn stamps_sort_as_instants_sort() {
        let stamps: Vec<String> = (0..400u64)
            .map(|index| {
                let seconds = Y2K + index * 3_000 * DAY + (index * 86_399) % 86_400;
                utc_fields(seconds, ((index * 999_999_937) % 1_000_000_000) as u32).rfc3339()
            })
            .collect();
        let mut sorted = stamps.clone();
        sorted.sort();
        assert_eq!(stamps, sorted);
        assert!(stamps.iter().all(|stamp| stamp.len() == 30));
    }
}
