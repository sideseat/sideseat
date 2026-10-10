//! A request composed from its thread: the shapes the captures show, and the mutations the rubric names.
//!
//! The rows carry the shipped carrier names, so the carrier facts these read - which carriers hold a request's
//! delta, its output and its frame - are the shipped assets' own.

use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use sideseat_ports::types::MessageSpanRow;

use super::*;
use crate::observations::{MessageSource, RawMessage};
use crate::sideml::types::ContentBlock;

fn at(seconds: i64) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .expect("a valid instant")
        .with_timezone(&Utc)
        + Duration::seconds(seconds)
}

fn message(key: &str, content: serde_json::Value, second: i64) -> RawMessage {
    RawMessage {
        source: MessageSource::Attribute {
            key: key.to_string(),
            time: at(second),
        },
        content,
        rendering: false,
        direction: None,
        stream: None,
    }
}

fn user(text: &str, second: i64) -> RawMessage {
    message(
        "new_context",
        json!({"role": "user", "content": text}),
        second,
    )
}

fn reminder(text: &str, second: i64) -> RawMessage {
    message(
        "system_reminders",
        json!({"role": "system", "content": text}),
        second,
    )
}

fn result(id: &str, text: &str, second: i64) -> RawMessage {
    message(
        "new_context",
        json!({"role": "tool", "content": [{"type": "tool_result", "tool_use_id": id, "content": text}]}),
        second,
    )
}

fn reply(text: &str, second: i64) -> RawMessage {
    message(
        "response.model_output",
        json!({"role": "assistant", "content": text}),
        second,
    )
}

fn frame(text: &str, second: i64) -> RawMessage {
    message(
        "system_prompt_preview",
        json!({"role": "system", "content": text}),
        second,
    )
}

fn span(id: &str, second: i64, observation: &str, messages: &[RawMessage]) -> MessageSpanRow {
    MessageSpanRow {
        request_frame: String::new(),
        trace_id: "trace".to_string(),
        span_id: id.to_string(),
        parent_span_id: Some("interaction".to_string()),
        span_timestamp: at(second),
        span_end_timestamp: Some(at(second) + Duration::milliseconds(500)),
        messages_json: serde_json::to_string(messages).expect("messages serialise"),
        tool_definitions_json: "[]".to_string(),
        tool_names_json: "[]".to_string(),
        log_messages_json: "[]".to_string(),
        body_cache_key: None,
        model: Some("model".to_string()),
        provider: None,
        status_code: None,
        exception_type: None,
        exception_message: None,
        exception_stacktrace: None,
        input_tokens: 0,
        output_tokens: 0,
        total_tokens: 0,
        cost_total: 0.0,
        observation_type: Some(observation.to_string()),
        session_id: Some("session".to_string()),
        ingested_at: at(second),
        scope_name: None,
        scope_version: None,
        span_name: Some(format!("probe.{observation}")),
        framework: None,
        response_model: None,
        response_id: None,
        temperature: None,
        top_p: None,
        max_tokens: None,
        finish_reasons: None,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        cost_input: 0.0,
        cost_output: 0.0,
        request_thread: String::new(),
        span_marks: 0,
    }
}

fn request(id: &str, second: i64, messages: &[RawMessage]) -> MessageSpanRow {
    span(id, second, "generation", messages)
}

fn tool(id: &str, second: i64, call: &str, name: &str) -> MessageSpanRow {
    span(
        id,
        second,
        "tool",
        &[message(
            "tool_name",
            json!({"role": "assistant", "content": [{"type": "tool_use", "id": call, "name": name, "input": {}}]}),
            second,
        )],
    )
}

/// The view as `role: content`, which is what a request's messages are judged by.
fn shown(result: &FeedResult) -> Vec<String> {
    result
        .messages
        .iter()
        .map(|block| {
            let content = match &block.content {
                ContentBlock::Text { text, .. } => text.clone(),
                ContentBlock::ToolUse { id, name, .. } => {
                    format!("call {name} {}", id.as_deref().unwrap_or("-"))
                }
                ContentBlock::ToolResult { tool_use_id, .. } => format!("result {tool_use_id:?}"),
                other => format!("{other:?}"),
            };
            format!("{}: {content}", block.role.as_str())
        })
        .collect()
}

fn composed(
    target: &MessageSpanRow,
    thread: &[MessageSpanRow],
    calls: &[MessageSpanRow],
) -> Vec<String> {
    shown(&compose(RequestContextRows {
        target: vec![target.clone()],
        thread: thread.to_vec(),
        calls: calls.to_vec(),
    }))
}

/// Three requests of one thread, each exporting its turn and its reminder: the third was sent every earlier turn,
/// every reminder and every answer, in the order they happened, framed by its own system prompt.
fn three_turns() -> [MessageSpanRow; 3] {
    [
        request(
            "r1",
            10,
            &[
                frame("sys", 10),
                user("q1", 10),
                reminder("env", 10),
                reply("a1", 11),
            ],
        ),
        request(
            "r2",
            20,
            &[
                frame("sys", 20),
                user("q2", 20),
                reminder("tok", 20),
                reply("a2", 21),
            ],
        ),
        request(
            "r3",
            30,
            &[
                frame("sys", 30),
                user("q3", 30),
                reminder("tok", 30),
                reply("a3", 31),
            ],
        ),
    ]
}

#[test]
fn a_request_is_composed_from_its_thread() {
    let [r1, r2, r3] = three_turns();
    let thread = [r1.clone(), r2.clone(), r3.clone()];
    assert_eq!(
        composed(&r3, &thread, &[]),
        [
            "system: sys",
            "user: q1",
            "system: env",
            "assistant: a1",
            "user: q2",
            "system: tok",
            "assistant: a2",
            "user: q3",
            "system: tok",
            "assistant: a3",
        ]
    );
    assert_eq!(
        composed(&r1, &thread, &[]),
        ["system: sys", "user: q1", "system: env", "assistant: a1"],
        "the first request is its own payload, and nothing after it is read"
    );
    let view = compose(RequestContextRows {
        target: vec![r3],
        thread: thread.to_vec(),
        calls: Vec::new(),
    });
    assert_eq!(
        view.metadata.composed_from_requests, 2,
        "it says what it was composed from"
    );
    assert!(
        view.messages.iter().any(|block| block.span_id == "r1"),
        "a composed block is the observation it came from"
    );
}

/// Duplicating a request's row, or reading the thread in another order, changes nothing.
#[test]
fn a_repeated_or_reordered_row_changes_nothing() {
    let [r1, r2, r3] = three_turns();
    let expected = composed(&r3, &[r1.clone(), r2.clone(), r3.clone()], &[]);
    assert_eq!(
        composed(&r3, &[r3.clone(), r2.clone(), r1.clone()], &[]),
        expected
    );
    assert_eq!(
        composed(&r3, &[r1.clone(), r2.clone(), r2.clone(), r1.clone()], &[]),
        expected
    );
}

/// A resumed process restates the history in its delta - its user turns in one carrier, its reminders in another
/// - and nothing is shown twice.
#[test]
fn a_restating_delta_shows_nothing_twice() {
    let r1 = request(
        "r1",
        10,
        &[user("q1", 10), reminder("env", 10), reply("a1", 11)],
    );
    let r2 = request(
        "r2",
        20,
        &[
            user("q1", 20),
            user("q2", 20),
            reminder("env", 20),
            reminder("tok", 20),
            reply("a2", 21),
        ],
    );
    let r3 = request(
        "r3",
        30,
        &[
            user("q1", 30),
            user("q2", 30),
            user("q3", 30),
            reminder("env", 30),
            reminder("tok", 30),
            reminder("tok", 30),
            reply("a3", 31),
        ],
    );
    assert_eq!(
        composed(&r3, &[r1, r2], &[]),
        [
            "user: q1",
            "system: env",
            "assistant: a1",
            "user: q2",
            "system: tok",
            "assistant: a2",
            "user: q3",
            "system: tok",
            "assistant: a3",
        ]
    );
}

/// A tool loop: the call is on a tool span, its result on the next request's delta, and the call is the thread's
/// because that delta answers it. A call no delta of this thread answers is another thread's.
#[test]
fn a_call_belongs_to_the_thread_whose_delta_answers_it() {
    let r1 = request("r1", 10, &[user("weather?", 10), reply("checking", 11)]);
    let r2 = request("r2", 20, &[result("t1", "72F", 20), reply("it is 72F", 21)]);
    let calls = [
        tool("s1", 12, "t1", "weather"),
        tool("s9", 13, "t9", "search"),
    ];
    assert_eq!(
        composed(&r2, &[r1, r2.clone()], &calls),
        [
            "user: weather?",
            "assistant: checking",
            "assistant: call weather t1",
            "tool: result Some(\"t1\")",
            "assistant: it is 72F",
        ]
    );
    // Without its tool span the call leaves the composed request, and nothing stands in for it.
    let r1 = request("r1", 10, &[user("weather?", 10), reply("checking", 11)]);
    assert_eq!(
        composed(&r2, &[r1, r2.clone()], &calls[1..]),
        [
            "user: weather?",
            "assistant: checking",
            "tool: result Some(\"t1\")",
            "assistant: it is 72F"
        ]
    );
}

/// A batch keeps its calls in the order their spans started, whatever order the results came back in.
#[test]
fn a_batch_keeps_its_calls_in_their_declared_order() {
    let r1 = request("r1", 10, &[user("both?", 10)]);
    let r2 = request("r2", 20, &[result("t2", "b", 20), result("t1", "a", 20)]);
    let calls = [
        tool("s2", 13, "t2", "second"),
        tool("s1", 12, "t1", "first"),
    ];
    assert_eq!(
        composed(&r2, &[r1, r2.clone()], &calls),
        [
            "user: both?",
            "assistant: call first t1",
            "assistant: call second t2",
            "tool: result Some(\"t2\")",
            "tool: result Some(\"t1\")",
        ]
    );
}

/// Dropping a predecessor's row drops what only that row carried, and invents nothing in its place.
#[test]
fn a_missing_predecessor_leaves_only_what_it_carried_out() {
    let [r1, r2, r3] = three_turns();
    assert_eq!(
        composed(&r3, &[r2.clone(), r3.clone()], &[]),
        [
            "system: sys",
            "user: q2",
            "system: tok",
            "assistant: a2",
            "user: q3",
            "system: tok",
            "assistant: a3"
        ],
        "without the first request"
    );
    assert_eq!(
        composed(&r3, &[r1, r3.clone()], &[]),
        [
            "system: sys",
            "user: q1",
            "system: env",
            "assistant: a1",
            "user: q3",
            "system: tok",
            "assistant: a3"
        ],
        "without the middle one"
    );
}

/// Moving a reminder from one request's delta to another's moves it in the composed order.
#[test]
fn a_moved_reminder_moves_in_the_composed_order() {
    let r1 = request(
        "r1",
        10,
        &[user("q1", 10), reminder("late", 10), reply("a1", 11)],
    );
    let r2 = request("r2", 20, &[user("q2", 20), reply("a2", 21)]);
    assert_eq!(
        composed(&r2, &[r1, r2.clone()], &[]),
        [
            "user: q1",
            "system: late",
            "assistant: a1",
            "user: q2",
            "assistant: a2"
        ]
    );
    let r1 = request("r1", 10, &[user("q1", 10), reply("a1", 11)]);
    let r2 = request(
        "r2",
        20,
        &[user("q2", 20), reminder("late", 20), reply("a2", 21)],
    );
    assert_eq!(
        composed(&r2, &[r1, r2.clone()], &[]),
        [
            "user: q1",
            "assistant: a1",
            "user: q2",
            "system: late",
            "assistant: a2"
        ]
    );
}

/// A break composes nothing past it: two requests at one position whose content differs, or a request that
/// failed, leave the target its own payload.
#[test]
fn a_break_composes_nothing_past_it() {
    let [r1, r2, r3] = three_turns();
    let own = composed(&r3, std::slice::from_ref(&r3), &[]);
    let tied = request("r2b", 20, &[user("other", 20)]);
    assert_eq!(
        composed(&r3, &[r1.clone(), r2.clone(), tied, r3.clone()], &[]),
        own
    );
    let mut failed = r2.clone();
    failed.status_code = Some(status::ERROR.to_string());
    assert_eq!(composed(&r3, &[r1, failed, r3.clone()], &[]), own);
}

/// A question asked again is a new turn: a delta restates the history only when every role it carries restates
/// all of it, and the reminder beside the repeated question is new.
#[test]
fn a_question_asked_again_is_a_new_turn() {
    let r1 = request(
        "r1",
        10,
        &[user("again?", 10), reminder("env", 10), reply("yes", 11)],
    );
    let r2 = request(
        "r2",
        20,
        &[user("again?", 20), reminder("tok", 20), reply("yes", 21)],
    );
    assert_eq!(
        composed(&r2, &[r1, r2.clone()], &[]),
        [
            "user: again?",
            "system: env",
            "assistant: yes",
            "user: again?",
            "system: tok",
            "assistant: yes"
        ]
    );
}

/// The cost of composing the last request of a long thread, measured rather than argued: run with
/// `cargo test --release -p sideseat-domain --lib composing_a_long_thread -- --ignored --nocapture`.
///
/// Two shapes. A live session exports one turn per request, so the rows read are the answer's own bytes. A
/// resumed process restates the whole history in every delta, so its rows grow with the square of the thread.
#[test]
#[ignore = "a measurement, not a check"]
fn composing_a_long_thread() {
    let measure = |requests: usize, restating: bool| {
        let rows: Vec<MessageSpanRow> = (0..requests)
            .map(|n| {
                let second = (n as i64) * 10;
                let mut messages = vec![frame(&"s".repeat(500), second)];
                let turns = if restating { 0..=n } else { n..=n };
                messages.extend(
                    turns
                        .clone()
                        .map(|k| user(&format!("question {k} {}", "q".repeat(80)), second)),
                );
                messages.extend(turns.map(|k| reminder(&format!("reminder {k}"), second)));
                messages.push(reply(
                    &format!("answer {n} {}", "a".repeat(300)),
                    second + 1,
                ));
                request(&format!("r{n:05}"), second, &messages)
            })
            .collect();
        let bytes: usize = rows.iter().map(|row| row.messages_json.len()).sum();
        // The floor for any reading of these bodies: decoding them, with no normalisation at all.
        let decoding = std::time::Instant::now();
        let decoded: usize = rows
            .iter()
            .map(|row| {
                serde_json::from_str::<Vec<RawMessage>>(&row.messages_json)
                    .map_or(0, |messages| messages.len())
            })
            .sum();
        let decoded_in = decoding.elapsed();
        let target = rows.last().expect("a thread").clone();
        let started = std::time::Instant::now();
        let view = compose(RequestContextRows {
            target: vec![target],
            thread: rows,
            calls: Vec::new(),
        });
        let elapsed = started.elapsed();
        let answer: usize = view
            .messages
            .iter()
            .map(|block| serde_json::to_string(&block.content).map_or(0, |text| text.len()))
            .sum();
        println!(
            "{requests} requests, restating={restating}: {} blocks, {answer} answer bytes, {bytes} row bytes read, \
             composed in {elapsed:?}; decoding the {decoded} stored messages alone takes {decoded_in:?}",
            view.messages.len()
        );
    };
    measure(5_000, false);
    measure(1_000, true);
}

/// **A thread longer than one view may read is composed from what fits, and says so.** The budget keeps the
/// newest requests, which is what the view is mostly made of, and the target's own rows are never dropped.
#[test]
fn a_thread_past_the_read_budget_is_composed_from_what_fits() {
    let rows: Vec<MessageSpanRow> = (0..6)
        .map(|n| {
            let second = n * 10;
            request(
                &format!("r{n}"),
                second,
                &[
                    user(&format!("q{n}"), second),
                    reply(&format!("a{n}"), second + 1),
                ],
            )
        })
        .collect();
    let target = rows.last().expect("a thread").clone();
    let whole = compose(RequestContextRows {
        target: vec![target.clone()],
        thread: rows.clone(),
        calls: Vec::new(),
    });
    assert_eq!(whole.metadata.composed_from_requests, 5);
    assert!(!whole.metadata.composition_truncated);
    assert_eq!(shown(&whole).len(), 12, "six requests, two blocks each");

    // A budget of three rows' worth: the three newest requests compose, and the answer says it is cut.
    let budget = rows.iter().map(|row| row.messages_json.len()).take(3).sum();
    let cut = compose_within(
        RequestContextRows {
            target: vec![target],
            thread: rows,
            calls: Vec::new(),
        },
        budget,
    );
    assert!(
        cut.metadata.composition_truncated,
        "the budget fired and is reported"
    );
    assert_eq!(cut.metadata.composed_from_requests, 2);
    assert_eq!(
        shown(&cut),
        [
            "user: q3",
            "assistant: a3",
            "user: q4",
            "assistant: a4",
            "user: q5",
            "assistant: a5"
        ],
        "the newest requests that fit, and the target's own payload"
    );
}

/// **A resumed process's restated items are never normalised.** The raw pass drops them by their stored bytes,
/// per carrier, before anything becomes a message - and its answer is the same as the role rule's.
#[test]
fn a_restating_delta_is_skipped_before_normalisation() {
    let rows: Vec<MessageSpanRow> = (0..4)
        .map(|n| {
            let second = n * 10;
            let mut messages: Vec<RawMessage> = Vec::new();
            for k in 0..=n {
                messages.push(user(&format!("q{k}"), second));
            }
            for k in 0..=n {
                messages.push(reminder(&format!("env{k}"), second));
            }
            messages.push(reply(&format!("a{n}"), second + 1));
            request(&format!("r{n}"), second, &messages)
        })
        .collect();
    let target = rows.last().expect("a thread").clone();
    let view = compose(RequestContextRows {
        target: vec![target],
        thread: rows,
        calls: Vec::new(),
    });
    assert_eq!(
        shown(&view),
        [
            "user: q0",
            "system: env0",
            "assistant: a0",
            "user: q1",
            "system: env1",
            "assistant: a1",
            "user: q2",
            "system: env2",
            "assistant: a2",
            "user: q3",
            "system: env3",
            "assistant: a3",
        ],
        "each turn once, in thread order"
    );
}
