//! Tests for tool call/result correlation.
//!
//! Each case corresponds to a rule in `correlate.rs`. The ADK case is the defect that
//! motivated the stage: real Google ADK captures carried a `tool_use_id` on every tool result
//! that referenced a call id no call had ever emitted.

use chrono::{TimeZone, Utc};
use serde_json::json;

use super::*;
use crate::data::types::MessageCategory;
use crate::domain::sideml::types::ChatRole;

fn base(trace_id: &str, entry_type: &str, content: ContentBlock, role: ChatRole) -> BlockEntry {
    BlockEntry {
        entry_type: entry_type.to_string(),
        content,
        role,
        trace_id: trace_id.to_string(),
        span_id: "span-1".to_string(),
        session_id: None,
        message_index: 0,
        entry_index: 0,
        parent_span_id: None,
        span_path: vec!["span-1".to_string()],
        timestamp: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
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
        source_type: "event".to_string(),
        event_name: None,
        source_attribute: None,
        category: MessageCategory::GenAIUserMessage,
        content_hash: "test".to_string(),
        is_semantic: true,
        uses_span_end: false,
        is_history: false,
    }
}

fn call(trace: &str, id: Option<&str>, name: &str, input: serde_json::Value) -> BlockEntry {
    base(
        trace,
        "tool_use",
        ContentBlock::ToolUse {
            id: id.map(str::to_owned),
            name: name.to_string(),
            input,
        },
        ChatRole::Assistant,
    )
}

fn result(trace: &str, id: Option<&str>, name: Option<&str>, text: &str) -> BlockEntry {
    base(
        trace,
        "tool_result",
        ContentBlock::ToolResult {
            tool_use_id: id.map(str::to_owned),
            name: name.map(str::to_owned),
            content: json!([{"type": "text", "text": text}]),
            is_error: false,
        },
        ChatRole::Tool,
    )
}

fn resolved_id(block: &BlockEntry) -> Option<String> {
    match &block.content {
        ContentBlock::ToolResult { tool_use_id, .. } => tool_use_id.clone(),
        _ => None,
    }
}

/// The Google ADK shape: the call carries an id, the result carries only the tool name.
#[test]
fn names_a_result_after_its_call() {
    let mut blocks = vec![
        call(
            "t1",
            Some("call-1"),
            "temperature_forecast",
            json!({"city": "NYC"}),
        ),
        result("t1", None, Some("temperature_forecast"), "25C"),
    ];
    correlate_tool_results(&mut blocks);
    assert_eq!(resolved_id(&blocks[1]).as_deref(), Some("call-1"));
}

/// Rule 1: a provider-supplied id is authoritative and must survive untouched.
#[test]
fn never_overwrites_a_real_id() {
    let mut blocks = vec![
        call("t1", Some("call-1"), "calc", json!({})),
        result("t1", Some("provider-id"), Some("calc"), "42"),
    ];
    correlate_tool_results(&mut blocks);
    assert_eq!(resolved_id(&blocks[1]).as_deref(), Some("provider-id"));
}

/// Rule 2: correlation never crosses a trace boundary.
#[test]
fn does_not_match_across_traces() {
    let mut blocks = vec![
        call("t1", Some("call-1"), "calc", json!({})),
        result("t2", None, Some("calc"), "42"),
    ];
    correlate_tool_results(&mut blocks);
    assert_eq!(
        resolved_id(&blocks[1]),
        None,
        "a call in one trace must not answer a result in another"
    );
}

/// Two different tools called in parallel: each result matches by name, in either order.
#[test]
fn matches_parallel_calls_by_name_in_any_result_order() {
    let mut blocks = vec![
        call("t1", Some("call-temp"), "temperature", json!({})),
        call("t1", Some("call-precip"), "precipitation", json!({})),
        result("t1", None, Some("precipitation"), "rain"),
        result("t1", None, Some("temperature"), "25C"),
    ];
    correlate_tool_results(&mut blocks);
    assert_eq!(resolved_id(&blocks[2]).as_deref(), Some("call-precip"));
    assert_eq!(resolved_id(&blocks[3]).as_deref(), Some("call-temp"));
}

/// Sequential calls to the SAME tool with different arguments: each result takes the nearest
/// unclaimed call, so the pairs do not cross.
#[test]
fn matches_sequential_same_name_calls_without_crossing() {
    let mut blocks = vec![
        call("t1", Some("call-a"), "lookup", json!({"q": "a"})),
        result("t1", None, Some("lookup"), "answer-a"),
        call("t1", Some("call-b"), "lookup", json!({"q": "b"})),
        result("t1", None, Some("lookup"), "answer-b"),
    ];
    correlate_tool_results(&mut blocks);
    assert_eq!(resolved_id(&blocks[1]).as_deref(), Some("call-a"));
    assert_eq!(resolved_id(&blocks[3]).as_deref(), Some("call-b"));
}

/// A call is claimed by at most one result: the second result finds nothing rather than
/// reusing an id already taken.
#[test]
fn a_call_is_claimed_only_once() {
    let mut blocks = vec![
        call("t1", Some("call-1"), "calc", json!({})),
        result("t1", None, Some("calc"), "42"),
        result("t1", None, Some("calc"), "43"),
    ];
    correlate_tool_results(&mut blocks);
    assert_eq!(resolved_id(&blocks[1]).as_deref(), Some("call-1"));
    assert_eq!(
        resolved_id(&blocks[2]),
        None,
        "a second result must not reuse a call already accounted for"
    );
}

/// Rule 5: an orphan result keeps no id. A fabricated reference is worse than none, because it
/// looks like a working one — which is exactly what the old synthetic ids did.
#[test]
fn leaves_an_orphan_result_without_an_id() {
    let mut blocks = vec![result("t1", None, Some("never_called"), "?")];
    correlate_tool_results(&mut blocks);
    assert_eq!(resolved_id(&blocks[0]), None);
    // And it is still present: dropping it would hide that the framework reported a result.
    assert_eq!(blocks.len(), 1);
}

/// A result with neither id nor name has nothing to match on and is left alone.
#[test]
fn leaves_a_nameless_result_alone() {
    let mut blocks = vec![
        call("t1", Some("call-1"), "calc", json!({})),
        result("t1", None, None, "42"),
    ];
    correlate_tool_results(&mut blocks);
    assert_eq!(resolved_id(&blocks[1]), None);
}

/// A result appearing BEFORE any call is not matched: correlation only looks backwards, so it
/// cannot invent a pairing from a call that had not happened yet.
#[test]
fn does_not_match_a_result_that_precedes_every_call() {
    let mut blocks = vec![
        result("t1", None, Some("calc"), "42"),
        call("t1", Some("call-1"), "calc", json!({})),
    ];
    correlate_tool_results(&mut blocks);
    assert_eq!(resolved_id(&blocks[0]), None);
}

/// A call with no id of its own cannot lend one.
#[test]
fn ignores_calls_without_ids() {
    let mut blocks = vec![
        call("t1", None, "calc", json!({})),
        result("t1", None, Some("calc"), "42"),
    ];
    correlate_tool_results(&mut blocks);
    assert_eq!(resolved_id(&blocks[1]), None);
}
