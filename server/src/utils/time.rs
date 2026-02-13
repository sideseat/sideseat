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

/// Convert microseconds since Unix epoch to DateTime<Utc>
pub fn micros_to_datetime(micros: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_micros(micros).unwrap_or_else(|| {
        tracing::warn!(micros, "Invalid timestamp, using epoch");
        DateTime::UNIX_EPOCH
    })
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
}
