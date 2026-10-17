// The call/result pairing suite (`~/sideseat-parked/rubric/pairing/design-call-result-pairing.md`): every parked
// counterexample of the retry-dedup and langsmith reviews, as the conversation each shape must reconstruct. A case the
// current passes get wrong is `#[ignore]`d with the finding that found it, until the execution table lands.

/// A retry after a failed tool asks for the same call again: same name, same arguments, a new id. The span that
/// sent the failure and received the retry shows both - the first call in its input, the retry as its output -
/// because a request never contains the answer to itself.
#[test]
#[ignore = "pairing: retry-dedup: the span view drops the sent call beside its retry"]
fn a_span_view_keeps_the_call_it_was_sent_beside_its_retry() {
    let t0 = chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
        .expect("a time")
        .with_timezone(&Utc);
    let call = |id: &str| json!({"type": "tool_use", "id": id, "name": "book_flight", "input": {"origin": "London"}});
    let messages = json!([
        {
            "source": {"attribute": {"key": "llm.input_messages", "time": t0.to_rfc3339()}},
            "content": {"role": "user", "content": "Book me a flight."}
        },
        {
            "source": {"attribute": {"key": "llm.input_messages", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": [call("call-first")]}
        },
        {
            "source": {"attribute": {"key": "llm.input_messages", "time": t0.to_rfc3339()}},
            "content": {"role": "tool", "content": [{"type": "tool_result", "tool_use_id": "call-first", "content": "offline", "is_error": true}]}
        },
        {
            "source": {"attribute": {"key": "llm.output_messages", "time": t0.to_rfc3339()}},
            "content": {"role": "assistant", "content": [call("call-retry")]}
        }
    ]);
    let row = make_span_row_with_timestamps(
        "trace-retry",
        "generation",
        Some("root"),
        &messages.to_string(),
        t0,
        Some(t0 + chrono::Duration::milliseconds(1)),
    );
    let calls: Vec<String> = process_span(vec![row], &FeedOptions::new())
        .messages
        .iter()
        .filter_map(|block| match &block.content {
            ContentBlock::ToolUse { id, .. } => id.clone(),
            _ => None,
        })
        .collect();
    assert_eq!(calls, ["call-first", "call-retry"]);
}

/// Across a trace the retry is told apart by occurrence ranks, and the same-span separation stays out of it:
/// each call shows once, whichever span holds its copies.
#[test]
fn a_trace_shows_a_call_and_its_retry_once_each() {
    let t0 = chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
        .expect("a time")
        .with_timezone(&Utc);
    let call = |id: &str| json!({"type": "tool_use", "id": id, "name": "book_flight", "input": {"origin": "London"}});
    let at = |key: &str, ms: i64| json!({"attribute": {"key": key, "time": (t0 + chrono::Duration::milliseconds(ms)).to_rfc3339()}});
    let first = json!([
        {"source": at("llm.input_messages", 0), "content": {"role": "user", "content": "Book me a flight."}},
        {"source": at("llm.output_messages", 1), "content": {"role": "assistant", "content": [call("call-first")]}}
    ]);
    let second = json!([
        {"source": at("llm.input_messages", 10), "content": {"role": "user", "content": "Book me a flight."}},
        {"source": at("llm.input_messages", 10), "content": {"role": "assistant", "content": [call("call-first")]}},
        {"source": at("llm.input_messages", 10), "content": {"role": "tool", "content": [{"type": "tool_result", "tool_use_id": "call-first", "content": "offline", "is_error": true}]}},
        {"source": at("llm.output_messages", 11), "content": {"role": "assistant", "content": [call("call-retry")]}}
    ]);
    let rows = vec![
        make_span_row_with_timestamps(
            "trace-retry",
            "gen-1",
            Some("root"),
            &first.to_string(),
            t0,
            Some(t0 + chrono::Duration::milliseconds(2)),
        ),
        make_span_row_with_timestamps(
            "trace-retry",
            "gen-2",
            Some("root"),
            &second.to_string(),
            t0 + chrono::Duration::milliseconds(10),
            Some(t0 + chrono::Duration::milliseconds(12)),
        ),
    ];
    let calls: Vec<String> = process_spans(rows, &FeedOptions::new())
        .messages
        .iter()
        .filter_map(|block| match &block.content {
            ContentBlock::ToolUse { id, .. } => id.clone(),
            _ => None,
        })
        .collect();
    assert_eq!(calls, ["call-first", "call-retry"]);
}

/// The retry's result moves with it: one span that sends the first call and emits the retry, its result recorded
/// beside the answer, shows the retry before its result and pairs them.
#[test]
#[ignore = "pairing: retry-dedup: a retry's result is ordered before the retry"]
fn a_retry_s_result_stays_paired_with_the_retry() {
    let t0 = chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
        .expect("a time")
        .with_timezone(&Utc);
    let call = |id: &str| json!({"type": "tool_use", "id": id, "name": "book_flight", "input": {"origin": "London"}});
    let result = |id: &str, text: &str| json!({"type": "tool_result", "tool_use_id": id, "content": text, "is_error": true});
    let at = |key: &str| json!({"attribute": {"key": key, "time": t0.to_rfc3339()}});
    let later = json!({"event": {"name": "gen_ai.choice", "time": (t0 + chrono::Duration::milliseconds(5)).to_rfc3339()}});
    let messages = json!([
        {"source": at("llm.input_messages"), "content": {"role": "user", "content": "Book me a flight."}},
        {"source": at("llm.input_messages"), "content": {"role": "assistant", "content": [call("call-first")]}},
        {"source": at("llm.input_messages"), "content": {"role": "tool", "content": [result("call-first", "offline")]}},
        {"source": later, "content": {"role": "assistant", "content": [call("call-retry")]}},
        {"source": at("output.value"), "content": {"role": "tool", "content": [result("call-retry", "still offline")]}}
    ]);
    let row = make_span_row_with_timestamps(
        "trace-retry",
        "generation",
        Some("root"),
        &messages.to_string(),
        t0,
        Some(t0 + chrono::Duration::milliseconds(10)),
    );
    let seen: Vec<String> = process_span(vec![row], &FeedOptions::new())
        .messages
        .iter()
        .filter_map(|block| match &block.content {
            ContentBlock::ToolUse { id, .. } => id.clone().map(|id| format!("call {id}")),
            ContentBlock::ToolResult { tool_use_id, .. } => {
                tool_use_id.clone().map(|id| format!("result {id}"))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        seen,
        [
            "call call-first",
            "result call-first",
            "call call-retry",
            "result call-retry"
        ]
    );
}

fn call_and_result_ids(rows: Vec<MessageSpanRow>) -> Vec<String> {
    process_span(rows, &FeedOptions::new())
        .messages
        .iter()
        .filter_map(|block| match &block.content {
            ContentBlock::ToolUse { id, .. } => {
                Some(format!("call {}", id.clone().unwrap_or_default()))
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                ..
            } => Some(format!(
                "result {} {}",
                tool_use_id.clone().unwrap_or_default(),
                serde_json::to_string(content).unwrap_or_default()
            )),
            _ => None,
        })
        .collect()
}

/// Two retries at once, and an accumulated state that lists only the second: its copy is that retry by its id,
/// whatever rank its own carrier gave it, and no first call is lost to it.
#[test]
#[ignore = "pairing: retry-dedup: a copy of a retry is ranked as another execution"]
fn every_copy_of_a_retry_follows_it_by_id() {
    let t0 = chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
        .expect("a time")
        .with_timezone(&Utc);
    let call = |id: &str| json!({"type": "tool_use", "id": id, "name": "book_flight", "input": {"origin": "London"}});
    let at = |key: &str| json!({"attribute": {"key": key, "time": t0.to_rfc3339()}});
    let messages = json!([
        {"source": at("llm.input_messages"), "content": {"role": "user", "content": "Book two flights."}},
        {"source": at("llm.input_messages"), "content": {"role": "assistant", "content": [call("a"), call("b")]}},
        {"source": at("llm.output_messages"), "content": {"role": "assistant", "content": [call("c"), call("d")]}},
        {"source": at("output.value"), "content": {"role": "assistant", "content": [call("d")]}}
    ]);
    let row = make_span_row_with_timestamps(
        "trace-retry",
        "generation",
        Some("root"),
        &messages.to_string(),
        t0,
        Some(t0 + chrono::Duration::milliseconds(1)),
    );
    let calls: Vec<String> = call_and_result_ids(vec![row])
        .into_iter()
        .filter(|seen| seen.starts_with("call "))
        .collect();
    assert_eq!(calls, ["call a", "call b", "call c", "call d"]);
}

/// An id-less copy of a retry's result - a framework's own record of it, naming no call - is still that result.
#[test]
fn an_id_less_copy_of_a_retry_s_result_is_that_result() {
    let t0 = chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
        .expect("a time")
        .with_timezone(&Utc);
    let call = |id: &str| json!({"type": "tool_use", "id": id, "name": "book_flight", "input": {"origin": "London"}});
    let at = |key: &str| json!({"attribute": {"key": key, "time": t0.to_rfc3339()}});
    let messages = json!([
        {"source": at("llm.input_messages"), "content": {"role": "user", "content": "Book me a flight."}},
        {"source": at("llm.input_messages"), "content": {"role": "assistant", "content": [call("first")]}},
        {"source": at("llm.input_messages"), "content": {"role": "tool", "content": [{"type": "tool_result", "tool_use_id": "first", "content": "offline", "is_error": true}]}},
        {"source": at("llm.output_messages"), "content": {"role": "assistant", "content": [call("retry")]}},
        {"source": at("llm.output_messages"), "content": {"role": "tool", "content": [{"type": "tool_result", "tool_use_id": "retry", "content": "still offline", "is_error": true}]}},
        {"source": at("output.value"), "content": {"role": "tool", "content": [{"type": "tool_result", "content": "still offline", "is_error": true}]}}
    ]);
    let row = make_span_row_with_timestamps(
        "trace-retry",
        "generation",
        Some("root"),
        &messages.to_string(),
        t0,
        Some(t0 + chrono::Duration::milliseconds(1)),
    );
    let results: Vec<String> = call_and_result_ids(vec![row])
        .into_iter()
        .filter(|seen| seen.starts_with("result ") && seen.contains("still offline"))
        .collect();
    assert_eq!(results.len(), 1, "{results:?}");
}

fn one_span(messages: serde_json::Value) -> Vec<String> {
    let t0 = chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
        .expect("a time")
        .with_timezone(&Utc);
    call_and_result_ids(vec![make_span_row_with_timestamps(
        "trace-retry",
        "generation",
        Some("root"),
        &messages.to_string(),
        t0,
        Some(t0 + chrono::Duration::milliseconds(1)),
    )])
}

fn retry_call(id: &str) -> serde_json::Value {
    json!({"type": "tool_use", "id": id, "name": "book_flight", "input": {"origin": "London"}})
}

fn retry_result(id: Option<&str>, content: serde_json::Value) -> serde_json::Value {
    match id {
        Some(id) => {
            json!({"type": "tool_result", "tool_use_id": id, "content": content, "is_error": true})
        }
        None => json!({"type": "tool_result", "content": content, "is_error": true}),
    }
}

fn at_key(key: &str) -> serde_json::Value {
    json!({"attribute": {"key": key, "time": "2025-01-01T00:00:00Z"}})
}

/// An id-less result the request carried answers an earlier call: it never follows the retry's result, however
/// alike they read.
#[test]
#[ignore = "pairing: retry-dedup: an id-less request result is taken for a named one"]
fn an_id_less_result_in_the_request_stays_its_own() {
    let seen = one_span(json!([
        {"source": at_key("llm.input_messages"), "content": {"role": "user", "content": "Book me a flight."}},
        {"source": at_key("llm.input_messages"), "content": {"role": "assistant", "content": [retry_call("first")]}},
        {"source": at_key("llm.input_messages"), "content": {"role": "tool", "content": [retry_result(None, json!("offline"))]}},
        {"source": at_key("llm.output_messages"), "content": {"role": "assistant", "content": [retry_call("retry")]}},
        {"source": at_key("llm.output_messages"), "content": {"role": "tool", "content": [retry_result(Some("retry"), json!("offline"))]}}
    ]));
    let results = seen.iter().filter(|s| s.starts_with("result ")).count();
    assert_eq!(results, 2, "{seen:?}");
}

/// An id-less copy follows the named result that says what it says, compared as the result identity compares
/// them (`50` is `"50"`), whatever rank its own carrier gave it. Where two retries' results say the same, the copy
/// could be either and is left as it stands rather than guessed into one.
#[test]
#[ignore = "pairing: retry-dedup: id-less copies attach to the wrong named result"]
fn id_less_copies_follow_their_own_named_result() {
    let seen = one_span(json!([
        {"source": at_key("llm.input_messages"), "content": {"role": "user", "content": "Book two flights."}},
        {"source": at_key("llm.input_messages"), "content": {"role": "assistant", "content": [retry_call("a"), retry_call("b")]}},
        {"source": at_key("llm.output_messages"), "content": {"role": "assistant", "content": [retry_call("c"), retry_call("d")]}},
        {"source": at_key("llm.output_messages"), "content": {"role": "tool", "content": [retry_result(Some("c"), json!(49)), retry_result(Some("d"), json!(50))]}},
        {"source": at_key("output.value"), "content": {"role": "assistant", "content": [retry_call("d")]}},
        {"source": at_key("output.value"), "content": {"role": "tool", "content": [retry_result(Some("d"), json!(50)), retry_result(None, json!("50"))]}}
    ]));
    let calls: Vec<&String> = seen.iter().filter(|s| s.starts_with("call ")).collect();
    assert_eq!(calls, ["call a", "call b", "call c", "call d"], "{seen:?}");
    let id_less = seen.iter().filter(|s| s.starts_with("result  ")).count();
    assert_eq!(id_less, 0, "an id-less copy survived on its own: {seen:?}");
}

/// An id-less retry is left as it was: nothing but its position pairs it with its result, so it is not moved.
#[test]
fn an_id_less_retry_keeps_its_result() {
    let call = json!({"type": "tool_use", "name": "book_flight", "input": {"origin": "London"}});
    let seen = one_span(json!([
        {"source": at_key("llm.input_messages"), "content": {"role": "user", "content": "Book me a flight."}},
        {"source": at_key("llm.input_messages"), "content": {"role": "assistant", "content": [call]}},
        {"source": at_key("llm.input_messages"), "content": {"role": "tool", "content": [retry_result(None, json!("offline"))]}},
        {"source": at_key("llm.output_messages"), "content": {"role": "assistant", "content": [call]}},
        {"source": at_key("llm.output_messages"), "content": {"role": "tool", "content": [retry_result(None, json!("offline"))]}}
    ]));
    let calls = seen.iter().filter(|s| s.starts_with("call ")).count();
    let results = seen.iter().filter(|s| s.starts_with("result ")).count();
    assert_eq!(calls, results, "as many results as calls: {seen:?}");
}

/// Two retries saying the same, one failing and one not: an id-less copy follows the one with its error flag.
#[test]
fn an_id_less_copy_follows_the_result_with_its_error_flag() {
    let ok = |id: Option<&str>| match id {
        Some(id) => {
            json!({"type": "tool_result", "tool_use_id": id, "content": 49, "is_error": false})
        }
        None => json!({"type": "tool_result", "content": 49, "is_error": false}),
    };
    let seen = one_span(json!([
        {"source": at_key("llm.input_messages"), "content": {"role": "user", "content": "Book two flights."}},
        {"source": at_key("llm.input_messages"), "content": {"role": "assistant", "content": [retry_call("a"), retry_call("b")]}},
        {"source": at_key("llm.output_messages"), "content": {"role": "assistant", "content": [retry_call("c"), retry_call("d")]}},
        {"source": at_key("llm.output_messages"), "content": {"role": "tool", "content": [retry_result(Some("c"), json!(49)), ok(Some("d"))]}},
        {"source": at_key("output.value"), "content": {"role": "assistant", "content": [retry_call("d")]}},
        {"source": at_key("output.value"), "content": {"role": "tool", "content": [ok(Some("d")), retry_result(None, json!(49))]}}
    ]));
    let id_less = seen.iter().filter(|s| s.starts_with("result  ")).count();
    assert_eq!(id_less, 0, "an id-less copy survived on its own: {seen:?}");
}

/// One id reused for two executions inside one span - the retry asked twice under the same id, the second time
/// with other arguments - keeps both executions and both results.
#[test]
#[ignore = "pairing: retry-dedup: a reused retry id merges two executions"]
fn a_reused_retry_id_keeps_both_executions_and_results() {
    let call = |args: &str| json!({"type": "tool_use", "id": "b", "name": "book_flight", "input": {"origin": args}});
    let seen = one_span(json!([
        {"source": at_key("llm.input_messages"), "content": {"role": "user", "content": "Book me a flight."}},
        {"source": at_key("llm.input_messages"), "content": {"role": "assistant", "content": [{"type": "tool_use", "id": "a", "name": "book_flight", "input": {"origin": "London"}}]}},
        {"source": at_key("llm.output_messages"), "content": {"role": "assistant", "content": [call("London")]}},
        {"source": at_key("llm.output_messages"), "content": {"role": "tool", "content": [retry_result(Some("b"), json!("first"))]}},
        {"source": at_key("llm.output_messages"), "content": {"role": "assistant", "content": [call("Paris")]}},
        {"source": at_key("llm.output_messages"), "content": {"role": "tool", "content": [retry_result(Some("b"), json!("second"))]}}
    ]));
    let calls = seen.iter().filter(|s| s.starts_with("call ")).count();
    assert_eq!(calls, 3, "{seen:?}");
    assert!(seen.iter().any(|s| s.contains("first")), "{seen:?}");
    assert!(seen.iter().any(|s| s.contains("second")), "{seen:?}");
}

/// An id-less copy that names another tool never follows a retry's result: it stays with the result of its own
/// tool.
#[test]
fn an_id_less_copy_naming_another_tool_stays_with_that_tool() {
    let named = |id: &str, tool: &str| json!({"type": "tool_result", "tool_use_id": id, "name": tool, "content": "ok", "is_error": false});
    let seen = one_span(json!([
        {"source": at_key("llm.input_messages"), "content": {"role": "user", "content": "Do both."}},
        {"source": at_key("llm.input_messages"), "content": {"role": "assistant", "content": [retry_call("a"), {"type": "tool_use", "id": "y", "name": "other", "input": {}}]}},
        {"source": at_key("llm.input_messages"), "content": {"role": "tool", "content": [named("y", "other")]}},
        {"source": at_key("llm.output_messages"), "content": {"role": "assistant", "content": [retry_call("x")]}},
        {"source": at_key("llm.output_messages"), "content": {"role": "tool", "content": [named("x", "book_flight")]}},
        {"source": at_key("output.value"), "content": {"role": "tool", "content": [{"type": "tool_result", "name": "other", "content": "ok", "is_error": false}]}}
    ]));
    let results = seen.iter().filter(|s| s.starts_with("result ")).count();
    assert!(results <= 3, "{seen:?}");
    assert!(seen.iter().any(|s| s.starts_with("result x ")), "{seen:?}");
    assert!(seen.iter().any(|s| s.starts_with("result y ")), "{seen:?}");
}

/// One id and one shape twice in the span's output - two executions alike - stay two calls beside the call they
/// retry, and each keeps its own result (redesign r1).
#[test]
#[ignore = "pairing: redesign r1: one result stands for two alike executions"]
fn two_alike_executions_of_a_retry_stay_two() {
    let seen = one_span(json!([
        {"source": at_key("llm.input_messages"), "content": {"role": "user", "content": "Book me a flight."}},
        {"source": at_key("llm.input_messages"), "content": {"role": "assistant", "content": [retry_call("a")]}},
        {"source": at_key("llm.output_messages"), "content": {"role": "assistant", "content": [retry_call("b"), retry_call("b")]}},
        {"source": at_key("llm.output_messages"), "content": {"role": "tool", "content": [retry_result(Some("b"), json!("first")), retry_result(Some("b"), json!("second"))]}}
    ]));
    let calls = seen.iter().filter(|s| s.starts_with("call ")).count();
    assert_eq!(calls, 3, "{seen:?}");
    // The pairing model answers each execution with its own result emission: both results stay.
    assert!(
        seen.iter()
            .any(|s| s.starts_with("result b ") && s.contains("first")),
        "{seen:?}"
    );
    assert!(
        seen.iter()
            .any(|s| s.starts_with("result b ") && s.contains("second")),
        "{seen:?}"
    );
}

/// A result that names another call never takes a retry's rank, whatever it says.
#[test]
fn a_result_naming_another_call_keeps_its_own_rank() {
    let named = |id: &str| json!({"type": "tool_result", "tool_use_id": id, "name": "book_flight", "content": "ok", "is_error": false});
    let seen = one_span(json!([
        {"source": at_key("llm.input_messages"), "content": {"role": "user", "content": "Book me a flight."}},
        {"source": at_key("llm.input_messages"), "content": {"role": "assistant", "content": [retry_call("a")]}},
        {"source": at_key("llm.input_messages"), "content": {"role": "tool", "content": [named("x")]}},
        {"source": at_key("llm.output_messages"), "content": {"role": "assistant", "content": [retry_call("b")]}},
        {"source": at_key("llm.output_messages"), "content": {"role": "tool", "content": [named("b")]}},
        {"source": at_key("output.value"), "content": {"role": "tool", "content": [named("x")]}}
    ]));
    let x = seen.iter().filter(|s| s.starts_with("result x ")).count();
    assert_eq!(x, 1, "{seen:?}");
}

/// A carrier that lists only some of the retries numbers their occurrences by id as the full one does: two calls
/// under one id stay two, however many same-shaped calls stand before them elsewhere.
#[test]
fn a_subset_carrier_numbers_a_retry_by_its_own_id() {
    let seen = one_span(json!([
        {"source": at_key("llm.input_messages"), "content": {"role": "user", "content": "Book me a flight."}},
        {"source": at_key("llm.input_messages"), "content": {"role": "assistant", "content": [retry_call("a")]}},
        {"source": at_key("llm.output_messages"), "content": {"role": "assistant", "content": [retry_call("c"), retry_call("b"), retry_call("b")]}},
        {"source": at_key("output.value"), "content": {"role": "assistant", "content": [retry_call("b"), retry_call("b")]}}
    ]));
    let b = seen.iter().filter(|s| *s == "call b").count();
    assert_eq!(b, 2, "{seen:?}");
}

/// An id-less result beside an id-less call is that call's, by position, and never a retry's however alike they
/// read.
#[test]
#[ignore = "pairing: retry-dedup: an id-less call's result is merged into a named call's"]
fn an_id_less_result_beside_an_id_less_call_stays_that_call_s() {
    let seen = one_span(json!([
        {"source": at_key("llm.input_messages"), "content": {"role": "user", "content": "Do both."}},
        {"source": at_key("llm.input_messages"), "content": {"role": "assistant", "content": [retry_call("a")]}},
        {"source": at_key("llm.output_messages"), "content": {"role": "assistant", "content": [retry_call("b"), {"type": "tool_use", "name": "other", "input": {}}]}},
        {"source": at_key("llm.output_messages"), "content": {"role": "tool", "content": [retry_result(Some("b"), json!("ok")), retry_result(None, json!("ok"))]}}
    ]));
    let results = seen.iter().filter(|s| s.starts_with("result ")).count();
    assert_eq!(results, 2, "{seen:?}");
}

// ---------------------------------------------------------------------------------------------------------------------
// Cases from the review findings the parked tests above do not already state.
// ---------------------------------------------------------------------------------------------------------------------

fn pairing_t0() -> chrono::DateTime<Utc> {
    chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
        .expect("a time")
        .with_timezone(&Utc)
}

fn shown_in_trace(rows: Vec<MessageSpanRow>) -> Vec<String> {
    process_trace_spans(rows, &FeedOptions::new())
        .messages
        .iter()
        .filter_map(|block| match &block.content {
            ContentBlock::ToolUse { id, .. } => {
                Some(format!("call {}", id.clone().unwrap_or_default()))
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                ..
            } => Some(format!(
                "result {} {}",
                tool_use_id.clone().unwrap_or_default(),
                serde_json::to_string(content).unwrap_or_default()
            )),
            _ => None,
        })
        .collect()
}

/// Two agent invocations in one trace: the first generates `a`, the second is sent `a` and generates a same-shaped
/// `b`, which its agent span reports too. The trace shows `a` and `b` once each (t2a r1).
#[test]
fn a_second_agent_s_retry_shows_once_in_the_trace() {
    let t0 = pairing_t0();
    let at = |key: &str, ms: i64| json!({"attribute": {"key": key, "time": (t0 + chrono::Duration::milliseconds(ms)).to_rfc3339()}});
    let row = |span: &str, parent: &str, kind: &str, messages: serde_json::Value, ms: i64| {
        make_span_row_full(
            "trace-agents",
            span,
            Some(parent),
            &messages.to_string(),
            t0 + chrono::Duration::milliseconds(ms),
            Some(t0 + chrono::Duration::milliseconds(ms + 5)),
            Some(kind),
        )
    };
    let rows = vec![
        row("agent-1", "root", "agent", json!([]), 0),
        row(
            "gen-1",
            "agent-1",
            "generation",
            json!([
                {"source": at("llm.input_messages", 1), "content": {"role": "user", "content": "Book me a flight."}},
                {"source": at("llm.output_messages", 2), "content": {"role": "assistant", "content": [retry_call("a")]}}
            ]),
            1,
        ),
        row(
            "agent-2",
            "root",
            "agent",
            json!([
                {"source": at("output.value", 10), "content": {"role": "assistant", "content": [retry_call("b")]}}
            ]),
            10,
        ),
        row(
            "gen-2",
            "agent-2",
            "generation",
            json!([
                {"source": at("llm.input_messages", 11), "content": {"role": "assistant", "content": [retry_call("a")]}},
                {"source": at("llm.output_messages", 12), "content": {"role": "assistant", "content": [retry_call("b")]}}
            ]),
            11,
        ),
    ];
    let calls: Vec<String> = shown_in_trace(rows)
        .into_iter()
        .filter(|s| s.starts_with("call "))
        .collect();
    assert_eq!(calls, ["call a", "call b"]);
}

/// Input call `a`, output retries `b` and `c`, and an independent id-less call of another tool: `b` says
/// `"different"`, `c` and the independent call both say `"ok"` without naming a call. Three results stay (redesign r2).
#[test]
#[ignore = "pairing: redesign r2: an independent anonymous result is merged into a retry's"]
fn an_independent_anonymous_result_is_not_taken_for_a_retry_s() {
    let anonymous =
        |content: &str| json!({"type": "tool_result", "content": content, "is_error": false});
    let seen = one_span(json!([
        {"source": at_key("llm.input_messages"), "content": {"role": "user", "content": "Do three things."}},
        {"source": at_key("llm.input_messages"), "content": {"role": "assistant", "content": [retry_call("a")]}},
        {"source": at_key("llm.output_messages"), "content": {"role": "assistant", "content": [retry_call("b"), retry_call("c"), {"type": "tool_use", "name": "other", "input": {}}]}},
        {"source": at_key("llm.output_messages"), "content": {"role": "tool", "content": [
            {"type": "tool_result", "tool_use_id": "b", "content": "different", "is_error": false},
            anonymous("ok"),
            anonymous("ok")
        ]}}
    ]));
    let results = seen.iter().filter(|s| s.starts_with("result ")).count();
    assert_eq!(results, 3, "{seen:?}");
}

/// A response with more than 16,384 same-shaped retries keeps every one: no rank encoding can alias two executions
/// (redesign r2).
#[test]
fn many_retries_of_one_shape_stay_distinct() {
    let calls: Vec<serde_json::Value> = (0..16_386).map(|i| retry_call(&format!("r{i}"))).collect();
    let seen = one_span(json!([
        {"source": at_key("llm.input_messages"), "content": {"role": "user", "content": "Retry a lot."}},
        {"source": at_key("llm.input_messages"), "content": {"role": "assistant", "content": [retry_call("a")]}},
        {"source": at_key("llm.output_messages"), "content": {"role": "assistant", "content": calls}}
    ]));
    let retries = seen.iter().filter(|s| s.starts_with("call r")).count();
    assert_eq!(retries, 16_386);
}

/// An accumulated `output.value` that lists `d` twice beside the emission `[c, d]` is a re-listing: one `d`
/// (redesign r3).
#[test]
fn an_accumulated_echo_creates_no_execution() {
    let seen = one_span(json!([
        {"source": at_key("llm.input_messages"), "content": {"role": "user", "content": "Book."}},
        {"source": at_key("llm.input_messages"), "content": {"role": "assistant", "content": [retry_call("a")]}},
        {"source": at_key("llm.output_messages"), "content": {"role": "assistant", "content": [retry_call("c"), retry_call("d")]}},
        {"source": at_key("output.value"), "content": {"role": "assistant", "content": [retry_call("d"), retry_call("d")]}}
    ]));
    let d = seen.iter().filter(|s| *s == "call d").count();
    assert_eq!(d, 1, "{seen:?}");
}

/// Output calls `b` (a retry), `c` and `d`: `b`'s and `c`'s named results both say `"ok"`, and `d` answers with an
/// unnamed `"ok"`. `d`'s result is its own (redesign r3).
#[test]
fn an_anonymous_result_among_named_alikes_stays_its_own() {
    let named = |id: &str| json!({"type": "tool_result", "tool_use_id": id, "content": "ok", "is_error": false});
    let seen = one_span(json!([
        {"source": at_key("llm.input_messages"), "content": {"role": "user", "content": "Do three things."}},
        {"source": at_key("llm.input_messages"), "content": {"role": "assistant", "content": [retry_call("a")]}},
        {"source": at_key("llm.output_messages"), "content": {"role": "assistant", "content": [
            retry_call("b"),
            {"type": "tool_use", "id": "c", "name": "other", "input": {"x": 1}},
            {"type": "tool_use", "id": "d", "name": "third", "input": {"y": 2}}
        ]}},
        {"source": at_key("llm.output_messages"), "content": {"role": "tool", "content": [
            named("b"),
            named("c"),
            {"type": "tool_result", "content": "ok", "is_error": false}
        ]}}
    ]));
    let results = seen.iter().filter(|s| s.starts_with("result ")).count();
    assert_eq!(results, 3, "{seen:?}");
}

/// A retry and, later under the same id, another execution with other arguments: the later execution's named result
/// and its anonymous copy stay one result (redesign r3).
#[test]
#[ignore = "pairing: redesign r3: a renumbered result's anonymous copy stays a second result"]
fn a_renumbered_result_s_anonymous_copy_follows_it() {
    let t0 = pairing_t0();
    let at = |key: &str, ms: i64| json!({"attribute": {"key": key, "time": (t0 + chrono::Duration::milliseconds(ms)).to_rfc3339()}});
    let call = |args: &str| json!({"type": "tool_use", "id": "b", "name": "book_flight", "input": {"origin": args}});
    let seen = call_and_result_ids(vec![make_span_row_with_timestamps(
        "trace-retry",
        "generation",
        Some("root"),
        &json!([
            {"source": at("llm.input_messages", 0), "content": {"role": "user", "content": "Book."}},
            {"source": at("llm.input_messages", 0), "content": {"role": "assistant", "content": [retry_call("a")]}},
            {"source": at("llm.output_messages", 1), "content": {"role": "assistant", "content": [call("London")]}},
            {"source": at("llm.output_messages", 2), "content": {"role": "tool", "content": [retry_result(Some("b"), json!("first"))]}},
            {"source": at("llm.output_messages", 3), "content": {"role": "assistant", "content": [call("Paris")]}},
            {"source": at("llm.output_messages", 4), "content": {"role": "tool", "content": [retry_result(Some("b"), json!("second"))]}},
            {"source": at("output.value", 5), "content": {"role": "tool", "content": [retry_result(None, json!("second"))]}}
        ])
        .to_string(),
        t0,
        Some(t0 + chrono::Duration::milliseconds(10)),
    )]);
    let second = seen.iter().filter(|s| s.contains("second")).count();
    assert_eq!(second, 1, "{seen:?}");
}

// Failure accounting: a failed tool's exception is redundant only where its own execution is answered by a result,
// and one result answers one execution (langsmith r1-r6, the pairing findings).

fn failing_tool(trace: &str, span: &str, parent: &str, message: &str, ms: i64) -> MessageSpanRow {
    let t0 = pairing_t0();
    let mut row = make_span_row_full(
        trace,
        span,
        Some(parent),
        "[]",
        t0 + chrono::Duration::milliseconds(ms),
        Some(t0 + chrono::Duration::milliseconds(ms + 1)),
        Some("tool"),
    );
    row.status_code = Some("ERROR".to_string());
    row.exception_type = Some("RuntimeError".to_string());
    row.exception_message = Some(message.to_string());
    row
}

fn exceptions_and_results(rows: Vec<MessageSpanRow>) -> (usize, usize) {
    let blocks = process_trace_spans(rows, &FeedOptions::new()).messages;
    let results = blocks
        .iter()
        .filter(|b| matches!(b.content, ContentBlock::ToolResult { .. }))
        .count();
    let exceptions = blocks
        .iter()
        .filter(|b| match &b.content {
            ContentBlock::Text { text, .. } => text.contains("RuntimeError"),
            _ => false,
        })
        .count();
    (exceptions, results)
}

/// Two tools fail alike and one result quotes the failure: one failure is answered, the other still says what went
/// wrong (langsmith r4).
#[test]
#[ignore = "pairing: langsmith r4: one quoting result suppresses every alike failure"]
fn one_result_answers_one_failure() {
    let t0 = pairing_t0();
    let generation = make_span_row_with_timestamps(
        "trace-fail",
        "gen",
        Some("root"),
        &json!([
            {"source": {"attribute": {"key": "llm.input_messages", "time": (t0 + chrono::Duration::milliseconds(10)).to_rfc3339()}},
             "content": {"role": "tool", "content": [{"type": "tool_result", "tool_use_id": "call-a", "content": "RuntimeError('offline')", "is_error": true}]}}
        ])
        .to_string(),
        t0 + chrono::Duration::milliseconds(10),
        Some(t0 + chrono::Duration::milliseconds(20)),
    );
    let rows = vec![
        failing_tool("trace-fail", "tool-a", "root", "offline", 1),
        failing_tool("trace-fail", "tool-b", "root", "offline", 2),
        generation,
    ];
    let (exceptions, results) = exceptions_and_results(rows);
    assert_eq!(
        exceptions + results,
        2,
        "two failures, each shown once: {exceptions} exceptions, {results} results"
    );
}

/// A previous turn's result re-sent as history under a reused id does not answer a new failure under that id
/// (langsmith r2, r3).
#[test]
fn a_historical_result_does_not_answer_a_new_failure() {
    let t0 = pairing_t0();
    let generation = make_span_row_with_timestamps(
        "trace-fail",
        "gen",
        Some("root"),
        &json!([
            {"source": {"attribute": {"key": "llm.input_messages", "time": (t0 + chrono::Duration::milliseconds(10)).to_rfc3339()}},
             "content": {"role": "tool", "content": [{"type": "tool_result", "tool_use_id": "call_1", "content": "RuntimeError('old failure')", "is_error": true}]}}
        ])
        .to_string(),
        t0 + chrono::Duration::milliseconds(10),
        Some(t0 + chrono::Duration::milliseconds(20)),
    );
    let rows = vec![
        failing_tool("trace-fail", "tool-new", "root", "new failure", 1),
        generation,
    ];
    let (exceptions, _) = exceptions_and_results(rows);
    assert_eq!(exceptions, 1, "the new failure still says what went wrong");
}

/// An accumulated output copy of a request's result whose call is not visible stays that one result beside a
/// retry's result saying the same (redesign r1).
#[test]
fn a_result_copy_without_its_call_stays_one_result() {
    let seen = one_span(json!([
        {"source": at_key("llm.input_messages"), "content": {"role": "user", "content": "Book."}},
        {"source": at_key("llm.input_messages"), "content": {"role": "assistant", "content": [retry_call("a")]}},
        {"source": at_key("llm.input_messages"), "content": {"role": "tool", "content": [retry_result(Some("x"), json!("offline"))]}},
        {"source": at_key("llm.output_messages"), "content": {"role": "assistant", "content": [retry_call("b")]}},
        {"source": at_key("llm.output_messages"), "content": {"role": "tool", "content": [retry_result(Some("b"), json!("offline"))]}},
        {"source": at_key("output.value"), "content": {"role": "tool", "content": [retry_result(Some("x"), json!("offline"))]}}
    ]));
    let x = seen.iter().filter(|s| s.starts_with("result x ")).count();
    let b = seen.iter().filter(|s| s.starts_with("result b ")).count();
    assert_eq!((x, b), (1, 1), "{seen:?}");
}

/// Retries `c` (failing) and `d` (succeeding) both return `49`; a carrier lists only `d`, and an output-side
/// id-less copy restates `c`'s failure. Each result shows once (t2a r5).
#[test]
fn a_partial_carrier_keeps_each_alike_result_its_own() {
    let result = |id: Option<&str>, is_error: bool| match id {
        Some(id) => {
            json!({"type": "tool_result", "tool_use_id": id, "content": 49, "is_error": is_error})
        }
        None => json!({"type": "tool_result", "content": 49, "is_error": is_error}),
    };
    let seen = one_span(json!([
        {"source": at_key("llm.input_messages"), "content": {"role": "user", "content": "Book."}},
        {"source": at_key("llm.input_messages"), "content": {"role": "assistant", "content": [retry_call("a")]}},
        {"source": at_key("llm.output_messages"), "content": {"role": "assistant", "content": [retry_call("c"), retry_call("d")]}},
        {"source": at_key("llm.output_messages"), "content": {"role": "tool", "content": [result(Some("c"), true), result(Some("d"), false)]}},
        {"source": at_key("output.value"), "content": {"role": "tool", "content": [result(Some("d"), false)]}},
        {"source": at_key("output.value"), "content": {"role": "tool", "content": [result(None, true)]}}
    ]));
    let results = seen.iter().filter(|s| s.starts_with("result ")).count();
    assert_eq!(results, 2, "{seen:?}");
}

/// How many blocks of the trace's view say `needle`, as a composed exception or as a result.
fn saying(rows: Vec<MessageSpanRow>, needle: &str) -> usize {
    process_trace_spans(rows, &FeedOptions::new())
        .messages
        .iter()
        .filter(|b| match &b.content {
            ContentBlock::Text { text, .. } => text.contains(needle),
            ContentBlock::ToolResult { content, .. } => {
                serde_json::to_string(content).is_ok_and(|s| s.contains(needle))
            }
            _ => false,
        })
        .count()
}

/// A failing tool span that records its own call under `id`, optionally with its own error result.
fn failing_call(span: &str, id: &str, message: &str, own_result: bool, ms: i64) -> MessageSpanRow {
    let t0 = pairing_t0();
    let at = json!({"attribute": {"key": "input.value", "time": (t0 + chrono::Duration::milliseconds(ms)).to_rfc3339()}});
    let mut messages = vec![
        json!({"source": at, "content": {"role": "assistant", "content": [
            {"type": "tool_use", "id": id, "name": "lookup", "input": {"q": span}}
        ]}}),
    ];
    if own_result {
        messages.push(json!({"source": {"attribute": {"key": "output.value", "time": (t0 + chrono::Duration::milliseconds(ms + 1)).to_rfc3339()}},
            "content": {"role": "tool", "content": [
                {"type": "tool_result", "tool_use_id": id, "content": format!("RuntimeError('{message}')"), "is_error": true}
            ]}}));
    }
    let mut row = failing_tool("trace-fail", span, "root", message, ms);
    row.messages_json = serde_json::Value::Array(messages).to_string();
    row
}

/// A generation that is sent `results` as tool results, recorded at `ms`.
fn sent_results(span: &str, results: &[serde_json::Value], ms: i64) -> MessageSpanRow {
    let t0 = pairing_t0();
    make_span_row_with_timestamps(
        "trace-fail",
        span,
        Some("root"),
        &json!([
            {"source": {"attribute": {"key": "llm.input_messages", "time": (t0 + chrono::Duration::milliseconds(ms)).to_rfc3339()}},
             "content": {"role": "tool", "content": results}}
        ])
        .to_string(),
        t0 + chrono::Duration::milliseconds(ms),
        Some(t0 + chrono::Duration::milliseconds(ms + 5)),
    )
}

fn error_result(id: &str, text: &str) -> serde_json::Value {
    json!({"type": "tool_result", "tool_use_id": id, "content": text, "is_error": true})
}

/// Two failing spans share a reused id and fail differently, with no result: both failures say what went wrong
/// (langsmith r1).
#[test]
#[ignore = "pairing: langsmith r1: a reused id's composed failure is merged into another's"]
fn two_failures_sharing_an_id_both_report() {
    let rows = vec![
        failing_call("tool-a", "call_0", "first", false, 1),
        failing_call("tool-b", "call_0", "second", false, 3),
    ];
    assert_eq!(saying(rows.clone(), "first"), 1);
    assert_eq!(saying(rows, "second"), 1);
}

/// One failure holds its own result; another fails alike and bare. The own result answers its own failure only
/// (langsmith r5).
#[test]
#[ignore = "pairing: langsmith r5: an own-span result also answers an alike failure by quoting"]
fn an_own_result_answers_only_its_own_failure() {
    let rows = vec![
        failing_call("tool-a", "call-a", "offline", true, 1),
        failing_call("tool-b", "call-b", "offline", false, 3),
    ];
    assert_eq!(saying(rows, "offline"), 2);
}

/// Two failures alike under distinct visible calls, and a result only for `call-b`: after dedup each failure is
/// still shown once (langsmith r5).
#[test]
#[ignore = "pairing: langsmith r5: a result naming call-b answers call-a's failure"]
fn a_result_answers_the_call_it_names() {
    let rows = vec![
        failing_call("tool-a", "call-a", "offline", false, 1),
        failing_call("tool-b", "call-b", "offline", false, 3),
        sent_results(
            "gen",
            &[error_result("call-b", "RuntimeError('offline')")],
            10,
        ),
    ];
    assert_eq!(saying(rows, "offline"), 2);
}

/// One failure holds its own result, another fails alike and bare, and a later generation re-sends the first
/// result: a re-send is a copy and answers nothing new (langsmith r6).
#[test]
#[ignore = "pairing: langsmith r6: a re-sent own result answers a second failure"]
fn a_re_sent_own_result_answers_nothing_new() {
    let rows = vec![
        failing_call("tool-a", "call-a", "offline", true, 1),
        failing_call("tool-b", "call-b", "offline", false, 3),
        sent_results(
            "gen",
            &[error_result("call-a", "RuntimeError('offline')")],
            10,
        ),
    ];
    assert_eq!(saying(rows, "offline"), 2);
}

/// Two alike bare failures and one result sent twice in two renderings: one result answers one failure
/// (langsmith r6).
#[test]
#[ignore = "pairing: langsmith r6: two renderings of one result answer two failures"]
fn one_result_in_two_renderings_answers_one_failure() {
    let rows = vec![
        failing_tool("trace-fail", "tool-a", "root", "offline", 1),
        failing_tool("trace-fail", "tool-b", "root", "offline", 2),
        sent_results(
            "gen-1",
            &[error_result("call-a", "RuntimeError('offline')")],
            10,
        ),
        sent_results(
            "gen-2",
            &[error_result("call-a", "RuntimeError('offline')\n")],
            20,
        ),
    ];
    assert_eq!(saying(rows, "offline"), 2);
}

/// Two sequential executions reuse one id, fail alike, and each is answered by its own identical result: both
/// results stay and neither exception repeats them (langsmith r5, r6).
#[test]
#[ignore = "pairing: langsmith r5/r6: reused-id executions' identical results merge"]
fn reused_id_executions_keep_their_identical_results() {
    let rows = vec![
        failing_call("tool-a", "call_1", "offline", false, 1),
        sent_results(
            "gen-1",
            &[error_result("call_1", "RuntimeError('offline')")],
            5,
        ),
        failing_call("tool-b", "call_1", "offline", false, 10),
        sent_results(
            "gen-2",
            &[error_result("call_1", "RuntimeError('offline')")],
            15,
        ),
    ];
    assert_eq!(saying(rows, "offline"), 2);
}

/// A call, and in the next turn two calls alike to it under their own ids: three executions, three calls (found by
/// the pairing generator).
#[test]
#[ignore = "pairing: generator: alike calls under distinct ids merge across turns by position rank"]
fn parallel_alike_calls_after_an_earlier_alike_one_stay_three() {
    let call =
        |id: &str| json!({"type": "tool_use", "id": id, "name": "lookup", "input": {"q": 1}});
    let t0 = pairing_t0();
    let at = |key: &str, ms: i64| json!({"attribute": {"key": key, "time": (t0 + chrono::Duration::milliseconds(ms)).to_rfc3339()}});
    let generation = |span: &str, messages: serde_json::Value, ms: i64| {
        make_span_row_with_timestamps(
            "trace-alike",
            span,
            Some("root"),
            &messages.to_string(),
            t0 + chrono::Duration::milliseconds(ms),
            Some(t0 + chrono::Duration::milliseconds(ms + 10)),
        )
    };
    let rows = vec![
        generation(
            "gen-0",
            json!([
                {"source": at("llm.input_messages", 0), "content": {"role": "user", "content": "Go."}},
                {"source": at("llm.output_messages", 10), "content": {"role": "assistant", "content": [call("c")]}}
            ]),
            0,
        ),
        generation(
            "gen-1",
            json!([
                {"source": at("llm.input_messages", 100), "content": {"role": "user", "content": "Go."}},
                {"source": at("llm.input_messages", 100), "content": {"role": "assistant", "content": [call("c")]}},
                {"source": at("llm.output_messages", 110), "content": {"role": "assistant", "content": [call("d"), call("a")]}}
            ]),
            100,
        ),
    ];
    let calls: Vec<String> = shown_in_trace(rows)
        .into_iter()
        .filter(|s| s.starts_with("call "))
        .collect();
    assert_eq!(calls, ["call c", "call d", "call a"]);
}
