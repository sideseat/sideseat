/// Session membership resolves at the traversal's instant, not at the current one.
///
/// A feed traversal reads its rows as of a watermark so that it is a view of one instant. Membership was
/// resolved against current data, so the two could disagree: a trace re-delivered into another session
/// mid-traversal is read with its old content but expanded under its *new* session, so the session it
/// actually replays is never loaded, its replayed history has nothing to collapse against, and the reader
/// sees the turn twice across pages.
#[tokio::test]
async fn membership_is_resolved_as_of_the_traversal_watermark() {
    let (_tmp, service) = create_test_service().await;
    let project = "p";

    let span = |session: &str, ingested: chrono::DateTime<Utc>| NormalizedSpan {
        project_id: Some(project.to_string()),
        trace_id: "trace-1".to_string(),
        span_id: "span-1".to_string(),
        span_name: "generation".to_string(),
        observation_type: Some(ObservationType::Generation),
        timestamp_start: Utc::now(),
        session_id: Some(session.to_string()),
        ingested_at: Some(ingested),
        ..Default::default()
    };

    let early = Utc::now() - chrono::Duration::seconds(60);
    let late = Utc::now();

    {
        let conn = service.conn();
        // Two deliveries of one span, the second moving it to another session.
        insert_batch(&conn, &[span("session-old", early)]).expect("first delivery");
        insert_batch(&conn, &[span("session-new", late)]).expect("re-delivery");
    }

    let as_of = early.timestamp_micros() + 1;
    let traces = &["trace-1".to_string()];

    let conn = service.conn();

    // Current membership: the re-delivery won, so the trace is in the new session.
    let now = get_trace_session_pairs(&conn, project, traces, None).expect("current");
    assert_eq!(
        now,
        vec![("trace-1".to_string(), "session-new".to_string())],
        "without a bound, the latest delivery decides membership"
    );

    // As of an instant before the re-delivery: the session the traversal is actually reading.
    let bounded = get_trace_session_pairs(&conn, project, traces, Some(as_of)).expect("bounded");
    assert_eq!(
        bounded,
        vec![("trace-1".to_string(), "session-old".to_string())],
        "a traversal must group the trace with the session its rows belong to"
    );

    // The same for both directions of the membership lookup.
    let sessions =
        get_session_ids_for_traces(&conn, project, traces, Some(as_of)).expect("sessions");
    assert_eq!(sessions, vec!["session-old".to_string()]);

    let old_traces =
        get_trace_ids_for_sessions(&conn, project, &["session-old".to_string()], Some(as_of))
            .expect("traces");
    assert_eq!(old_traces, vec!["trace-1".to_string()]);
    let new_traces =
        get_trace_ids_for_sessions(&conn, project, &["session-new".to_string()], Some(as_of))
            .expect("traces");
    assert!(
        new_traces.is_empty(),
        "the new session did not exist at the watermark"
    );
}

/// A session message query *with* a watermark binds two deduplicated relations in one statement.
///
/// The session branch resolves membership against its own copy of the dedup relation, and the outer
/// query against another - so with a watermark there are two `?` placeholders from two relations plus the
/// condition binds, in an order the statement text decides. Get that order wrong and the query either
/// fails or silently answers about the wrong instant, and no other test exercised the combination: the
/// watermark tests all use the `trace_ids` branch and the session tests all run unbounded.
#[tokio::test]
async fn a_session_query_with_a_watermark_binds_in_the_right_order() {
    use crate::repositories::messages::get_messages;
    use sideseat_ports::types::MessageQueryParams;

    let (_tmp, service) = create_test_service().await;
    let project = "p";

    let payload = |text: &str| {
        Some(
                serde_json::json!([{
                    "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:00Z"}},
                    "content": {"role": "user", "content": text}
                }])
                .to_string(),
            )
    };

    let early = Utc::now() - chrono::Duration::seconds(60);
    let late = Utc::now();

    let span = |session: &str, text: &str, ingested: chrono::DateTime<Utc>| NormalizedSpan {
        project_id: Some(project.to_string()),
        trace_id: "trace-1".to_string(),
        span_id: "span-1".to_string(),
        span_name: "generation".to_string(),
        observation_type: Some(ObservationType::Generation),
        timestamp_start: early,
        session_id: Some(session.to_string()),
        messages: payload(text),
        ingested_at: Some(ingested),
        ..Default::default()
    };

    {
        let conn = service.conn();
        insert_batch(&conn, &[span("session-old", "the first answer", early)]).expect("first");
        insert_batch(&conn, &[span("session-new", "the corrected answer", late)])
            .expect("re-delivery");
    }

    let conn = service.conn();
    let as_of = early.timestamp_micros() + 1;

    // As of before the re-delivery: the trace is in the old session and carries the old content.
    let bounded = get_messages(
        &conn,
        &MessageQueryParams {
            project_id: ProjectId::from(project),
            session_id: Some("session-old".to_string()),
            ingested_before_us: Some(as_of),
            ..Default::default()
        },
    )
    .expect("the bounded session query must execute");
    assert_eq!(
        bounded.rows.len(),
        1,
        "the trace belonged to this session then"
    );
    assert!(
        bounded.rows[0].messages_json.contains("the first answer"),
        "membership and content must describe the same instant, got {}",
        bounded.rows[0].messages_json
    );

    // The session it moved *to* did not exist at that instant.
    let future_session = get_messages(
        &conn,
        &MessageQueryParams {
            project_id: ProjectId::from(project),
            session_id: Some("session-new".to_string()),
            ingested_before_us: Some(as_of),
            ..Default::default()
        },
    )
    .expect("query");
    assert!(
        future_session.rows.is_empty(),
        "a session created after the watermark must not appear in the traversal"
    );

    // Unbounded, the re-delivery wins in both membership and content.
    let current = get_messages(
        &conn,
        &MessageQueryParams {
            project_id: ProjectId::from(project),
            session_id: Some("session-new".to_string()),
            ..Default::default()
        },
    )
    .expect("query");
    assert_eq!(current.rows.len(), 1);
    assert!(
        current.rows[0]
            .messages_json
            .contains("the corrected answer"),
        "the current read must return the corrected content"
    );
}

/// The session list reports each trace under exactly one session.
///
/// A trace whose earliest span names A and whose child names B belongs to A everywhere else. The list
/// selected distinct `session_id` from spans, so it returned **two** sessions and attributed that
/// trace's full spans, tokens and cost to each - and opening B showed nothing, because every read
/// resolves it to A.
#[tokio::test]
async fn the_session_list_reports_each_trace_under_one_session() {
    let (_tmp, service) = create_test_service().await;
    let project = "p";
    let t0 = Utc::now() - chrono::Duration::seconds(60);

    let span = |span_id: &str, session: &str, offset: i64, tokens: i64| NormalizedSpan {
        project_id: Some(project.to_string()),
        trace_id: "trace-1".to_string(),
        span_id: span_id.to_string(),
        span_name: "generation".to_string(),
        observation_type: Some(ObservationType::Generation),
        timestamp_start: t0 + chrono::Duration::seconds(offset),
        timestamp_end: Some(t0 + chrono::Duration::seconds(offset + 1)),
        session_id: Some(session.to_string()),
        environment: Some("prod".to_string()),
        gen_ai_usage_total_tokens: tokens,
        gen_ai_usage_input_tokens: tokens,
        ..Default::default()
    };

    {
        let conn = service.conn();
        insert_batch(
            &conn,
            &[
                span("span-1", "session-canonical", 0, 100),
                span("span-2", "session-stray", 1, 50),
            ],
        )
        .expect("insert");
    }

    let conn = service.conn();
    let (sessions, total) = list_sessions(
        &conn,
        &ListSessionsParams {
            project_id: ProjectId::from(project),
            page: 1,
            limit: 50,
            ..Default::default()
        },
    )
    .expect("list_sessions");

    let ids: Vec<&str> = sessions.iter().map(|s| s.session_id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["session-canonical"],
        "only the trace's canonical session is a session"
    );
    assert_eq!(
        total, 1,
        "and the count must agree with what the list can return"
    );
    assert_eq!(
        sessions[0].total_tokens, 150,
        "the whole trace's tokens belong to its one session, counted once"
    );

    // The filter-option suggestions agree too: a dropdown must not claim more sessions than the list
    // can return. This was the ninth surface of the same question.
    let options =
        get_session_filter_options(&conn, project, &["environment".to_string()], None, None)
            .expect("filter options");
    if let Some(env_counts) = options.get("environment") {
        for opt in env_counts {
            assert_eq!(
                opt.count, 1,
                "environment {:?} belongs to one session, not {}",
                opt.value, opt.count
            );
        }
    }

    // And the dashboard agrees with the list. Parity between the backends cannot establish this - they
    // can agree on being wrong - so the number itself is pinned here.
    let stats = crate::repositories::stats::get_project_stats(
        &conn,
        &sideseat_ports::types::StatsParams {
            project_id: ProjectId::from(project),
            from_timestamp: t0 - chrono::Duration::seconds(10),
            to_timestamp: t0 + chrono::Duration::seconds(600),
            timezone: "UTC".parse().unwrap(),
        },
        t0 + chrono::Duration::seconds(600),
    )
    .expect("project stats");
    assert_eq!(
        stats.counts.sessions, 1,
        "project statistics must not count a session no trace canonically belongs to"
    );
}

/// Dropdown values describe the same winning entities and session-wide fields as the lists.
#[tokio::test]
async fn filter_options_use_winning_deliveries_and_session_wide_values() {
    let (_tmp, service) = create_test_service().await;
    let project = "p";
    let t0 = Utc::now() - chrono::Duration::seconds(60);

    let old = NormalizedSpan {
        project_id: Some(project.to_string()),
        trace_id: "trace-revision".to_string(),
        span_id: "root".to_string(),
        span_name: "generation".to_string(),
        observation_type: Some(ObservationType::Generation),
        timestamp_start: t0,
        session_id: Some("session-revision".to_string()),
        environment: Some("old-environment".to_string()),
        tags: vec!["old-tag".to_string()],
        ingested_at: Some(t0),
        ..Default::default()
    };
    let new = NormalizedSpan {
        environment: Some("new-environment".to_string()),
        tags: vec!["new-tag".to_string()],
        ingested_at: Some(t0 + chrono::Duration::seconds(1)),
        ..old.clone()
    };
    let session_root = NormalizedSpan {
        project_id: Some(project.to_string()),
        trace_id: "trace-child-value".to_string(),
        span_id: "session-root".to_string(),
        span_name: "root".to_string(),
        timestamp_start: t0,
        session_id: Some("session-child-value".to_string()),
        ..Default::default()
    };
    let session_child = NormalizedSpan {
        span_id: "session-child".to_string(),
        parent_span_id: Some("session-root".to_string()),
        session_id: None,
        user_id: Some("child-user".to_string()),
        ..session_root.clone()
    };

    {
        let conn = service.conn();
        insert_batch(&conn, &[old, new, session_root, session_child]).expect("insert");
    }

    let conn = service.conn();
    let trace = get_trace_filter_options(
        &conn,
        project,
        &["environment".to_string(), "trace_name".to_string()],
        None,
        None,
    )
    .expect("trace options");
    let trace_values = &trace["environment"];
    assert!(
        trace_values
            .iter()
            .any(|row| row.value == "new-environment")
    );
    assert!(
        !trace_values
            .iter()
            .any(|row| row.value == "old-environment")
    );
    assert!(
        trace["trace_name"]
            .iter()
            .any(|row| row.value == "generation"),
        "the aggregate trace-name option query must execute and expose displayed names"
    );

    let span = get_span_filter_options(
        &conn,
        project,
        &["environment".to_string()],
        None,
        None,
        false,
    )
    .expect("span options");
    let span_values = &span["environment"];
    assert!(span_values.iter().any(|row| row.value == "new-environment"));
    assert!(!span_values.iter().any(|row| row.value == "old-environment"));

    let tags = get_trace_tags_options(&conn, project, None, None).expect("tag options");
    assert!(tags.iter().any(|row| row.value == "new-tag"));
    assert!(!tags.iter().any(|row| row.value == "old-tag"));

    let sessions = get_session_filter_options(&conn, project, &["user_id".to_string()], None, None)
        .expect("session options");
    assert_eq!(sessions["user_id"][0].value, "child-user");
    assert_eq!(sessions["user_id"][0].count, 1);
}

/// An *advanced* session filter agrees with the dedicated session parameter.
///
/// Two ways of asking the same question lived in the same query: `params.session_id` went through the
/// canonical relation while an advanced filter on `session_id` compared the span's own column. So
/// filtering by a session that only a later span named returned that child, though every view displays
/// its trace under a different session.
#[tokio::test]
async fn an_advanced_session_filter_agrees_with_the_session_parameter() {
    use sideseat_ports::filters::{Filter, StringOp};

    let (_tmp, service) = create_test_service().await;
    let project = "p";
    let t0 = Utc::now() - chrono::Duration::seconds(60);

    let span = |span_id: &str, session: &str, offset: i64| NormalizedSpan {
        project_id: Some(project.to_string()),
        trace_id: "trace-1".to_string(),
        span_id: span_id.to_string(),
        span_name: "generation".to_string(),
        observation_type: Some(ObservationType::Generation),
        timestamp_start: t0 + chrono::Duration::seconds(offset),
        timestamp_end: Some(t0 + chrono::Duration::seconds(offset + 1)),
        session_id: Some(session.to_string()),
        ..Default::default()
    };

    {
        let conn = service.conn();
        insert_batch(
            &conn,
            &[
                span("span-1", "session-canonical", 0),
                span("span-2", "session-stray", 1),
            ],
        )
        .expect("insert");
    }

    let conn = service.conn();
    let eq = |value: &str| Filter::String {
        column: "session_id".to_string(),
        operator: StringOp::Eq,
        value: value.to_string(),
    };

    for (session, expect) in [("session-canonical", 2usize), ("session-stray", 0)] {
        let spans = list_spans(
            &conn,
            &ListSpansParams {
                project_id: ProjectId::from(project),
                page: 1,
                limit: 50,
                filters: vec![eq(session)],
                ..Default::default()
            },
        )
        .expect("list_spans");
        assert_eq!(
            spans.0.len(),
            expect,
            "span list, advanced filter session_id = {session}"
        );

        // And the dedicated parameter agrees, which is the whole point.
        let by_param = list_spans(
            &conn,
            &ListSpansParams {
                project_id: ProjectId::from(project),
                page: 1,
                limit: 50,
                session_id: Some(session.to_string()),
                ..Default::default()
            },
        )
        .expect("list_spans");
        assert_eq!(
            spans.0.len(),
            by_param.0.len(),
            "the advanced filter and the session parameter must agree for {session}"
        );
    }
}

/// A negated session filter means the same thing in the trace list and in the span list.
///
/// The trace list matched the *displayed* aggregate, so `session_id NOT IN ('a')` evaluated
/// `trace_display_first(session_id) NOT IN ('a')`, which is NULL for a trace with no session at all -
/// and NULL is not true, so the trace was absent from "none of a" while the span list, which negates the
/// canonical subquery, returned its spans. Both routes are now the subquery.
#[tokio::test]
async fn a_negated_session_filter_agrees_between_the_trace_and_span_lists() {
    use sideseat_ports::filters::{Filter, OptionsOp};

    let (_tmp, service) = create_test_service().await;
    let project = "p";
    let t0 = Utc::now() - chrono::Duration::seconds(60);

    let span = |trace: &str, span_id: &str, session: Option<&str>| NormalizedSpan {
        project_id: Some(project.to_string()),
        trace_id: trace.to_string(),
        span_id: span_id.to_string(),
        span_name: "generation".to_string(),
        observation_type: Some(ObservationType::Generation),
        timestamp_start: t0,
        timestamp_end: Some(t0 + chrono::Duration::seconds(1)),
        session_id: session.map(str::to_string),
        ..Default::default()
    };

    {
        let conn = service.conn();
        insert_batch(
            &conn,
            &[
                span("t-in-a", "s1", Some("session-a")),
                span("t-sessionless", "s2", None),
            ],
        )
        .expect("insert");
    }

    let conn = service.conn();
    let none_of_a = Filter::StringOptions {
        column: "session_id".to_string(),
        operator: OptionsOp::NoneOf,
        value: vec!["session-a".to_string()],
    };

    let (traces, total) = list_traces(
        &conn,
        &trace_filter_params(project, vec![none_of_a.clone()]),
    )
    .expect("list_traces");
    let ids: Vec<&str> = traces.iter().map(|t| t.trace_id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["t-sessionless"],
        "a trace with no session is not in session A, so it matches \"none of A\""
    );
    assert_eq!(total, 1, "the count must agree with the page");

    let (spans, _) = list_spans(
        &conn,
        &ListSpansParams {
            project_id: ProjectId::from(project),
            page: 1,
            limit: 50,
            filters: vec![none_of_a],
            ..Default::default()
        },
    )
    .expect("list_spans");
    let span_traces: Vec<&str> = spans.iter().map(|s| s.trace_id.as_str()).collect();
    assert_eq!(
        span_traces, ids,
        "the two lists must select the same traces for one filter"
    );
}

/// The name a trace displays is the same in the list, in the detail view and to a filter.
///
/// Two roots stamped with the same start instant is not exotic - a millisecond-resolution clock and two
/// spans opened together produce it. Ordering by the timestamp alone left the choice to the engine, so
/// the list, the detail view and a `trace_name` filter could each pick a different span's name, and the
/// same query could answer differently twice. `span_id` makes the order total.
#[tokio::test]
async fn a_trace_displays_one_name_however_it_is_asked_for() {
    use sideseat_ports::filters::{Filter, StringOp};

    let (_tmp, service) = create_test_service().await;
    let project = "p";
    let t0 = Utc::now() - chrono::Duration::seconds(60);

    // Inserted in the order that contradicts the tie-break, so arrival order cannot pass for it.
    let root = |span_id: &str, name: &str| NormalizedSpan {
        project_id: Some(project.to_string()),
        trace_id: "t1".to_string(),
        span_id: span_id.to_string(),
        span_name: name.to_string(),
        observation_type: Some(ObservationType::Generation),
        timestamp_start: t0,
        timestamp_end: Some(t0 + chrono::Duration::seconds(1)),
        ..Default::default()
    };

    {
        let conn = service.conn();
        insert_batch(&conn, &[root("z-root", "zeta"), root("a-root", "alpha")]).expect("insert");
    }

    let conn = service.conn();
    let (traces, _) = list_traces(&conn, &trace_filter_params(project, vec![])).expect("list");
    assert_eq!(
        traces[0].trace_name.as_deref(),
        Some("alpha"),
        "the earliest span in the total order (timestamp, span_id) names the trace"
    );

    let detail = get_trace(&conn, project, "t1")
        .expect("query")
        .expect("trace exists");
    assert_eq!(
        detail.trace_name, traces[0].trace_name,
        "the detail view and the list must show the same name"
    );

    let (filtered, _) = list_traces(
        &conn,
        &trace_filter_params(
            project,
            vec![Filter::String {
                column: "trace_name".to_string(),
                operator: StringOp::Eq,
                value: traces[0].trace_name.clone().expect("a displayed name"),
            }],
        ),
    )
    .expect("list");
    assert_eq!(
        filtered.len(),
        1,
        "a filter for the displayed name must return the trace displaying it"
    );
}

/// No query hand-writes the displayed trace name; every one takes it from `trace_display_name`.
///
/// It was hand-written in three DuckDB projections and one ClickHouse projection, and the shared helper
/// was used only by the *filter* - so the filter's total order and the projections' partial one were
/// different expressions, and a trace with two roots at one instant was displayed under one name and
/// matched under another. A new copy compiles and passes every behavioural test, so this reads the source.
#[test]
fn the_displayed_trace_name_is_defined_once_per_backend() {
    for (path, source) in [
        (
            "duckdb/repositories/query.rs",
            include_str!("../query.rs") as &str,
        ),
        (
            "clickhouse/repositories/query.rs",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../adapter-clickhouse/src/repositories/query.rs"
            )),
        ),
    ] {
        // Assembled at runtime, or this test's own source is the first thing it finds.
        let column = "span_name";
        let needles = [format!("FIRST(s.{column}"), format!("If(s.{column}")];
        for (number, line) in source.lines().enumerate() {
            let sql = line.trim_start();
            if sql.starts_with("//") {
                continue;
            }
            for needle in &needles {
                assert!(
                    !sql.contains(needle.as_str()),
                    "{path}:{} hand-writes the displayed trace name; \
                         call trace_display_name instead: {sql}",
                    number + 1
                );
            }
        }
    }
}

/// A trace with no value for a displayed column matches "none of" that column.
///
/// The trace list renders a filter on a displayed-but-per-span column against the aggregate the row
/// shows, and the negation as written is `FIRST(user_id ORDER BY ...) NOT IN ('x')` - which is NULL for a
/// trace that has no user id anywhere, so it was dropped. Those are precisely the traces that are not x.
/// Every negation is now the complement of its positive form, in its own subquery.
#[tokio::test]
async fn a_trace_with_no_value_matches_none_of_that_value() {
    use sideseat_ports::filters::{Filter, OptionsOp};

    let (_tmp, service) = create_test_service().await;
    let project = "p";
    let t0 = Utc::now() - chrono::Duration::seconds(60);

    let span = |trace: &str, user: Option<&str>| NormalizedSpan {
        project_id: Some(project.to_string()),
        trace_id: trace.to_string(),
        span_id: format!("{trace}-s1"),
        span_name: "generation".to_string(),
        observation_type: Some(ObservationType::Generation),
        timestamp_start: t0,
        timestamp_end: Some(t0 + chrono::Duration::seconds(1)),
        user_id: user.map(str::to_string),
        ..Default::default()
    };

    {
        let conn = service.conn();
        insert_batch(
            &conn,
            &[span("t-user", Some("alice")), span("t-none", None)],
        )
        .expect("insert");
    }

    let conn = service.conn();
    let (traces, total) = list_traces(
        &conn,
        &trace_filter_params(
            project,
            vec![Filter::StringOptions {
                column: "user_id".to_string(),
                operator: OptionsOp::NoneOf,
                value: vec!["alice".to_string()],
            }],
        ),
    )
    .expect("list_traces");
    let ids: Vec<&str> = traces.iter().map(|t| t.trace_id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["t-none"],
        "a trace with no user id is not alice, so it matches \"none of alice\""
    );
    assert_eq!(total, 1, "the count must agree with the page");
}

/// A filter reads the delivery the list displays, not one it superseded.
///
/// Every filter subquery and both count queries read the raw table, on the recorded grounds that
/// "duplicates share identical data for all filterable columns". A corrected re-delivery is exactly the
/// case that breaks: it is a second row with *different* values, and superseding the first is what
/// `DEDUP_SPANS` is for. Measured before the fix, on one re-delivered root span: the row displayed `new`
/// and 5,000 tokens, while `trace_name = new` returned nothing, "none of old" excluded it, `model = old`
/// matched it, and the filter's own total was 5,100 - the sum of both deliveries.
#[tokio::test]
async fn a_filter_reads_the_delivery_the_row_displays() {
    use sideseat_ports::filters::{Filter, NumberOp, OptionsOp, StringOp};

    let (_tmp, service) = create_test_service().await;
    let project = "p";
    let t0 = Utc::now() - chrono::Duration::seconds(60);

    // The same span twice: same ids, same timestamp, corrected name, model and tokens.
    let root = |name: &str, tokens: i64| NormalizedSpan {
        project_id: Some(project.to_string()),
        trace_id: "t1".to_string(),
        span_id: "r1".to_string(),
        span_name: name.to_string(),
        observation_type: Some(ObservationType::Generation),
        timestamp_start: t0,
        timestamp_end: Some(t0 + chrono::Duration::seconds(1)),
        gen_ai_usage_total_tokens: tokens,
        gen_ai_request_model: Some(name.to_string()),
        ..Default::default()
    };

    {
        let conn = service.conn();
        insert_batch(&conn, &[root("old", 100)]).expect("insert");
    }
    {
        let conn = service.conn();
        insert_batch(&conn, &[root("new", 5000)]).expect("re-deliver");
    }

    let conn = service.conn();
    let (rows, _) = list_traces(&conn, &trace_filter_params(project, vec![])).expect("list");
    assert_eq!(rows[0].trace_name.as_deref(), Some("new"), "premise");
    assert_eq!(rows[0].total_tokens, 5_000, "premise");

    let matches = |filters: Vec<Filter>| {
        let (rows, total) =
            list_traces(&conn, &trace_filter_params(project, filters)).expect("list");
        // The count and the page must agree: they are two statements about one question.
        assert_eq!(
            rows.len() as u64,
            total,
            "the count disagrees with the page"
        );
        rows.len()
    };

    assert_eq!(
        matches(vec![Filter::String {
            column: "trace_name".to_string(),
            operator: StringOp::Eq,
            value: "new".to_string(),
        }]),
        1,
        "the row displays `new`, so a filter for `new` must return it"
    );
    assert_eq!(
        matches(vec![Filter::StringOptions {
            column: "trace_name".to_string(),
            operator: OptionsOp::NoneOf,
            value: vec!["old".to_string()],
        }]),
        1,
        "nothing displays `old`, so the trace is not excluded by \"none of old\""
    );
    assert_eq!(
        matches(vec![Filter::String {
            column: "gen_ai_request_model".to_string(),
            operator: StringOp::Eq,
            value: "old".to_string(),
        }]),
        0,
        "the superseded delivery's model is not the trace's model"
    );
    assert_eq!(
        matches(vec![Filter::Number {
            column: "total_tokens".to_string(),
            operator: NumberOp::Gt,
            value: 5_000.0,
        }]),
        0,
        "the filter's total must be the 5,000 displayed, not 5,100 summed over both deliveries"
    );
}

/// An input preview is the first thing the trace received; an output preview the last it produced.
///
/// Both are chosen among the root spans first and fall back to any span. The fallback for the output side
/// was already descending, and the root branch had no ordering at all - so once a tie-break made it
/// deterministic it deterministically chose the *earliest* root output, which is the wrong end of the
/// trace. Two roots is what makes the two ends distinguishable.
#[tokio::test]
async fn a_trace_shows_its_first_input_and_its_last_output() {
    let (_tmp, service) = create_test_service().await;
    let project = "p";
    let t0 = Utc::now() - chrono::Duration::seconds(60);

    let root = |span_id: &str, offset: i64, text: &str| NormalizedSpan {
        project_id: Some(project.to_string()),
        trace_id: "t1".to_string(),
        span_id: span_id.to_string(),
        span_name: "generation".to_string(),
        observation_type: Some(ObservationType::Generation),
        timestamp_start: t0 + chrono::Duration::seconds(offset),
        timestamp_end: Some(t0 + chrono::Duration::seconds(offset + 1)),
        input_preview: Some(format!("in-{text}")),
        output_preview: Some(format!("out-{text}")),
        ..Default::default()
    };

    {
        let conn = service.conn();
        insert_batch(&conn, &[root("r1", 0, "first"), root("r2", 5, "last")]).expect("insert");
    }

    let conn = service.conn();
    let (rows, _) = list_traces(&conn, &trace_filter_params(project, vec![])).expect("list");
    assert_eq!(rows[0].input_preview.as_deref(), Some("in-first"));
    assert_eq!(rows[0].output_preview.as_deref(), Some("out-last"));

    let detail = get_trace(&conn, project, "t1")
        .expect("query")
        .expect("trace exists");
    assert_eq!(
        (detail.input_preview, detail.output_preview),
        (
            rows[0].input_preview.clone(),
            rows[0].output_preview.clone()
        ),
        "the detail view and the list must show the same previews"
    );
}
