// ============================================================================
// Batch redelivery check: which records match their span's winning revision
// ============================================================================

fn revision(span_id: &str, digest: &str, ingested_second: i64) -> NormalizedSpan {
    NormalizedSpan {
        project_id: Some("project".to_string()),
        trace_id: "trace".to_string(),
        span_id: span_id.to_string(),
        span_name: span_id.to_string(),
        timestamp_start: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
        ingested_at: Some(
            Utc.timestamp_opt(1_700_000_000 + ingested_second, 0)
                .unwrap(),
        ),
        content_digest: digest.to_string(),
        ..Default::default()
    }
}

/// Only a record whose digest equals its span's *winning* revision matches: an older revision's digest, a
/// span the store has never seen, and another project's span with the same ids all do not.
#[tokio::test]
async fn matching_content_is_judged_against_each_spans_winning_revision() {
    let (_dir, service) = create_test_service().await;
    {
        let conn = service.conn();
        insert_batch(
            &conn,
            &[
                revision("a", "old", 1),
                revision("a", "new", 2),
                revision("b", "only", 1),
                NormalizedSpan {
                    project_id: Some("other".to_string()),
                    ..revision("c", "elsewhere", 1)
                },
            ],
        )
        .unwrap();
    }
    let record =
        |span: &str, digest: &str| ("trace".to_string(), span.to_string(), digest.to_string());
    let records = vec![
        record("a", "old"),
        record("a", "new"),
        record("b", "only"),
        record("c", "elsewhere"),
        record("d", "never"),
    ];
    let matching = spans_with_matching_content(&service.conn(), "project", &records).unwrap();
    assert_eq!(
        matching,
        HashSet::from([record("a", "new"), record("b", "only")])
    );
    assert!(
        spans_match_content(
            &service.conn(),
            "project",
            &[record("a", "new"), record("b", "only")]
        )
        .unwrap()
    );
    assert!(!spans_match_content(&service.conn(), "project", &[record("a", "old")]).unwrap());
}
