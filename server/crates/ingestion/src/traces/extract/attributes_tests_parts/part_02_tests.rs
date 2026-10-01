
#[test]
fn test_strands_agents_framework_detection_via_span_name_and_agent_attr() {
    let empty_attrs = HashMap::new();
    let resource_attrs = HashMap::new();

    // span name variants (space, hyphen, underscore, case-insensitive)
    for span_name in &[
        "invoke_agent Strands Agent",
        "invoke_agent strands agent",
        "invoke_agent Strands-Agent",
        "invoke_agent strands_agent",
        "Strands Agent runner",
    ] {
        assert_eq!(
            detect_framework(span_name, &empty_attrs, &resource_attrs),
            Framework::StrandsAgents.as_str(),
            "span name '{span_name}' should detect Strands"
        );
    }

    // gen_ai.agent.name variants
    for agent_name in &[
        "Strands Agent",
        "strands agent",
        "Strands-Agent",
        "strands_agent",
    ] {
        let attrs = make_attrs(&[("gen_ai.agent.name", agent_name)]);
        assert_eq!(
            detect_framework("chat", &attrs, &resource_attrs),
            Framework::StrandsAgents.as_str(),
            "gen_ai.agent.name='{agent_name}' should detect Strands"
        );
    }

    // bare invoke_agent without Strands in name should NOT match
    assert_ne!(
        detect_framework("invoke_agent", &empty_attrs, &resource_attrs),
        Framework::StrandsAgents.as_str(),
        "bare 'invoke_agent' should not match"
    );
}

#[test]
fn test_strands_agents_performance_metrics() {
    // Strands sets TTFT and request duration
    let attrs = make_attrs(&[
        ("gen_ai.server.time_to_first_token", "150"),
        ("gen_ai.server.request.duration", "2500"),
    ]);

    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "chat");

    assert_eq!(span.gen_ai_server_ttft_ms, Some(150));
    // Note: request_duration uses a different key
}

#[test]
fn test_strands_agents_tool_status_extraction() {
    // Strands sets gen_ai.tool.status on tool spans
    let attrs = make_attrs(&[
        ("gen_ai.tool.name", "get_weather"),
        ("gen_ai.tool.status", "success"),
    ]);

    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "execute_tool get_weather");

    assert_eq!(span.gen_ai_tool_name, Some("get_weather".to_string()));
    // Tool status is available in attributes
}

#[test]
fn test_token_config_extract() {
    let attrs = make_attrs(&[("gen_ai.usage.input_tokens", "100")]);
    assert_eq!(INPUT_TOKENS.extract(&attrs), 100);

    let attrs = make_attrs(&[("llm.token_count.prompt", "200")]);
    assert_eq!(INPUT_TOKENS.extract(&attrs), 200);

    let attrs = make_attrs(&[]);
    assert_eq!(INPUT_TOKENS.extract(&attrs), 0);
}

#[test]
fn test_traceloop_framework_detection_from_attrs() {
    let span_attrs = make_attrs(&[("traceloop.entity.input", "{}")]);
    let resource_attrs = HashMap::new();

    assert_eq!(
        detect_framework("test", &span_attrs, &resource_attrs),
        Framework::TraceLoop.as_str(),
        "Should detect TraceLoop from traceloop.* attributes"
    );
}

#[test]
fn test_traceloop_framework_detection_from_sdk_name() {
    let span_attrs = HashMap::new();
    let resource_attrs = make_attrs(&[("telemetry.sdk.name", "opentelemetry-traceloop")]);

    assert_eq!(
        detect_framework("test", &span_attrs, &resource_attrs),
        Framework::TraceLoop.as_str(),
        "Should detect TraceLoop from telemetry.sdk.name"
    );
}

#[test]
fn test_vercel_ai_sdk_detection_from_prompt_messages() {
    let span_attrs = make_attrs(&[("ai.prompt.messages", r#"[]"#)]);
    let resource_attrs = HashMap::new();

    assert_eq!(
        detect_framework("test", &span_attrs, &resource_attrs),
        Framework::VercelAISdk.as_str(),
        "Should detect Vercel AI SDK from ai.prompt.messages"
    );
}

#[test]
fn test_vercel_ai_sdk_detection_from_telemetry() {
    let span_attrs = make_attrs(&[("ai.telemetry.functionId", "my-function")]);
    let resource_attrs = HashMap::new();

    assert_eq!(
        detect_framework("test", &span_attrs, &resource_attrs),
        Framework::VercelAISdk.as_str(),
        "Should detect Vercel AI SDK from ai.telemetry.functionId"
    );
}

#[test]
fn test_azure_openai_framework_detection() {
    // Test gen_ai.system = "azure_openai"
    let span_attrs = make_attrs(&[("gen_ai.system", "azure_openai")]);
    let resource_attrs = HashMap::new();
    assert_eq!(
        detect_framework("chat", &span_attrs, &resource_attrs),
        Framework::AzureOpenAI.as_str(),
        "Should detect Azure OpenAI from gen_ai.system=azure_openai"
    );

    // Test gen_ai.system = "azure.openai"
    let span_attrs2 = make_attrs(&[("gen_ai.system", "azure.openai")]);
    assert_eq!(
        detect_framework("chat", &span_attrs2, &resource_attrs),
        Framework::AzureOpenAI.as_str(),
        "Should detect Azure OpenAI from gen_ai.system=azure.openai"
    );

    // Test azure.openai. prefix
    let span_attrs3 = make_attrs(&[("azure.openai.deployment", "my-gpt4")]);
    assert_eq!(
        detect_framework("chat", &span_attrs3, &resource_attrs),
        Framework::AzureOpenAI.as_str(),
        "Should detect Azure OpenAI from azure.openai. attribute prefix"
    );

    // Test gen_ai.provider.name = "azure_openai"
    let span_attrs4 = make_attrs(&[("gen_ai.provider.name", "azure_openai")]);
    assert_eq!(
        detect_framework("chat", &span_attrs4, &resource_attrs),
        Framework::AzureOpenAI.as_str(),
        "Should detect Azure OpenAI from gen_ai.provider.name=azure_openai"
    );

    let current_semconv = make_attrs(&[("gen_ai.provider.name", "azure.ai.openai")]);
    assert_eq!(
        detect_framework("chat", &current_semconv, &resource_attrs),
        Framework::AzureOpenAI.as_str(),
        "Should detect the current Azure OpenAI semantic-convention provider"
    );

    let openinference = make_attrs(&[
        ("llm.provider", "azure"),
        ("llm.system", "openai"),
        ("openinference.span.kind", "LLM"),
    ]);
    assert_eq!(
        detect_framework("ChatCompletion", &openinference, &resource_attrs),
        Framework::AzureOpenAI.as_str(),
        "OpenInference reports Azure and OpenAI as two independently insufficient attributes"
    );
    for incomplete in [
        make_attrs(&[("llm.provider", "azure")]),
        make_attrs(&[("llm.system", "openai")]),
    ] {
        assert_ne!(
            detect_framework("ChatCompletion", &incomplete, &resource_attrs),
            Framework::AzureOpenAI.as_str(),
            "neither half of the OpenInference identity is sufficient alone"
        );
    }
}

#[test]
fn test_google_adk_model_from_llm_request() {
    let attrs = make_attrs(&[(
        "gcp.vertex.agent.llm_request",
        r#"{"model":"gemini-2.0-flash","contents":[{"role":"user","parts":[{"text":"hi"}]}]}"#,
    )]);
    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "call_llm");
    assert_eq!(
        span.gen_ai_request_model,
        Some("gemini-2.0-flash".to_string())
    );
}

#[test]
fn test_google_adk_tokens_from_llm_response() {
    let attrs = make_attrs(&[(
        "gcp.vertex.agent.llm_response",
        r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"hello"}]}}],"usage_metadata":{"prompt_token_count":3788,"candidates_token_count":92,"total_token_count":3880}}"#,
    )]);
    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "call_llm");
    assert_eq!(span.gen_ai_usage_input_tokens, 3788);
    assert_eq!(span.gen_ai_usage_output_tokens, 92);
    assert_eq!(span.gen_ai_usage_total_tokens, 3880);
}

#[test]
fn test_google_adk_standard_attrs_not_overwritten() {
    let attrs = make_attrs(&[
        ("gen_ai.request.model", "claude-3-haiku"),
        ("gen_ai.usage.input_tokens", "100"),
        ("gen_ai.usage.output_tokens", "50"),
        (
            "gcp.vertex.agent.llm_request",
            r#"{"model":"gemini-2.0-flash"}"#,
        ),
        (
            "gcp.vertex.agent.llm_response",
            r#"{"usage_metadata":{"prompt_token_count":9999,"candidates_token_count":8888}}"#,
        ),
    ]);
    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "call_llm");
    assert_eq!(
        span.gen_ai_request_model,
        Some("claude-3-haiku".to_string()),
        "Standard model should not be overwritten by ADK fallback"
    );
    assert_eq!(
        span.gen_ai_usage_input_tokens, 100,
        "Standard tokens should not be overwritten by ADK fallback"
    );
    assert_eq!(span.gen_ai_usage_output_tokens, 50);
}

#[test]
fn test_crewai_tokens_from_output_value() {
    let attrs = make_attrs(&[
        ("crew_key", "test-crew"),
        (
            "output.value",
            r#"{"raw":"result","token_usage":{"total_tokens":1234,"prompt_tokens":567,"cached_prompt_tokens":100,"completion_tokens":678,"successful_requests":3}}"#,
        ),
    ]);
    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "Crew.kickoff");
    assert_eq!(span.gen_ai_usage_input_tokens, 567);
    assert_eq!(span.gen_ai_usage_output_tokens, 678);
    assert_eq!(span.gen_ai_usage_total_tokens, 1245);
    assert_eq!(span.gen_ai_usage_cache_read_tokens, 100);
}

#[test]
fn test_crewai_tokens_total_honors_reported_total() {
    // When total_tokens > prompt + completion, honor the reported total
    let attrs = make_attrs(&[
        ("crew_key", "test-crew"),
        (
            "output.value",
            r#"{"raw":"result","token_usage":{"total_tokens":2000,"prompt_tokens":500,"completion_tokens":600}}"#,
        ),
    ]);
    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "Crew.kickoff");
    assert_eq!(span.gen_ai_usage_input_tokens, 500);
    assert_eq!(span.gen_ai_usage_output_tokens, 600);
    assert_eq!(
        span.gen_ai_usage_total_tokens, 2000,
        "Should use reported total_tokens when it exceeds prompt + completion"
    );
}

#[test]
fn test_crewai_tokens_standard_attrs_not_overwritten() {
    let attrs = make_attrs(&[
        ("crew_key", "test-crew"),
        ("gen_ai.usage.input_tokens", "200"),
        ("gen_ai.usage.output_tokens", "100"),
        (
            "output.value",
            r#"{"raw":"result","token_usage":{"total_tokens":9999,"prompt_tokens":8888,"completion_tokens":7777}}"#,
        ),
    ]);
    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "Crew.kickoff");
    assert_eq!(
        span.gen_ai_usage_input_tokens, 200,
        "Standard tokens should not be overwritten by CrewAI fallback"
    );
    assert_eq!(span.gen_ai_usage_output_tokens, 100);
}

#[test]
fn test_crewai_tokens_not_extracted_without_crewai_attrs() {
    let attrs = make_attrs(&[(
        "output.value",
        r#"{"raw":"result","token_usage":{"prompt_tokens":567,"completion_tokens":678}}"#,
    )]);
    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "some.span");
    assert_eq!(
        span.gen_ai_usage_input_tokens, 0,
        "Should not extract CrewAI tokens without CrewAI attributes"
    );
    assert_eq!(span.gen_ai_usage_output_tokens, 0);
}

#[test]
fn test_crewai_tokens_no_token_usage_field() {
    let attrs = make_attrs(&[
        ("crew_key", "test-crew"),
        (
            "output.value",
            r#"{"raw":"result","agent":"Weather Forecaster"}"#,
        ),
    ]);
    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "Task._execute_core");
    assert_eq!(span.gen_ai_usage_input_tokens, 0);
    assert_eq!(span.gen_ai_usage_output_tokens, 0);
}

#[test]
fn test_crewai_model_from_crew_agents() {
    let attrs = make_attrs(&[(
        "crew_agents",
        r#"[{"key":"abc","id":"1","role":"Forecaster","llm":"global.anthropic.claude-haiku-4-5-20251001-v1:0","tools_names":["temp"]}]"#,
    )]);
    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "Crew.kickoff");
    assert_eq!(
        span.gen_ai_request_model.as_deref(),
        Some("global.anthropic.claude-haiku-4-5-20251001-v1:0")
    );
}

#[test]
fn test_crewai_model_standard_attrs_priority() {
    let attrs = make_attrs(&[
        ("gen_ai.request.model", "claude-3-5-sonnet"),
        ("crew_agents", r#"[{"llm":"bedrock/some-other-model"}]"#),
    ]);
    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "Crew.kickoff");
    assert_eq!(
        span.gen_ai_request_model.as_deref(),
        Some("claude-3-5-sonnet")
    );
}

#[test]
fn test_crewai_model_missing_llm_field() {
    let attrs = make_attrs(&[("crew_agents", r#"[{"key":"abc","role":"Forecaster"}]"#)]);
    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "Crew.kickoff");
    assert_eq!(span.gen_ai_request_model, None);
}

// ============================================================================
// SPAN NAME RESOLUTION
// ============================================================================

#[test]
fn test_resolve_span_name_logfire_template() {
    let attrs = make_attrs(&[
        (
            "logfire.msg_template",
            "Chat Completion with {request_data[model]!r}",
        ),
        ("logfire.msg", "Chat Completion with 'gpt-4o'"),
    ]);
    let mut span = SpanData {
        span_name: "Chat Completion with {request_data[model]!r}".to_string(),
        ..Default::default()
    };

    resolve_span_name(&mut span, &attrs);

    assert_eq!(span.span_name, "Chat Completion with 'gpt-4o'");
}

#[test]
fn test_resolve_span_name_no_template_unchanged() {
    // Without logfire.msg_template, span name stays as-is even if logfire.msg exists
    let attrs = make_attrs(&[("logfire.msg", "some resolved name")]);
    let mut span = SpanData {
        span_name: "original span name".to_string(),
        ..Default::default()
    };

    resolve_span_name(&mut span, &attrs);

    assert_eq!(span.span_name, "original span name");
}

#[test]
fn test_resolve_span_name_template_without_msg_unchanged() {
    // Template exists but resolved msg is missing — keep original
    let attrs = make_attrs(&[("logfire.msg_template", "Chat {model}")]);
    let mut span = SpanData {
        span_name: "Chat {model}".to_string(),
        ..Default::default()
    };

    resolve_span_name(&mut span, &attrs);

    assert_eq!(span.span_name, "Chat {model}");
}

#[test]
fn test_resolve_span_name_empty_msg_unchanged() {
    // Template exists but resolved msg is empty — keep original
    let attrs = make_attrs(&[
        ("logfire.msg_template", "Chat {model}"),
        ("logfire.msg", ""),
    ]);
    let mut span = SpanData {
        span_name: "Chat {model}".to_string(),
        ..Default::default()
    };

    resolve_span_name(&mut span, &attrs);

    assert_eq!(span.span_name, "Chat {model}");
}

#[test]
fn test_resolve_span_name_braces_in_name_no_template() {
    // Span name with braces but no logfire.msg_template — NOT a template, keep as-is
    let attrs = make_attrs(&[]);
    let mut span = SpanData {
        span_name: "process {\"key\": \"value\"}".to_string(),
        ..Default::default()
    };

    resolve_span_name(&mut span, &attrs);

    assert_eq!(span.span_name, "process {\"key\": \"value\"}");
}

#[test]
fn test_bare_token_names_are_scoped_to_claude_code_spans() {
    // The Claude Code CLI uses bare token names. They are too generic to trust
    // globally: another framework emitting `input_tokens` with unrelated semantics
    // must not have tokens (and therefore cost) attributed to it.
    let attrs = make_attrs(&[
        ("input_tokens", "10"),
        ("output_tokens", "205"),
        ("cache_read_tokens", "7"),
        ("cache_creation_tokens", "17649"),
    ]);

    let mut other = SpanData::default();
    extract_genai_as_production_does(&mut other, &attrs, "some.other.framework.span");
    assert_eq!(
        other.gen_ai_usage_input_tokens, 0,
        "must not leak to others"
    );
    assert_eq!(other.gen_ai_usage_output_tokens, 0);
    assert_eq!(other.gen_ai_usage_cache_read_tokens, 0);
    assert_eq!(other.gen_ai_usage_cache_write_tokens, 0);

    let mut cc = SpanData::default();
    extract_genai_as_production_does(&mut cc, &attrs, "claude_code.llm_request");
    assert_eq!(cc.gen_ai_usage_input_tokens, 10);
    assert_eq!(cc.gen_ai_usage_output_tokens, 205);
    assert_eq!(cc.gen_ai_usage_cache_read_tokens, 7);
    assert_eq!(cc.gen_ai_usage_cache_write_tokens, 17649);
}

#[test]
fn test_semconv_conversation_id_populates_session() {
    // gen_ai.conversation.id is the standard semconv session identifier. Without it in the
    // fallback chain, spans from any compliant emitter never group into a session.
    // session_id is populated by apply_span_fields, not extract_genai.
    let attrs = make_attrs(&[("gen_ai.conversation.id", "conv-42")]);
    let mut span = SpanData::default();
    apply_span_fields(&mut span, "", &attrs, &[]);
    assert_eq!(span.session_id.as_deref(), Some("conv-42"));
}

#[test]
fn test_openinference_agent_name_is_extracted() {
    let attrs = make_attrs(&[("agent.name", "ResearchAgent")]);
    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "agent");
    assert_eq!(span.gen_ai_agent_name.as_deref(), Some("ResearchAgent"));
}

#[test]
fn test_openinference_agent_name_does_not_change_category() {
    // categorize_span keys on the raw gen_ai.agent.name attribute, so adding agent.name to
    // the extraction chain must not reclassify existing OpenInference spans.
    let attrs = make_attrs(&[("agent.name", "ResearchAgent")]);
    let mut with_alias = SpanData::default();
    extract_genai_as_production_does(&mut with_alias, &attrs, "some.span");
    let mut without = SpanData::default();
    extract_genai_as_production_does(&mut without, &make_attrs(&[]), "some.span");
    assert_eq!(with_alias.observation_type, without.observation_type);
    assert_eq!(with_alias.span_category, without.span_category);
}

#[test]
fn test_dotted_cache_and_reasoning_token_spellings() {
    let attrs = make_attrs(&[
        ("gen_ai.usage.cache_read.input_tokens", "11"),
        ("gen_ai.usage.cache_creation.input_tokens", "22"),
        ("gen_ai.usage.reasoning.output_tokens", "33"),
    ]);
    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "chat");
    assert_eq!(span.gen_ai_usage_cache_read_tokens, 11);
    assert_eq!(span.gen_ai_usage_cache_write_tokens, 22);
    assert_eq!(span.gen_ai_usage_reasoning_tokens, 33);
}

#[test]
fn test_openinference_instrumented_frameworks_are_detected_specifically() {
    // Each of these is instrumented through OpenInference and emits both its own
    // attributes and openinference.*. The specific rule must win, otherwise every one of
    // them lands as the generic OpenInference framework.
    for (attr, expected) in [
        ("agno.agent.id", Framework::Agno),
        ("smolagents.task", Framework::Smolagents),
        ("agentscope.agent.reply_id", Framework::AgentScope),
        ("langflow.flow_id", Framework::Langflow),
        ("ag2.span.type", Framework::Ag2),
    ] {
        let attrs = make_attrs(&[(attr, "x"), ("openinference.span.kind", "AGENT")]);
        assert_eq!(
            detect_framework("some.span", &attrs, &HashMap::new()),
            expected.as_str(),
            "{attr} should detect as {expected:?}, not OpenInference"
        );
    }
}

#[test]
fn test_plain_openinference_still_detected() {
    // The new rules must not steal spans that carry only openinference.*.
    let attrs = make_attrs(&[("openinference.span.kind", "LLM")]);
    assert_eq!(
        detect_framework("some.span", &attrs, &HashMap::new()),
        Framework::OpenInference.as_str()
    );
}

#[test]
fn test_new_framework_rules_do_not_match_unrelated_services() {
    // service.name matching is a substring test, so keying "agno" on service.name would
    // also match "diagnostics". These rules use attribute prefixes only - prove it.
    let resource = make_attrs(&[("service.name", "diagnostics-api")]);
    assert_ne!(
        detect_framework("some.span", &HashMap::new(), &resource),
        Framework::Agno.as_str()
    );
}

#[test]
fn test_haystack_and_browser_use_detection() {
    let haystack = make_attrs(&[
        ("haystack.component.name", "retriever"),
        ("haystack.component.type", "InMemoryBM25Retriever"),
    ]);
    assert_eq!(
        detect_framework("haystack.component.run", &haystack, &HashMap::new()),
        Framework::Haystack.as_str()
    );

    let browser = make_attrs(&[("gen_ai.provider.name", "browser_use")]);
    assert_eq!(
        detect_framework("agent.step", &browser, &HashMap::new()),
        Framework::BrowserUse.as_str()
    );
}

#[test]
fn test_browser_use_rule_does_not_capture_other_providers() {
    // The rule is an exact attr_equals, so a different provider must not match it.
    for provider in ["anthropic", "openai", "bedrock"] {
        let attrs = make_attrs(&[("gen_ai.provider.name", provider)]);
        assert_ne!(
            detect_framework("chat", &attrs, &HashMap::new()),
            Framework::BrowserUse.as_str(),
            "provider {provider} must not detect as BrowserUse"
        );
    }
}

// ============================================================================
// FALLBACK-CHAIN REGRESSIONS
// ============================================================================

/// An empty value does not win a fallback chain.
///
/// `session.id=""` alongside a real `gen_ai.conversation.id` used to store the empty string, and retrieval
/// treats a stored empty session id as *no session* - so the conversation got no session view, a trace read
/// could not load its sibling traces, and the project feed could not widen its context, which lets replayed
/// history through as duplicates. Present-but-empty is what a chain exists to step over.
#[test]
fn an_empty_value_does_not_win_a_fallback_chain() {
    let attrs = make_attrs(&[(keys::SESSION_ID, ""), ("gen_ai.conversation.id", "conv-1")]);
    assert_eq!(
        get_first(&attrs, &[keys::SESSION_ID, "gen_ai.conversation.id"]),
        Some("conv-1".to_string()),
        "an empty primary must not shadow a real fallback"
    );

    // And a real primary still wins.
    let attrs = make_attrs(&[
        (keys::SESSION_ID, "sess-1"),
        ("gen_ai.conversation.id", "conv-1"),
    ]);
    assert_eq!(
        get_first(&attrs, &[keys::SESSION_ID, "gen_ai.conversation.id"]),
        Some("sess-1".to_string())
    );
}

/// A span reporting only one flat token counter still gets the other from the framework's JSON.
///
/// The ADK and Logfire fallbacks were gated on *both* counters being zero, so input=100 with output absent
/// kept output at 0 even though `usage_metadata.candidates_token_count` had it - understating the total and
/// therefore the cost, which is the number a user is billed against.
#[test]
fn a_partial_usage_counter_still_reaches_the_adk_fallback() {
    let mut span = SpanData::default();
    let attrs = make_attrs(&[
        ("gen_ai.usage.input_tokens", "100"),
        (
            keys::GCP_VERTEX_LLM_RESPONSE,
            r#"{"usage_metadata":{"prompt_token_count":100,"candidates_token_count":20}}"#,
        ),
    ]);
    extract_genai_as_production_does(&mut span, &attrs, "chat");
    assert_eq!(span.gen_ai_usage_input_tokens, 100);
    assert_eq!(
        span.gen_ai_usage_output_tokens, 20,
        "the absent output counter must be filled from the ADK JSON"
    );
    assert_eq!(span.gen_ai_usage_total_tokens, 120);
}

/// The same, for Logfire's `response_data.usage`.
#[test]
fn a_partial_usage_counter_still_reaches_the_logfire_fallback() {
    let mut span = SpanData::default();
    let attrs = make_attrs(&[
        ("gen_ai.usage.input_tokens", "100"),
        (
            keys::RESPONSE_DATA,
            r#"{"usage":{"input_tokens":100,"output_tokens":42}}"#,
        ),
    ]);
    extract_genai_as_production_does(&mut span, &attrs, "chat");
    assert_eq!(span.gen_ai_usage_input_tokens, 100);
    assert_eq!(
        span.gen_ai_usage_output_tokens, 42,
        "the absent output counter must be filled from the Logfire JSON"
    );
}

/// A JSON-sourced `max_tokens` is not overwritten by an absent flat attribute.
///
/// The flat assignment ran unconditionally, so it replaced the `request_data` fallback with `None` on every
/// span that lacked the flat attribute - which is every Logfire span, meaning the value never survived.
#[test]
fn a_json_max_tokens_survives_an_absent_flat_attribute() {
    let mut span = SpanData::default();
    let attrs = make_attrs(&[(
        keys::REQUEST_DATA,
        r#"{"max_completion_tokens":1024,"messages":[]}"#,
    )]);
    extract_genai_as_production_does(&mut span, &attrs, "chat");
    assert_eq!(
        span.gen_ai_max_tokens,
        Some(1024),
        "the JSON fallback must not be overwritten by the absent flat attribute"
    );

    // A present flat attribute still wins.
    let mut span = SpanData::default();
    let attrs = make_attrs(&[
        (keys::GEN_AI_MAX_TOKENS, "512"),
        (keys::REQUEST_DATA, r#"{"max_completion_tokens":1024}"#),
    ]);
    extract_genai_as_production_does(&mut span, &attrs, "chat");
    assert_eq!(span.gen_ai_max_tokens, Some(512));
}

/// A genuine zero token count is not replaced by a framework fallback.
///
/// The fallbacks used `0` as "missing", which is not recoverable from the value: a completion that genuinely
/// produced no output tokens had its reported 0 overwritten by whatever the framework's JSON said, so the
/// stored total and the cost described a different response than the one the provider reported.
#[test]
fn a_reported_zero_is_not_overwritten_by_a_fallback() {
    let mut span = SpanData::default();
    let attrs = make_attrs(&[
        ("gen_ai.usage.input_tokens", "100"),
        ("gen_ai.usage.output_tokens", "0"),
        (
            keys::RESPONSE_DATA,
            r#"{"usage":{"input_tokens":100,"output_tokens":42}}"#,
        ),
    ]);
    extract_genai_as_production_does(&mut span, &attrs, "chat");
    assert_eq!(
        span.gen_ai_usage_output_tokens, 0,
        "a reported zero must survive; the fallback is for an *absent* counter"
    );
    assert_eq!(span.gen_ai_usage_total_tokens, 100);
}

/// A JSON `max_tokens` is reachable even when the model and provider are already known.
///
/// The whole `request_data` parse was gated on the model *or* system being absent, so a well-populated span -
/// the common Logfire shape - skipped it entirely and lost `max_tokens` and the operation-name fallback.
#[test]
fn a_json_max_tokens_is_reachable_when_model_and_system_are_known() {
    let mut span = SpanData::default();
    let attrs = make_attrs(&[
        ("gen_ai.request.model", "gpt-4o"),
        ("gen_ai.system", "openai"),
        (keys::REQUEST_DATA, r#"{"max_completion_tokens":1024}"#),
    ]);
    extract_genai_as_production_does(&mut span, &attrs, "chat");
    assert_eq!(
        span.gen_ai_max_tokens,
        Some(1024),
        "the request_data fallback must not be gated on fields it does not fill"
    );
}

/// A reported zero survives the MLflow fallback too, and an earlier JSON source is not overwritten.
///
/// The fallbacks run in sequence, so testing the *stored value* had two failure modes: a genuine 0 was treated
/// as missing, and a later JSON source overwrote a count an earlier one had legitimately supplied. Both are
/// fixed by tracking whether each counter has been supplied at all.
#[test]
fn a_reported_zero_survives_the_mlflow_fallback() {
    let mut span = SpanData::default();
    let attrs = make_attrs(&[
        ("gen_ai.usage.input_tokens", "100"),
        ("gen_ai.usage.output_tokens", "0"),
        (
            keys::MLFLOW_CHAT_TOKEN_USAGE,
            r#"{"prompt_tokens":100,"completion_tokens":42}"#,
        ),
    ]);
    extract_genai_as_production_does(&mut span, &attrs, "chat");
    assert_eq!(
        span.gen_ai_usage_output_tokens, 0,
        "a reported zero must survive the MLflow fallback"
    );
}

/// The first JSON source to supply a counter keeps it; a later one does not overwrite it.
#[test]
fn a_later_json_fallback_does_not_overwrite_an_earlier_one() {
    let mut span = SpanData::default();
    // No flat counters. MLflow supplies both; Logfire's `response_data` would supply different numbers.
    let attrs = make_attrs(&[
        (
            keys::MLFLOW_CHAT_TOKEN_USAGE,
            r#"{"prompt_tokens":11,"completion_tokens":22}"#,
        ),
        (
            keys::RESPONSE_DATA,
            r#"{"usage":{"input_tokens":99,"output_tokens":88}}"#,
        ),
    ]);
    extract_genai_as_production_does(&mut span, &attrs, "chat");
    assert_eq!(span.gen_ai_usage_input_tokens, 11);
    assert_eq!(
        span.gen_ai_usage_output_tokens, 22,
        "the first source to supply a counter must keep it"
    );
}

/// A provider that reports its cache counters *beside* its input must have them in the total.
///
/// Anthropic's `input_tokens` excludes what it charged to cache creation, so `input + output` is not the
/// total - a prompt-caching turn showed 215 tokens where 17,864 were billed. The rule has to be the
/// pricing module's, or the number on screen contradicts the cost beside it.
#[test]
fn a_separately_reported_cache_counter_is_in_the_synthesised_total() {
    let mut attrs = HashMap::new();
    attrs.insert("gen_ai.system".to_string(), "anthropic".to_string());
    attrs.insert("gen_ai.usage.input_tokens".to_string(), "10".to_string());
    attrs.insert("gen_ai.usage.output_tokens".to_string(), "205".to_string());
    attrs.insert(
        "gen_ai.usage.cache_creation_input_tokens".to_string(),
        "17649".to_string(),
    );

    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "chat anthropic");

    assert_eq!(span.gen_ai_usage_cache_write_tokens, 17_649);
    assert_eq!(
        span.gen_ai_usage_total_tokens, 17_864,
        "an Anthropic total must include the cache creation it was billed for"
    );
}

/// And a provider that reports them *inside* its input must not have them counted twice.
#[test]
fn an_included_cache_counter_is_not_added_to_the_total() {
    let mut attrs = HashMap::new();
    attrs.insert("gen_ai.system".to_string(), "openai".to_string());
    attrs.insert("gen_ai.usage.input_tokens".to_string(), "1000".to_string());
    attrs.insert("gen_ai.usage.output_tokens".to_string(), "50".to_string());
    attrs.insert(
        "gen_ai.usage.cache_read_input_tokens".to_string(),
        "800".to_string(),
    );

    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "chat openai");

    assert_eq!(span.gen_ai_usage_cache_read_tokens, 800);
    assert_eq!(
        span.gen_ai_usage_total_tokens, 1_050,
        "OpenAI's cached tokens are already inside its prompt total"
    );
}

/// Gemini reports thoughts separately while counting cached content inside its prompt total - the mirror
/// image of Anthropic, and the reason the two conventions are separate questions.
#[test]
fn gemini_counts_its_thoughts_beside_the_output_and_its_cache_inside_the_input() {
    let mut attrs = HashMap::new();
    attrs.insert("gen_ai.system".to_string(), "gemini".to_string());
    attrs.insert("gen_ai.usage.input_tokens".to_string(), "500".to_string());
    attrs.insert("gen_ai.usage.output_tokens".to_string(), "80".to_string());
    attrs.insert(
        "gen_ai.usage.cache_read_input_tokens".to_string(),
        "400".to_string(),
    );
    attrs.insert(
        "gen_ai.usage.reasoning_tokens".to_string(),
        "300".to_string(),
    );

    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "chat gemini");

    assert_eq!(
        span.gen_ai_usage_total_tokens,
        500 + 80 + 300,
        "thoughts are extra, cached content is not"
    );
}

/// A total the provider states is honoured when it exceeds what the counters account for, which is how a
/// provider reporting counters this code does not know about still shows the right number.
#[test]
fn a_reported_total_larger_than_the_counters_is_honoured() {
    let mut attrs = HashMap::new();
    attrs.insert("gen_ai.system".to_string(), "anthropic".to_string());
    attrs.insert("gen_ai.usage.input_tokens".to_string(), "10".to_string());
    attrs.insert("gen_ai.usage.output_tokens".to_string(), "20".to_string());
    attrs.insert("gen_ai.usage.total_tokens".to_string(), "9999".to_string());

    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "chat anthropic");

    assert_eq!(span.gen_ai_usage_total_tokens, 9_999);
}

/// A framework fallback fills only the side that was not reported.
///
/// The gate was `input == 0 && output == 0`, which conflates "the provider said 0" with "nobody said
/// anything" - in both directions. With a flat input of 200 and no output attribute the CrewAI fallback was
/// skipped entirely, so the output stayed 0 and its cost was never charged.
#[test]
fn a_fallback_fills_the_missing_side_when_the_other_was_reported() {
    let mut attrs = HashMap::new();
    attrs.insert("crew_key".to_string(), "crew-1".to_string());
    attrs.insert("gen_ai.usage.input_tokens".to_string(), "200".to_string());
    attrs.insert(
        "output.value".to_string(),
        serde_json::json!({
            "token_usage": {"prompt_tokens": 200, "completion_tokens": 100, "total_tokens": 300}
        })
        .to_string(),
    );

    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "crew");

    assert_eq!(span.gen_ai_usage_input_tokens, 200);
    assert_eq!(
        span.gen_ai_usage_output_tokens, 100,
        "the output side was never reported, so the fallback must supply it"
    );
    assert_eq!(span.gen_ai_usage_total_tokens, 300);
}
