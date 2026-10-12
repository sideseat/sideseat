fn key(epoch: i64, n: i64) -> i64 {
    sideseat_ports::types::raw_version(epoch, n).expect("in domain")
}

fn versioned(raw_id: &str, version: i64, body: &[u8]) -> sideseat_ports::types::RawRecordRow {
    sideseat_ports::types::RawRecordRow {
        project_id: sideseat_ports::types::ProjectId::from("project"),
        raw_id: raw_id.to_string(),
        signal: sideseat_ports::types::StagedSignal::Traces,
        received_at: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
        origin: sideseat_ports::types::RawOrigin::Received,
        version,
        signal_until: Utc.timestamp_opt(1_700_000_100, 0).unwrap(),
        hold_until: None,
        trace_ids: vec!["t1".to_string()],
        record: body.to_vec(),
    }
}

fn stored_versions(conn: &duckdb::Connection, raw_id: &str) -> Vec<i64> {
    let mut statement = conn
        .prepare("SELECT version_key FROM otel_raw WHERE raw_id = ? ORDER BY version_key")
        .unwrap();
    statement
        .query_map([raw_id], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// The collection purge is bounded by its decision's epoch: a version keyed under a later epoch - written after
/// the decision - survives however late the purge runs, and the record's trace index goes only with its last
/// version (`RawRecordOwnership.tla`, `RPurge`).
#[tokio::test]
async fn a_collection_purge_never_reaches_a_version_of_a_later_epoch() {
    let (_dir, service) = create_test_service().await;
    let conn = service.conn();
    let project = sideseat_ports::types::ProjectId::from("project");
    crate::repositories::raw::append_versions(
        &conn,
        &[
            versioned("r", key(0, 1), b"first"),
            versioned("r", key(0, 3), b"repair"),
            versioned("r", key(2, 1), b"after-the-decision"),
        ],
    )
    .unwrap();
    crate::repositories::raw::purge_versions(&conn, &project, "r", 1).unwrap();
    assert_eq!(stored_versions(&conn, "r"), vec![key(2, 1)]);
    let indexed: i64 = conn
        .query_row(
            "SELECT count(*) FROM otel_raw_traces WHERE raw_id = 'r'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        indexed, 1,
        "a record with a version left keeps its trace index"
    );

    crate::repositories::raw::purge_versions(&conn, &project, "r", 2).unwrap();
    assert!(stored_versions(&conn, "r").is_empty());
    let indexed: i64 = conn
        .query_row(
            "SELECT count(*) FROM otel_raw_traces WHERE raw_id = 'r'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(indexed, 0, "the last version takes the trace index with it");
}

/// The supersede purge deletes strictly below the latest it read: the latest, and anything written since,
/// stays.
#[tokio::test]
async fn a_supersede_purge_keeps_the_latest_it_read_and_everything_after() {
    let (_dir, service) = create_test_service().await;
    let conn = service.conn();
    let project = sideseat_ports::types::ProjectId::from("project");
    crate::repositories::raw::append_versions(
        &conn,
        &[
            versioned("r", key(0, 1), b"with-deleted-content"),
            versioned("r", key(0, 2), b"rewrite"),
            versioned("r", key(1, 1), b"written-since"),
        ],
    )
    .unwrap();
    crate::repositories::raw::delete_superseded(&conn, &project, "r", key(0, 2)).unwrap();
    assert_eq!(stored_versions(&conn, "r"), vec![key(0, 2), key(1, 1)]);
}

/// A pass reads only entries enqueued before the landing horizon; at a horizon of zero, every entry is due.
#[tokio::test]
async fn the_pending_read_waits_out_the_landing_horizon() {
    let (_dir, service) = create_test_service().await;
    let conn = service.conn();
    let project = sideseat_ports::types::ProjectId::from("project");
    crate::repositories::raw::enqueue(&conn, &project, &["r".to_string()]).unwrap();
    let past = Utc::now() - chrono::Duration::minutes(1);
    assert!(
        crate::repositories::raw::pending(&conn, 10, Some(past))
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        crate::repositories::raw::pending(&conn, 10, None)
            .unwrap()
            .len(),
        1
    );
}

/// The span identities naming a record, for the collection's removal of rows no trace owns.
#[tokio::test]
async fn the_spans_naming_a_record_are_found_by_its_id() {
    let (_dir, service) = create_test_service().await;
    let conn = service.conn();
    let project = sideseat_ports::types::ProjectId::from("project");
    let span = |trace: &str, span: &str, raw: &str| NormalizedSpan {
        project_id: Some("project".to_string()),
        trace_id: trace.to_string(),
        span_id: span.to_string(),
        timestamp_start: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
        raw_id: Some(raw.to_string()),
        ..Default::default()
    };
    let mut record = versioned("r", key(0, 1), b"r");
    record.trace_ids = vec!["t1".to_string(), "t2".to_string()];
    let mut other = versioned("other", key(0, 1), b"other");
    other.trace_ids = vec!["t3".to_string(), "t1".to_string()];
    crate::repositories::raw::append_versions(&conn, &[record, other]).unwrap();
    crate::repositories::span::insert_batch(
        &conn,
        &[
            span("t1", "s1", "r"),
            span("t2", "s2", "r"),
            span("t3", "s3", "other"),
            // Another record's row in a trace this record also holds is not this record's.
            span("t1", "s9", "other"),
        ],
    )
    .unwrap();
    assert_eq!(
        crate::repositories::raw::spans_naming(&conn, &project, "r").unwrap(),
        vec![
            ("t1".to_string(), "s1".to_string()),
            ("t2".to_string(), "s2".to_string())
        ]
    );
}
