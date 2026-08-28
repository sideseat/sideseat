//! Tests for tool call/result correlation.
//!
//! Each case corresponds to a rule in `correlate.rs`. The ADK case is the defect that
//! motivated the stage: real Google ADK captures carried a `tool_use_id` on every tool result
//! that referenced a call id no call had ever emitted.

use crate::domain::sideml::provenance::PositionPath;
use chrono::{TimeZone, Utc};
use serde_json::json;

use super::*;
use crate::data::types::MessageCategory;
use crate::domain::sideml::types::ChatRole;

fn base(trace_id: &str, entry_type: &str, content: ContentBlock, role: ChatRole) -> BlockEntry {
    BlockEntry {
        position: PositionPath::default(),
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
        tool_use_id_correlated: false,
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

/// Concurrent calls to the same tool pair in call order, not in reverse.
///
/// Taking the *nearest* preceding call reversed every parallel group: ADK's three parallel
/// `generate_image` calls had their results attached back to front, so each image pointed at
/// another prompt. Both Gemini and the OpenAI-shaped protocols return results in request order.
#[test]
fn pairs_concurrent_same_name_calls_in_call_order() {
    let mut blocks = vec![
        call("t1", Some("call-a"), "lookup", json!({"q": "a"})),
        call("t1", Some("call-b"), "lookup", json!({"q": "b"})),
        call("t1", Some("call-c"), "lookup", json!({"q": "c"})),
        result("t1", None, Some("lookup"), "answer-a"),
        result("t1", None, Some("lookup"), "answer-b"),
        result("t1", None, Some("lookup"), "answer-c"),
    ];
    correlate_tool_results(&mut blocks);
    assert_eq!(resolved_id(&blocks[3]).as_deref(), Some("call-a"));
    assert_eq!(resolved_id(&blocks[4]).as_deref(), Some("call-b"));
    assert_eq!(resolved_id(&blocks[5]).as_deref(), Some("call-c"));
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

/// A correlated id is withdrawn when its call does not survive to the response.
///
/// Correlation always links to a call in the same block list, but dedup and history marking run
/// afterwards and can remove it - and a result referencing a block the caller never receives is
/// the dangling reference this module exists to prevent, reached from the other side. It cost a
/// real invariant failure in `adk/tool_use` to notice.
#[test]
fn withdraws_a_correlated_id_whose_call_was_dropped() {
    let mut blocks = vec![
        call("t1", Some("call-a"), "lookup", json!({"q": "a"})),
        result("t1", None, Some("lookup"), "answer-a"),
    ];
    correlate_tool_results(&mut blocks);
    assert_eq!(resolved_id(&blocks[1]).as_deref(), Some("call-a"));

    // Whatever dedup does to the call, the result must not keep pointing at it.
    let without_call = withdraw_unbacked_ids(vec![blocks[1].clone()]);
    assert_eq!(
        resolved_id(&without_call[0]),
        None,
        "the result kept an id for a call the response does not contain"
    );
    assert!(!without_call[0].tool_use_id_correlated);

    // With the call still present, nothing is withdrawn.
    let intact = withdraw_unbacked_ids(blocks.clone());
    assert_eq!(resolved_id(&intact[1]).as_deref(), Some("call-a"));
}

/// A provider's own id is never withdrawn, even with no matching call in scope: a span view
/// legitimately holds a result whose call is in a sibling span.
#[test]
fn keeps_a_provider_id_with_no_call_in_scope() {
    let blocks = withdraw_unbacked_ids(vec![result(
        "t1",
        Some("call-elsewhere"),
        Some("lookup"),
        "answer",
    )]);
    assert_eq!(
        resolved_id(&blocks[0]).as_deref(),
        Some("call-elsewhere"),
        "withdrawing a provider id would break the span view, where the call is out of scope"
    );
}

/// Documents the failure mode FIFO trades for: a call whose result never arrived.
///
/// If a call is never answered - the tool errored, the run was cut short - it stays unclaimed, and
/// the next result for that tool name takes it. Nearest-preceding would get this one right and
/// every parallel group wrong, which is the worse trade: an unanswered call is the exception,
/// parallel calls are routine, and the reversal was silently wrong in four fixtures.
///
/// Recorded as a test rather than a comment so the day a signal for this appears - a provider id
/// on the result, an explicit error status on the call - it is clear what changes.
#[test]
fn an_unanswered_call_claims_the_next_result_of_the_same_name() {
    let mut blocks = vec![
        call(
            "t1",
            Some("call-never-answered"),
            "lookup",
            json!({"q": "a"}),
        ),
        call("t1", Some("call-answered"), "lookup", json!({"q": "b"})),
        result("t1", None, Some("lookup"), "answer-b"),
    ];
    correlate_tool_results(&mut blocks);
    assert_eq!(
        resolved_id(&blocks[2]).as_deref(),
        Some("call-never-answered"),
        "known limit: with one result for two calls, the older call is assumed to be the \
         answered one - the name and response alone say nothing else"
    );
}

/// Withdrawing an id must not leave behind the duplicate dedup was relying on the id to tell
/// apart.
///
/// Withdrawal runs after dedup, which treats two results with the same text and different call
/// ids as two messages. Clear both ids and they become one message reported twice - and dedup has
/// already run, so nothing else would catch it.
#[test]
fn collapses_results_that_become_identical_after_withdrawal() {
    let mut blocks = vec![
        call("t1", Some("call-a"), "lookup", json!({"q": "a"})),
        call("t1", Some("call-b"), "lookup", json!({"q": "b"})),
        result("t1", None, Some("lookup"), "same text"),
        result("t1", None, Some("lookup"), "same text"),
    ];
    correlate_tool_results(&mut blocks);
    assert_eq!(resolved_id(&blocks[2]).as_deref(), Some("call-a"));
    assert_eq!(resolved_id(&blocks[3]).as_deref(), Some("call-b"));

    // Both calls dropped by dedup or history marking: the two results are now the same message.
    let surviving = withdraw_unbacked_ids(vec![blocks[2].clone(), blocks[3].clone()]);
    assert_eq!(
        surviving.len(),
        1,
        "two results that are indistinguishable once their ids are withdrawn were both returned"
    );
    assert_eq!(resolved_id(&surviving[0]), None);

    // With call-a surviving, result A keeps its id and result B loses one - so the two are still
    // distinguishable and both are returned (three blocks in, three out).
    let surviving = withdraw_unbacked_ids(vec![
        blocks[0].clone(),
        blocks[2].clone(),
        blocks[3].clone(),
    ]);
    assert_eq!(
        surviving.len(),
        3,
        "the result whose call survived must keep its id, so both results remain distinct"
    );
    assert_eq!(resolved_id(&surviving[1]).as_deref(), Some("call-a"));
    assert_eq!(resolved_id(&surviving[2]), None);
}

/// A result that arrives with its own id claims that call, so a later id-less result cannot adopt
/// it.
///
/// Leaving the call unclaimed meant the second result took an id that was already answered. Two
/// results with one id are one message to dedup, which identifies a result by its id, so one of
/// them was dropped no matter what it contained - and a framework only has to supply ids for some
/// results and not others to hit it.
#[test]
fn a_result_with_its_own_id_claims_the_call_it_names() {
    let mut blocks = vec![
        call("t1", Some("call-a"), "lookup", json!({"q": "a"})),
        call("t1", Some("call-b"), "lookup", json!({"q": "b"})),
        result("t1", Some("call-a"), Some("lookup"), "answer-a"),
        result("t1", None, Some("lookup"), "answer-b"),
    ];
    correlate_tool_results(&mut blocks);
    assert_eq!(resolved_id(&blocks[2]).as_deref(), Some("call-a"));
    assert_eq!(
        resolved_id(&blocks[3]).as_deref(),
        Some("call-b"),
        "the id-less result adopted a call that had already answered"
    );
}

/// The collision repair must not depend on which of the two came first.
///
/// Dropping only the withdrawn block left both in place when the withdrawn one came first, and
/// dropped one when it came second. Position cannot decide which of two identical results is the
/// duplicate.
#[test]
fn collision_repair_is_order_independent() {
    let withdrawn_first = vec![
        // A withdrawn result: correlated, then its call disappeared.
        {
            let mut b = result("t1", Some("call-gone"), Some("lookup"), "same text");
            b.tool_use_id_correlated = true;
            b
        },
        // A result that never had an id at all.
        result("t1", None, Some("lookup"), "same text"),
    ];
    let withdrawn_second: Vec<_> = withdrawn_first.iter().rev().cloned().collect();

    for (label, blocks) in [
        ("withdrawn first", withdrawn_first),
        ("withdrawn second", withdrawn_second),
    ] {
        let surviving = withdraw_unbacked_ids(blocks);
        assert_eq!(
            surviving.len(),
            1,
            "{label}: the same pair must collapse to one either way round"
        );
    }
}

/// Claiming a call must claim every copy of it.
///
/// One call is flattened once per span that carries it, so `pending` holds the same call several
/// times. Marking only the first left the rest available, and the next id-less result adopted an id
/// that had already been answered - two results with one id, which dedup resolves by dropping one.
#[test]
fn claiming_a_call_claims_every_copy_of_it() {
    // The same call twice (as a bubbled-up event produces), then two results.
    let mut blocks = vec![
        call("t1", Some("call-a"), "lookup", json!({"q": "a"})),
        call("t1", Some("call-a"), "lookup", json!({"q": "a"})),
        result("t1", None, Some("lookup"), "answer-a"),
        result("t1", None, Some("lookup"), "answer-b"),
    ];
    correlate_tool_results(&mut blocks);
    assert_eq!(resolved_id(&blocks[2]).as_deref(), Some("call-a"));
    assert_eq!(
        resolved_id(&blocks[3]),
        None,
        "the second result adopted a call that had already been answered"
    );

    // Same via a provider-supplied id: the copies of that call must all be spent.
    let mut blocks = vec![
        call("t1", Some("call-a"), "lookup", json!({"q": "a"})),
        call("t1", Some("call-a"), "lookup", json!({"q": "a"})),
        result("t1", Some("call-a"), Some("lookup"), "answer-a"),
        result("t1", None, Some("lookup"), "answer-b"),
    ];
    correlate_tool_results(&mut blocks);
    assert_eq!(resolved_id(&blocks[3]), None);
}

/// Withdrawal must not merge results that only look alike.
///
/// The collision key is the block identity, which for a tool result covers the tool name and the
/// error flag, not just the text. Keyed on text alone, two tools both reporting "ok" - or a success
/// and a failure whose text matches - became one message once their ids were withdrawn.
#[test]
fn withdrawal_keeps_results_that_differ_by_tool_or_error() {
    let mut different_tools = vec![
        {
            let mut b = result("t1", Some("call-gone"), Some("lookup"), "ok");
            b.tool_use_id_correlated = true;
            b
        },
        result("t1", None, Some("write"), "ok"),
    ];
    // content_hash is set by the pipeline, so mirror what it would compute here.
    for block in &mut different_tools {
        block.content_hash = format!("{:016x}", super::super::compute_block_hash(&block.content));
    }
    assert_eq!(
        withdraw_unbacked_ids(different_tools).len(),
        2,
        "two different tools each reporting ok are two messages"
    );

    let mut success_and_failure = vec![
        {
            let mut b = result("t1", Some("call-gone"), Some("lookup"), "ok");
            b.tool_use_id_correlated = true;
            b
        },
        {
            let mut b = result("t1", None, Some("lookup"), "ok");
            if let ContentBlock::ToolResult { is_error, .. } = &mut b.content {
                *is_error = true;
            }
            b
        },
    ];
    for block in &mut success_and_failure {
        block.content_hash = format!("{:016x}", super::super::compute_block_hash(&block.content));
    }
    assert_eq!(
        withdraw_unbacked_ids(success_and_failure).len(),
        2,
        "a success and a failure with matching text are two messages"
    );
}
