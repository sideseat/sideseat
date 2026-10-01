
#[test]
fn test_autogen_tool_call_summary_nested_is_skipped() {
    // ToolCallSummaryMessage in nested {"message": {...}} format (autogen process spans).
    // Must be skipped — it concatenates tool results as Python repr() noise.
    let message_json = r#"{"message":{"id":"test-id","source":"weather_assistant","models_usage":null,"metadata":{},"created_at":"2026-01-01T00:00:00Z","content":"{'status': 'success', 'content': [{'json': {'city': 'NYC'}}]}","type":"ToolCallSummaryMessage","tool_calls":[],"results":[]}}"#;
    let attrs = make_attrs(&[("message", message_json)]);
    let mut messages = Vec::new();
    let found = try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "try_autogen should claim the span");
    assert_eq!(
        messages.len(),
        0,
        "ToolCallSummaryMessage should produce no messages"
    );
}

#[test]
fn test_autogen_tool_call_summary_direct_is_skipped() {
    // ToolCallSummaryMessage in direct format (no nesting)
    let message_json =
        r#"{"type":"ToolCallSummaryMessage","source":"agent","content":"tool result text"}"#;
    let attrs = make_attrs(&[("message", message_json)]);
    let mut messages = Vec::new();
    let found = try_autogen(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "try_autogen should claim the span");
    assert_eq!(
        messages.len(),
        0,
        "ToolCallSummaryMessage should produce no messages"
    );
}

#[test]
fn test_autogen_tool_execution_event_full_pipeline() {
    // Simulate the full extractor pipeline with real AutoGen process span attributes
    let message_json = r#"{"message":{"id":"4c7c875e","source":"memory_code_assistant","models_usage":null,"metadata":{},"created_at":"2026-02-12T01:18:52.894771Z","content":[{"content":"49\n","name":"execute_python_code","call_id":"toolu_01PtmR8iwDwcKnnJ7L12Lg1j","is_error":false}],"type":"ToolCallExecutionEvent"}}"#;
    let attrs = make_attrs(&[
        ("message", message_json),
        ("messaging.destination", "RoundRobinGroupChatManager_xyz"),
        ("messaging.operation", "process"),
        ("recipient_agent_class", "RoundRobinGroupChatManager"),
        ("recipient_agent_type", "RoundRobinGroupChatManager_xyz"),
        ("sender_agent_class", "ChatAgentContainer"),
        ("sender_agent_type", "memory_code_assistant_xyz"),
    ]);
    let mut messages = Vec::new();
    let mut tool_defs = Vec::new();
    extract_messages_from_attrs(
        &mut messages,
        &mut tool_defs,
        &attrs,
        "autogen process RoundRobinGroupChatManager_xyz",
        Utc::now(),
        ExtractionMode::FirstMatch,
        false,
    );

    assert_eq!(
        messages.len(),
        1,
        "ToolCallExecutionEvent should extract 1 tool_result message via full pipeline"
    );
    assert_eq!(messages[0].content["role"].as_str(), Some("tool"));
}

#[test]
fn test_autogen_tool_execution_event_via_extract_messages_for_span() {
    // Full end-to-end test using extract_messages_for_span (the actual ingestion entry point)
    use opentelemetry_proto::tonic::trace::v1::Span;

    let message_json = r#"{"message":{"id":"4c7c875e","source":"memory_code_assistant","models_usage":null,"metadata":{},"created_at":"2026-02-12T01:18:52.894771Z","content":[{"content":"49\n","name":"execute_python_code","call_id":"toolu_01PtmR8iwDwcKnnJ7L12Lg1j","is_error":false}],"type":"ToolCallExecutionEvent"}}"#;

    let otlp_span = Span {
        name: "autogen process RoundRobinGroupChatManager_xyz".to_string(),
        attributes: vec![
            make_kv("message", message_json),
            make_kv("messaging.destination", "RoundRobinGroupChatManager_xyz"),
            make_kv("messaging.operation", "process"),
            make_kv("recipient_agent_class", "RoundRobinGroupChatManager"),
            make_kv("recipient_agent_type", "RoundRobinGroupChatManager_xyz"),
            make_kv("sender_agent_class", "ChatAgentContainer"),
            make_kv("sender_agent_type", "memory_code_assistant_xyz"),
        ],
        events: vec![],
        ..Default::default()
    };

    let span_attrs = crate::otlp::extract_attributes(&otlp_span.attributes);
    let (messages, _tool_defs, _tool_names) = extract_messages_for_span(
        &otlp_span,
        &span_attrs,
        Utc::now(),
        ExtractionMode::FirstMatch,
    );

    assert_eq!(
        messages.len(),
        1,
        "extract_messages_for_span should extract 1 message from ToolCallExecutionEvent. Got: {:?}",
        messages.iter().map(|m| &m.content).collect::<Vec<_>>()
    );
    assert_eq!(messages[0].content["role"].as_str(), Some("tool"));
}

// ============================================================================
// Logfire request_data / response_data extraction
// ============================================================================

#[test]
fn test_logfire_chat_completions_request_response() {
    let attrs = make_attrs(&[
        (
            "request_data",
            r#"{"messages":[{"role":"user","content":"Hello"}],"model":"gpt-4o"}"#,
        ),
        (
            "response_data",
            r#"{"message":{"role":"assistant","content":"Hi!"},"usage":{"prompt_tokens":5}}"#,
        ),
    ]);

    let mut messages = Vec::new();
    let found = try_logfire_events(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract from request_data/response_data");
    assert_eq!(messages.len(), 2, "Should have request + response messages");

    // request_data stored as-is (with messages wrapper)
    assert!(
        matches!(&messages[0].source, MessageSource::Attribute { key, .. } if key == "request_data")
    );
    assert!(messages[0].content.get("messages").is_some());

    // response_data stored as-is (with message wrapper)
    assert!(
        matches!(&messages[1].source, MessageSource::Attribute { key, .. } if key == "response_data")
    );
    assert!(messages[1].content.get("message").is_some());
}

#[test]
fn test_logfire_responses_api_skipped() {
    // Responses API: request_data has no messages array
    let attrs = make_attrs(&[("request_data", r#"{"model":"gpt-4o","stream":true}"#)]);

    let mut messages = Vec::new();
    let found = try_logfire_events(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(
        !found,
        "Should not extract from Responses API request_data (no messages)"
    );
    assert!(messages.is_empty());
}

#[test]
fn test_logfire_events_take_precedence() {
    // When events attribute is present, request_data/response_data should be skipped
    let events_json = r#"[{"event.name":"gen_ai.user.message","content":"Hello!"}]"#;
    let attrs = make_attrs(&[
        ("events", events_json),
        (
            "request_data",
            r#"{"messages":[{"role":"user","content":"Hello!"}]}"#,
        ),
    ]);

    let mut messages = Vec::new();
    let found = try_logfire_events(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(
        messages.len(),
        1,
        "Should only have event-based message, not request_data"
    );
    assert!(
        matches!(&messages[0].source, MessageSource::Event { name, .. } if name == "gen_ai.user.message"),
        "Message should come from events, not request_data"
    );
}

#[test]
fn test_logfire_request_only() {
    // Only request_data, no response_data
    let attrs = make_attrs(&[(
        "request_data",
        r#"{"messages":[{"role":"user","content":"Hello"}],"model":"gpt-4o"}"#,
    )]);

    let mut messages = Vec::new();
    let found = try_logfire_events(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found);
    assert_eq!(messages.len(), 1, "Should have only request_data message");
}

#[test]
fn test_logfire_empty_messages_skipped() {
    // request_data with empty messages array should be skipped
    let attrs = make_attrs(&[("request_data", r#"{"messages":[],"model":"gpt-4o"}"#)]);

    let mut messages = Vec::new();
    let found = try_logfire_events(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(!found, "Should not extract when messages array is empty");
    assert!(messages.is_empty());
}

#[test]
fn test_logfire_streaming_response() {
    // Streaming response_data with combined_chunk_content
    let attrs = make_attrs(&[(
        "response_data",
        r#"{"combined_chunk_content":"Hello from streaming!","chunk_count":5}"#,
    )]);

    let mut messages = Vec::new();
    let found = try_logfire_events(&mut messages, &mut Vec::new(), &attrs, "", Utc::now());

    assert!(found, "Should extract streaming response_data");
    assert_eq!(messages.len(), 1);
    assert!(
        matches!(&messages[0].source, MessageSource::Attribute { key, .. } if key == "response_data")
    );
    assert!(messages[0].content.get("combined_chunk_content").is_some());
}

// ============================================================================
// CLAUDE CODE CLI (CLAUDE AGENT SDK)
// ============================================================================

#[test]
fn test_claude_code_strips_bracket_tags() {
    assert_eq!(
        strip_bracket_tag("[USER PROMPT]\nhello there"),
        "hello there"
    );
    assert_eq!(
        strip_bracket_tag("[TOOL INPUT: Glob]\n{\"pattern\":\"*.py\"}"),
        "{\"pattern\":\"*.py\"}"
    );
    // No marker, and a bracket that is not a marker, both pass through.
    assert_eq!(strip_bracket_tag("plain text"), "plain text");
    assert_eq!(strip_bracket_tag("[not a marker"), "[not a marker");
}

#[test]
fn test_claude_code_llm_request_messages() {
    // Verbatim shape captured from a real detailed-beta-tracing span.
    let attrs = make_attrs(&[
        ("span.type", "llm_request"),
        ("user_system_prompt", "Think the problem through."),
        (
            "new_context",
            "[USER PROMPT]\nHow many Python files are here?",
        ),
        ("response.model_output", "There are 2 Python files."),
    ]);
    let mut messages = Vec::new();
    let mut tools = Vec::new();
    assert!(try_claude_code(
        &mut messages,
        &mut tools,
        &attrs,
        "claude_code.llm_request",
        Utc::now()
    ));

    let roles: Vec<_> = messages
        .iter()
        .map(|m| m.content["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, vec!["system", "user", "assistant"]);
    assert_eq!(
        messages[1].content["content"],
        json!("How many Python files are here?")
    );
    assert_eq!(
        messages[2].content["content"],
        json!("There are 2 Python files.")
    );
}

#[test]
fn test_claude_code_tool_call_becomes_tool_use_block() {
    let attrs = make_attrs(&[
        ("span.type", "tool"),
        ("tool_name", "Glob"),
        (
            "tool_input",
            "[TOOL INPUT: Glob]\n{\"pattern\":\"**/*.py\"}",
        ),
        ("tool_use_id", "toolu_abc123"),
    ]);
    let mut messages = Vec::new();
    let mut tools = Vec::new();
    assert!(try_claude_code(
        &mut messages,
        &mut tools,
        &attrs,
        "claude_code.tool",
        Utc::now()
    ));

    assert_eq!(messages.len(), 1);
    let block = &messages[0].content["content"][0];
    assert_eq!(block["type"], json!("tool_use"));
    assert_eq!(block["name"], json!("Glob"));
    assert_eq!(block["id"], json!("toolu_abc123"));
    assert_eq!(block["input"]["pattern"], json!("**/*.py"));
}

#[test]
fn test_claude_code_requires_span_type_guard() {
    // tool_name and span.type are both generic names; only the claude_code. span-name
    // prefix gates this extractor, so other frameworks are never hijacked.
    let attrs = make_attrs(&[
        ("span.type", "tool"),
        ("tool_name", "Glob"),
        ("response.model_output", "hi"),
    ]);
    let mut messages = Vec::new();
    let mut tools = Vec::new();
    assert!(!try_claude_code(
        &mut messages,
        &mut tools,
        &attrs,
        "some.other.span",
        Utc::now()
    ));
    assert!(messages.is_empty());
}

#[test]
fn test_claude_code_ignores_redacted_and_empty_values() {
    // With no content gates set the CLI still emits span.type, but nothing usable.
    let attrs = make_attrs(&[("span.type", "llm_request"), ("new_context", "   ")]);
    let mut messages = Vec::new();
    let mut tools = Vec::new();
    assert!(!try_claude_code(
        &mut messages,
        &mut tools,
        &attrs,
        "claude_code.llm_request",
        Utc::now()
    ));
    assert!(messages.is_empty());
}

#[test]
fn test_claude_code_tool_output_event_is_a_message_event() {
    assert!(is_message_event("tool.output"));
}

#[test]
fn test_claude_code_new_context_tag_drives_role() {
    let now = Utc::now();

    // Id-tagged TOOL RESULT becomes a tool_result block linked by tool_use_id.
    let attrs = make_attrs(&[
        ("span.type", "llm_request"),
        (
            "new_context",
            "[TOOL RESULT: toolu_bdrk_01GJ]\nconfig.py\ninventory.py",
        ),
    ]);
    let mut messages = Vec::new();
    let mut tools = Vec::new();
    assert!(try_claude_code(
        &mut messages,
        &mut tools,
        &attrs,
        "claude_code.llm_request",
        now
    ));
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content["role"], json!("tool"));
    let block = &messages[0].content["content"][0];
    assert_eq!(block["type"], json!("tool_result"));
    assert_eq!(block["tool_use_id"], json!("toolu_bdrk_01GJ"));
    assert_eq!(block["content"], json!("config.py\ninventory.py"));

    // Name-tagged TOOL RESULT is the structured duplicate and is skipped.
    let attrs = make_attrs(&[
        ("span.type", "llm_request"),
        ("new_context", "[TOOL RESULT: Glob]\n{\"numFiles\":2}"),
    ]);
    let mut messages = Vec::new();
    let mut tools = Vec::new();
    assert!(!try_claude_code(
        &mut messages,
        &mut tools,
        &attrs,
        "s",
        now
    ));
    assert!(messages.is_empty());

    // Both user tags stay user.
    for tag in ["USER PROMPT", "USER"] {
        let attrs = make_attrs(&[
            ("span.type", "llm_request"),
            ("new_context", &format!("[{tag}]\nhello")),
        ]);
        let mut messages = Vec::new();
        let mut tools = Vec::new();
        assert!(try_claude_code(
            &mut messages,
            &mut tools,
            &attrs,
            "claude_code.llm_request",
            now
        ));
        assert_eq!(messages[0].content["role"], json!("user"), "tag {tag}");
        assert_eq!(messages[0].content["content"], json!("hello"));
    }
}

#[test]
fn test_claude_code_splits_parallel_tool_results() {
    // Verbatim shape from a subagents run: two parallel tool calls put both results
    // in one new_context. Each must keep its own tool_use_id.
    let attrs = make_attrs(&[
        ("span.type", "llm_request"),
        (
            "new_context",
            "[TOOL RESULT: toolu_first]\norders.ts\nshipping.ts\n\n---\n\n\
             [TOOL RESULT: toolu_second]\nNo files found",
        ),
    ]);
    let mut messages = Vec::new();
    let mut tools = Vec::new();
    assert!(try_claude_code(
        &mut messages,
        &mut tools,
        &attrs,
        "claude_code.llm_request",
        Utc::now()
    ));

    assert_eq!(messages.len(), 2);
    let ids: Vec<_> = messages
        .iter()
        .map(|m| m.content["content"][0]["tool_use_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["toolu_first", "toolu_second"]);
    assert_eq!(
        messages[0].content["content"][0]["content"],
        json!("orders.ts\nshipping.ts")
    );
    assert_eq!(
        messages[1].content["content"][0]["content"],
        json!("No files found")
    );
    // No leaked marker from the second section.
    for m in &messages {
        let body = m.content["content"][0]["content"].as_str().unwrap();
        assert!(!body.contains("[TOOL RESULT"), "leaked marker in {body:?}");
    }
}

#[test]
fn test_claude_code_unknown_tool_id_prefix_degrades_instead_of_dropping() {
    // If Anthropic ever changes the tool-use id prefix, a TOOL RESULT section must
    // still reach the feed (unlinked) rather than being silently discarded — silent
    // data loss is a far worse failure than a missing tool_use_id.
    let attrs = make_attrs(&[(
        "new_context",
        "[TOOL RESULT: xyz_9f2b]\nconfig.py\ninventory.py",
    )]);
    let mut messages = Vec::new();
    let mut tools = Vec::new();
    assert!(try_claude_code(
        &mut messages,
        &mut tools,
        &attrs,
        "claude_code.llm_request",
        Utc::now()
    ));
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content["role"], json!("tool"));
    assert_eq!(
        messages[0].content["content"][0]["content"],
        json!("config.py\ninventory.py")
    );
}

/// The current conventions' tool attributes become a call and a result, with the name and the id.
///
/// `gen_ai.tool.call.arguments` / `.result` are how the present OTel GenAI conventions report a tool call -
/// the Vercel AI SDK's current integration and Microsoft Agent Framework both use them - and they appear on
/// a *tool* span, where attribute extraction used to be skipped wholesale. Read but stripped of the name and
/// id they sit beside, they became a nameless call the pipeline discarded, which is worse than not reading
/// them: every layer looked healthy and the tool call was gone.
#[test]
fn semconv_tool_attributes_become_a_named_correlated_pair() {
    let attrs = make_attrs(&[
        ("gen_ai.operation.name", "execute_tool"),
        ("gen_ai.tool.name", "calculator"),
        ("gen_ai.tool.call.id", "call_1"),
        ("gen_ai.tool.call.arguments", r#"{"expression":"2+2"}"#),
        ("gen_ai.tool.call.result", r#"{"value":4}"#),
    ]);

    let mut messages = Vec::new();
    assert!(try_otel_genai_messages(
        &mut messages,
        &mut Vec::new(),
        &attrs,
        "execute_tool calculator",
        Utc::now()
    ));
    assert_eq!(messages.len(), 2, "a call and its result");

    let call = &messages[0].content["content"][0];
    assert_eq!(call["type"], "tool_use");
    assert_eq!(
        call["name"], "calculator",
        "the tool's name is beside the arguments; use it"
    );
    assert_eq!(call["id"], "call_1");
    assert_eq!(call["input"]["expression"], "2+2");

    let result = &messages[1].content["content"][0];
    assert_eq!(result["type"], "tool_result");
    assert_eq!(
        result["tool_use_id"], "call_1",
        "the result names the call, so the pair survives dedup as a pair"
    );
    assert_eq!(result["content"]["value"], 4);
}

// ============================================================================
// MESSAGE-RULE EQUIVALENCE: the declared rules against the extractors they replaced
// ============================================================================

use sideseat_domain::rules::message_rules::compile;
use sideseat_domain::rules::{MessageContext, ruleset, schema};

fn rule_attrs(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// The rules produce **semantically** what the functions they replaced produced.
///
/// Semantic, not byte: the comparison canonicalises object keys (see `canonical` below), because for an
/// indexed family the baseline had no member order to reproduce - it walked a randomised `HashMap`. So this
/// oracle does not police serialised member order, and the goldens are what catch a change to that: they
/// record the content string, and they are what failed when a projection language re-ordered a provider's
/// payload.
///
/// Compared as *serialised observations*, not counts: a rule that emitted the right number of messages
/// with the wrong carrier tag, the wrong role envelope or an unparsed payload would pass a count check
/// and corrupt every downstream view, since the carrier tag is what claiming, ordering and dedup all key
/// on.
#[test]
fn the_rules_reproduce_the_extractors_they_replaced() {
    let time = chrono::Utc::now();
    // Every carrier the migrated rules read, in the shapes that distinguish the parse modes: valid JSON,
    // an object, a bare string that is not JSON, and a numeric scalar.
    // A span *name* per case, because a rule may be gated on it - and a gated rule compared under a
    // name it cannot match proves nothing at all.
    let mut cases: Vec<(&str, HashMap<String, String>)> =
        include!("../messages_rule_cases/part_01_tests.rs");
    cases.extend(include!("../messages_rule_cases/part_02_tests.rs"));

    // The tool definitions CrewAI's metadata carriers yield, **frozen as outputs** rather than exempted.
    // The retired reader's code cannot be frozen here: its grammar moved into the sealed `tool_repr`
    // module, so copying it would compare that module against itself. Its *outputs* can be, and are the
    // thing that matters - taken from the pre-`9b013f86` implementation for exactly these shapes.
    let frozen_crew_tools = |attrs: &HashMap<String, String>| -> Vec<RawToolDefinition> {
        // **Literal captured outputs** for the exact shapes the cases carry - not a reimplementation. My
        // first attempt re-derived the name collection and was wrong twice over: it read `tools_names` *or*
        // `tools` where the retired reader read both, and it dropped the rich-definition quality selection
        // entirely - so it could have agreed with the rules for the wrong reason. A literal is either right
        // or visibly wrong.
        //
        // The retired reader's *code* cannot be frozen here: its grammar moved into the sealed `tool_repr`
        // module, so a copy would compare that module against itself.
        const CAPTURED: &[(&str, &str, &str)] = &[
            (
                "crew_tasks",
                r#"[{"name":"research"}]"#,
                r#"[{"type":"function","function":{"name":"research"}}]"#,
            ),
            (
                "crew_tasks",
                r#"[{"name":"n"}]"#,
                r#"[{"type":"function","function":{"name":"n"}}]"#,
            ),
        ];
        // Every `crew_*` payload among the cases must have a literal. Matching on the exact string means an
        // edited case would silently stop applying, and the comparison would then pass with *neither* side
        // producing anything - a green oracle that checks nothing, which is worse than a named exemption.
        // The tool carriers only - `crew_key` / `crew_id` / `task_key` are markers that gate the rules,
        // not payloads they read.
        for (carrier, raw) in attrs
            .iter()
            .filter(|(key, _)| matches!(key.as_str(), "crew_tasks" | "crew_agents"))
        {
            assert!(
                CAPTURED.iter().any(|(c, r, _)| c == carrier && r == raw),
                "`{carrier}` carries a payload with no captured output: {raw}\nAdd it to CAPTURED, or the \
                 oracle compares nothing on this carrier."
            );
        }
        CAPTURED
            .iter()
            .filter(|(carrier, raw, _)| attrs.get(*carrier).map(String::as_str) == Some(*raw))
            .map(|(carrier, _, expected)| {
                RawToolDefinition::from_attr(
                    carrier,
                    time,
                    serde_json::from_str(expected).expect("the captured output parses"),
                )
            })
            .collect()
    };

    let mut disagreements = Vec::new();
    for (span_name, case) in &cases {
        let mut legacy_msgs: Vec<RawMessage> = Vec::new();
        let mut legacy_tools: Vec<RawToolDefinition> = Vec::new();
        let mut legacy_found = false;
        // The caller's tool-span gate, replicated: `extract_per_carrier` skips every extractor but the
        // conventions on a tool execution span, so the legacy side must be compared under that same rule.
        // Without it the oracle compares at two different levels - the rules apply the gate internally
        // (it is a declared rule property now) while these functions expected their caller to.
        let is_tool_span = is_tool_execution_span(case);
        // Preserve the retired readers' rank order and claim each carrier per reader, exactly as the old
        // dispatcher did. Without claiming, this side reports
        // duplicates production never produced: two dialects do read `message`, and the earlier extractor
        // owned it. The oracle was blind to that until consolidating the extractors made the two rules run
        // in one call, where nothing discarded the second.
        let mut legacy_claimed: HashSet<String> = HashSet::new();
        for f in [
            try_otel_genai_messages,
            try_gen_ai_indexed,
            try_openinference,
            try_vercel_ai,
            try_logfire_events,
            try_google_adk,
            try_langgraph,
            try_mlflow,
            try_traceloop,
            try_pydantic_ai,
            try_langsmith,
            try_livekit,
            try_claude_code,
            try_crewai,
            try_autogen,
        ] {
            if is_tool_span {
                continue;
            }
            let mut produced = Vec::new();
            let tools_before = legacy_tools.len();
            if !f(&mut produced, &mut legacy_tools, case, span_name, time) {
                continue;
            }
            // A reviewed delta, and the only one: an extractor whose whole contribution was a *tool
            // definition* reported success, and the caller reads that as "the message payload was handled"
            // and stops asking. A span stating a dialect's tool list has said nothing about its
            // conversation, so it does not count here either. An extractor that produced nothing at all
            // still counts - that is the claim case, whose entire purpose is to stop the generic reader.
            let tools_only = produced.is_empty() && legacy_tools.len() > tools_before;
            legacy_found |= !tools_only;
            // Recorded after the whole batch, never per message: one extractor legitimately emits several
            // observations for one carrier, and claiming as it goes would keep only the first.
            let mut newly_claimed = Vec::new();
            for message in produced {
                let carrier = carrier_of(&message.source);
                if legacy_claimed.contains(&carrier) {
                    continue;
                }
                newly_claimed.push(carrier);
                legacy_msgs.push(message);
            }
            legacy_claimed.extend(newly_claimed);
        }
        if is_tool_span {
            // Only the conventions read a tool span, and they still do - as declared rules.
            legacy_found |=
                try_otel_genai_messages(&mut legacy_msgs, &mut legacy_tools, case, "span", time);
        }
        // The convention's single-tool triple, frozen from the retired always-on path. It is *appended*,
        // which is where it ran: after every other definition had been collected. Not a reviewed delta any
        // more - there is a counterpart now, so the comparison is real.
        legacy_tools.extend(frozen_crew_tools(case));
        if let Some(triple) = legacy_single_tool_definition(case, time) {
            legacy_tools.push(triple);
        }

        let mut rule_msgs: Vec<RawMessage> = Vec::new();
        let mut rule_tools: Vec<RawToolDefinition> = Vec::new();
        let rule_found = try_declared_rules(
            &mut rule_msgs,
            &mut rule_tools,
            case,
            span_name,
            time,
            &mut std::collections::HashSet::new(),
        );
        // The metadata axis, which production reads on every span through `extract_tool_definitions`. The
        // retired extractors pushed tool definitions into the same vector, so both axes are collected here
        // or a declaration that moved to the always-on path would look like a loss.
        for emission in sideseat_domain::rules::ruleset().messages.tool_definitions(
            &sideseat_domain::rules::MessageContext::for_span("", case, is_tool_span),
        ) {
            if emission.target == sideseat_domain::rules::schema::EmitTarget::ToolDefinitions {
                rule_tools.push(RawToolDefinition::from_attr(
                    emission.carrier.name(),
                    time,
                    emission.value,
                ));
            }
        }

        // Compared as sets of serialised observations: the retired dispatch order decided which reader
        // claimed a carrier, never the order observations sit in the vector - the old dispatcher
        // claims by carrier name, and the pipeline sorts by provenance afterwards.
        // Compared with object keys sorted, deliberately.
        //
        // Member *order* cannot be compared against this baseline, because for an indexed family the
        // baseline had none: the code being replaced walked the attribute `HashMap`, whose order is
        // randomised per process, straight into a map that preserves insertion order. So the legacy bytes
        // are noise for those carriers, and the rules sort instead. Order does not affect the normalised
        // content hash either - the feed sorts keys before hashing - only the persisted bytes and the
        // reconstruction cache digest. What it *does* affect is reproducibility, which is checked on its
        // own (`a_swept_payload_is_ordered_deterministically`) rather than against a nondeterministic
        // oracle.
        fn canonical(value: &JsonValue) -> JsonValue {
            match value {
                JsonValue::Object(map) => {
                    let mut sorted: Vec<(&String, &JsonValue)> = map.iter().collect();
                    sorted.sort_by_key(|(k, _)| k.as_str());
                    JsonValue::Object(
                        sorted
                            .into_iter()
                            .map(|(k, v)| (k.clone(), canonical(v)))
                            .collect(),
                    )
                }
                JsonValue::Array(items) => JsonValue::Array(items.iter().map(canonical).collect()),
                other => other.clone(),
            }
        }
        // A reviewed delta is a tool *definition* on one of these carriers - decided on the typed
        // observation, before rendering, so it cannot match a message nor a tool whose *content* merely
        // mentions the string. See the comment below the constant for why each is here.
        // Tool *definitions* only. `ai.toolCall.args` / `.result` are deliberately absent: those rules emit
        // messages, and their message exemption below is the justified one - exempting their tool
        // definitions as well would mask a future bogus metadata emission on carriers that should never
        // produce one.
        const REVIEWED_DELTA_CARRIERS: &[&str] = &[];

        let is_reviewed_delta_tool = |t: &RawToolDefinition| -> bool {
            matches!(&t.source, ToolDefinitionSource::Attribute { key, .. }
                if REVIEWED_DELTA_CARRIERS.contains(&key.as_str()))
        };
        // Vercel's tool-call attributes now also reach a tool span as *messages* - the call and its result.
        // The retired code read them only when nothing else had produced a message, and this harness
        // applies the tool-span exclusion, so the legacy side has nothing here. Keyed on the typed message
        // source, so only these two carriers are exempt and any other moved message is still compared.
        const REVIEWED_DELTA_MESSAGE_CARRIERS: &[&str] =
            &["ai.toolCall.args", "ai.toolCall.result"];
        let is_reviewed_delta_msg = |m: &RawMessage| -> bool {
            matches!(&m.source, MessageSource::Attribute { key, .. }
                if REVIEWED_DELTA_MESSAGE_CARRIERS.contains(&key.as_str()))
        };
        // Messages are compared as a *set* and tool definitions **in order**, which is not a stylistic
        // split. Message order is decided later, by the reconstruction pipeline, so the order they sit in
        // this vector says nothing; a tool definition's order is semantic here, because merging keeps the
        // *first* of two equal-quality definitions sharing a name - so the position decides which
        // description and schema win. Sorting them together hid exactly that regression, and only a
        // dedicated test caught it.
        let render = |msgs: &[RawMessage], tools: &[RawToolDefinition]| -> Vec<String> {
            let mut messages: Vec<String> = msgs
                .iter()
                .filter(|m| !is_reviewed_delta_msg(m))
                .map(|m| format!("msg {:?} {}", m.source, canonical(&m.content)))
                .collect();
            messages.sort();
            messages.extend(
                tools
                    .iter()
                    .filter(|t| !is_reviewed_delta_tool(t))
                    .map(|t| format!("tool {:?} {}", t.source, canonical(&t.content))),
            );
            messages
        };
        // Why each carrier is a reviewed delta:
        //
        // - Vercel's tool-call attributes now reach a tool span. The code being replaced read them only
        //   when nothing else had produced a message, so any recognised event dropped the call and its
        //   result; and because this harness applies the caller's tool-span exclusion, the legacy side
        //   produces nothing for these carriers. Pinned by
        //   `an_event_does_not_suppress_a_tool_span_s_own_attributes`.
        //
        // - CrewAI's `crew_tasks` / `crew_agents` **tool definitions**. Their reference is the sealed
        //   `tool_repr` grammar, validated by the dedicated `test_crewai_tool_definitions_*` tests when it
        //   moved - never by this message oracle, whose retired side had no CrewAI tool extraction at all.
        //
        // Both are dropped from the tool vector *before* rendering, keyed on the typed source carrier, so a
        // message that moved is still compared and a tool on another carrier is untouched.
        let legacy = render(&legacy_msgs, &legacy_tools);
        let rules = render(&rule_msgs, &rule_tools);
        // **One reviewed role delta.** The retired extractor wrote `role: "documents"` for retrieved material,
        // which is not a role in `ChatRole`'s vocabulary - it folded to `User` through the *unknown-role
        // default* rather than through any declaration, so the rule said one thing and meant another. The
        // assets say `context`, which is the declared vocabulary for exactly this and folds to `User` by
        // declaration.
        //
        // No reader sees a difference: both normalise to `User`, and the goldens did not move. What changes is
        // the stored raw string, and stored spans keep whichever they were written with - both still display
        // as `User`. Rewritten here rather than exempted, so every other difference in these cases is still
        // compared.
        let legacy: Vec<String> = legacy
            .into_iter()
            .map(|rendered| rendered.replace(r#""role":"documents""#, r#""role":"context""#))
            .collect();
        // The `found` flags can differ only by a reviewed delta: on a tool span the rules recognise
        // Vercel's tool-call carriers and the harness-excluded legacy side does not. Compared only when the
        // rendered observations agree, so `found` is not a second channel that can hide a real change.
        let found_differs_by_delta = legacy == rules
            && rule_found
            && !legacy_found
            && rule_msgs.iter().any(is_reviewed_delta_msg);
        if (legacy != rules || legacy_found != rule_found) && !found_differs_by_delta {
            disagreements.push(format!(
                "  span `{span_name}` {case:?}\n    table: found={legacy_found} {legacy:?}\n    \
                 rules: found={rule_found} {rules:?}"
            ));
        }
    }
    assert!(
        disagreements.is_empty(),
        "message extraction changed for {} case(s):\n{}",
        disagreements.len(),
        disagreements.join("\n")
    );
}

/// What has moved, and what has not - counted, so the boundary cannot quietly stop moving.
#[test]
fn declared_message_rules_cover_what_they_claim() {
    let plan = &ruleset().messages;
    assert_eq!(
        plan.rule_count(),
        94,
        "the assets declare {} message rules. Production has no extractor registry: even last-resort \
         carriers are declared with `stage: fallback`. A dialect moves whole or not at all, so there are \
         no partially migrated carriers to count.",
        plan.rule_count()
    );
    for rule in plan.rules() {
        assert!(
            rule.doc.is_some(),
            "message rule `{}` has no doc: the reason a carrier is read the way it is belongs beside \
             the declaration",
            rule.rule_id
        );
    }
    // The carriers really are in the assets, so a clean engine is not a vacuous one.
    let all: String = schema::embedded_sources()
        .values()
        .map(|b| String::from_utf8_lossy(b).to_string())
        .collect();
    for carrier in [
        "traceloop.entity.input",
        "mlflow.spanInputs",
        "mlflow.chat.tools",
        "tool_arguments",
        "tool_response",
    ] {
        assert!(
            all.contains(carrier),
            "carrier `{carrier}` is declared in no asset, so nothing declares it at all"
        );
    }
}
