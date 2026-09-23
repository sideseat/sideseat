//! Logical byte accounting shared by analytical signals.

use serde::Serialize;
use sideseat_ports::types::{NormalizedLog, NormalizedMetric, NormalizedSpan};

/// Stable attributable size of one logical row.
///
/// Receipt time, legal hold and the accounting value itself are system-managed and deliberately excluded.
/// Otherwise the same producer payload has a different charge on every retry, and patching a hold changes
/// usage without changing any producer-owned data.
fn serialized_bytes(value: &impl Serialize) -> u64 {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len() as u64)
        .unwrap_or(u64::MAX)
}

pub fn span_logical_bytes(value: &NormalizedSpan) -> u64 {
    let mut value = value.clone();
    value.ingested_at = None;
    value.hold_until = None;
    value.logical_bytes = 0;
    serialized_bytes(&value)
}

pub fn metric_logical_bytes(value: &NormalizedMetric) -> u64 {
    let mut value = value.clone();
    value.ingested_at = None;
    value.hold_until = None;
    value.logical_bytes = 0;
    serialized_bytes(&value)
}

pub fn log_logical_bytes(value: &NormalizedLog) -> u64 {
    let mut value = value.clone();
    value.ingested_at = None;
    value.hold_until = None;
    value.logical_bytes = 0;
    serialized_bytes(&value)
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
        let expected = log_logical_bytes(&log);
        log.ingested_at = Some(DateTime::UNIX_EPOCH + TimeDelta::days(10));
        log.hold_until = Some(DateTime::UNIX_EPOCH + TimeDelta::days(20));
        log.logical_bytes = 999_999;
        assert_eq!(log_logical_bytes(&log), expected);
    }
}
