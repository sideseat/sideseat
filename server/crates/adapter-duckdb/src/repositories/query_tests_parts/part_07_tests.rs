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

/// Two rewrites keyed alike: readers take the last stored, and the supersede purge takes the other with the
/// versions below - otherwise the one the purge left kept the deleted content the later rewrite removed, under a
/// key no purge below the latest reaches.
#[tokio::test]
async fn a_supersede_purge_takes_an_earlier_version_of_the_latest_s_own_key() {
    let (_dir, service) = create_test_service().await;
    let conn = service.conn();
    let project = sideseat_ports::types::ProjectId::from("project");
    crate::repositories::raw::append_versions(
        &conn,
        &[
            versioned("r", key(0, 1), b"with-two-deleted-spans"),
            versioned("r", key(0, 2), b"rewrite-without-one"),
            versioned("r", key(0, 2), b"rewrite-without-both"),
        ],
    )
    .unwrap();
    crate::repositories::raw::delete_superseded(&conn, &project, "r", key(0, 2)).unwrap();
    let bodies: Vec<Vec<u8>> = conn
        .prepare("SELECT record FROM otel_raw WHERE raw_id = 'r'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(bodies, vec![b"rewrite-without-both".to_vec()]);
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

/// A request's calls are read in every trace its thread sits in, however many: the reads once named only the first
/// chunk of traces, so a call in any later one was never composed into the view.
#[tokio::test]
async fn a_call_in_a_trace_past_the_first_chunk_is_read() {
    let (_tmp, service) = create_test_service().await;
    let project = "p";
    let traces: Vec<String> = (0..sideseat_query_sql::keyed::KEYED_CHUNK + 88)
        .map(|n| format!("{n:032x}"))
        .collect();
    let start = Utc::now() - chrono::Duration::seconds(60);
    let messages = r#"[{"source":{"attribute":{"key":"probe","time":"2025-01-01T00:00:00Z"}},"content":{"role":"user","content":"q"}}]"#;
    let last = traces.len() - 1;
    let spans: Vec<NormalizedSpan> = traces
        .iter()
        .enumerate()
        .map(|(n, trace)| NormalizedSpan {
            project_id: Some(project.to_string()),
            trace_id: trace.clone(),
            span_id: format!("{n:016x}"),
            span_name: "tool".to_string(),
            // Later traces start earlier, so the chunks' rows interleave, and the second trace ties with the last.
            timestamp_start: start
                - chrono::Duration::milliseconds(if n == 2 { last } else { n } as i64),
            gen_ai_tool_call_id: Some(format!("call-{n}")),
            messages: Some(messages.to_string()),
            ..Default::default()
        })
        .collect();
    {
        let conn = service.conn();
        insert_batch(&conn, &spans).expect("spans");
    }
    let conn = service.conn();
    let mut call_trace_ids = traces.clone();
    call_trace_ids.reverse();
    let context = crate::repositories::messages::get_request_context(
        &conn,
        &sideseat_ports::types::RequestContextParams {
            project_id: sideseat_ports::types::ProjectId::from(project),
            thread: "no-thread".to_string(),
            before_us: Utc::now().timestamp_micros(),
            call_ids: vec![
                format!("call-{last}"),
                "call-1".to_string(),
                "call-2".to_string(),
            ],
            call_trace_ids,
            ingested_before_us: None,
        },
    )
    .expect("the request context");
    assert_eq!(
        context
            .calls
            .iter()
            .map(|row| row.trace_id.as_str())
            .collect::<Vec<_>>(),
        vec![
            traces[2].as_str(),
            traces[last].as_str(),
            traces[1].as_str()
        ],
        "the call in the last chunk is read, and the chunks' rows are in one order, a tie broken by span"
    );
}

/// A trace is its root's session, else its earliest span's: a child that started before its parent - on another
/// host's clock - and names another session no longer moves the trace, and a rootless trace still has a session.
#[tokio::test]
async fn a_trace_is_its_roots_session_else_its_earliest_spans() {
    let (_tmp, service) = create_test_service().await;
    let at = |trace: &str, span: &str, parent: Option<&str>, session: &str, second: i64| {
        NormalizedSpan {
            project_id: Some("p".to_string()),
            trace_id: trace.to_string(),
            span_id: span.to_string(),
            parent_span_id: parent.map(str::to_string),
            span_name: span.to_string(),
            session_id: Some(session.to_string()),
            timestamp_start: Utc.timestamp_opt(1_700_000_000 + second, 0).unwrap(),
            ..Default::default()
        }
    };
    {
        let conn = service.conn();
        insert_batch(
            &conn,
            &[
                at("rooted", "root", None, "root-session", 5),
                at("rooted", "early-child", Some("root"), "child-session", 0),
                at("rootless", "late", Some("gone"), "late-session", 9),
                at("rootless", "early", Some("gone"), "early-session", 1),
            ],
        )
        .expect("spans");
    }
    let conn = service.conn();
    let traces = ["rooted".to_string(), "rootless".to_string()];
    assert_eq!(
        get_trace_session_pairs(&conn, "p", &traces, None).expect("pairs"),
        vec![
            ("rooted".to_string(), "root-session".to_string()),
            ("rootless".to_string(), "early-session".to_string()),
        ]
    );
    assert_eq!(
        get_trace_ids_for_sessions(&conn, "p", &["child-session".to_string()], None)
            .expect("child"),
        Vec::<String>::new(),
        "a child's session does not take a trace whose root names another"
    );
    assert_eq!(
        get_trace_ids_for_sessions(&conn, "p", &["root-session".to_string()], None).expect("root"),
        vec!["rooted".to_string()]
    );
    // What the trace response reports, and what the dashboard counts, follow the same rule.
    assert_eq!(
        get_trace(&conn, "p", "rooted")
            .expect("trace")
            .and_then(|trace| trace.session_id)
            .as_deref(),
        Some("root-session")
    );
    let stats = crate::repositories::stats::get_project_stats(
        &conn,
        &sideseat_ports::types::StatsParams {
            project_id: sideseat_ports::types::ProjectId::from("p"),
            from_timestamp: Utc.timestamp_opt(1_699_990_000, 0).unwrap(),
            to_timestamp: Utc.timestamp_opt(1_700_010_000, 0).unwrap(),
            timezone: "UTC".parse().unwrap(),
        },
        Utc.timestamp_opt(1_700_010_000, 0).unwrap(),
    )
    .expect("stats");
    assert_eq!(stats.counts.sessions, 2, "root-session and early-session");
}

/// A trace's membership as stored: its canonical session, whether a root naming a session is stored - a root naming
/// none decides nothing - and its first receipt.
#[tokio::test]
async fn a_traces_membership_says_whether_its_root_is_stored() {
    let (_tmp, service) = create_test_service().await;
    let at =
        |trace: &str, span: &str, parent: Option<&str>, session: Option<&str>, received: i64| {
            NormalizedSpan {
                project_id: Some("p".to_string()),
                trace_id: trace.to_string(),
                span_id: span.to_string(),
                parent_span_id: parent.map(str::to_string),
                span_name: span.to_string(),
                session_id: session.map(str::to_string),
                timestamp_start: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
                ingested_at: Some(Utc.timestamp_opt(received, 0).unwrap()),
                ..Default::default()
            }
        };
    {
        let conn = service.conn();
        insert_batch(
            &conn,
            &[
                at("rooted", "root", None, Some("s"), 1_700_000_200),
                at("rooted", "child", Some("root"), Some("s"), 1_700_000_100),
                at("rootless", "child", Some("gone"), Some("d"), 1_700_000_300),
                at("sessionless", "root", None, None, 1_700_000_400),
            ],
        )
        .expect("spans");
    }
    let conn = service.conn();
    let traces = [
        "rooted".to_string(),
        "rootless".to_string(),
        "sessionless".to_string(),
        "absent".to_string(),
    ];
    let membership = |trace: &str, session: Option<&str>, rooted: bool, received: i64| {
        sideseat_ports::types::TraceMembership {
            trace_id: trace.to_string(),
            session_id: session.map(str::to_string),
            rooted,
            first_received_us: received * 1_000_000,
        }
    };
    assert_eq!(
        get_trace_memberships(&conn, "p", &traces).expect("memberships"),
        vec![
            membership("rooted", Some("s"), true, 1_700_000_100),
            membership("rootless", Some("d"), false, 1_700_000_300),
            membership("sessionless", None, false, 1_700_000_400),
        ]
    );
}
