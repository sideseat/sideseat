/// Deleting a session reports every trace it removed, not the caller's earlier snapshot.
///
/// The delete re-resolves the session, so it also removes a trace that joined after the caller resolved
/// it - correctly, since the caller is answered 204. But the caller tombstones and reclaims files for
/// what it resolved, so such a trace used to lose its rows while keeping its file associations forever,
/// with no tombstone: the trace sweep walks tombstones and had none, and the session sweep resolves
/// sessions through analytics rows that were gone. Returning the ids is what lets the caller cover it.
#[tokio::test]
async fn deleting_a_session_reports_every_trace_it_removed() {
    let (_tmp, service) = create_test_service().await;
    let project = "p";
    let t0 = Utc::now() - chrono::Duration::seconds(60);

    let span = |trace: &str, offset: i64| NormalizedSpan {
        project_id: Some(project.to_string()),
        trace_id: trace.to_string(),
        span_id: format!("{trace}-s1"),
        span_name: "generation".to_string(),
        observation_type: Some(ObservationType::Generation),
        timestamp_start: t0 + chrono::Duration::seconds(offset),
        timestamp_end: Some(t0 + chrono::Duration::seconds(offset + 1)),
        session_id: Some("session-1".to_string()),
        ..Default::default()
    };

    {
        let conn = service.conn();
        insert_batch(&conn, &[span("trace-early", 0)]).expect("insert");
    }

    // What a caller resolves before deleting. Scoped: the connection is exclusive, so holding it while
    // opening another deadlocks.
    {
        let conn = service.conn();
        let snapshot = get_trace_ids_for_sessions(&conn, project, &["session-1".to_string()], None)
            .expect("resolve");
        assert_eq!(snapshot, vec!["trace-early".to_string()], "premise");
    }

    // A trace joins the session after that resolution - a writer that passed the fence earlier.
    {
        let conn = service.conn();
        insert_batch(&conn, &[span("trace-late", 5)]).expect("insert late");
    }

    let conn = service.conn();
    let mut deleted = delete_sessions(&conn, project, &["session-1".to_string()]).expect("delete");
    deleted.sort();
    assert_eq!(
        deleted,
        vec!["trace-early".to_string(), "trace-late".to_string()],
        "the deletion removed both traces, so it must report both"
    );

    // And both really are gone, so the report is not a claim about rows that survived.
    for trace in ["trace-early", "trace-late"] {
        assert!(
            get_trace(&conn, project, trace).expect("query").is_none(),
            "{trace} is still readable after its session was deleted"
        );
    }
}

/// A filter with an empty value list changes nothing, on every list.
///
/// An empty option list means "no value chosen", which the renderers answer with `1=1` - neutral in the
/// query's own WHERE and *not* neutral once wrapped in a subquery over a narrower relation. `session_id
/// any of []` became `trace_id IN (traces that have a session)`, so every sessionless trace vanished from
/// a list nobody had filtered. Such a filter now contributes no condition at all.
#[tokio::test]
async fn an_empty_value_list_is_not_a_filter() {
    use sideseat_ports::filters::{Filter, OptionsOp};

    let (_tmp, service) = create_test_service().await;
    let project = "p";
    let t0 = Utc::now() - chrono::Duration::seconds(60);

    let span = |trace: &str, session: Option<&str>| NormalizedSpan {
        project_id: Some(project.to_string()),
        trace_id: trace.to_string(),
        span_id: format!("{trace}-s1"),
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
                span("t-in-a", Some("session-a")),
                span("t-sessionless", None),
            ],
        )
        .expect("insert");
    }

    let conn = service.conn();
    let unfiltered = list_traces(&conn, &trace_filter_params(project, vec![]))
        .expect("list_traces")
        .0
        .len();
    assert_eq!(unfiltered, 2, "premise of the test");

    for operator in [OptionsOp::AnyOf, OptionsOp::NoneOf] {
        for column in ["session_id", "user_id", "gen_ai_request_model"] {
            let empty = vec![Filter::StringOptions {
                column: column.to_string(),
                operator: operator.clone(),
                value: vec![],
            }];
            let (rows, total) = list_traces(&conn, &trace_filter_params(project, empty.clone()))
                .expect("list_traces");
            assert_eq!(
                (rows.len(), total),
                (unfiltered, unfiltered as u64),
                "an empty {column} list must leave the trace list alone"
            );

            let (spans, _) = list_spans(
                &conn,
                &ListSpansParams {
                    project_id: ProjectId::from(project),
                    page: 1,
                    limit: 50,
                    filters: empty,
                    ..Default::default()
                },
            )
            .expect("list_spans");
            assert_eq!(
                spans.len(),
                2,
                "an empty {column} list must leave the span list alone"
            );
        }
    }
}

/// A session-list filter selects sessions, whichever span of the session carries the value.
///
/// Three defects in one predicate, all the trace list's already-fixed shape by another route: a filter on
/// a column only the session's *children* carry matched nothing, because a child names no session and the
/// row predicate requires one; `none of alice` returned a session that used alice in one span and bob in
/// the next; and it dropped every session with no value at all, which is exactly the sessions that are
/// not alice.
#[tokio::test]
async fn a_session_list_filter_selects_sessions_not_span_rows() {
    use sideseat_ports::filters::{Filter, OptionsOp, StringOp};

    let (_tmp, service) = create_test_service().await;
    let project = "p";
    let t0 = Utc::now() - chrono::Duration::seconds(60);

    let span = |trace: &str, span_id: &str, session: &str, user: Option<&str>| NormalizedSpan {
        project_id: Some(project.to_string()),
        trace_id: trace.to_string(),
        span_id: span_id.to_string(),
        span_name: "generation".to_string(),
        observation_type: Some(ObservationType::Generation),
        timestamp_start: t0,
        timestamp_end: Some(t0 + chrono::Duration::seconds(1)),
        session_id: Some(session.to_string()),
        user_id: user.map(str::to_string),
        ..Default::default()
    };

    {
        let conn = service.conn();
        insert_batch(
            &conn,
            &[
                // session-1 used alice once and bob once, so it is *not* "none of alice".
                span("t1", "s1", "session-1", Some("alice")),
                span("t1", "s2", "session-1", Some("bob")),
                // session-2's root names the session and carries no user; its child carries the
                // user and the model, and names no session - the ordinary framework shape.
                span("t2", "s1", "session-2", None),
                NormalizedSpan {
                    user_id: Some("carol".to_string()),
                    gen_ai_request_model: Some("haiku".to_string()),
                    parent_span_id: Some("s1".to_string()),
                    session_id: None,
                    span_id: "s2".to_string(),
                    ..span("t2", "s2", "session-2", None)
                },
            ],
        )
        .expect("insert");
    }

    let conn = service.conn();
    let ask = |params: ListSessionsParams| {
        let (rows, total) = list_sessions(&conn, &params).expect("list_sessions");
        let mut ids: Vec<String> = rows.into_iter().map(|r| r.session_id).collect();
        ids.sort();
        (ids, total)
    };
    let base = || ListSessionsParams {
        project_id: ProjectId::from(project),
        page: 1,
        limit: 50,
        ..Default::default()
    };
    let only_two = (vec!["session-2".to_string()], 1);

    assert_eq!(
        ask(ListSessionsParams {
            filters: vec![Filter::StringOptions {
                column: "user_id".to_string(),
                operator: OptionsOp::NoneOf,
                value: vec!["alice".to_string()],
            }],
            ..base()
        }),
        only_two,
        "the session that used alice is not \"none of alice\"; the one with no user is"
    );

    assert_eq!(
        ask(ListSessionsParams {
            filters: vec![Filter::String {
                column: "gen_ai_request_model".to_string(),
                operator: StringOp::Eq,
                value: "haiku".to_string(),
            }],
            ..base()
        }),
        only_two,
        "the model sits on a child span that names no session, and it is still the session's"
    );

    // The dedicated parameters take the same route, for the same reason.
    assert_eq!(
        ask(ListSessionsParams {
            user_id: Some("carol".to_string()),
            ..base()
        }),
        only_two,
        "a user recorded on a child span is the session's user"
    );
    assert_eq!(
        ask(ListSessionsParams {
            user_id: Some("nobody".to_string()),
            ..base()
        }),
        (vec![], 0),
        "and a user nobody has selects nothing, so the subquery is not a no-op"
    );
}

/// A negated aggregate filter binds in step with its neighbours, in every order.
///
/// It contributes its own subquery - optionally with a `gen_totals` join whose scope binds are rendered
/// *before* the subquery's own project id - in the middle of a filter list. A mistake in that order is a
/// query that runs and compares the wrong values, so it is checked by asking the same question with the
/// filters permuted and requiring one answer.
#[tokio::test]
async fn a_negated_aggregate_filter_binds_in_step_with_its_neighbours() {
    use sideseat_ports::filters::{Filter, NumberOp, OptionsOp, StringOp};

    let (_tmp, service) = create_test_service().await;
    let project = "p";
    let t0 = Utc::now() - chrono::Duration::seconds(60);

    let span = |trace: &str, user: Option<&str>, model: &str, tokens: i64| NormalizedSpan {
        project_id: Some(project.to_string()),
        trace_id: trace.to_string(),
        span_id: format!("{trace}-s1"),
        span_name: "generation".to_string(),
        observation_type: Some(ObservationType::Generation),
        timestamp_start: t0,
        timestamp_end: Some(t0 + chrono::Duration::seconds(1)),
        user_id: user.map(str::to_string),
        gen_ai_request_model: Some(model.to_string()),
        gen_ai_usage_input_tokens: tokens,
        gen_ai_usage_total_tokens: tokens,
        ..Default::default()
    };

    {
        let conn = service.conn();
        insert_batch(
            &conn,
            &[
                span("t-alice", Some("alice"), "haiku", 3000),
                span("t-none", None, "haiku", 3000),
                span("t-other", Some("bob"), "sonnet", 100),
            ],
        )
        .expect("insert");
    }

    let conn = service.conn();
    let names = |filters: Vec<Filter>| {
        let (rows, total) =
            list_traces(&conn, &trace_filter_params(project, filters)).expect("list_traces");
        let mut ids: Vec<String> = rows.into_iter().map(|t| t.trace_id).collect();
        ids.sort();
        (ids, total)
    };

    let not_alice = Filter::StringOptions {
        column: "user_id".to_string(),
        operator: OptionsOp::NoneOf,
        value: vec!["alice".to_string()],
    };
    let haiku = Filter::String {
        column: "gen_ai_request_model".to_string(),
        operator: StringOp::Eq,
        value: "haiku".to_string(),
    };
    let big = Filter::Number {
        column: "total_tokens".to_string(),
        operator: NumberOp::Gt,
        value: 2500.0,
    };

    // "Not alice, on haiku, over 2500 tokens" is t-none alone, whatever order the filters arrive in.
    let expected = (vec!["t-none".to_string()], 1);
    assert_eq!(
        names(vec![not_alice.clone(), haiku.clone(), big.clone()]),
        expected
    );
    assert_eq!(
        names(vec![haiku.clone(), not_alice.clone(), big.clone()]),
        expected
    );
    assert_eq!(names(vec![big, haiku, not_alice]), expected);

    // And the path that carries a `gen_totals` join, whose scope binds are rendered *ahead* of the
    // subquery's own project id. A time window makes the scope carry a bind of its own, so the two
    // groups are distinguishable: with them swapped the project id is compared against a timestamp.
    // `total_tokens` is an aggregate, so "none of 3000" is the complement over traces totalling 3000.
    let (rows, total) = list_traces(
        &conn,
        &ListTracesParams {
            project_id: ProjectId::from(project),
            page: 1,
            limit: 50,
            from_timestamp: Some(t0 - chrono::Duration::seconds(5)),
            filters: vec![Filter::StringOptions {
                column: "total_tokens".to_string(),
                operator: OptionsOp::NoneOf,
                value: vec!["3000".to_string()],
            }],
            ..Default::default()
        },
    )
    .expect("list_traces");
    let mut ids: Vec<String> = rows.into_iter().map(|t| t.trace_id).collect();
    ids.sort();
    assert_eq!(
        (ids, total),
        (vec!["t-other".to_string()], 1),
        "only the trace that does not total 3000 survives, and the count must agree"
    );
}

/// The session filter's binds stay in step with other filters, and with negation.
///
/// It contributes a subquery with its own placeholders in the middle of a filter list, so its binds have
/// to interleave with the others in exactly the order the conditions are joined. A mistake there is a
/// silently wrong answer, not an error - the query runs and compares the wrong values.
#[tokio::test]
async fn a_session_filter_binds_in_step_with_its_neighbours() {
    use sideseat_ports::filters::{Filter, OptionsOp, StringOp};

    let (_tmp, service) = create_test_service().await;
    let project = "p";
    let t0 = Utc::now() - chrono::Duration::seconds(60);

    let span =
        |trace: &str, span_id: &str, session: &str, model: &str, offset: i64| NormalizedSpan {
            project_id: Some(project.to_string()),
            trace_id: trace.to_string(),
            span_id: span_id.to_string(),
            span_name: "generation".to_string(),
            observation_type: Some(ObservationType::Generation),
            timestamp_start: t0 + chrono::Duration::seconds(offset),
            timestamp_end: Some(t0 + chrono::Duration::seconds(offset + 1)),
            session_id: Some(session.to_string()),
            gen_ai_request_model: Some(model.to_string()),
            ..Default::default()
        };

    {
        let conn = service.conn();
        insert_batch(
            &conn,
            &[
                span("trace-a", "s1", "session-a", "haiku", 0),
                span("trace-b", "s1", "session-b", "sonnet", 10),
            ],
        )
        .expect("insert");
    }

    let conn = service.conn();
    let count = |filters: Vec<Filter>| {
        list_spans(
            &conn,
            &ListSpansParams {
                project_id: ProjectId::from(project),
                page: 1,
                limit: 50,
                filters,
                ..Default::default()
            },
        )
        .expect("list_spans")
        .0
        .len()
    };

    let session = |value: &str| Filter::String {
        column: "session_id".to_string(),
        operator: StringOp::Eq,
        value: value.to_string(),
    };
    let model = |value: &str| Filter::String {
        column: "gen_ai_request_model".to_string(),
        operator: StringOp::Eq,
        value: value.to_string(),
    };

    // The session filter before and after another filter: both orders must agree, which they only do
    // if each filter's binds follow its own condition.
    assert_eq!(count(vec![session("session-a"), model("haiku")]), 1);
    assert_eq!(count(vec![model("haiku"), session("session-a")]), 1);
    // And a combination that matches nothing, so a swapped bind cannot pass by luck.
    assert_eq!(count(vec![session("session-a"), model("sonnet")]), 0);
    assert_eq!(count(vec![model("sonnet"), session("session-a")]), 0);

    // Negation is the complement over *sessions*, not the negated row predicate.
    assert_eq!(
        count(vec![Filter::StringOptions {
            column: "session_id".to_string(),
            operator: OptionsOp::NoneOf,
            value: vec!["session-a".to_string()],
        }]),
        1,
        "excluding session-a must leave exactly the other trace's span"
    );
}

/// A session *filter* on the trace and span lists also honours the canonical session.
///
/// Parity between the backends is not enough on its own - they can agree on being wrong - so this pins
/// the answer itself. The filter asked whether *any* span of the trace named the session, so a row
/// displayed under one session matched a filter for another.
#[tokio::test]
async fn a_session_filter_selects_only_traces_canonically_in_it() {
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
    for (session, expect_traces, expect_spans) in [
        ("session-canonical", 1usize, 2usize),
        ("session-stray", 0, 0),
    ] {
        let traces = list_traces(
            &conn,
            &ListTracesParams {
                project_id: ProjectId::from(project),
                session_id: Some(session.to_string()),
                page: 1,
                limit: 50,
                ..Default::default()
            },
        )
        .expect("list_traces");
        assert_eq!(
            traces.0.len(),
            expect_traces,
            "trace list filtered by {session}"
        );

        let spans = list_spans(
            &conn,
            &ListSpansParams {
                project_id: ProjectId::from(project),
                session_id: Some(session.to_string()),
                page: 1,
                limit: 50,
                ..Default::default()
            },
        )
        .expect("list_spans");
        assert_eq!(
            spans.0.len(),
            expect_spans,
            "span list filtered by {session}"
        );
    }
}

/// A trace belongs to exactly one session, and deleting another session leaves it alone.
///
/// Every *view* already treats a trace's session as the one on its earliest span, but membership asked
/// whether *any* span of the trace named the session. So a trace whose spans name two sessions belonged
/// to both: both sessions' reads returned it in full, and deleting either deleted the whole trace -
/// taking content the UI was displaying under the other. That last one is data loss, not a display
/// inconsistency.
#[tokio::test]
async fn a_trace_belongs_to_one_session_across_reads_and_deletion() {
    use crate::repositories::messages::get_messages;
    use sideseat_ports::types::MessageQueryParams;

    let (_tmp, service) = create_test_service().await;
    let project = "p";
    let t0 = Utc::now() - chrono::Duration::seconds(60);

    let payload = |text: &str| {
        Some(
                serde_json::json!([{
                    "source": {"event": {"name": "gen_ai.user.message", "time": "2025-01-01T00:00:00Z"}},
                    "content": {"role": "user", "content": text}
                }])
                .to_string(),
            )
    };

    // One trace, two spans naming different sessions. The earliest span is the canonical one.
    let span = |span_id: &str, session: &str, offset: i64, text: &str| NormalizedSpan {
        project_id: Some(project.to_string()),
        trace_id: "trace-1".to_string(),
        span_id: span_id.to_string(),
        span_name: "generation".to_string(),
        observation_type: Some(ObservationType::Generation),
        timestamp_start: t0 + chrono::Duration::seconds(offset),
        session_id: Some(session.to_string()),
        messages: payload(text),
        ..Default::default()
    };

    {
        let conn = service.conn();
        insert_batch(
            &conn,
            &[
                span("span-1", "session-canonical", 0, "the first turn"),
                span(
                    "span-2",
                    "session-stray",
                    1,
                    "a later span with another session",
                ),
            ],
        )
        .expect("insert");
    }

    let rows_for = |session: &str| {
        let conn = service.conn();
        get_messages(
            &conn,
            &MessageQueryParams {
                project_id: ProjectId::from(project),
                session_id: Some(session.to_string()),
                ..Default::default()
            },
        )
        .expect("query")
        .rows
    };

    assert_eq!(
        rows_for("session-canonical").len(),
        2,
        "the canonical session holds the whole trace"
    );
    assert!(
        rows_for("session-stray").is_empty(),
        "a session named only by a later span does not own the trace"
    );

    // And which sessions the trace reports is the same single answer.
    {
        let conn = service.conn();
        let sessions = get_session_ids_for_traces(&conn, project, &["trace-1".to_string()], None)
            .expect("sessions");
        assert_eq!(sessions, vec!["session-canonical".to_string()]);
    }

    // Deleting the stray session must not touch the trace. This is the data-loss case.
    {
        let conn = service.conn();
        let deleted =
            delete_sessions(&conn, project, &["session-stray".to_string()]).expect("delete");
        assert!(
            deleted.is_empty(),
            "no trace belongs to the stray session: {deleted:?}"
        );
    }
    assert_eq!(
        rows_for("session-canonical").len(),
        2,
        "the trace survived the other session's deletion"
    );
}

/// What the canonical session-membership subquery costs, measured against the predicate it replaced.
///
/// `make bench-http` showed the concurrent session-read p95 at 154.6 ms against its 150 ms ceiling right
/// after this area changed - but on a machine at load 13-35, where CLAUDE.md already records this figure
/// moving between 43 and 135 ms for another operation. That number cannot settle the question.
///
/// This can, without a quiet machine: both formulations run **interleaved in one process**, so whatever
/// else the host is doing hits them equally and the *ratio* is meaningful even when the absolute times
/// are not. The concern is structural rather than speculative - membership now deduplicates, which is a
/// window function over the project's spans and cannot use an index, so the read may be paying a second
/// full pass.
///
/// `cargo test --locked --release -p sideseat-server bench_session_membership -- --ignored --nocapture`
#[tokio::test]
#[ignore]
async fn bench_session_membership() {
    const TRACES: usize = 4_000;
    const SPANS_PER_TRACE: usize = 5;
    const ITERATIONS: usize = 15;

    let (_tmp, service) = create_test_service().await;
    let project = "p";
    let t0 = Utc::now() - chrono::Duration::seconds(7_200);

    // One session holding a tenth of the traces, which is the shape a session read faces.
    let mut spans = Vec::with_capacity(TRACES * SPANS_PER_TRACE);
    for t in 0..TRACES {
        let session = (t % 10 == 0).then(|| format!("session-{}", t / 10));
        for sp in 0..SPANS_PER_TRACE {
            spans.push(NormalizedSpan {
                project_id: Some(project.to_string()),
                trace_id: format!("trace-{t:05}"),
                span_id: format!("span-{sp}"),
                span_name: "generation".to_string(),
                observation_type: Some(ObservationType::Generation),
                timestamp_start: t0 + chrono::Duration::milliseconds((t * 10 + sp) as i64),
                session_id: session.clone(),
                messages: Some(
                    serde_json::json!([{
                        "source": {"event": {"name": "gen_ai.user.message",
                                             "time": "2025-01-01T00:00:00Z"}},
                        "content": {"role": "user", "content": "a turn of conversation"}
                    }])
                    .to_string(),
                ),
                ..Default::default()
            });
        }
    }
    {
        let conn = service.conn();
        for chunk in spans.chunks(2_000) {
            insert_batch(&conn, chunk).expect("insert");
        }
    }

    // The predicate this replaced, kept here only as the comparison arm.
    const LOOSE: &str =
        "SELECT DISTINCT trace_id FROM otel_spans WHERE project_id = ? AND session_id = ?";

    let conn = service.conn();
    let production_query = sideseat_query_sql::messages::session_trace_ids(
        project,
        "session-7",
        None,
        Backend::Duckdb,
    );
    // The form this replaced: deduplicate the whole project, then pick the canonical session. Kept
    // here only as a comparison arm, to show what the narrowing buys.
    const CANONICAL_WIDE: &str = "SELECT trace_id FROM ( \
               SELECT trace_id, arg_min(session_id, (timestamp_start, span_id)) AS canonical_session \
               FROM (SELECT * FROM otel_spans \
                     QUALIFY ROW_NUMBER() OVER (PARTITION BY project_id, trace_id, span_id \
                                                ORDER BY ingested_at DESC, rowid DESC) = 1) \
               WHERE project_id = ? AND session_id IS NOT NULL AND session_id != '' \
               GROUP BY trace_id \
             ) WHERE canonical_session = ?";

    let run = |subquery: &str, subquery_binds: usize| -> (std::time::Duration, Vec<String>) {
        let sql = format!(
            "SELECT DISTINCT trace_id FROM {DEDUP_SPANS} \
                 WHERE project_id = ? AND trace_id IN ({subquery})"
        );
        let mut binds: Vec<String> = vec![project.to_string()];
        // The subquery's own binds, in the order its `?` appear.
        if subquery_binds == 4 {
            binds.extend([
                project.to_string(),
                project.to_string(),
                "session-7".to_string(),
                "session-7".to_string(),
            ]);
        } else {
            binds.extend([project.to_string(), "session-7".to_string()]);
        }
        let started = std::time::Instant::now();
        let mut stmt = conn.prepare(&sql).expect("prepare");
        let params: Vec<&dyn duckdb::ToSql> =
            binds.iter().map(|v| v as &dyn duckdb::ToSql).collect();
        // The trace ids, not a count. Two formulations selecting the same *number* of different traces
        // would pass an equality check on counts, so a faster wrong answer could have been adopted on
        // the strength of it.
        let mut rows = stmt.query(params.as_slice()).expect("query");
        let mut ids: Vec<String> = Vec::new();
        while let Some(row) = rows.next().expect("row") {
            ids.push(row.get(0).expect("trace_id"));
        }
        assert!(!ids.is_empty(), "the fixture must match traces, got none");
        ids.sort();
        (started.elapsed(), ids)
    };

    // The two correct forms must select the same rows, or a speed comparison means nothing.
    let (_, want) = run(CANONICAL_WIDE, 2);
    let (_, got) = run(production_query.sql(), 4);
    assert_eq!(
        want, got,
        "narrowing the deduplication must not change *which* traces are selected"
    );

    // Interleaved, so whatever else the host is doing hits every arm equally - which is what makes the
    // ratio meaningful on a machine this benchmark cannot have to itself.
    let _ = run(LOOSE, 2);
    let mut wide = Vec::with_capacity(ITERATIONS);
    let mut loose = Vec::with_capacity(ITERATIONS);
    let mut production = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        wide.push(run(CANONICAL_WIDE, 2).0);
        loose.push(run(LOOSE, 2).0);
        production.push(run(production_query.sql(), 4).0);
    }
    wide.sort();
    loose.sort();
    production.sort();

    let median = |v: &[std::time::Duration]| v[v.len() / 2].as_secs_f64() * 1000.0;

    eprintln!(
        "bench_session_membership: {} traces x {} spans, {ITERATIONS} interleaved iterations\n  \
             loose (raw, any span - the old, incorrect predicate): {:.2} ms\n  \
             canonical, dedup whole project:                       {:.2} ms  ({:.2}x loose)\n  \
             canonical, dedup candidates only (production):        {:.2} ms  ({:.2}x loose)",
        TRACES,
        SPANS_PER_TRACE,
        median(&loose),
        median(&wide),
        median(&wide) / median(&loose),
        median(&production),
        median(&production) / median(&loose),
    );
}

/// Session-membership SQL is owned by the typed builder, not either adapter.
///
/// It has four placeholders in a non-obvious order (project, project, session, session), so a copy that
/// drifts is a silently wrong answer rather than an error - no test fails, the query simply selects
/// different traces. Eleven call sites ask this question, and they had already drifted twice: some read
/// the raw table instead of the deduplicated one, and both message reads kept a hand-written copy of the
/// SQL that a later change to the shared definition would not have reached.
///
/// Checked on the source text because a second copy compiles and passes every behavioural test. The
/// membership *test* is what identifies it - `WHERE canonical_session = ?` - since the two mirror
/// queries legitimately select the same expression without testing it that way.
#[test]
fn the_session_membership_subquery_is_owned_by_the_builder() {
    const MEMBERSHIP_TEST: &str = "WHERE canonical_session = ?";
    for (name, source) in [
        ("duckdb/query.rs", include_str!("../query.rs")),
        ("duckdb/messages.rs", include_str!("../messages.rs")),
        (
            "clickhouse/query.rs",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../adapter-clickhouse/src/repositories/query.rs"
            )),
        ),
        (
            "clickhouse/messages.rs",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../adapter-clickhouse/src/repositories/messages.rs"
            )),
        ),
    ] {
        let code = source
            .split("#[cfg(test)]")
            .next()
            .expect("source before its test module");
        assert!(
            !code.contains(MEMBERSHIP_TEST),
            "{name} reintroduced session-membership SQL outside the typed builder"
        );
    }

    let builder = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../query-sql/src/messages.rs"
    ));
    let code = builder
        .split("#[cfg(test)]")
        .next()
        .expect("builder source before tests");
    assert_eq!(
        code.matches(MEMBERSHIP_TEST).count(),
        2,
        "the builder must own exactly one DuckDB and one ClickHouse lowering"
    );
}
