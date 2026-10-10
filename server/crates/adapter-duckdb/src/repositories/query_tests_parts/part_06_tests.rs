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

fn raw_version(
    raw_id: &str,
    version: i64,
    traces: &[&str],
    body: &[u8],
) -> sideseat_ports::types::RawRecordRow {
    sideseat_ports::types::RawRecordRow {
        project_id: sideseat_ports::types::ProjectId::from("project"),
        raw_id: raw_id.to_string(),
        signal: sideseat_ports::types::StagedSignal::Traces,
        received_at: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
        origin: sideseat_ports::types::RawOrigin::Received,
        version,
        signal_until: Utc.timestamp_opt(1_700_000_100, 0).unwrap(),
        hold_until: None,
        trace_ids: traces.iter().map(|t| t.to_string()).collect(),
        record: body.to_vec(),
    }
}

fn read_back(mut row: sideseat_ports::types::RawRecordRow) -> sideseat_ports::types::RawRecordRow {
    row.trace_ids.clear();
    row
}

/// A raw record presented twice - a redelivery, a redrive, a re-derivation - is stored once, and so is its
/// trace index.
#[tokio::test]
async fn a_raw_record_is_stored_once_however_often_it_is_presented() {
    let (_dir, service) = create_test_service().await;
    let record = raw_version("raw", 1, &["t1", "t2"], b"SSR1\0\0body");
    {
        let conn = service.conn();
        crate::repositories::raw::insert(&conn, &[record.clone(), record.clone()]).unwrap();
        crate::repositories::raw::insert(&conn, std::slice::from_ref(&record)).unwrap();
    }
    let count = |table: &str| -> i64 {
        service
            .conn()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    };
    assert_eq!(count("otel_raw"), 1);
    assert_eq!(count("otel_raw_traces"), 2);
    let stored =
        crate::repositories::raw::get(&service.conn(), &record.project_id, &["raw".to_string()])
            .unwrap();
    assert_eq!(stored, vec![read_back(record)]);
}

/// The latest version is the highest, then the last written - the answer `ReplacingMergeTree(version)` gives.
#[tokio::test]
async fn the_latest_raw_version_wins_by_version_then_by_write() {
    let (_dir, service) = create_test_service().await;
    let conn = service.conn();
    let project = sideseat_ports::types::ProjectId::from("project");
    crate::repositories::raw::append_versions(
        &conn,
        &[
            raw_version("raw", 7, &["t"], b"seven"),
            raw_version("raw", 9, &["t"], b"nine-first"),
            raw_version("raw", 9, &["t"], b"nine-last"),
            raw_version("raw", 8, &["t"], b"eight"),
        ],
    )
    .unwrap();
    let latest = crate::repositories::raw::get(&conn, &project, &["raw".to_string()]).unwrap();
    assert_eq!(latest.len(), 1);
    assert_eq!(latest[0].record, b"nine-last");
    let page = crate::repositories::raw::page(&conn, &project, None, 10).unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].record, b"nine-last");
}

/// A trace deletion enqueues every record holding the trace - found through the trace index, so a record
/// whose rows are already gone is still reached - and the queue is cleared by exactly the entries read.
#[tokio::test]
async fn a_trace_deletion_enqueues_its_records_even_without_rows() {
    let (_dir, service) = create_test_service().await;
    let conn = service.conn();
    let project = sideseat_ports::types::ProjectId::from("project");
    crate::repositories::raw::insert(
        &conn,
        &[
            raw_version("with-rows", 1, &["t1"], b"a"),
            raw_version("rows-expired", 1, &["t1", "t2"], b"b"),
            raw_version("other", 1, &["t3"], b"c"),
        ],
    )
    .unwrap();
    let span = NormalizedSpan {
        project_id: Some("project".to_string()),
        trace_id: "t1".to_string(),
        span_id: "s1".to_string(),
        timestamp_start: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
        raw_id: Some("with-rows".to_string()),
        ..Default::default()
    };
    crate::repositories::span::insert_batch(&conn, &[span]).unwrap();
    assert_eq!(
        crate::repositories::raw::named(
            &conn,
            &project,
            &["with-rows".to_string(), "rows-expired".to_string()]
        )
        .unwrap(),
        std::collections::HashSet::from(["with-rows".to_string()])
    );

    delete_traces(&conn, "project", &["t1".to_string()]).unwrap();
    let pending = crate::repositories::raw::pending(&conn, 10).unwrap();
    let mut queued: Vec<&str> = pending.iter().map(|entry| entry.raw_id.as_str()).collect();
    queued.sort_unstable();
    assert_eq!(queued, vec!["rows-expired", "with-rows"]);

    // An entry enqueued after the read survives the clear of the entries read.
    crate::repositories::raw::enqueue(&conn, &project, &["with-rows".to_string()]).unwrap();
    crate::repositories::raw::clear(&conn, &pending).unwrap();
    let left = crate::repositories::raw::pending(&conn, 10).unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].raw_id, "with-rows");

    crate::repositories::raw::delete(&conn, &project, &["rows-expired".to_string()]).unwrap();
    let indexed: i64 = conn
        .query_row(
            "SELECT count(*) FROM otel_raw_traces WHERE raw_id = 'rows-expired'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(indexed, 0, "a deleted record leaves no trace-index rows");
}

/// A project hold reaches the raw records and their index, and a project deletion removes every raw table's
/// rows for it.
#[tokio::test]
async fn holds_and_project_deletion_reach_the_raw_tables() {
    let (_dir, service) = create_test_service().await;
    let conn = service.conn();
    let project = sideseat_ports::types::ProjectId::from("project");
    crate::repositories::raw::insert(&conn, &[raw_version("raw", 1, &["t1"], b"a")]).unwrap();
    crate::repositories::raw::enqueue(&conn, &project, &["raw".to_string()]).unwrap();
    let until = Utc.timestamp_opt(1_900_000_000, 0).unwrap();
    patch_project_hold(&conn, "project", until).unwrap();
    let held = crate::repositories::raw::get(&conn, &project, &["raw".to_string()]).unwrap();
    assert_eq!(held[0].hold_until, Some(until));
    let index_hold: i64 = conn
        .query_row(
            "SELECT count(*) FROM otel_raw_traces WHERE hold_until IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(index_hold, 1);

    delete_project_data(&conn, "project").unwrap();
    for table in ["otel_raw", "otel_raw_traces", "otel_raw_pending"] {
        let rows: i64 = conn
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(rows, 0, "{table} keeps rows of a deleted project");
    }
}

/// Deleting a session takes its spans' search terms with them, as deleting a trace does: a term outliving its span
/// is matched against the next span stored under the same identity.
#[tokio::test]
async fn deleting_a_session_takes_its_spans_search_terms() {
    let (_dir, service) = create_test_service().await;
    let conn = service.conn();
    insert_batch(
        &conn,
        &[NormalizedSpan {
            session_id: Some("session-1".to_string()),
            ..revision("span", "digest", 0)
        }],
    )
    .expect("insert");
    conn.execute_batch(
        "INSERT INTO span_terms VALUES ('project', 'trace', 'span', 'prompt', 'alpha', now())",
    )
    .expect("terms");

    let deleted = delete_sessions(&conn, "project", &["session-1".to_string()]).expect("delete");
    assert_eq!(deleted, vec!["trace".to_string()]);
    let terms: i64 = conn
        .query_row("SELECT COUNT(*) FROM span_terms", [], |row| row.get(0))
        .expect("count");
    assert_eq!(terms, 0, "the session's span terms outlived it");
}

/// A session is the traces whose earliest span carries its id. An id seen only on a trace's later spans names no
/// session, and reading it finds none: the aggregate over no spans was a row of nulls, and the read failed.
#[tokio::test]
async fn a_session_no_trace_belongs_to_is_not_found() {
    let (_dir, service) = create_test_service().await;
    let conn = service.conn();
    let start = chrono::Utc::now();
    insert_batch(
        &conn,
        &[
            NormalizedSpan {
                session_id: Some("first".to_string()),
                timestamp_start: start,
                ..revision("root", "digest-root", 0)
            },
            NormalizedSpan {
                session_id: Some("later".to_string()),
                timestamp_start: start + chrono::TimeDelta::seconds(1),
                ..revision("child", "digest-child", 0)
            },
        ],
    )
    .expect("insert");
    assert!(
        get_session(&conn, "project", "first")
            .expect("read")
            .is_some()
    );
    assert!(
        get_session(&conn, "project", "later")
            .expect("read")
            .is_none()
    );
    assert!(
        get_session(&conn, "project", "absent")
            .expect("read")
            .is_none()
    );
}

/// The reconciler's delete takes only a record no row names, and its rewrite is stored only while the record is,
/// each deciding in its own transaction: a row written up to the delete keeps its record, and a record collected
/// since a rewrite was read stays collected.
#[tokio::test]
async fn the_reconciler_s_delete_and_rewrite_decide_in_their_own_step() {
    let (_dir, service) = create_test_service().await;
    let project = sideseat_ports::types::ProjectId::from("project");
    let named_span = sideseat_ports::types::NormalizedSpan {
        project_id: Some("project".to_string()),
        trace_id: "t1".to_string(),
        span_id: "s1".to_string(),
        raw_id: Some("raw-a".to_string()),
        span_name: "call".to_string(),
        timestamp_start: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
        ingested_at: Some(Utc.timestamp_opt(1_700_000_000, 0).unwrap()),
        ..Default::default()
    };
    service
        .write(|conn| {
            crate::repositories::raw::insert(
                conn,
                &[
                    raw_version("raw-a", 1, &["t1"], b"SSR1\0\0a"),
                    raw_version("raw-b", 1, &["t2"], b"SSR1\0\0b"),
                ],
            )?;
            crate::repositories::span::insert_batch(conn, std::slice::from_ref(&named_span))
        })
        .unwrap();
    let ids = ["raw-a".to_string(), "raw-b".to_string()];

    let deleted = service
        .write(|conn| crate::repositories::raw::delete_unnamed(conn, &project, &ids))
        .unwrap();
    assert_eq!(deleted, ["raw-b".to_string()].into_iter().collect());
    let stored = |service: &crate::DuckdbService| -> Vec<String> {
        crate::repositories::raw::get(&service.conn(), &project, &ids)
            .unwrap()
            .into_iter()
            .map(|row| format!("{}/{}", row.raw_id, row.version))
            .collect()
    };
    assert_eq!(stored(&service), ["raw-a/1"], "the named record is kept");

    let appended = service
        .write(|conn| {
            crate::repositories::raw::append_rewrites(
                conn,
                &[
                    raw_version("raw-a", 2, &[], b"SSR1\0\0a2"),
                    raw_version("raw-b", 2, &[], b"SSR1\0\0b2"),
                ],
            )
        })
        .unwrap();
    assert_eq!(appended, ["raw-a".to_string()].into_iter().collect());
    assert_eq!(
        stored(&service),
        ["raw-a/2"],
        "the collected record stays collected"
    );
    // And the rewrite stored is queued with it: one prepared before a later deletion is looked at again, whatever
    // becomes of the reconciler that appended it.
    let queued: Vec<String> = crate::repositories::raw::pending(&service.conn(), 10)
        .unwrap()
        .into_iter()
        .map(|entry| entry.raw_id)
        .collect();
    assert_eq!(
        queued,
        ["raw-a"],
        "the rewrite is queued, the refused one is not"
    );
}
