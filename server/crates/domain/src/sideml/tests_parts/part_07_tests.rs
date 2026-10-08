// === Edge Case Tests ===

#[test]
fn blank_text_is_removed_only_when_a_sibling_carries_the_message() {
    let tool_call = normalize(&json!({
        "role": "assistant",
        "contents": [{
            "message_content": {
                "type": "text",
                "text": ""
            }
        }],
        "tool_calls": [{
            "tool_call": {
                "id": "call-final",
                "function": {
                    "name": "final_answer",
                    "arguments": "{\"answer\":\"done\"}"
                }
            }
        }]
    }));
    assert_eq!(tool_call.content.len(), 1);
    assert!(matches!(
        &tool_call.content[0],
        ContentBlock::ToolUse { id: Some(id), name, .. }
            if id == "call-final" && name == "final_answer"
    ));

    let standalone = normalize(&json!({
        "role": "assistant",
        "content": [{"type": "text", "text": ""}]
    }));
    assert!(matches!(
        &standalone.content[..],
        [ContentBlock::Text { text }] if text.is_empty()
    ));
}

#[test]
fn test_empty_tool_name_handling() {
    // Test that empty tool names are handled gracefully
    let raw = json!({
        "role": "assistant",
        "tool_calls": [{
            "id": "call_123",
            "function": {
                "name": "",
                "arguments": "{}"
            }
        }]
    });

    let message = normalize(&raw);

    // Should not panic, and tool call should still be created
    assert!(!message.content.is_empty());
}

#[test]
fn test_null_role_handling() {
    // Test that null role defaults correctly
    let raw = json!({
        "role": null,
        "content": "Hello"
    });

    let message = normalize(&raw);

    // Null role should default to User
    assert_eq!(message.role, ChatRole::User);
}

#[test]
fn test_non_string_role_handling() {
    // Test that non-string role (edge case) defaults correctly
    let raw = json!({
        "role": 123,
        "content": "Hello"
    });

    let message = normalize(&raw);

    // Non-string role should default to User
    assert_eq!(message.role, ChatRole::User);
}

// === Vercel AI SDK Structured Output Tests ===

#[test]
fn test_vercel_ai_response_object_as_content() {
    // Vercel AI SDK stores structured output in "object" field (from ai.response.object)
    // This is extracted by try_vercel_ai and should be normalized as content
    let raw = json!({
        "role": "assistant",
        "object": {
            "recipe": {
                "name": "Chocolate Chip Cookies",
                "ingredients": ["flour", "sugar", "butter", "chocolate chips"],
                "steps": ["Mix dry ingredients", "Add wet ingredients", "Bake at 350F"]
            }
        }
    });

    let message = normalize(&raw);

    assert_eq!(message.role, ChatRole::Assistant);
    assert!(
        !message.content.is_empty(),
        "Content should not be empty for object field"
    );

    // The object should be normalized as a json content block
    match &message.content[0] {
        ContentBlock::Json { data } => {
            assert!(data.get("recipe").is_some(), "Should contain recipe object");
            assert_eq!(data["recipe"]["name"], "Chocolate Chip Cookies");
        }
        other => panic!("Expected Json content block, got {:?}", other),
    }
}

#[test]
fn test_vercel_ai_response_object_simple_value() {
    // Vercel AI can return simple values as structured output
    let raw = json!({
        "role": "assistant",
        "object": {
            "answer": 42,
            "confidence": 0.95
        }
    });

    let message = normalize(&raw);

    assert_eq!(message.role, ChatRole::Assistant);
    assert!(!message.content.is_empty());

    match &message.content[0] {
        ContentBlock::Json { data } => {
            assert_eq!(data["answer"], 42);
            assert_eq!(data["confidence"], 0.95);
        }
        other => panic!("Expected Json content block, got {:?}", other),
    }
}

#[test]
fn test_vercel_ai_content_takes_precedence_over_object() {
    // When both content and object are present, content should take precedence
    let raw = json!({
        "role": "assistant",
        "content": "Hello from content field",
        "object": {"ignored": true}
    });

    let message = normalize(&raw);

    assert_eq!(message.role, ChatRole::Assistant);
    assert!(!message.content.is_empty());

    // Content field should be used, not object
    match &message.content[0] {
        ContentBlock::Text { text } => {
            assert_eq!(text, "Hello from content field");
        }
        other => panic!("Expected Text content block, got {:?}", other),
    }
}

// ============================================================================
// REGRESSION TESTS
// ============================================================================
// Tests for specific real-world issues that were fixed.
// Each test documents the original issue with trace ID when available.

#[test]
fn regression_langgraph_contents_field_with_message_content_wrapper() {
    // Regression test for trace 0bda91d1d9f955fd3e2dab4b9664333b
    // Issue: LangGraph/ChatBedrockConverse uses OpenInference format with:
    // - "contents" field (plural) instead of "content"
    // - Nested "message_content" wrapper around actual content
    // - Sparse arrays from unflatten (missing indices create empty {})
    //
    // This combination caused assistant messages to show as empty.
    let raw = json!({
        "role": "assistant",
        "contents": [
            {},  // Sparse array placeholder (index 0 was missing)
            {
                "message_content": {
                    "type": "text",
                    "text": "Hello! Here's your 3-day weather forecast for New York City..."
                }
            }
        ]
    });

    let message = normalize(&raw);

    assert_eq!(message.role, ChatRole::Assistant);
    assert_eq!(
        message.content.len(),
        1,
        "Should have exactly 1 content block"
    );

    match &message.content[0] {
        ContentBlock::Text { text } => {
            assert!(
                text.contains("weather forecast"),
                "Should extract text from message_content wrapper"
            );
        }
        other => panic!("Expected Text, got {:?}", other),
    }
}

#[test]
fn regression_empty_json_block_from_sparse_array() {
    // Regression test for trace 771e923f9f7781491b41abc00d9d21fa
    // Issue: Assistant message displayed as "{}" because sparse array
    // placeholder was normalized to json type instead of being filtered.
    let raw = json!({
        "role": "assistant",
        "contents": [
            {
                "message_content": {
                    "type": "text",
                    "text": "I'll help you with the weather forecast."
                }
            },
            {}  // This placeholder was incorrectly showing as "{}"
        ]
    });

    let message = normalize(&raw);

    assert_eq!(
        message.content.len(),
        1,
        "Empty placeholder should be filtered"
    );

    // Verify no json blocks with empty data
    for block in &message.content {
        if let ContentBlock::Json { data } = block {
            assert!(
                !data.as_object().is_some_and(|o| o.is_empty()),
                "Should not have empty json blocks from sparse array placeholders"
            );
        }
    }
}

#[test]
fn regression_openinference_tool_use_with_sparse_array() {
    // Regression test: Tool calls from LangGraph with sparse indices
    // Real pattern: llm.output_messages.0.message.contents.N.message_content
    // where N has gaps (e.g., 0 and 2 exist, but 1 doesn't)
    let raw = json!({
        "role": "assistant",
        "contents": [
            {},  // Placeholder
            {
                "message_content": {
                    "type": "tool_use",
                    "id": "tooluse_RHFjqfWtcReRCoWcxaIv4n",
                    "name": "temperature_forecast",
                    "input": {"city": "New York City", "days": 3}
                }
            },
            {
                "message_content": {
                    "type": "tool_use",
                    "id": "tooluse_kKnD6F5gFUOkblpMf6V81i",
                    "name": "precipitation_forecast",
                    "input": {"city": "New York City", "days": 3}
                }
            }
        ]
    });

    let message = normalize(&raw);

    assert_eq!(message.content.len(), 2, "Should have 2 tool_use blocks");

    let tool_names: Vec<&str> = message
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolUse { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();

    assert!(tool_names.contains(&"temperature_forecast"));
    assert!(tool_names.contains(&"precipitation_forecast"));
}

#[test]
fn regression_openinference_reasoning_content_thinking() {
    // Regression test: Extended thinking from OpenInference format
    // Pattern: reasoning_content wrapper with text and signature
    let raw = json!({
        "role": "assistant",
        "contents": [
            {
                "reasoning_content": {
                    "text": "Let me analyze the weather data for NYC...",
                    "signature": "thinking_sig_abc123"
                }
            },
            {
                "message_content": {
                    "type": "text",
                    "text": "Based on my analysis, here's the forecast."
                }
            }
        ]
    });

    let message = normalize(&raw);

    assert_eq!(message.content.len(), 2, "Should have thinking + text");

    // First block should be thinking
    match &message.content[0] {
        ContentBlock::Thinking { text, signature } => {
            assert!(text.contains("analyze the weather"));
            assert_eq!(signature.as_deref(), Some("thinking_sig_abc123"));
        }
        other => panic!("Expected Thinking, got {:?}", other),
    }

    // Second block should be text
    match &message.content[1] {
        ContentBlock::Text { text } => {
            assert!(text.contains("Based on my analysis"));
        }
        other => panic!("Expected Text, got {:?}", other),
    }
}

#[test]
fn regression_unflatten_creates_nested_sparse_arrays() {
    // Regression test: Unflatten with complex nested sparse structures
    // Can occur with deeply nested OpenInference message formats
    //
    // The unflatten algorithm creates placeholders at multiple levels when
    // indexed attributes have gaps.
    let raw = json!({
        "role": "assistant",
        // After unflatten of something like:
        // "contents.2.message_content.text": "Hello"
        // We get:
        "contents": [
            {},  // placeholder for index 0
            {},  // placeholder for index 1
            {
                "message_content": {
                    "type": "text",
                    "text": "Hello after two sparse indices"
                }
            }
        ]
    });

    let message = normalize(&raw);

    assert_eq!(message.content.len(), 1, "Should filter both placeholders");

    match &message.content[0] {
        ContentBlock::Text { text } => {
            assert_eq!(text, "Hello after two sparse indices");
        }
        other => panic!("Expected Text, got {:?}", other),
    }
}

// === Plain Data / Structured Output Tests ===

#[test]
fn test_is_plain_data_value() {
    // Plain data objects → true
    assert!(is_plain_data_value(&json!({"name": "Jane", "age": 28})));
    assert!(is_plain_data_value(
        &json!({"score": 0.95, "label": "positive"})
    ));
    assert!(is_plain_data_value(&json!({"items": [1, 2, 3]})));

    // Message-structure objects → false
    assert!(!is_plain_data_value(
        &json!({"role": "user", "content": "hi"})
    ));
    assert!(!is_plain_data_value(&json!({"content": "hello"})));
    assert!(!is_plain_data_value(&json!({"type": "text", "text": "hi"})));
    assert!(!is_plain_data_value(&json!({"tool_calls": []})));
    assert!(!is_plain_data_value(&json!({"finish_reason": "stop"})));
    assert!(!is_plain_data_value(&json!({"toolUse": {}})));
    // Both spellings of one provider's part answer alike, which they did not: `function_call` was bare data and
    // `functionCall` was message-shaped, so the same part was wrapped under one spelling and read as a message
    // with no content member under the other. Bare data is the answer that wraps it into a message whose single
    // block is the part, which is what content normalisation then reads.
    assert!(is_plain_data_value(&json!({"function_call": {}})));
    assert!(is_plain_data_value(&json!({"functionCall": {}})));
    assert!(!is_plain_data_value(&json!({"parts": []})));
    assert!(!is_plain_data_value(&json!({"choices": []})));

    // Non-objects → false
    assert!(!is_plain_data_value(&json!("string")));
    assert!(!is_plain_data_value(&json!(42)));
    assert!(!is_plain_data_value(&json!([1, 2])));
    assert!(!is_plain_data_value(&json!(null)));
    assert!(!is_plain_data_value(&json!({})));
}

#[test]
fn test_normalize_plain_data_object() {
    let raw = json!({"name": "Jane Doe", "age": 28});
    let message = normalize(&raw);

    assert_eq!(message.role, ChatRole::User);
    assert_eq!(message.content.len(), 1);
    match &message.content[0] {
        ContentBlock::Json { data } => {
            assert_eq!(data["name"], "Jane Doe");
            assert_eq!(data["age"], 28);
        }
        other => panic!("Expected Json block, got {:?}", other),
    }
}

#[test]
fn test_normalize_plain_data_doesnt_affect_messages() {
    // A proper message with "content" key should still work normally
    let raw = json!({"role": "assistant", "content": "Hello!"});
    let message = normalize(&raw);
    assert_eq!(message.role, ChatRole::Assistant);
    assert_eq!(message.content.len(), 1);
    match &message.content[0] {
        ContentBlock::Text { text } => assert_eq!(text, "Hello!"),
        other => panic!("Expected Text, got {:?}", other),
    }

    // A message with tool_calls should still work
    let raw = json!({"tool_calls": [{"id": "1", "function": {"name": "test", "arguments": "{}"}}]});
    let message = normalize(&raw);
    assert_eq!(message.role, ChatRole::Assistant);
    assert!(
        message
            .content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolUse { .. }))
    );
}

/// The declared `event_roles` answer exactly as the Rust table they replaced did.
///
/// Every name the retired table knew, on both kinds of span, plus names it did not know - because a table
/// that answers *more* than the one it replaced is as much a change as one that answers less, and the
/// unrecognised case is where a new role would silently start overriding one derived from content.
#[test]
fn the_declared_event_roles_reproduce_the_table_they_replaced() {
    let names = [
        "gen_ai.system.message",
        "gen_ai.user.message",
        "gen_ai.content.prompt",
        "gen_ai.tool.message",
        "gen_ai.tool.result",
        "tool.output",
        "gen_ai.assistant.message",
        "gen_ai.choice",
        "gen_ai.content.completion",
        // Names no asset speaks for, which must stay unanswered.
        "gen_ai.unknown.message",
        "tool.input",
        "",
    ];
    for name in names {
        for is_tool_span in [false, true] {
            assert_eq!(
                normalize::role_from_event_name_with_context(name, is_tool_span),
                normalize::role_from_event_name_with_context_legacy(name, is_tool_span),
                "`{name}` on {} disagrees with the table the assets replaced",
                if is_tool_span {
                    "a tool span"
                } else {
                    "a chat span"
                }
            );
        }
    }
}

/// A **tagged** source name takes its declared role, end to end, not only through the lookup helper.
///
/// This is the shape the event-role move first left open. A dialect tags a bundled tool result
/// `gen_ai.tool.result` with `tag_as`, which makes the emission an *attribute* whose key the engine chose. A
/// bundle of **one** is not split - splitting exists to separate results that would otherwise share an
/// identity, and one needs no separating - so it reached role derivation as an attribute, matched nothing,
/// and was normalised as a **user** message: the tool's answer presented as the user's question.
///
/// Two-sided on purpose. A *producer's own* attribute of the same name must **not** take the role, because
/// the declaration is a statement about names this engine assigns; and a reading that already stated a role
/// keeps it, because that is more specific than the name it was tagged with.
#[test]
fn a_tagged_source_name_takes_its_declared_role() {
    let one_result = json!({
        "content": [{
            "toolResult": {
                "toolUseId": "call_1",
                "content": [{"text": "17"}],
                "status": "success"
            }
        }],
        "tool_call_id": "call_1"
    });

    let tagged = vec![RawMessage {
        source: MessageSource::Attribute {
            key: "gen_ai.tool.result".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 15, 10, 30, 0).unwrap(),
        },
        content: one_result.clone(),
        rendering: false,
    }];
    let out = to_sideml(&tagged);
    assert_eq!(out.len(), 1, "one bundled result is one message");
    assert_eq!(
        out[0].sideml.role,
        ChatRole::Tool,
        "a tagged `gen_ai.tool.result` is a tool's answer; as a user message it reads as the question"
    );

    // A **declared** name that no rule tags. `gen_ai.choice` has a declared role and is an *event* name, so
    // an attribute carrying that key is a producer's own and must not take it. Chosen deliberately over an
    // undeclared key like `gen_ai.prompt.0.content`: that one answers nothing either way, so it cannot tell
    // "only tags consult the declarations" from "any attribute key does", which is the property at stake.
    let untagged = vec![RawMessage {
        source: MessageSource::Attribute {
            key: "gen_ai.choice".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 15, 10, 30, 0).unwrap(),
        },
        content: one_result.clone(),
        rendering: false,
    }];
    assert_ne!(
        to_sideml(&untagged)[0].sideml.role,
        ChatRole::Assistant,
        "a producer's own attribute must not be given the role this engine declares for its own tags"
    );

    // A role the reading states but this pipeline cannot read is **not** authoritative: it normalises to
    // `user` further down, so treating it as a statement turned the tool's answer into the user's question on
    // the strength of a value nothing understands.
    let mut bogus = one_result.clone();
    bogus["role"] = json!("bogus");
    let unreadable = vec![RawMessage {
        source: MessageSource::Attribute {
            key: "gen_ai.tool.result".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 15, 10, 30, 0).unwrap(),
        },
        content: bogus,
        rendering: false,
    }];
    assert_eq!(
        to_sideml(&unreadable)[0].sideml.role,
        ChatRole::Tool,
        "an unrecognised role is not a statement, and the tag's declaration should stand"
    );

    // A reading that states a role this pipeline *can* read keeps it: more specific than the tag.
    let mut stated = one_result.clone();
    stated["role"] = json!("assistant");
    let explicit = vec![RawMessage {
        source: MessageSource::Attribute {
            key: "gen_ai.tool.result".to_string(),
            time: Utc.with_ymd_and_hms(2024, 1, 15, 10, 30, 0).unwrap(),
        },
        content: stated,
        rendering: false,
    }];
    assert_eq!(
        to_sideml(&explicit)[0].sideml.role,
        ChatRole::Assistant,
        "a role the reading stated is more specific than the tag's declaration"
    );
}

/// Every declared source name is one that can occur, and every name the retired table knew is declared.
///
/// The equivalence oracle beside this one samples names; this compares the **key sets**, in both directions.
/// Sampling leaves a new declaration invisible - adding `new.magic` would have kept it green - and a
/// declaration for a name nothing produces can never answer, which reads as protection that is not there.
#[test]
fn the_declared_source_names_are_exactly_the_ones_that_can_occur() {
    let ruleset = crate::rules::ruleset();
    // The names that declare a *role*; a name declaring only a direction says nothing this table did.
    let declared: std::collections::BTreeSet<&str> = ruleset
        .event_roles
        .iter()
        .filter(|(_, declared)| declared.role.is_some() || declared.in_tool_span.is_some())
        .map(|(name, _)| name.as_str())
        .collect();

    // Every name the retired table answered for must still be declared, or a role silently disappears.
    let retired = [
        "gen_ai.system.message",
        "gen_ai.user.message",
        "gen_ai.content.prompt",
        "gen_ai.tool.message",
        "gen_ai.tool.result",
        "tool.output",
        "gen_ai.assistant.message",
        "gen_ai.choice",
        "gen_ai.content.completion",
    ];
    for name in retired {
        assert!(
            declared.contains(name),
            "`{name}` had a role in the table the assets replaced and no longer has one"
        );
    }
    // And nothing else, so a new declaration is a deliberate change to this list rather than a silent one.
    let expected: std::collections::BTreeSet<&str> = retired.into_iter().collect();
    assert_eq!(
        declared, expected,
        "the declared source names have changed; if that is intended, state the new one here"
    );

    // Each must be producible: an event some asset lists, or a name some rule assigns. The compile refuses
    // otherwise, and this states the property the refusal exists for.
    for name in &declared {
        assert!(
            ruleset.message_events.contains_key(*name)
                || ruleset.tagged_source_names.contains(*name),
            "`{name}` declares a role and nothing produces it"
        );
    }
}

/// A source name has **three** possible authorities on a tool span, and each has one spelling.
///
/// Absence means "the same role as elsewhere", which is what makes the third state unspellable without a flag of
/// its own: a name that speaks for ordinary spans and says *nothing* on a tool span had no way to say so, because
/// absence falls back to `role`. And "the same role" had two spellings - absence, and a `role_in_tool_span`
/// equal to `role` - which one shipped declaration used and seven did not, for the identical fact.
#[test]
fn a_source_name_states_one_of_three_authorities_on_a_tool_span() {
    use crate::sideml::normalize::role_from_event_name_with_context as role_of;

    // A role of its own: the same name means the opposite thing on a tool span.
    assert_eq!(role_of("gen_ai.choice", false), Some(ChatRole::Assistant));
    assert_eq!(role_of("gen_ai.choice", true), Some(ChatRole::Tool));

    // The same role on both, said by absence - and this is the declaration that used to spell it twice.
    assert_eq!(
        role_of("gen_ai.assistant.message", false),
        Some(ChatRole::Assistant)
    );
    assert_eq!(
        role_of("gen_ai.assistant.message", true),
        Some(ChatRole::Assistant),
        "absence means the same role, so removing the redundant spelling must not change the answer"
    );

    // A name no asset speaks for states nothing on either kind, which is the state the flag makes declarable for
    // a name that *does* speak elsewhere.
    assert_eq!(role_of("acme.unknown.event", false), None);
    assert_eq!(role_of("acme.unknown.event", true), None);
}

/// Authority is its own declared fact, and the two questions are separate.
///
/// They were fused, differently on each path. Whether a stated role survives event-name derivation was a
/// hardcoded Rust list; whether it outranks a *tagged attribute name* was that list **or whatever the role alias
/// table happened to fold**. The folding table's job is mapping spellings onto four canonical roles, which is not
/// a statement about authority - so adding a spelling there silently granted it authority over a declared tag,
/// and removing one silently took it away, with neither change naming the authority it moved.
///
/// `tool` is the case that proves the two are not one set: authoritative over a tag, and deliberately **not**
/// surviving event derivation, because an event name really is evidence of it (`gen_ai.tool.message` on a chat
/// span, `gen_ai.choice` on a tool span).
#[test]
fn role_authority_is_two_declared_facts_and_not_a_by_product_of_folding() {
    let authority = &crate::rules::ruleset().role_authority;

    // Roles nothing derives from a name: deriving would overwrite the specific fact with a guess.
    for role in ["tool_call", "tools", "data", "context", "documents"] {
        assert!(
            authority.survives_event_derivation(role),
            "`{role}` must survive event-name derivation"
        );
        assert!(
            authority.outranks_a_tag(role),
            "`{role}` must outrank a tag"
        );
    }

    // The separating case. Both directions, or this passes for a set that happens to hold everything.
    assert!(authority.outranks_a_tag("tool"));
    assert!(
        !authority.survives_event_derivation("tool"),
        "`tool` is derivable from an event name, so a stated one must not block derivation"
    );
    for role in ["user", "assistant", "system", "human", "ai", "choice"] {
        assert!(authority.outranks_a_tag(role));
        assert!(
            !authority.survives_event_derivation(role),
            "`{role}` is an ordinary conversation role and event names speak for it"
        );
    }

    // Case-insensitive, since a payload states whatever it likes.
    assert!(authority.survives_event_derivation("Tool_Call"));
    assert!(authority.outranks_a_tag("USER"));

    // And a spelling nothing declares carries no authority - which is what makes the declaration load-bearing
    // rather than decorative.
    assert!(!authority.outranks_a_tag("narrator"));
    assert!(!authority.survives_event_derivation("narrator"));

    // Every spelling given a meaning states its authority too. Nothing *requires* that - the meaning and the
    // authority are separate facts on one entry - but each shipped alias was decided, and a new one declaring
    // none would be a change worth seeing here.
    for (spelling, _) in authority.meanings() {
        assert!(
            authority.declared_spellings().any(|d| d == spelling),
            "`{spelling}` is given a meaning and declares no authority"
        );
    }
}

/// The authority decision does not consult the folding table - checked on the **source**, because no behavioural
/// test can see it.
///
/// The declared authority set and the set the alias table folds coincide today, deliberately: declaring anything
/// else would have been a behaviour change no evidence asked for. So reintroducing `try_from_str(stated).is_some()
/// ||` beside the declaration changes no answer and passes every assertion about authority - which is exactly the
/// shape of the defect, a gate that agrees with what it replaced until someone edits the other thing.
///
/// What must hold is structural: the authority plan answers from its declarations alone.
#[test]
fn the_authority_decision_never_consults_the_folding_table() {
    let source = include_str!("../../rules/mod.rs");
    // The two authority questions, each read from its own declarations - the meanings sit in the same plan, and
    // neither question may consult them.
    for question in [
        "pub fn survives_event_derivation(",
        "pub fn outranks_a_tag(",
    ] {
        let start = source.find(question).expect("the authority question");
        let body = &source[start..];
        let end = body.find("\n    }\n").expect("the method ends");
        let body = &body[..end];
        assert!(
            !body.contains("try_from_str") && !body.contains("ChatRole") && !body.contains("means"),
            "the authority plan consults the role meanings, which decide spelling rather than authority - the \
             fusion this separation removed"
        );
    }

    // And the two call sites read the plan rather than deciding for themselves.
    let normalize = include_str!("../normalize.rs");
    let start = normalize
        .find("fn derive_role_from_source_with_context")
        .expect("the deriving function");
    let body = &normalize[start..];
    let end = body.find("\n}\n").expect("the function ends");
    let body = &body[..end];
    assert!(
        !body.contains("try_from_str"),
        "role derivation decides authority from the folding table again"
    );
    assert!(
        body.contains("survives_event_derivation") && body.contains("outranks_a_tag"),
        "role derivation should ask the declared authority for both of its questions"
    );
}

/// OpenInference's Bedrock instrumentation reports a Converse tool result in the user turn it travels
/// in: role `user`, the result's text, and its `tool_call_id`. The id is what makes it an answer.
#[test]
fn a_user_turn_carrying_a_tool_call_id_is_a_tool_result() {
    let message = normalize(&json!({
        "role": "user",
        "content": "80% chance of rain in Tokyo tomorrow.",
        "tool_call_id": "tooluse_1"
    }));

    assert_eq!(message.role, ChatRole::Tool);
    assert!(
        matches!(&message.content[..], [ContentBlock::ToolResult { tool_use_id: Some(id), .. }] if id == "tooluse_1"),
        "{:?}",
        message.content
    );
    // Without an id a user turn is what the user said.
    assert_eq!(
        normalize(&json!({"role": "user", "content": "hi"})).role,
        ChatRole::User
    );
}

/// A tool that returns a number reports it as a string. Decoded as JSON it became a number with no
/// content block to hold it, and the result vanished.
#[test]
fn scalar_content_is_kept_as_text() {
    for content in [json!("395.0"), json!(395.0), json!("true")] {
        let message = normalize(&json!({"role": "tool", "content": content, "tool_call_id": "t"}));
        let [ContentBlock::ToolResult { content, .. }] = &message.content[..] else {
            panic!("{content:?} lost its result: {:?}", message.content);
        };
        assert!(content.to_string().contains("395") || content.to_string().contains("true"));
    }
}

/// A tool result whose text is a number keeps its value. OpenInference writes an MCP calculator's result as
/// the string `395.0`; parsed as JSON it became a number, which content normalisation renders as nothing, so
/// the result vanished and the call looked unanswered.
#[test]
fn a_tool_result_that_reads_as_a_number_keeps_its_value() {
    for text in ["395.0", "true", "null"] {
        let message =
            normalize(&json!({"role": "tool", "content": text, "tool_call_id": "call-1"}));
        assert!(
            matches!(
                &message.content[..],
                [ContentBlock::ToolResult { tool_use_id: Some(id), content, .. }]
                    if id == "call-1" && *content != json!([])
            ),
            "{text}: {:?}",
            message.content
        );
    }
}

/// A message that carries both a content list and a flattened `content` is read from the list. OpenInference's
/// Agno instrumentor writes the reasoning and the answer as `contents` and only the answer as `content`; read
/// singular-first, the reasoning was lost.
#[test]
fn a_content_list_beside_flattened_content_keeps_the_reasoning() {
    let message = normalize(&json!({
        "role": "assistant",
        "content": "17 minutes.",
        "contents": [
            {"message_content": {"type": "reasoning", "text": "Pair the two slowest."}},
            {"message_content": {"type": "text", "text": "17 minutes."}}
        ]
    }));
    assert!(
        matches!(
            &message.content[..],
            [ContentBlock::Thinking { text, .. }, ContentBlock::Text { text: answer }]
                if text == "Pair the two slowest." && answer == "17 minutes."
        ),
        "{:?}",
        message.content
    );
}

/// The conventions' `gen_ai.choice` event wraps the model's turn in `message`, beside `index` and
/// `finish_reason`. Read as it stood, the whole message was the content and rendered as raw JSON.
#[test]
fn a_choice_envelope_is_read_as_the_message_it_holds() {
    let message = normalize(&json!({
        "role": "assistant",
        "index": 0,
        "finish_reason": "tool_use",
        "message": {"role": "assistant", "content": [
            {"text": "Checking."},
            {"toolUse": {"toolUseId": "call-1", "name": "get_weather", "input": {"city": "Rome"}}}
        ]}
    }));
    assert!(
        matches!(
            &message.content[..],
            [ContentBlock::Text { text }, ContentBlock::ToolUse { id: Some(id), name, .. }]
                if text == "Checking." && id == "call-1" && name == "get_weather"
        ),
        "{:?}",
        message.content
    );
    assert!(
        message.finish_reason.is_some(),
        "the envelope's finish reason is the message's"
    );
}

/// A Converse tool result block reads in the canonical form a tool message's content takes, so one result
/// carried both ways is one result: a lone text is its string, a lone structured value is that value.
#[test]
fn a_converse_tool_result_takes_the_canonical_content_form() {
    let block = |content: serde_json::Value| {
        normalize(&json!({"role": "user", "content": [
            {"toolResult": {"toolUseId": "call-1", "content": content}}
        ]}))
    };
    for (content, want) in [
        (
            json!([{"text": "10% chance of rain."}]),
            json!("10% chance of rain."),
        ),
        (json!([{"json": {"city": "Rome"}}]), json!({"city": "Rome"})),
    ] {
        let message = block(content);
        assert!(
            matches!(&message.content[..], [ContentBlock::ToolResult { content, .. }] if *content == want),
            "{:?}",
            message.content
        );
    }
}

/// A call stated as a content block and again in the message's call list is one call: one id within one
/// message names one call. A second call with its own id is still a second call.
#[test]
fn a_call_listed_beside_its_own_content_block_is_one_call() {
    let message = normalize(&json!({
        "role": "assistant",
        "content": [{"type": "tool_use", "id": "call-1", "name": "book_flight", "input": {"origin": "London"}}],
        "tool_calls": [
            {"id": "call-1", "name": "book_flight", "args": {"origin": "London"}},
            {"id": "call-2", "name": "book_flight", "args": {"origin": "London"}}
        ]
    }));
    let ids: Vec<_> = message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, .. } => id.clone(),
            _ => None,
        })
        .collect();
    assert_eq!(ids, vec!["call-1".to_string(), "call-2".to_string()]);
}
