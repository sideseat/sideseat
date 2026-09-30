use super::*;
use crate::sideml::provenance::PositionPath;
use crate::sideml::types::{ChatRole, ContentBlock};
use chrono::TimeZone;
use sideseat_ports::types::MessageCategory;

fn block(span: &str, text: &str) -> BlockEntry {
    BlockEntry {
        scope_version: None,
        span_name: None,
        scope_name: None,
        position: PositionPath::default(),
        entry_type: "text".to_string(),
        content: ContentBlock::Text {
            text: text.to_string(),
        },
        role: ChatRole::User,
        trace_id: "trace-1".to_string(),
        span_id: span.to_string(),
        session_id: None,
        message_index: 0,
        entry_index: 0,
        parent_span_id: None,
        span_path: vec![span.to_string()],
        timestamp: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
        order_time: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
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
        source_attribute: Some("carrier".to_string()),
        category: MessageCategory::GenAIUserMessage,
        content_hash: text.to_string(),
        is_semantic: true,
        uses_span_end: false,
        is_history: false,
        is_cross_trace_history: false,
        tool_use_id_correlated: false,
        promoted_to_span_output: false,
    }
}

fn evidence(carrier: usize, position: i32) -> OrderEvidence {
    OrderEvidence {
        accumulator: false,
        ancestor_spans: Vec::new(),
        emission: None,
        message_index: position,
        entry_index: 0,
        effective: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
        credible: false,
        span: 0,
        carrier,
        carrier_ordered: true,
        is_output: false,
        from_generation: false,
        detached_frame: false,
        tool_reference: None,
        input_family: None,
    }
}

#[test]
fn attribute_roots_share_one_carrier_but_event_roots_do_not() {
    let positioned =
        |source_attribute: Option<&str>, event_name: Option<&str>, root: usize, text: &str| {
            let mut entry = block("span-a", text);
            entry.source_attribute = source_attribute.map(str::to_string);
            entry.event_name = event_name.map(str::to_string);
            entry.source_type = if source_attribute.is_some() {
                "attribute"
            } else {
                "event"
            }
            .to_string();
            entry.position = PositionPath::root(root);
            entry
        };

    let attributes = vec![
        positioned(Some("gcp.vertex.agent.llm_request"), None, 3, "first"),
        positioned(Some("gcp.vertex.agent.llm_request"), None, 9, "second"),
    ];
    let attribute_evidence = collect_order_evidence(&attributes, &HashMap::new());
    assert_eq!(
        attribute_evidence[0].carrier, attribute_evidence[1].carrier,
        "one attribute value expanded into several roots is still one ordered payload"
    );

    let events = vec![
        positioned(None, Some("gen_ai.choice"), 3, "first"),
        positioned(None, Some("gen_ai.choice"), 9, "second"),
    ];
    let event_evidence = collect_order_evidence(&events, &HashMap::new());
    assert_ne!(
        event_evidence[0].carrier, event_evidence[1].carrier,
        "two events with the same name are distinct payload instances"
    );
}

/// Contradictory evidence still yields every block exactly once.
///
/// Two carriers state opposite orders for the same pair - carrier 0 says `a` then `b`, carrier 1
/// says `b` then `a` - which is a genuine contradiction in the telemetry and the only way the
/// resolver's stall branch is reached. That branch had **never executed in the test suite**: no
/// corpus fixture cycles at this constraint density, so the one path whose job is to keep a
/// contradiction from becoming a lost message was unverified.
///
/// What is asserted is the property that must survive a contradiction: the answer is still a total
/// order over exactly the survivors. Which of the two wins is deliberately not asserted - it is a
/// deterministic tie-break over a contradiction, not a fact about the conversation.
#[test]
fn contradictory_evidence_still_returns_every_block_exactly_once() {
    let survivors = vec![block("span-a", "a"), block("span-b", "b")];
    // Four observations: carrier 0 orders (a, b), carrier 1 orders (b, a).
    let evidence_set = vec![
        evidence(0, 0),
        evidence(0, 1),
        evidence(1, 0),
        evidence(1, 1),
    ];
    let lineage = vec![Some(0), Some(1), Some(1), Some(0)];

    let resolved = resolve(
        &evidence_set,
        &survivors,
        &lineage,
        &[0, 0],
        &HashMap::new(),
        Constraints::PRODUCTION,
    );

    assert_eq!(
        resolved.len(),
        survivors.len(),
        "a contradiction must not drop or duplicate a block"
    );
    let mut seen: Vec<&str> = resolved.iter().map(|b| b.span_id.as_str()).collect();
    seen.sort_unstable();
    assert_eq!(
        seen,
        vec!["span-a", "span-b"],
        "every survivor appears exactly once"
    );
}

#[test]
fn reused_call_ids_pair_by_repeat_ordinal() {
    let mut result_0 = block("result-0", "result-0");
    result_0.entry_type = "tool_result".to_string();
    result_0.role = ChatRole::Tool;
    result_0.tool_use_id = Some("reused".to_string());
    let mut call_1 = block("call-1", "call-1");
    call_1.entry_type = "tool_use".to_string();
    call_1.role = ChatRole::Assistant;
    call_1.tool_use_id = Some("reused".to_string());
    let mut result_1 = block("result-1", "result-1");
    result_1.entry_type = "tool_result".to_string();
    result_1.role = ChatRole::Tool;
    result_1.tool_use_id = Some("reused".to_string());
    let mut call_0 = block("call-0", "call-0");
    call_0.entry_type = "tool_use".to_string();
    call_0.role = ChatRole::Assistant;
    call_0.tool_use_id = Some("reused".to_string());
    let survivors = vec![result_0, call_1, result_1, call_0];
    let ordinals = [0, 1, 1, 0];

    let precedence = causal_precedence(&[], &survivors, &[], &ordinals);
    assert_eq!(precedence.successors_of(1), &[2]);
    assert_eq!(precedence.successors_of(3), &[0]);

    let resolved = resolve(
        &[],
        &survivors,
        &[],
        &ordinals,
        &HashMap::new(),
        Constraints::FULL,
    );
    assert_eq!(
        resolved
            .iter()
            .map(|block| block.span_id.as_str())
            .collect::<Vec<_>>(),
        vec!["call-1", "result-1", "call-0", "result-0"]
    );
}

#[test]
fn exact_tool_causality_overrides_a_same_span_dataflow_conflict() {
    let mut call = block("generation", "call");
    call.entry_type = "tool_use".to_string();
    call.role = ChatRole::Assistant;
    call.tool_use_id = Some("call-1".to_string());
    let mut result = block("generation", "result");
    result.entry_type = "tool_result".to_string();
    result.role = ChatRole::Tool;
    result.tool_use_id = Some("call-1".to_string());
    let survivors = vec![call, result];

    let mut call_evidence = evidence(0, 0);
    call_evidence.from_generation = true;
    call_evidence.is_output = true;
    let mut result_evidence = evidence(1, 1);
    result_evidence.from_generation = true;
    result_evidence.is_output = false;

    CYCLES_BROKEN_IN_TESTS.with(|count| *count.borrow_mut() = 0);
    let resolved = resolve(
        &[call_evidence, result_evidence],
        &survivors,
        &[Some(0), Some(1)],
        &[0, 0],
        &HashMap::new(),
        Constraints::PRODUCTION,
    );

    assert_eq!(
        CYCLES_BROKEN_IN_TESTS.with(|count| *count.borrow()),
        0,
        "the result cannot cause the call whose exact id it answers"
    );
    assert_eq!(
        resolved
            .iter()
            .map(|block| block.entry_type.as_str())
            .collect::<Vec<_>>(),
        vec!["tool_use", "tool_result"]
    );
}

/// A re-listing is only discounted on evidence from *below* it, and only when it is a re-listing.
///
/// The rule that fixed the aggregator ordering defect, and the two ways it must not fire. Both matter:
/// without the descendant requirement, "some other span also has this message" would discount every
/// replay, which is most of the corpus; without the both-sides requirement it would discount a turn's
/// intro text and the call it introduces, whose contraction is a documented repair.
#[test]
fn a_relisting_is_discounted_only_on_evidence_from_below_it() {
    // Three blocks of one turn: the assistant's answer, its call, and the result answering it.
    let turn_block = |role: ChatRole, entry: &str, span: &str, path: &[&str]| {
        let mut b = block(span, "x");
        b.role = role;
        b.entry_type = entry.to_string();
        b.span_path = path.iter().map(|s| s.to_string()).collect();
        b
    };
    let block = turn_block;
    let survivors = vec![
        block(ChatRole::Assistant, "text", "root", &["root"]),
        block(ChatRole::Assistant, "tool_use", "root", &["root"]),
        block(ChatRole::Tool, "tool_result", "root", &["root"]),
    ];
    // One accumulator instance on `root` claiming all three - the whole turn, both sides.
    let relisting = |accumulator: bool, ancestors: Vec<usize>, span: usize| {
        (0..3)
            .map(|i| OrderEvidence {
                emission: Some(0),
                message_index: i,
                entry_index: 0,
                effective: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
                credible: true,
                span,
                carrier: 0,
                carrier_ordered: true,
                is_output: true,
                from_generation: false,
                accumulator,
                ancestor_spans: ancestors.clone(),
                detached_frame: false,
                tool_reference: None,
                input_family: None,
            })
            .collect::<Vec<_>>()
    };
    // A witness instance, on a span whose ancestry includes `root` (span 0).
    let witness = |instance: usize, member: usize, ancestors: Vec<usize>| OrderEvidence {
        emission: Some(instance),
        message_index: member as i32,
        entry_index: 0,
        effective: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
        credible: true,
        span: 1,
        carrier: 1,
        carrier_ordered: true,
        is_output: true,
        from_generation: true,
        accumulator: false,
        ancestor_spans: ancestors,
        detached_frame: false,
        tool_reference: None,
        input_family: None,
    };
    let lineage = |n: usize| move |o: usize| Some(o % n);

    // Fully witnessed from below: discounted.
    let mut evidence = relisting(true, Vec::new(), 0);
    evidence.extend([
        witness(1, 0, vec![0]),
        witness(1, 1, vec![0]),
        witness(1, 2, vec![0]),
    ]);
    let of = lineage(3);
    assert!(
        redundant_relistings(&evidence, &survivors, &of).contains(&0),
        "a whole-turn re-listing whose every message a descendant produced has no authority over order"
    );

    // The same, witnessed by a *sibling* rather than a descendant: not discounted.
    let mut evidence = relisting(true, vec![9], 0);
    evidence.extend([
        witness(1, 0, vec![9]),
        witness(1, 1, vec![9]),
        witness(1, 2, vec![9]),
    ]);
    assert!(
        redundant_relistings(&evidence, &survivors, &of).is_empty(),
        "a sibling holding the same messages is a replay, and every replay would qualify"
    );

    // One witness missing: not discounted, because the re-listing is the only evidence for that message.
    let mut evidence = relisting(true, Vec::new(), 0);
    evidence.extend([witness(1, 0, vec![0]), witness(1, 1, vec![0])]);
    assert!(
        redundant_relistings(&evidence, &survivors, &of).is_empty(),
        "partial coverage keeps all of it: splitting one emission's order across two readings would \
         be a guess"
    );

    // A genuine emission - one side of a turn only - is never discounted, however well witnessed.
    let one_sided = vec![
        block(ChatRole::Assistant, "text", "root", &["root"]),
        block(ChatRole::Assistant, "tool_use", "root", &["root"]),
    ];
    let mut evidence: Vec<OrderEvidence> =
        relisting(true, Vec::new(), 0).into_iter().take(2).collect();
    evidence.extend([witness(1, 0, vec![0]), witness(1, 1, vec![0])]);
    let of_two = lineage(2);
    assert!(
        redundant_relistings(&evidence, &one_sided, &of_two).is_empty(),
        "a turn's intro text and the call it introduces are one response; contracting them is a \
         documented repair, not a re-listing"
    );
}
