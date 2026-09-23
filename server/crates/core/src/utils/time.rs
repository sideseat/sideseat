//! Time utility functions

use chrono::{DateTime, TimeZone, Utc};

/// Convert nanoseconds since Unix epoch to DateTime<Utc>
pub fn nanos_to_datetime(nanos: u64) -> DateTime<Utc> {
    let secs = (nanos / 1_000_000_000) as i64;
    let nsecs = (nanos % 1_000_000_000) as u32;
    Utc.timestamp_opt(secs, nsecs).single().unwrap_or_else(|| {
        tracing::warn!(nanos, "Invalid timestamp, using epoch");
        DateTime::UNIX_EPOCH
    })
}

/// Convert nanoseconds since Unix epoch to ISO 8601 string (microsecond precision)
pub fn nanos_to_iso(nanos: u64) -> String {
    nanos_to_datetime(nanos).to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}

/// Convert microseconds since Unix epoch to DateTime<Utc>
pub fn micros_to_datetime(micros: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_micros(micros).unwrap_or_else(|| {
        tracing::warn!(micros, "Invalid timestamp, using epoch");
        DateTime::UNIX_EPOCH
    })
}

/// The window of instants every analytics backend can store, as a half-open range of years.
///
/// ClickHouse's `DateTime64(6)` - the type both span and metric timestamps use - covers 1900-01-01 to
/// 2299-12-31. DuckDB's `TIMESTAMP` covers far more, and `chrono` more still, which is exactly the problem:
/// a timestamp only one of them can hold makes the two backends disagree about the same export, and the
/// disagreement is silent.
///
/// It is not merely a rounding difference either. The ClickHouse row conversion reached the column through
/// `timestamp_nanos_opt`, whose range is the *nanosecond* one (1677-2262) and whose `None` fell back to the
/// **epoch** - so a datapoint dated past 2262 was stored at 1970, where the schema's 90-day TTL deleted it,
/// after the export had been answered 200. Storing at the epoch is the single worst available answer,
/// because it is the one value the retention policy destroys.
///
/// So an instant outside this window is refused at ingestion and reported as rejected, rather than being
/// quietly moved somewhere it can be represented. An exporter with a broken clock learns; a
/// silently-relocated record does not.
pub const STORABLE_YEAR_RANGE: std::ops::RangeInclusive<i32> = 1900..=2299;

/// Whether an instant can be stored, and read back unchanged, by every analytics backend.
///
/// See [`STORABLE_YEAR_RANGE`] for why the bound exists and why the answer is a refusal rather than a
/// substitution.
pub fn is_storable(dt: DateTime<Utc>) -> bool {
    use chrono::Datelike;
    STORABLE_YEAR_RANGE.contains(&dt.year())
}

/// Clamp an instant into [`STORABLE_YEAR_RANGE`], for a storage row conversion that must not fail.
///
/// Ingestion already refuses an unstorable instant, so this is the backstop for a value that reached a
/// storage-row conversion anyway - a new write path that forgot the check, or a `time`-representable year
/// (up to 9999) that `DateTime64(6)` cannot hold. It must never land on the **epoch**: that is inside the
/// retention window's past, where the 90-day TTL deletes the row after the export was answered 200. So it
/// clamps to the nearest representable bound and returns whether it had to, for the caller to shout about.
pub fn clamp_to_storable(dt: DateTime<Utc>) -> (DateTime<Utc>, bool) {
    use chrono::Datelike;
    let year = dt.year();
    if year < *STORABLE_YEAR_RANGE.start() {
        // 1900-01-01T00:00:00Z, the earliest DateTime64(6) instant.
        (
            Utc.with_ymd_and_hms(*STORABLE_YEAR_RANGE.start(), 1, 1, 0, 0, 0)
                .single()
                .unwrap_or(DateTime::UNIX_EPOCH),
            true,
        )
    } else if year > *STORABLE_YEAR_RANGE.end() {
        // 2299-12-31T23:59:59Z, the latest whole-second DateTime64(6) instant.
        (
            Utc.with_ymd_and_hms(*STORABLE_YEAR_RANGE.end(), 12, 31, 23, 59, 59)
                .single()
                .unwrap_or(DateTime::UNIX_EPOCH),
            true,
        )
    } else {
        (dt, false)
    }
}

/// Parse ISO 8601 / RFC 3339 timestamp string to DateTime<Utc>
pub fn parse_iso_timestamp(ts: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(ts)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| {
            tracing::warn!(ts, "Invalid ISO timestamp, using epoch");
            DateTime::UNIX_EPOCH
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, Timelike};

    #[test]
    fn test_nanos_to_datetime_epoch() {
        let dt = nanos_to_datetime(0);
        assert_eq!(dt.year(), 1970);
        assert_eq!(dt.month(), 1);
        assert_eq!(dt.day(), 1);
    }

    #[test]
    fn test_nanos_to_datetime_known_value() {
        // 2024-01-01 00:00:00 UTC = 1704067200 seconds
        let nanos = 1704067200_u64 * 1_000_000_000;
        let dt = nanos_to_datetime(nanos);
        assert_eq!(dt.year(), 2024);
        assert_eq!(dt.month(), 1);
        assert_eq!(dt.day(), 1);
    }

    #[test]
    fn test_nanos_to_datetime_with_subsecond() {
        // 1 second + 500ms = 1.5 seconds in nanos
        let nanos = 1_500_000_000;
        let dt = nanos_to_datetime(nanos);
        assert_eq!(dt.timestamp(), 1);
        assert_eq!(dt.timestamp_subsec_nanos(), 500_000_000);
    }

    #[test]
    fn test_micros_to_datetime_epoch() {
        let dt = micros_to_datetime(0);
        assert_eq!(dt.year(), 1970);
        assert_eq!(dt.month(), 1);
        assert_eq!(dt.day(), 1);
    }

    #[test]
    fn test_micros_to_datetime_known_value() {
        // 2024-01-01 00:00:00 UTC = 1704067200 seconds = 1704067200000000 micros
        let micros = 1704067200_i64 * 1_000_000;
        let dt = micros_to_datetime(micros);
        assert_eq!(dt.year(), 2024);
        assert_eq!(dt.month(), 1);
        assert_eq!(dt.day(), 1);
    }

    #[test]
    fn test_micros_to_datetime_with_subsecond() {
        // 1 second + 500ms = 1.5 seconds in micros
        let micros = 1_500_000;
        let dt = micros_to_datetime(micros);
        assert_eq!(dt.timestamp(), 1);
        assert_eq!(dt.timestamp_subsec_micros(), 500_000);
    }

    #[test]
    fn test_parse_iso_timestamp_valid() {
        let dt = parse_iso_timestamp("2024-01-15T10:30:00Z");
        assert_eq!(dt.year(), 2024);
        assert_eq!(dt.month(), 1);
        assert_eq!(dt.day(), 15);
        assert_eq!(dt.hour(), 10);
        assert_eq!(dt.minute(), 30);
    }

    #[test]
    fn test_parse_iso_timestamp_with_offset() {
        let dt = parse_iso_timestamp("2024-01-15T10:30:00+05:00");
        // Should be converted to UTC: 10:30 + 5:00 offset = 05:30 UTC
        assert_eq!(dt.hour(), 5);
        assert_eq!(dt.minute(), 30);
    }

    #[test]
    fn test_parse_iso_timestamp_invalid() {
        let dt = parse_iso_timestamp("not-a-timestamp");
        assert_eq!(dt, DateTime::UNIX_EPOCH);
    }

    #[test]
    fn clamp_to_storable_leaves_an_in_range_instant_alone() {
        let dt = Utc.with_ymd_and_hms(2024, 6, 1, 12, 0, 0).unwrap();
        let (out, clamped) = clamp_to_storable(dt);
        assert_eq!(out, dt);
        assert!(!clamped);
    }

    #[test]
    fn clamp_to_storable_never_lands_on_the_epoch() {
        // Year 3000: representable by `time` (so the old `from_unix_timestamp` succeeded and passed it
        // through unclamped) but not by ClickHouse's DateTime64(6). It must clamp to 2299, never the epoch
        // the retention TTL would delete.
        let far_future = Utc.with_ymd_and_hms(3000, 1, 1, 0, 0, 0).unwrap();
        let (out, clamped) = clamp_to_storable(far_future);
        assert!(clamped);
        assert_eq!(out.year(), 2299);
        assert_ne!(out, DateTime::UNIX_EPOCH);
        assert!(is_storable(out));

        // Far past clamps to the low bound, also not the epoch.
        let far_past = Utc.with_ymd_and_hms(1800, 1, 1, 0, 0, 0).unwrap();
        let (out, clamped) = clamp_to_storable(far_past);
        assert!(clamped);
        assert_eq!(out.year(), 1900);
        assert!(is_storable(out));
    }

    #[test]
    fn clamp_to_storable_boundaries_are_inclusive() {
        for year in [*STORABLE_YEAR_RANGE.start(), *STORABLE_YEAR_RANGE.end()] {
            let dt = Utc.with_ymd_and_hms(year, 6, 1, 0, 0, 0).unwrap();
            let (out, clamped) = clamp_to_storable(dt);
            assert_eq!(out, dt, "a boundary year must be stored as itself");
            assert!(!clamped);
        }
    }

    // ================================================================
    // Regression: nanos_to_iso (moved from persist.rs)
    // ================================================================

    #[test]
    fn test_nanos_to_iso_epoch() {
        assert_eq!(nanos_to_iso(0), "1970-01-01T00:00:00.000000Z");
    }

    #[test]
    fn test_nanos_to_iso_known_timestamp() {
        // 2024-01-01 00:00:00 UTC
        let nanos = 1704067200_u64 * 1_000_000_000;
        assert_eq!(nanos_to_iso(nanos), "2024-01-01T00:00:00.000000Z");
    }

    #[test]
    fn test_nanos_to_iso_microsecond_precision() {
        // 1 second + 123456 microseconds
        let nanos = 1_000_000_000 + 123_456_000;
        let iso = nanos_to_iso(nanos);
        assert_eq!(iso, "1970-01-01T00:00:01.123456Z");
    }

    #[test]
    fn test_nanos_to_iso_sub_microsecond_truncated() {
        // Nanoseconds below microsecond precision should be truncated
        let nanos = 1_000_000_000 + 123_456_789;
        let iso = nanos_to_iso(nanos);
        // chrono's Micros precision rounds/truncates sub-microsecond
        assert!(
            iso.starts_with("1970-01-01T00:00:01.123456"),
            "Sub-microsecond nanos should be truncated, got: {}",
            iso
        );
    }

    #[test]
    fn test_nanos_to_iso_uses_utc_suffix() {
        let iso = nanos_to_iso(0);
        assert!(
            iso.ends_with('Z'),
            "Should use Z suffix for UTC, got: {}",
            iso
        );
    }
}
