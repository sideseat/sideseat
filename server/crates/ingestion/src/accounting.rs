//! Logical byte accounting shared by analytical signals.

use sideseat_ports::types::{NormalizedLog, NormalizedMetric, NormalizedSpan};

/// Stable attributable size of one logical row: the length of its JSON serialisation.
///
/// Receipt time, legal hold and the accounting value itself are system-managed and deliberately excluded.
/// Otherwise the same producer payload has a different charge on every retry, and patching a hold changes
/// usage without changing any producer-owned data.
///
/// Measured, never materialised: the row is serialised into a counting sink with the excluded fields
/// temporarily reset, and they are restored afterwards. Cloning the row and building the JSON to read its
/// length cost about a sixth of the whole ingest CPU phase, for a number.
macro_rules! logical_bytes {
    ($value:expr) => {{
        let value = $value;
        let ingested_at = value.ingested_at.take();
        let hold_until = value.hold_until.take();
        let logical_bytes = std::mem::take(&mut value.logical_bytes);
        let mut sink = CountingSink(0);
        let size = match serde_json::to_writer(&mut sink, &*value) {
            Ok(()) => sink.0,
            Err(_) => u64::MAX,
        };
        value.ingested_at = ingested_at;
        value.hold_until = hold_until;
        value.logical_bytes = logical_bytes;
        size
    }};
}

/// An `io::Write` that only counts.
struct CountingSink(u64);

impl std::io::Write for CountingSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0 += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub fn span_logical_bytes(value: &mut NormalizedSpan) -> u64 {
    logical_bytes!(value)
}

pub fn metric_logical_bytes(value: &mut NormalizedMetric) -> u64 {
    logical_bytes!(value)
}

pub fn log_logical_bytes(value: &mut NormalizedLog) -> u64 {
    logical_bytes!(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, TimeDelta};

    #[test]
    fn system_managed_fields_do_not_change_logical_size() {
        let mut log = NormalizedLog {
            project_id: Some("project".to_string()),
            body_text: Some("producer payload".to_string()),
            ..Default::default()
        };
        let expected = log_logical_bytes(&mut log);
        log.ingested_at = Some(DateTime::UNIX_EPOCH + TimeDelta::days(10));
        log.hold_until = Some(DateTime::UNIX_EPOCH + TimeDelta::days(20));
        log.logical_bytes = 999_999;
        assert_eq!(log_logical_bytes(&mut log), expected);
        // Measuring restores what it set aside.
        assert_eq!(log.logical_bytes, 999_999);
        assert_eq!(
            log.hold_until,
            Some(DateTime::UNIX_EPOCH + TimeDelta::days(20))
        );
    }

    /// The figure is the length of the JSON the row serialises to without its system-managed fields - the
    /// definition stored usage was computed with, so a redelivery charges the same before and after.
    #[test]
    fn logical_size_is_the_serialised_length_without_system_fields() {
        let mut span = NormalizedSpan {
            project_id: Some("project".to_string()),
            trace_id: "t".repeat(32),
            span_id: "s".repeat(16),
            ingested_at: Some(DateTime::UNIX_EPOCH + TimeDelta::days(3)),
            logical_bytes: 77,
            ..Default::default()
        };
        let mut reference = span.clone();
        reference.ingested_at = None;
        reference.hold_until = None;
        reference.logical_bytes = 0;
        let expected = serde_json::to_vec(&reference).unwrap().len() as u64;
        assert_eq!(span_logical_bytes(&mut span), expected);
        assert_eq!(span.logical_bytes, 77);
    }
}
