use super::*;
use crate::sideml::provenance::PositionPath;
use crate::sideml::types::{ChatRole, ContentBlock};
use chrono::{DateTime, TimeZone, Utc};
use sideseat_ports::types::MessageCategory;

const END: &str = "output.value";

fn at(seconds: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(1_700_000_000 + seconds, 0).unwrap()
}

fn block(span: &str, attribute: &str, message_index: i32, recorded: i64) -> BlockEntry {
    BlockEntry {
        scope_version: None,
        span_name: None,
        scope_name: None,
        position: PositionPath::default(),
        entry_type: "text".to_string(),
        content: ContentBlock::Text {
            text: format!("{span}/{message_index}"),
            citations: Vec::new(),
        },
        role: ChatRole::Assistant,
        trace_id: "trace1".to_string(),
        span_id: span.to_string(),
        session_id: None,
        message_index,
        entry_index: 0,
        parent_span_id: None,
        span_path: vec![span.to_string()],
        timestamp: at(recorded),
        order_time: at(recorded),
        occurrence_ordinal: 0,
        observation_type: None,
        model: None,
        provider: None,
        name: None,
        finish_reason: None,
        tool_use_id: None,
        tool_name: None,
        tokens: None,
        cost: None,
        status_code: None,
        is_error: false,
        source_type: "attribute".to_string(),
        event_name: None,
        source_attribute: Some(attribute.to_string()),
        category: MessageCategory::GenAIAssistantMessage,
        content_hash: format!("{span}{message_index}"),
        is_semantic: true,
        uses_span_end: false,
        is_history: false,
        is_cross_trace_history: false,
        yields: false,
        marker_report: Default::default(),
        tool_use_id_correlated: false,
        promoted_to_span_output: false,
        is_rendering: false,
        request_thread: String::new(),
        declared_direction: None,
    }
}

/// Two spans, each open from 10s to 20s.
fn spans() -> HashMap<String, SpanTimestamps> {
    ["a", "b"]
        .into_iter()
        .map(|span| {
            (
                span.to_string(),
                SpanTimestamps {
                    span_start: at(10),
                    span_end: Some(at(20)),
                },
            )
        })
        .collect()
}

/// The date each test declares, by attribute: `start` by its span's start, the span's output (`output.value`, an
/// output carrier the assets classify) by its end, every other carrier its own time.
fn by_attribute(block: &BlockEntry) -> CarrierDate {
    match block.source_attribute.as_deref() {
        Some("start") => CarrierDate::SpanStart,
        Some(END) => CarrierDate::SpanEnd,
        _ => CarrierDate::Own,
    }
}

fn times_of(blocks: &[BlockEntry]) -> Vec<DateTime<Utc>> {
    blocks.iter().map(|b| b.timestamp).collect()
}

#[test]
fn a_carrier_dated_by_its_span_start_dates_every_observation_there() {
    let mut blocks = vec![
        block("a", "start", 0, 18),
        block("a", "start", 1, 19),
        block("a", "own", 0, 18),
    ];
    at_span_start(&mut blocks, &spans(), by_attribute);
    assert_eq!(times_of(&blocks), vec![at(10), at(10), at(18)]);
}

#[test]
fn a_sole_return_is_dated_at_its_span_end() {
    let mut blocks = vec![block("a", END, 0, 10), block("a", "own", 0, 10)];
    at_span_end(&mut blocks, &spans(), by_attribute);
    assert_eq!(times_of(&blocks), vec![at(20), at(10)]);
}

#[test]
fn a_carrier_re_listing_several_keeps_their_own_times() {
    let mut blocks = vec![block("a", END, 0, 10), block("a", END, 1, 11)];
    at_span_end(&mut blocks, &spans(), by_attribute);
    assert_eq!(times_of(&blocks), vec![at(10), at(11)]);
}

#[test]
fn a_re_delivered_copy_is_one_place_and_dated_as_the_first() {
    let mut blocks = vec![block("a", END, 0, 10), block("a", END, 0, 10)];
    at_span_end(&mut blocks, &spans(), by_attribute);
    assert_eq!(times_of(&blocks), vec![at(20), at(20)]);
}

#[test]
fn each_span_s_carrier_is_counted_on_its_own() {
    // One observation on each of two spans: each is its span's sole return. A second trace reusing the span id is
    // another carrier as well.
    let mut other_trace = block("a", END, 1, 12);
    other_trace.trace_id = "trace2".to_string();
    let mut blocks = vec![block("a", END, 0, 10), block("b", END, 0, 11), other_trace];
    at_span_end(&mut blocks, &spans(), by_attribute);
    assert_eq!(times_of(&blocks), vec![at(20), at(20), at(20)]);
}

#[test]
fn a_span_with_no_recorded_end_or_no_times_keeps_the_own_time() {
    let mut open = spans();
    open.get_mut("a").unwrap().span_end = None;
    let mut blocks = vec![
        block("a", END, 0, 10),
        block("c", END, 0, 11),
        block("c", "start", 1, 12),
    ];
    at_span_end(&mut blocks, &open, by_attribute);
    at_span_start(&mut blocks, &open, by_attribute);
    assert_eq!(times_of(&blocks), vec![at(10), at(11), at(12)]);
}

#[test]
fn a_date_moves_the_time_and_nothing_else() {
    let mut blocks = vec![block("a", END, 0, 10), block("b", "start", 0, 15)];
    blocks[1].is_history = true;
    let before: Vec<(bool, bool, bool)> = blocks
        .iter()
        .map(|b| (b.uses_span_end, b.is_history, b.promoted_to_span_output))
        .collect();
    at_span_end(&mut blocks, &spans(), by_attribute);
    at_span_start(&mut blocks, &spans(), by_attribute);
    let after: Vec<(bool, bool, bool)> = blocks
        .iter()
        .map(|b| (b.uses_span_end, b.is_history, b.promoted_to_span_output))
        .collect();
    assert_eq!(before, after);
    assert_eq!(times_of(&blocks), vec![at(20), at(10)]);
}

#[test]
fn the_assets_declare_what_dates_their_carriers() {
    let mut call = block("a", "own", 0, 18);
    call.source_type = "event".to_string();
    call.source_attribute = None;
    call.event_name = Some("codex.tool_result.call".to_string());
    assert_eq!(declared(&call), CarrierDate::SpanStart);
    let unclassified = block("a", "some.carrier.nobody.declared", 0, 10);
    assert_eq!(declared(&unclassified), CarrierDate::Own);
}

#[test]
fn a_span_time_that_is_no_moment_leaves_the_own_time() {
    // An omitted start arrives as the epoch; a start after the record, or an end before it, cannot date it.
    let mut times = spans();
    times.get_mut("a").unwrap().span_start = DateTime::<Utc>::UNIX_EPOCH;
    times.insert(
        "b".to_string(),
        SpanTimestamps {
            span_start: at(30),
            span_end: Some(at(5)),
        },
    );
    let mut blocks = vec![
        block("a", "start", 0, 18),
        block("b", "start", 0, 18),
        block("b", END, 1, 18),
    ];
    at_span_start(&mut blocks, &times, by_attribute);
    at_span_end(&mut blocks, &times, by_attribute);
    assert_eq!(times_of(&blocks), vec![at(18), at(18), at(18)]);
}

#[test]
fn only_what_the_span_returned_is_dated_at_its_end() {
    // A carrier's one observation that is a re-listed prompt happened before the span: a reading declaring it
    // input, or a user's or system turn in an output snapshot, keeps its own time.
    let mut input = block("a", END, 0, 10);
    input.declared_direction = Some(crate::rules::schema::ReadingDirection::Input);
    let mut user = block("b", END, 0, 11);
    user.role = ChatRole::User;
    let mut system = block("a", END, 0, 12);
    system.role = ChatRole::System;
    system.trace_id = "trace2".to_string();
    let mut blocks = vec![input, user, system, block("b", END, 0, 13)];
    blocks[3].trace_id = "trace3".to_string();
    at_span_end(&mut blocks, &spans(), by_attribute);
    assert_eq!(times_of(&blocks), vec![at(10), at(11), at(12), at(20)]);
}
