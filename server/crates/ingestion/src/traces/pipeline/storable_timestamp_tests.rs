use super::*;
use chrono::TimeZone;

fn span_at(secs: i64) -> NormalizedSpan {
    NormalizedSpan {
        trace_id: "t".to_string(),
        span_id: "s".to_string(),
        timestamp_start: chrono::Utc.timestamp_opt(secs, 0).single().expect("valid"),
        ..Default::default()
    }
}

/// A span dated past what the storage column can hold is dropped, not stored at the epoch.
///
/// The epoch is the single worst destination: it is inside the retention window's *past*, so the row is
/// deleted by the 90-day TTL shortly after the export was answered 200. The old conversion sent every
/// timestamp beyond 2262 there.
#[test]
fn a_span_dated_beyond_the_storable_range_is_rejected() {
    // Year 10000, comfortably past `DateTime64(6)`'s 2299 ceiling and past the nanosecond range that
    // produced the epoch fallback.
    let mut spans = vec![span_at(253_402_300_800), span_at(1_704_067_200)];
    let dropped = drop_unstorable_spans(&mut spans);
    assert_eq!(dropped, 1, "the far-future span must be rejected");
    assert_eq!(spans.len(), 1, "the ordinary span must be kept");
    assert_eq!(
        spans[0].timestamp_start.timestamp(),
        1_704_067_200,
        "the surviving span must be untouched"
    );
}

/// An unstorable *end* time is rejected too: it is stored in its own column, with the same TTL.
#[test]
fn an_unstorable_end_time_rejects_the_span() {
    let mut span = span_at(1_704_067_200);
    span.timestamp_end = chrono::Utc.timestamp_opt(253_402_300_800, 0).single();
    let mut spans = vec![span];
    assert_eq!(drop_unstorable_spans(&mut spans), 1);
    assert!(spans.is_empty());
}

/// The ordinary case costs nothing, and the bounds themselves are inclusive.
#[test]
fn timestamps_inside_the_range_are_kept() {
    // 1900-01-01 and 2299-01-01, the two ends of the documented window.
    let mut spans = vec![span_at(-2_208_988_800), span_at(10_382_659_200)];
    assert_eq!(drop_unstorable_spans(&mut spans), 0);
    assert_eq!(spans.len(), 2);
}
