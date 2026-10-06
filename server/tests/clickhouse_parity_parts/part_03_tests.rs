async fn assert_list_and_filter_parity(
    duck: &sideseat_adapter_duckdb::DuckdbRepository,
    ch: &sideseat_adapter_clickhouse::ClickhouseRepository,
    spans: &[NormalizedSpan],
) {
    let (duck_traces, _) = duck
        .list_traces(&trace_params())
        .await
        .expect("duckdb traces for pagination");
    // --- pagination, sorting, filters and time bounds ----------------------
    // Paged over traces and sessions as well as spans, and over both cursor feeds, because the
    // claim was "pagination" while only the span list actually asked for a second page - so the
    // total-order fix those queries needed could have regressed unnoticed.
    let mut trace_pages: Vec<Vec<String>> = Vec::new();
    // Enough pages to cover the fixture whatever its size, so adding a trace does not silently
    // stop the coverage assertion below from meaning anything.
    let trace_page_count = (duck_traces.len() as u32).div_ceil(2);
    for page in 1..=trace_page_count {
        let params = ListTracesParams {
            page,
            limit: 2,
            ..trace_params()
        };
        let (d, d_total) = duck.list_traces(&params).await.expect("duckdb trace page");
        let (c, c_total) = ch
            .list_traces(&params)
            .await
            .expect("clickhouse trace page");
        assert_eq!(d_total, c_total, "trace page {page}: totals differ");
        assert_eq!(
            d.iter().map(describe_trace).collect::<Vec<_>>(),
            c.iter().map(describe_trace).collect::<Vec<_>>(),
            "trace page {page} differs between backends"
        );
        trace_pages.push(d.iter().map(|t| t.trace_id.clone()).collect());
    }
    let paged_traces: Vec<&String> = trace_pages.iter().flatten().collect();
    let distinct_traces: std::collections::BTreeSet<&&String> = paged_traces.iter().collect();
    assert_eq!(
        paged_traces.len(),
        distinct_traces.len(),
        "a trace appeared on two pages: {trace_pages:?}"
    );
    assert_eq!(
        distinct_traces.len(),
        duck_traces.len(),
        "paging did not cover every trace: {trace_pages:?}"
    );

    let mut session_pages: Vec<String> = Vec::new();
    for page in 1..=2 {
        let params = ListSessionsParams {
            project_id: ProjectId::from(PROJECT),
            page,
            limit: 1,
            ..Default::default()
        };
        let (d, d_total) = duck
            .list_sessions(&params)
            .await
            .expect("duckdb session page");
        let (c, c_total) = ch
            .list_sessions(&params)
            .await
            .expect("clickhouse session page");
        assert_eq!(d_total, c_total, "session page {page}: totals differ");
        assert_eq!(
            d.iter().map(describe_session).collect::<Vec<_>>(),
            c.iter().map(describe_session).collect::<Vec<_>>(),
            "session page {page} differs between backends"
        );
        assert_eq!(d.len(), 1, "session page {page} should hold one session");
        session_pages.push(d[0].session_id.clone());
    }
    // Two backends both returning page one twice would satisfy equality; the pages have to be
    // different sessions and together cover the fixture's two.
    assert_eq!(
        session_pages
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        2,
        "the two session pages returned the same session: {session_pages:?}"
    );

    // A single unpaginated unfiltered call exercises none of the offset arithmetic, ORDER BY
    // translation or predicate building, which is where two dialects have the most room to differ.
    let mut page_ids: Vec<Vec<String>> = Vec::new();
    // Derived from the fixture, so adding a span does not quietly leave the last one unpaged and
    // turn the coverage assertion below into nothing.
    let span_page_count = (spans.len() as u32).div_ceil(3);
    for page in 1..=span_page_count {
        let params = ListSpansParams {
            project_id: ProjectId::from(PROJECT),
            page,
            limit: 3,
            order_by: Some(sideseat_ports::types::OrderBy {
                column: "timestamp_start".to_string(),
                direction: sideseat_ports::types::OrderDirection::Asc,
            }),
            ..Default::default()
        };
        let (d, d_total) = duck.list_spans(&params).await.expect("duckdb page");
        let (c, c_total) = ch.list_spans(&params).await.expect("clickhouse page");
        assert_eq!(d_total, c_total, "page {page}: totals differ");
        assert_eq!(
            d.iter().map(describe_span).collect::<Vec<_>>(),
            c.iter().map(describe_span).collect::<Vec<_>>(),
            "page {page} of list_spans differs between backends"
        );
        page_ids.push(d.iter().map(|s| s.span_id.clone()).collect());
    }
    // Pages must be disjoint and cover the fixture, or the offset arithmetic is wrong in a way
    // both backends could share.
    let paged: Vec<&String> = page_ids.iter().flatten().collect();
    let distinct: std::collections::BTreeSet<&&String> = paged.iter().collect();
    assert_eq!(
        paged.len(),
        distinct.len(),
        "paging returned the same span twice: {page_ids:?}"
    );
    assert_eq!(
        paged.len(),
        spans.len(),
        "paging did not cover every span: {page_ids:?}"
    );

    for (label, params) in [
        (
            "sorted by cost desc",
            ListTracesParams {
                order_by: Some(sideseat_ports::types::OrderBy {
                    column: "total_cost".to_string(),
                    direction: sideseat_ports::types::OrderDirection::Desc,
                }),
                ..trace_params()
            },
        ),
        (
            "filtered by environment",
            ListTracesParams {
                environment: Some(vec!["test".to_string()]),
                ..trace_params()
            },
        ),
        (
            "filtered by user",
            ListTracesParams {
                user_id: Some("user-1".to_string()),
                ..trace_params()
            },
        ),
        // One case per filter variant: each is rendered by its own arm, in a dialect where the
        // wrong shape either raises or silently matches everything.
        (
            "filtered by a token count",
            ListTracesParams {
                filters: vec![Filter::Number {
                    column: "total_tokens".to_string(),
                    operator: NumberOp::Gt,
                    value: 100.0,
                }],
                ..trace_params()
            },
        ),
        (
            "filtered by a decimal cost",
            ListTracesParams {
                filters: vec![Filter::Number {
                    column: "total_cost".to_string(),
                    operator: NumberOp::Gte,
                    value: 0.001,
                }],
                ..trace_params()
            },
        ),
        (
            // The Name filter the UI offers is a select over the displayed names, so it has to
            // match the displayed name: "agent" must return the traces shown as "agent" and not
            // every trace that merely contains an agent span.
            "filtered by the displayed name",
            ListTracesParams {
                filters: vec![Filter::StringOptions {
                    column: "trace_name".to_string(),
                    operator: OptionsOp::AnyOf,
                    value: vec!["agent".to_string()],
                }],
                ..trace_params()
            },
        ),
        (
            // A name filter combined with another one. The name has to be computed from the trace's
            // whole span set, as the projection computes it - not from the rows the model filter
            // left behind, which for a root-agent/generation-child trace is the child, so the row
            // came back labelled "agent" after being selected as "generation".
            "filtered by name and model together",
            ListTracesParams {
                filters: vec![
                    Filter::StringOptions {
                        column: "trace_name".to_string(),
                        operator: OptionsOp::AnyOf,
                        value: vec!["agent".to_string()],
                    },
                    Filter::String {
                        column: "gen_ai_request_model".to_string(),
                        operator: StringOp::Eq,
                        value: "claude-haiku".to_string(),
                    },
                ],
                ..trace_params()
            },
        ),
        (
            "filtered by a name substring",
            ListTracesParams {
                filters: vec![Filter::String {
                    column: "trace_name".to_string(),
                    operator: StringOp::Contains,
                    value: "gen".to_string(),
                }],
                ..trace_params()
            },
        ),
        (
            "filtered by an exact model",
            ListTracesParams {
                filters: vec![Filter::String {
                    column: "gen_ai_request_model".to_string(),
                    operator: StringOp::Eq,
                    value: "claude-haiku".to_string(),
                }],
                ..trace_params()
            },
        ),
        (
            "filtered by tags, any of",
            ListTracesParams {
                filters: vec![Filter::StringOptions {
                    column: "tags".to_string(),
                    operator: OptionsOp::AnyOf,
                    value: vec!["beta".to_string(), "gamma".to_string()],
                }],
                ..trace_params()
            },
        ),
        (
            "filtered by tags, none of",
            ListTracesParams {
                filters: vec![Filter::StringOptions {
                    column: "tags".to_string(),
                    operator: OptionsOp::NoneOf,
                    value: vec!["alpha".to_string()],
                }],
                ..trace_params()
            },
        ),
        (
            "filtered by environment options",
            ListTracesParams {
                filters: vec![Filter::StringOptions {
                    column: "environment".to_string(),
                    operator: OptionsOp::AnyOf,
                    value: vec!["test".to_string()],
                }],
                ..trace_params()
            },
        ),
        (
            // "None of" over a nullable column. A trace with no user id at all is not one of the
            // listed users, so it belongs in the result - which is what the complement form
            // (`trace_id NOT IN (traces whose user is listed)`) gives. Rendering the negation
            // directly instead made both dialects evaluate `NULL NOT IN (...)` to NULL and drop
            // those traces silently, and made a trace with two users match because one of them was
            // someone else. This is the case that pins the quantifier, because every other filtered
            // column is populated for every row.
            //
            // Named against a user the fixture *has*, so the answer is a strict subset: with an absent
            // user every trace matches, and a filter that had been dropped entirely would look the same.
            "filtered by none of a nullable column",
            ListTracesParams {
                filters: vec![Filter::StringOptions {
                    column: "user_id".to_string(),
                    operator: OptionsOp::NoneOf,
                    value: vec!["user-1".to_string()],
                }],
                ..trace_params()
            },
        ),
        (
            // The complement, on the column whose filter is a statement about the trace. A trace with
            // no session at all is not in session-3, so it belongs in the result - and both backends
            // have to negate the canonical subquery rather than the displayed aggregate to say so.
            "filtered by none of a session",
            ListTracesParams {
                filters: vec![Filter::StringOptions {
                    column: "session_id".to_string(),
                    operator: OptionsOp::NoneOf,
                    value: vec!["session-3".to_string()],
                }],
                ..trace_params()
            },
        ),
        (
            // Against the session the row *displays*. trace-i records its session on the root span
            // and nothing on its generation child, so a row-level check called it session-less
            // while the list showed session-3.
            "filtered by a null session",
            ListTracesParams {
                filters: vec![Filter::Null {
                    column: "session_id".to_string(),
                    operator: NullOp::IsNull,
                }],
                ..trace_params()
            },
        ),
        (
            "filtered by a datetime",
            ListTracesParams {
                filters: vec![Filter::Datetime {
                    column: "start_time".to_string(),
                    operator: DatetimeOp::Gte,
                    value: ts(10).to_rfc3339(),
                }],
                ..trace_params()
            },
        ),
        (
            "time bounded",
            ListTracesParams {
                from_timestamp: Some(ts(-60)),
                to_timestamp: Some(ts(15)),
                ..trace_params()
            },
        ),
        (
            // A negated filter on an aggregate, *with* a time window - the one shape whose subquery also
            // carries a `gen_totals` join, whose scope binds are rendered ahead of the subquery's own
            // project id. trace-a totals 330, so the complement is every other trace; a swapped bind
            // group compares a project id against a timestamp and answers differently.
            "filtered by none of a token total, time bounded",
            ListTracesParams {
                from_timestamp: Some(ts(-60)),
                filters: vec![Filter::StringOptions {
                    column: "total_tokens".to_string(),
                    operator: OptionsOp::NoneOf,
                    value: vec!["330".to_string()],
                }],
                ..trace_params()
            },
        ),
        (
            "genai only",
            ListTracesParams {
                include_nongenai: false,
                ..trace_params()
            },
        ),
        (
            // trace-a's spans hold 110 and 220 tokens and the list displays their sum, 330. A
            // filter applied to one span row hid it from `> 250` because neither span reaches the
            // threshold, and returned it for `< 150` because one span is under - the row visible on
            // screen contradicted the filter that was supposed to have selected it.
            "filtered by a token total no single span reaches",
            ListTracesParams {
                filters: vec![Filter::Number {
                    column: "total_tokens".to_string(),
                    operator: NumberOp::Gt,
                    value: 250.0,
                }],
                ..trace_params()
            },
        ),
        (
            // trace-i carries its session id on the root span and its tokens on the generation
            // child. ANDed on one span row, the two conditions ask for a span with both, which no
            // span in that trace has, so the trace disappeared from a list that showed it under
            // either filter alone.
            "filtered by a session and a token count together",
            ListTracesParams {
                filters: vec![
                    Filter::String {
                        column: "session_id".to_string(),
                        operator: StringOp::Eq,
                        value: "session-3".to_string(),
                    },
                    Filter::Number {
                        column: "total_tokens".to_string(),
                        operator: NumberOp::Gt,
                        value: 100.0,
                    },
                ],
                ..trace_params()
            },
        ),
    ] {
        let (d, d_total) = duck.list_traces(&params).await.expect("duckdb traces");
        let (c, c_total) = ch.list_traces(&params).await.expect("clickhouse traces");
        assert_eq!(d_total, c_total, "{label}: totals differ");
        assert_eq!(
            d.iter().map(describe_trace).collect::<Vec<_>>(),
            c.iter().map(describe_trace).collect::<Vec<_>>(),
            "list_traces {label} differs between backends"
        );
        // What makes each case non-vacuous, stated per case. `d.len() <= total` let a filter that
        // was dropped entirely pass, which is the failure being guarded against; but a sort
        // returns everything by design, and two of these filters do match every trace in this
        // fixture, so "strict subset" cannot be the rule for all of them.
        assert!(!d.is_empty(), "{label} selected no traces at all");
        match label {
            // Order is the observable, so compare it against the default ordering.
            "sorted by cost desc" => assert_ne!(
                d.iter().map(|t| &t.trace_id).collect::<Vec<_>>(),
                duck_traces.iter().map(|t| &t.trace_id).collect::<Vec<_>>(),
                "the sort returned the default order, so it exercises nothing"
            ),
            // Every span in the fixture carries this environment, so matching all of them is
            // correct here and the parity comparison is what this case contributes.
            "filtered by environment" | "filtered by environment options" => assert_eq!(
                d.len(),
                duck_traces.len(),
                "the fixture changed: this filter no longer matches every trace"
            ),
            // Excludes the two plain traces but must keep trace-g, whose GenAI attributes sit on a
            // span with no observation type.
            "genai only" => {
                let kept: Vec<&String> = d.iter().map(|t| &t.trace_id).collect();
                // trace-g qualifies through attributes, trace-h through token usage, trace-j
                // through cost alone - the three shapes instrumentation produces without an
                // observation type. Removing any one clause from the predicate fails here.
                for expected in ["trace-g", "trace-h", "trace-j"] {
                    assert!(
                        kept.contains(&&expected.to_string()),
                        "the GenAI filter dropped {expected}, whose GenAI data is on a plain span: \
                         {kept:?}"
                    );
                }
                // Visible is not enough: trace-j's cost has to be *counted*. Token and cost
                // aggregation admitted a row only when it reported tokens, so a span reporting cost
                // alone was listed with a cost of zero - and then sorted, filtered and totalled as
                // free.
                let cost_only = d
                    .iter()
                    .find(|t| t.trace_id == "trace-j")
                    .expect("trace-j is in the list");
                assert!(
                    (cost_only.total_cost - 0.004).abs() < 1e-9,
                    "the cost-only trace reports {} rather than the 0.004 it was billed",
                    cost_only.total_cost
                );
                assert!(
                    !kept.contains(&&"trace-e".to_string())
                        && !kept.contains(&&"trace-f".to_string()),
                    "the GenAI filter kept a trace with no GenAI data at all: {kept:?}"
                );
            }
            "filtered by name and model together" => {
                let names: Vec<Option<&String>> = d.iter().map(|t| t.trace_name.as_ref()).collect();
                assert!(
                    names.iter().all(|n| n.map(String::as_str) == Some("agent")),
                    "combining the filters returned a trace displayed under another name: {names:?}"
                );
                assert_eq!(
                    d.len(),
                    2,
                    "both agent-named traces have a span carrying the model: {names:?}"
                );
            }
            "filtered by the displayed name" => {
                let names: Vec<Option<&String>> = d.iter().map(|t| t.trace_name.as_ref()).collect();
                assert!(
                    names.iter().all(|n| n.map(String::as_str) == Some("agent")),
                    "the name filter returned traces displayed under another name: {names:?}"
                );
                assert_eq!(
                    d.len(),
                    2,
                    "the fixture has two traces displayed as agent: {names:?}"
                );
            }
            // Every returned row must show a total that satisfies the filter, and the trace whose
            // total only exists as a sum must be among them.
            "filtered by a token total no single span reaches" => {
                let kept: Vec<&String> = d.iter().map(|t| &t.trace_id).collect();
                assert!(
                    kept.contains(&&"trace-a".to_string()),
                    "the trace displaying 330 tokens across two spans of 110 and 220 is missing: \
                     {kept:?}"
                );
                for t in &d {
                    assert!(
                        t.total_tokens > 250,
                        "trace {} is displayed with {} tokens but was selected by `> 250`",
                        t.trace_id,
                        t.total_tokens
                    );
                }
            }
            // trace-a and trace-b hold a session on every span, trace-i on the root only, trace-c
            // on both of its spans; only trace-d and the plain traces display none.
            "filtered by a null session" => {
                let kept: Vec<&String> = d.iter().map(|t| &t.trace_id).collect();
                assert!(
                    !kept.contains(&&"trace-i".to_string()),
                    "trace-i displays session-3 on its root span, so it is not session-less: \
                     {kept:?}"
                );
                for t in &d {
                    assert!(
                        t.session_id.is_none(),
                        "trace {} is displayed under session {:?} but matched `is null`",
                        t.trace_id,
                        t.session_id
                    );
                }
            }
            "filtered by a session and a token count together" => {
                let kept: Vec<&String> = d.iter().map(|t| &t.trace_id).collect();
                assert_eq!(
                    kept,
                    vec![&"trace-i".to_string()],
                    "the session is on the root span and the tokens on its child; both describe \
                     the trace: {kept:?}"
                );
                assert!(
                    d[0].total_tokens > 100,
                    "the row was selected by a token filter it does not satisfy: {}",
                    d[0].total_tokens
                );
            }
            _ => assert!(
                d.len() < duck_traces.len(),
                "{label} selected all {} traces, so a dropped filter would pass",
                d.len()
            ),
        }
    }

    // The span and session lists take filters through their own column mappers, so the wiring is
    // exercised per query rather than assumed from the trace case.
    for (label, params) in [
        (
            "span filtered by model",
            ListSpansParams {
                project_id: ProjectId::from(PROJECT),
                page: 1,
                limit: 50,
                filters: vec![Filter::String {
                    column: "gen_ai_request_model".to_string(),
                    operator: StringOp::Eq,
                    value: "claude-haiku".to_string(),
                }],
                ..Default::default()
            },
        ),
        (
            "span filtered by token count",
            ListSpansParams {
                project_id: ProjectId::from(PROJECT),
                page: 1,
                limit: 50,
                filters: vec![Filter::Number {
                    column: "gen_ai_usage_total_tokens".to_string(),
                    operator: NumberOp::Gte,
                    value: 100.0,
                }],
                ..Default::default()
            },
        ),
    ] {
        let (d, d_total) = duck.list_spans(&params).await.expect("duckdb spans");
        let (c, c_total) = ch.list_spans(&params).await.expect("clickhouse spans");
        assert_eq!(d_total, c_total, "{label}: totals differ");
        assert_eq!(
            d.iter().map(describe_span).collect::<Vec<_>>(),
            c.iter().map(describe_span).collect::<Vec<_>>(),
            "list_spans {label} differs between backends"
        );
        assert!(
            !d.is_empty() && d.len() < spans.len(),
            "{label} selected {} of {} spans, so it exercises nothing",
            d.len(),
            spans.len()
        );
    }

    let session_filtered = ListSessionsParams {
        project_id: ProjectId::from(PROJECT),
        page: 1,
        limit: 50,
        filters: vec![Filter::String {
            column: "user_id".to_string(),
            operator: StringOp::Eq,
            value: "user-1".to_string(),
        }],
        ..Default::default()
    };
    let (d, d_total) = duck
        .list_sessions(&session_filtered)
        .await
        .expect("duckdb sessions");
    let (c, c_total) = ch
        .list_sessions(&session_filtered)
        .await
        .expect("clickhouse sessions");
    assert_eq!(d_total, c_total, "filtered session totals differ");
    assert_eq!(
        d.iter().map(describe_session).collect::<Vec<_>>(),
        c.iter().map(describe_session).collect::<Vec<_>>(),
        "filtered list_sessions differs between backends"
    );
    assert_eq!(
        d.len(),
        1,
        "the session filter selected {} sessions, so it exercises nothing",
        d.len()
    );
    // session-1 spans trace-a and trace-b, and only trace-b carries user-1. The filter selects the
    // session through that trace; it must not shrink the session to it - selection and membership
    // are separate questions, and one predicate for both listed the session with one trace and
    // trace-b's tokens alone while opening it showed both.
    assert_eq!(
        d[0].trace_count, 2,
        "the filtered session lost the trace that does not name its user: {:?}",
        d[0].trace_count
    );
    assert_eq!(
        d[0].total_tokens, 385,
        "the session's tokens must cover both of its traces (110 + 220 + 55), not only the trace \
         the filter matched: {} reported",
        d[0].total_tokens
    );

    // The complement, on the list whose entities are sessions. A session that never used user-1 belongs in
    // "none of user-1" - including one that names no user at all, which the row predicate dropped because
    // `NULL NOT IN (…)` is NULL. Both backends have to answer it the same way and both have to answer it at
    // all, so the result is required to be a strict, non-empty subset.
    let session_negated = ListSessionsParams {
        filters: vec![Filter::StringOptions {
            column: "user_id".to_string(),
            operator: OptionsOp::NoneOf,
            value: vec!["user-1".to_string()],
        }],
        ..session_filtered.clone()
    };
    let (d_all, _) = duck
        .list_sessions(&ListSessionsParams {
            filters: vec![],
            ..session_filtered.clone()
        })
        .await
        .expect("duckdb sessions");
    let (d, d_total) = duck
        .list_sessions(&session_negated)
        .await
        .expect("duckdb sessions");
    let (c, c_total) = ch
        .list_sessions(&session_negated)
        .await
        .expect("clickhouse sessions");
    assert_eq!(d_total, c_total, "negated session totals differ");
    assert_eq!(
        d.iter().map(describe_session).collect::<Vec<_>>(),
        c.iter().map(describe_session).collect::<Vec<_>>(),
        "negated list_sessions differs between backends"
    );
    assert!(
        !d.is_empty() && d.len() < d_all.len(),
        "\"none of user-1\" returned {} of {} sessions, so it exercises nothing",
        d.len(),
        d_all.len()
    );
    assert!(
        !d.iter().any(|s| s.session_id == "session-1"),
        "session-1 used user-1, so it is not \"none of user-1\": {:?}",
        d.iter().map(|s| &s.session_id).collect::<Vec<_>>()
    );

    // Every column the API accepts as a trace sort must actually sort by it. One that is accepted
    // and unmapped falls through to min_ts, so the list comes back in time order while the UI shows
    // the chosen column as active - which was true of total_tokens.
    for column in sideseat_ports::filters::columns::TRACE_SORTABLE {
        let params = ListTracesParams {
            order_by: Some(sideseat_ports::types::OrderBy {
                column: column.to_string(),
                direction: sideseat_ports::types::OrderDirection::Desc,
            }),
            ..trace_params()
        };
        let (d, _) = duck.list_traces(&params).await.expect("duckdb sorted");
        let (c, _) = ch.list_traces(&params).await.expect("clickhouse sorted");
        assert_eq!(
            d.iter().map(describe_trace).collect::<Vec<_>>(),
            c.iter().map(describe_trace).collect::<Vec<_>>(),
            "sorting traces by {column} differs between backends"
        );
        // Descending by the requested column, whatever it is.
        let values: Vec<f64> = d
            .iter()
            .map(|t| match *column {
                "start_time" => t.start_time.timestamp_micros() as f64,
                "end_time" => t.end_time.unwrap_or(t.start_time).timestamp_micros() as f64,
                "duration_ms" => t.duration_ms.unwrap_or(0) as f64,
                "total_tokens" => t.total_tokens as f64,
                "total_cost" => t.total_cost,
                other => panic!("{other} is sortable but this test does not read it"),
            })
            .collect();
        assert!(
            values.windows(2).all(|w| w[0] >= w[1]),
            "sorting traces by {column} descending produced {values:?}"
        );
    }

    for column in sideseat_ports::filters::columns::SESSION_SORTABLE {
        let params = ListSessionsParams {
            project_id: ProjectId::from(PROJECT),
            page: 1,
            limit: 50,
            order_by: Some(sideseat_ports::types::OrderBy {
                column: column.to_string(),
                direction: sideseat_ports::types::OrderDirection::Desc,
            }),
            ..Default::default()
        };
        let (d, _) = duck.list_sessions(&params).await.expect("duckdb sorted");
        let (c, _) = ch.list_sessions(&params).await.expect("clickhouse sorted");
        assert_eq!(
            d.iter().map(describe_session).collect::<Vec<_>>(),
            c.iter().map(describe_session).collect::<Vec<_>>(),
            "sorting sessions by {column} differs between backends"
        );
        let values: Vec<f64> = d
            .iter()
            .map(|s| match *column {
                "start_time" => s.start_time.timestamp_micros() as f64,
                "end_time" => s.end_time.unwrap_or(s.start_time).timestamp_micros() as f64,
                "trace_count" => s.trace_count as f64,
                "span_count" => s.span_count as f64,
                "observation_count" => s.observation_count as f64,
                other => panic!("{other} is sortable but this test does not read it"),
            })
            .collect();
        assert!(
            values.windows(2).all(|w| w[0] >= w[1]),
            "sorting sessions by {column} descending produced {values:?}"
        );
    }
}
