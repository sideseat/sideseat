use std::collections::HashMap;

use crate::traces::extract::framework_oracle::Framework;
use sideseat_ports::types::{ObservationType, SpanCategory};

use super::*;

fn make_attrs(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

// ============================================================================
// HELPER FUNCTION TESTS
// ============================================================================

/// The two steps production runs per span, in production's order: every declared field, then the token
/// accounting. A test calling only one of them was asserting against half the pipeline.
pub(crate) fn extract_genai_as_production_does(
    span: &mut SpanData,
    attrs: &HashMap<String, String>,
    span_name: &str,
) {
    let tokens = apply_span_fields(span, span_name, attrs, &[]);
    extract_genai(span, attrs, span_name, &tokens);
}

#[test]
fn test_contains_ascii_ignore_case() {
    // Basic cases
    assert!(contains_ascii_ignore_case(
        "amazon.titan-embed-text-v2:0",
        "embed"
    ));
    assert!(contains_ascii_ignore_case(
        "text-EMBEDDING-ada-002",
        "embed"
    ));
    assert!(contains_ascii_ignore_case("Embed-english-v3.0", "embed"));

    // Negative cases
    assert!(!contains_ascii_ignore_case("gpt-4", "embed"));
    assert!(!contains_ascii_ignore_case("claude-3", "embed"));

    // Edge cases
    assert!(contains_ascii_ignore_case("embed", "embed"));
    assert!(contains_ascii_ignore_case("EMBED", "embed"));
    assert!(!contains_ascii_ignore_case("embe", "embed"));
    assert!(contains_ascii_ignore_case("anything", ""));
    assert!(!contains_ascii_ignore_case("", "embed"));
}

#[test]
fn test_autogen_framework_detection() {
    let span_attrs = make_attrs(&[("gen_ai.system", "autogen")]);
    let resource_attrs = HashMap::new();
    assert_eq!(
        detect_framework("test", &span_attrs, &resource_attrs),
        Framework::AutoGen.as_str()
    );

    let span_attrs2 = HashMap::new();
    let resource_attrs2 = HashMap::new();
    assert_eq!(
        detect_framework("autogen process Agent", &span_attrs2, &resource_attrs2),
        Framework::AutoGen.as_str()
    );
}

#[test]
fn test_aws_bedrock_agent_id_extraction() {
    // AWS Bedrock agent ID should be extracted
    let attrs = make_attrs(&[("aws.bedrock.agent.id", "agent-abc123")]);

    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "test");

    assert_eq!(span.gen_ai_agent_id, Some("agent-abc123".to_string()));
}

#[test]
fn test_aws_bedrock_framework_detection_from_attrs() {
    // AWS Bedrock should be detected from aws.bedrock.* attributes
    let span_attrs = make_attrs(&[("aws.bedrock.agent.id", "agent-123")]);
    let resource_attrs = HashMap::new();

    assert_eq!(
        detect_framework("test", &span_attrs, &resource_attrs),
        Framework::AWSBedrock.as_str(),
        "Should detect AWS Bedrock from aws.bedrock.* attributes"
    );
}

#[test]
fn test_aws_bedrock_framework_detection_from_gen_ai_system() {
    // AWS Bedrock should be detected from gen_ai.system = "aws_bedrock"
    let span_attrs = make_attrs(&[("gen_ai.system", "aws_bedrock")]);
    let resource_attrs = HashMap::new();

    assert_eq!(
        detect_framework("test", &span_attrs, &resource_attrs),
        Framework::AWSBedrock.as_str(),
        "Should detect AWS Bedrock from gen_ai.system"
    );
}

#[test]
fn test_aws_bedrock_framework_detection_from_gen_ai_system_dotted() {
    // AWS Bedrock should also be detected from gen_ai.system = "aws.bedrock"
    let span_attrs = make_attrs(&[("gen_ai.system", "aws.bedrock")]);
    let resource_attrs = HashMap::new();

    assert_eq!(
        detect_framework("test", &span_attrs, &resource_attrs),
        Framework::AWSBedrock.as_str(),
        "Should detect AWS Bedrock from gen_ai.system with dotted format"
    );
}

#[test]
fn test_categorize_span_agent_from_operation() {
    let attrs = make_attrs(&[("gen_ai.operation.name", "invoke_agent")]);
    assert_eq!(categorize_span("test", &attrs), SpanCategory::Agent);

    let attrs2 = make_attrs(&[("gen_ai.operation.name", "execute_event_loop_cycle")]);
    assert_eq!(categorize_span("test", &attrs2), SpanCategory::Agent);
}

#[test]
fn test_categorize_span_db() {
    let attrs = make_attrs(&[("db.system", "postgresql")]);
    assert_eq!(categorize_span("test", &attrs), SpanCategory::DB);
}

#[test]
fn test_categorize_span_from_semantic_kind() {
    let attrs = make_attrs(&[("openinference.span.kind", "CHAIN")]);
    assert_eq!(
        categorize_span("RunnableSequence", &attrs),
        SpanCategory::Chain
    );

    let attrs2 = make_attrs(&[("openinference.span.kind", "RETRIEVER")]);
    assert_eq!(categorize_span("test", &attrs2), SpanCategory::Retriever);
}

#[test]
fn test_categorize_span_http() {
    let attrs = make_attrs(&[("http.method", "GET")]);
    assert_eq!(categorize_span("test", &attrs), SpanCategory::HTTP);
}

#[test]
fn test_categorize_span_rpc_not_llm() {
    // RPC spans should be HTTP even if they have GenAI attributes
    let attrs = make_attrs(&[("rpc.system", "aws-api"), ("gen_ai.operation.name", "chat")]);
    assert_eq!(
        categorize_span("Bedrock Runtime.Converse", &attrs),
        SpanCategory::HTTP,
        "RPC spans should be HTTP even with GenAI attributes"
    );
}

#[test]
fn test_categorize_span_embedding_model_with_text_completion() {
    // Embedding models should be Embedding even with text_completion operation
    let attrs = make_attrs(&[
        ("gen_ai.operation.name", "text_completion"),
        ("gen_ai.request.model", "amazon.titan-embed-text-v2:0"),
    ]);
    assert_eq!(
        categorize_span("text_completion", &attrs),
        SpanCategory::Embedding,
        "Embedding models should be categorized as Embedding"
    );
}

#[test]
fn test_categorize_span_llm() {
    let attrs = make_attrs(&[("gen_ai.operation.name", "chat")]);
    assert_eq!(categorize_span("test", &attrs), SpanCategory::LLM);
}

#[test]
fn test_categorize_span_tool_from_operation() {
    let attrs = make_attrs(&[("gen_ai.operation.name", "execute_tool")]);
    assert_eq!(categorize_span("test", &attrs), SpanCategory::Tool);
}

#[test]
fn test_crewai_framework_detection() {
    let span_attrs = make_attrs(&[("crew_key", "13e3a57e3b2ed1f7f043b80d762f69e8")]);
    let resource_attrs = make_attrs(&[("service.name", "crewAI-telemetry")]);
    assert_eq!(
        detect_framework("Crew.kickoff", &span_attrs, &resource_attrs),
        Framework::CrewAI.as_str()
    );
}

#[test]
fn test_detect_observation_type_agent() {
    let attrs = make_attrs(&[("gen_ai.agent.name", "Weather Forecaster")]);
    assert_eq!(
        detect_observation_type("agent", &attrs),
        ObservationType::Agent
    );

    let attrs2 = make_attrs(&[("gen_ai.agent.id", "123")]);
    assert_eq!(
        detect_observation_type("test", &attrs2),
        ObservationType::Agent
    );
}

#[test]
fn test_detect_observation_type_embedding() {
    let attrs = make_attrs(&[("gen_ai.operation.name", "embeddings")]);
    assert_eq!(
        detect_observation_type("test", &attrs),
        ObservationType::Embedding
    );
}

#[test]
fn test_detect_observation_type_from_model() {
    let attrs = make_attrs(&[("gen_ai.request.model", "gpt-4")]);
    assert_eq!(
        detect_observation_type("test", &attrs),
        ObservationType::Generation
    );
}

#[test]
fn test_detect_observation_type_from_name() {
    let attrs = HashMap::new();
    assert_eq!(
        detect_observation_type("my-retriever-span", &attrs),
        ObservationType::Retriever
    );
}

#[test]
fn test_detect_observation_type_from_openinference() {
    let attrs = make_attrs(&[("openinference.span.kind", "AGENT")]);
    assert_eq!(
        detect_observation_type("test", &attrs),
        ObservationType::Agent
    );
}

#[test]
fn test_detect_observation_type_generation() {
    let attrs = make_attrs(&[("gen_ai.operation.name", "chat")]);
    assert_eq!(
        detect_observation_type("test", &attrs),
        ObservationType::Generation
    );
}

#[test]
fn test_detect_observation_type_tool_from_operation() {
    let attrs = make_attrs(&[("gen_ai.operation.name", "execute_tool")]);
    let obs = detect_observation_type("execute_tool weather_forecast", &attrs);
    assert_eq!(obs, ObservationType::Tool);
}

/// An MCP client's `tools/call` span is a tool call even when the caller's agent name was propagated onto it,
/// as Logfire copies Pydantic AI's baggage onto every span. The agent name used to make it an `agent` observation
/// with an `agent` category, or - where the span also named the tool - an `agent` with a `tool` category.
#[test]
fn an_mcp_tool_call_outweighs_a_propagated_agent_name() {
    for attrs in [
        make_attrs(&[
            ("mcp.method.name", "tools/call"),
            ("gen_ai.agent.name", "agent"),
        ]),
        make_attrs(&[
            ("mcp.method.name", "tools/call"),
            ("gen_ai.agent.name", "agent"),
            ("gen_ai.tool.name", "calculate"),
        ]),
    ] {
        assert_eq!(
            detect_observation_type("tools/call calculate", &attrs),
            ObservationType::Tool
        );
        assert_eq!(
            categorize_span("tools/call calculate", &attrs),
            SpanCategory::Tool
        );
    }
}

#[test]
fn traceloop_tool_kind_outweighs_the_owning_agent_name() {
    use sideseat_domain::rules::schema::SpanFact;

    let attrs = make_attrs(&[
        ("traceloop.span.kind", "tool"),
        ("gen_ai.agent.name", "weather-agent"),
        ("gen_ai.tool.name", "get_weather"),
    ]);

    assert_eq!(
        detect_observation_type("get_weather.tool", &attrs),
        ObservationType::Tool
    );
    assert_eq!(
        categorize_span("get_weather.tool", &attrs),
        SpanCategory::Tool
    );
    assert!(
        sideseat_domain::rules::ruleset()
            .span_facts
            .holds(SpanFact::ToolExecution, &attrs)
    );
}

#[test]
fn traceloop_decorator_kinds_classify_from_their_explicit_attribute() {
    for (kind, observation, category) in [
        ("agent", ObservationType::Agent, SpanCategory::Agent),
        ("task", ObservationType::Chain, SpanCategory::Chain),
        ("workflow", ObservationType::Chain, SpanCategory::Chain),
    ] {
        let attrs = make_attrs(&[("traceloop.span.kind", kind)]);
        assert_eq!(detect_observation_type("opaque", &attrs), observation);
        assert_eq!(categorize_span("opaque", &attrs), category);
    }
}

#[test]
fn test_detect_observation_type_rpc_not_retriever() {
    // RPC spans should not be classified as Retriever even if name contains "retriev"
    let attrs = make_attrs(&[("rpc.system", "aws-api")]);
    let obs = detect_observation_type("Bedrock AgentCore.RetrieveMemoryRecords", &attrs);
    assert_eq!(obs, ObservationType::Span);
}

#[test]
fn test_detect_observation_type_http_not_retriever() {
    // HTTP spans should not be classified as Retriever even if name contains "retriev"
    let attrs = make_attrs(&[("http.method", "GET")]);
    let obs = detect_observation_type("retrieve-data", &attrs);
    assert_eq!(obs, ObservationType::Span);
}

#[test]
fn test_regression_rpc_with_genai_attrs_not_generation() {
    // Regression: AWS Bedrock API calls have rpc.system=aws-api AND gen_ai.operation.name=chat
    // These should be classified as Span, not Generation (the actual LLM work happens elsewhere)
    let attrs = make_attrs(&[
        ("rpc.system", "aws-api"),
        ("gen_ai.operation.name", "chat"),
        ("gen_ai.request.model", "anthropic.claude-3-sonnet"),
    ]);
    let obs = detect_observation_type("Bedrock Runtime.Converse", &attrs);
    assert_eq!(
        obs,
        ObservationType::Span,
        "RPC spans should be Span even with GenAI attributes"
    );
}

#[test]
fn test_regression_embedding_model_with_text_completion_op() {
    // Regression: Some telemetry reports embedding models with gen_ai.operation.name=text_completion
    // These should be classified as Embedding based on model name, not Generation
    let attrs = make_attrs(&[
        ("gen_ai.operation.name", "text_completion"),
        ("gen_ai.request.model", "amazon.titan-embed-text-v2:0"),
    ]);
    let obs = detect_observation_type("text_completion amazon.titan-embed-text-v2:0", &attrs);
    assert_eq!(
        obs,
        ObservationType::Embedding,
        "Embedding models should be Embedding even with text_completion operation"
    );
}

#[test]
fn test_extract_agent_tool_from_span_name() {
    let attrs = HashMap::new();
    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "execute_tool get_weather");
    assert_eq!(span.gen_ai_tool_name, Some("get_weather".to_string()));
}

#[test]
fn test_extract_genai_agent_fields() {
    let attrs = make_attrs(&[
        ("gen_ai.agent.name", "Weather Forecaster"),
        ("gen_ai.agent.id", "79845d4d-7678-4dcd-a6df-e49191c3153d"),
        ("gen_ai.tool.name", "get_weather"),
        ("gen_ai.tool.call.id", "tooluse_ehAKs6dKRFS5DAfnsNn_xQ"),
    ]);
    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "execute_tool");

    assert_eq!(
        span.gen_ai_agent_name,
        Some("Weather Forecaster".to_string())
    );
    assert_eq!(
        span.gen_ai_agent_id,
        Some("79845d4d-7678-4dcd-a6df-e49191c3153d".to_string())
    );
    assert_eq!(span.gen_ai_tool_name, Some("get_weather".to_string()));
    assert_eq!(
        span.gen_ai_tool_call_id,
        Some("tooluse_ehAKs6dKRFS5DAfnsNn_xQ".to_string())
    );
}

#[test]
fn test_extract_genai_models() {
    let attrs = make_attrs(&[
        ("gen_ai.request.model", "gpt-4"),
        ("gen_ai.response.model", "gpt-4-0613"),
    ]);
    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "test");
    assert_eq!(span.gen_ai_request_model, Some("gpt-4".to_string()));
    assert_eq!(span.gen_ai_response_model, Some("gpt-4-0613".to_string()));
}

#[test]
fn test_extract_genai_performance_metrics() {
    let attrs = make_attrs(&[
        ("gen_ai.server.time_to_first_token", "993"),
        ("gen_ai.server.request_duration", "1143"),
    ]);
    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "chat");

    assert_eq!(span.gen_ai_server_ttft_ms, Some(993));
    assert_eq!(span.gen_ai_server_request_duration_ms, Some(1143));
}

#[test]
fn test_extract_genai_system() {
    let attrs = make_attrs(&[("gen_ai.system", "openai")]);
    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "test");
    assert_eq!(span.gen_ai_system, Some("openai".to_string()));

    let attrs = make_attrs(&[("llm.provider", "anthropic")]);
    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "test");
    assert_eq!(span.gen_ai_system, Some("anthropic".to_string()));
}

#[test]
fn test_extract_genai_usage() {
    let attrs = make_attrs(&[
        ("gen_ai.usage.input_tokens", "100"),
        ("gen_ai.usage.output_tokens", "50"),
    ]);
    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "test");
    assert_eq!(span.gen_ai_usage_input_tokens, 100);
    assert_eq!(span.gen_ai_usage_output_tokens, 50);
    assert_eq!(span.gen_ai_usage_total_tokens, 150);
}

#[test]
fn test_extract_session_from_metadata() {
    let attrs = make_attrs(&[(
        "metadata",
        r#"{"thread_id": "langgraph-demo-dea531b92e3b4dd0", "user_id": "demo-user"}"#,
    )]);
    let mut span = SpanData::default();
    apply_span_fields(&mut span, "", &attrs, &[]);

    assert_eq!(
        span.session_id,
        Some("langgraph-demo-dea531b92e3b4dd0".to_string())
    );
    assert_eq!(span.user_id, Some("demo-user".to_string()));
}

#[test]
fn test_extract_tags_all_sources() {
    let attrs = make_attrs(&[
        ("tags", r#"["base"]"#),
        ("langsmith.tags", r#"["langsmith"]"#),
        ("tag.tags", r#"["openinference"]"#),
    ]);
    let mut span = SpanData::default();
    apply_span_fields(&mut span, "", &attrs, &[]);

    assert!(span.tags.contains(&"base".to_string()));
    assert!(span.tags.contains(&"langsmith".to_string()));
    assert!(span.tags.contains(&"openinference".to_string()));
    assert_eq!(span.tags.len(), 3);
}

#[test]
fn test_extract_tags_merge() {
    let attrs = make_attrs(&[
        ("tags", r#"["production", "weather"]"#),
        ("langsmith.tags", r#"["test", "weather"]"#),
    ]);
    let mut span = SpanData::default();
    apply_span_fields(&mut span, "", &attrs, &[]);

    assert!(span.tags.contains(&"production".to_string()));
    assert!(span.tags.contains(&"weather".to_string()));
    assert!(span.tags.contains(&"test".to_string()));
    assert_eq!(span.tags.iter().filter(|t| *t == "weather").count(), 1);
}

#[test]
fn test_extract_tags_openinference_tag_tags() {
    let attrs = make_attrs(&[
        ("tags", r#"["existing"]"#),
        ("tag.tags", r#"["openinference", "phoenix"]"#),
    ]);
    let mut span = SpanData::default();
    apply_span_fields(&mut span, "", &attrs, &[]);

    assert!(span.tags.contains(&"existing".to_string()));
    assert!(span.tags.contains(&"openinference".to_string()));
    assert!(span.tags.contains(&"phoenix".to_string()));
    assert_eq!(span.tags.len(), 3);
}

#[test]
fn test_extract_usage_openinference() {
    let attrs = make_attrs(&[
        ("llm.token_count.prompt", "618"),
        ("llm.token_count.completion", "73"),
        ("llm.token_count.total", "691"),
    ]);
    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "test");
    assert_eq!(span.gen_ai_usage_input_tokens, 618);
    assert_eq!(span.gen_ai_usage_output_tokens, 73);
    assert_eq!(span.gen_ai_usage_total_tokens, 691);
}

#[test]
fn test_get_first() {
    let attrs = make_attrs(&[("key2", "value2")]);
    assert_eq!(
        get_first(&attrs, &["key1", "key2"]),
        Some("value2".to_string())
    );
    assert_eq!(get_first(&attrs, &["key3"]), None);
}

#[test]
fn test_langchain_framework_detection() {
    let span_attrs = make_attrs(&[("langsmith.tags", "[\"test\"]")]);
    let resource_attrs = HashMap::new();
    assert_eq!(
        detect_framework("ChatBedrock", &span_attrs, &resource_attrs),
        Framework::LangChain.as_str()
    );
}

#[test]
fn test_langgraph_framework_detection() {
    let span_attrs = HashMap::new();
    let resource_attrs = HashMap::new();
    assert_eq!(
        detect_framework("LangGraph", &span_attrs, &resource_attrs),
        Framework::LangGraph.as_str()
    );

    let span_attrs2 = make_attrs(&[("metadata", r#"{"langgraph_step": 1}"#)]);
    assert_eq!(
        detect_framework("agent", &span_attrs2, &resource_attrs),
        Framework::LangGraph.as_str()
    );
}

#[test]
fn test_langgraph_framework_detection_by_attrs() {
    let span_attrs = make_attrs(&[("langgraph.node", "agent")]);
    let resource_attrs = HashMap::new();
    assert_eq!(
        detect_framework("test", &span_attrs, &resource_attrs),
        Framework::LangGraph.as_str()
    );

    let span_attrs2 = HashMap::new();
    let resource_attrs2 = HashMap::new();
    assert_eq!(
        detect_framework("LangGraph.agent", &span_attrs2, &resource_attrs2),
        Framework::LangGraph.as_str()
    );
}

#[test]
fn test_langsmith_framework_detection() {
    let span_attrs = make_attrs(&[("langsmith.span.kind", "llm")]);
    let resource_attrs = HashMap::new();
    assert_eq!(
        detect_framework("ChatOpenAI", &span_attrs, &resource_attrs),
        Framework::LangChain.as_str()
    );
}

#[test]
fn test_langsmith_session_id_extraction() {
    let attrs = make_attrs(&[
        ("langsmith.trace.session_id", "session-abc-123"),
        ("langsmith.span.kind", "chain"),
    ]);
    let mut span = SpanData::default();
    apply_span_fields(&mut span, "", &attrs, &[]);

    assert_eq!(span.session_id, Some("session-abc-123".to_string()));
}

#[test]
fn test_livekit_framework_detection() {
    let span_attrs = make_attrs(&[("lk.input_text", "Hello")]);
    let resource_attrs = HashMap::new();

    assert_eq!(
        detect_framework("speech_to_text", &span_attrs, &resource_attrs),
        Framework::LiveKit.as_str(),
        "Should detect LiveKit from lk.* attributes"
    );
}

#[test]
fn test_logfire_framework_detection() {
    // Logfire should be detected from logfire.* attributes
    let span_attrs = make_attrs(&[("logfire.msg", "test span")]);
    let resource_attrs = HashMap::new();

    assert_eq!(
        detect_framework("test", &span_attrs, &resource_attrs),
        Framework::Logfire.as_str(),
        "Should detect Logfire from logfire.msg attribute"
    );
}

#[test]
fn test_logfire_framework_detection_from_sdk() {
    // Logfire should be detected from telemetry.sdk.name
    let span_attrs = HashMap::new();
    let resource_attrs = make_attrs(&[("telemetry.sdk.name", "logfire")]);

    assert_eq!(
        detect_framework("test", &span_attrs, &resource_attrs),
        Framework::Logfire.as_str(),
        "Should detect Logfire from telemetry.sdk.name"
    );
}

#[test]
fn test_mlflow_framework_detection() {
    let span_attrs = make_attrs(&[("mlflow.spanInputs", "{}")]);
    let resource_attrs = HashMap::new();

    assert_eq!(
        detect_framework("test", &span_attrs, &resource_attrs),
        Framework::MLFlow.as_str(),
        "Should detect MLflow from mlflow.* attributes"
    );
}

#[test]
fn test_openai_agents_framework_detection_from_attrs() {
    // OpenAI Agents SDK should be detected from openai.agents.* attributes
    let span_attrs = make_attrs(&[("openai.agents.span.type", "generation")]);
    let resource_attrs = HashMap::new();

    assert_eq!(
        detect_framework("test", &span_attrs, &resource_attrs),
        Framework::OpenAIAgents.as_str(),
        "Should detect OpenAI Agents SDK from openai.agents.* attributes"
    );
}

#[test]
fn test_openai_agents_framework_detection_from_service_name() {
    // OpenAI Agents SDK should be detected from service.name
    let span_attrs = HashMap::new();
    let resource_attrs = make_attrs(&[("service.name", "openai-agents")]);

    assert_eq!(
        detect_framework("test", &span_attrs, &resource_attrs),
        Framework::OpenAIAgents.as_str(),
        "Should detect OpenAI Agents SDK from service.name"
    );
}

#[test]
fn test_openai_agents_framework_detection_from_service_name_contains() {
    // OpenAI Agents SDK should be detected from service.name containing openai-agents
    let span_attrs = HashMap::new();
    let resource_attrs = make_attrs(&[("service.name", "my-app-openai-agents-v1")]);

    assert_eq!(
        detect_framework("test", &span_attrs, &resource_attrs),
        Framework::OpenAIAgents.as_str(),
        "Should detect OpenAI Agents SDK from service.name containing openai-agents"
    );
}

#[test]
fn test_pydantic_ai_agent_name_extraction() {
    // gen_ai.agent.name should be extracted for agent spans
    let attrs = make_attrs(&[("gen_ai.agent.name", "weather_agent")]);

    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "invoke_agent weather_agent");

    assert_eq!(
        span.gen_ai_agent_name,
        Some("weather_agent".to_string()),
        "Should extract agent name from gen_ai.agent.name"
    );
}

#[test]
fn test_pydantic_ai_logfire_msg_not_override_explicit_tool_name() {
    // If gen_ai.tool.name is set, logfire.msg should not override it
    let attrs = make_attrs(&[
        ("gen_ai.tool.name", "explicit_tool"),
        ("logfire.msg", "descriptive_name"),
    ]);

    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "running tool");

    assert_eq!(
        span.gen_ai_tool_name,
        Some("explicit_tool".to_string()),
        "gen_ai.tool.name should take priority over logfire.msg"
    );
}

#[test]
fn test_pydantic_ai_tool_name_from_logfire_msg() {
    // Pydantic AI uses logfire.msg for descriptive span/tool names
    let attrs = make_attrs(&[("logfire.msg", "get_weather")]);

    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "running tool");

    assert_eq!(
        span.gen_ai_tool_name,
        Some("get_weather".to_string()),
        "Should extract tool name from logfire.msg"
    );
}

#[test]
fn test_session_id_priority_session_id_over_telemetry() {
    // session.id should take priority over ai.telemetry.metadata.sessionId
    let attrs = make_attrs(&[
        ("session.id", "primary-session"),
        ("ai.telemetry.metadata.sessionId", "fallback-session"),
    ]);

    let mut span = SpanData::default();
    apply_span_fields(&mut span, "", &attrs, &[]);

    assert_eq!(
        span.session_id,
        Some("primary-session".to_string()),
        "session.id should take priority over ai.telemetry.metadata.sessionId"
    );
}

#[test]
fn test_strands_agents_agent_span_attributes() {
    // Strands agent spans have specific attributes
    let attrs = make_attrs(&[
        ("gen_ai.operation.name", "invoke_agent"),
        ("gen_ai.agent.name", "weather_agent"),
        ("gen_ai.request.model", "claude-3-opus"),
        ("gen_ai.agent.tools", r#"["get_weather", "send_email"]"#),
    ]);

    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "invoke_agent weather_agent");

    assert_eq!(span.gen_ai_operation_name, Some("invoke_agent".to_string()));
    assert_eq!(span.gen_ai_agent_name, Some("weather_agent".to_string()));
    assert_eq!(span.gen_ai_request_model, Some("claude-3-opus".to_string()));
}

#[test]
fn test_strands_agents_cache_read_tokens() {
    // Strands uses gen_ai.usage.cache_read_input_tokens
    let attrs = make_attrs(&[("gen_ai.usage.cache_read_input_tokens", "75")]);

    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "chat");

    assert_eq!(span.gen_ai_usage_cache_read_tokens, 75);
}

#[test]
fn test_strands_agents_cache_write_tokens() {
    // Strands uses gen_ai.usage.cache_write_input_tokens
    let attrs = make_attrs(&[
        ("gen_ai.usage.input_tokens", "100"),
        ("gen_ai.usage.output_tokens", "50"),
        ("gen_ai.usage.cache_write_input_tokens", "25"),
    ]);

    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "chat");

    assert_eq!(span.gen_ai_usage_input_tokens, 100);
    assert_eq!(span.gen_ai_usage_output_tokens, 50);
    assert_eq!(span.gen_ai_usage_cache_write_tokens, 25);
}

#[test]
fn test_claude_agent_sdk_token_extraction() {
    // Verbatim attribute shape captured from a real claude_code.llm_request span.
    // The CLI uses bare token names, not the gen_ai.usage.* convention, so without
    // the fallbacks every token and cost is reported as 0.
    let attrs = make_attrs(&[
        (
            "gen_ai.request.model",
            "global.anthropic.claude-haiku-4-5-20251001-v1:0",
        ),
        ("gen_ai.system", "anthropic"),
        ("input_tokens", "10"),
        ("output_tokens", "205"),
        ("cache_creation_tokens", "17649"),
        ("cache_read_tokens", "0"),
        ("span.type", "llm_request"),
    ]);
    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "claude_code.llm_request");

    assert_eq!(span.gen_ai_usage_input_tokens, 10);
    assert_eq!(span.gen_ai_usage_output_tokens, 205);
    assert_eq!(span.gen_ai_usage_cache_write_tokens, 17649);
    assert_eq!(span.gen_ai_usage_cache_read_tokens, 0);
}

#[test]
fn test_standard_usage_keys_win_over_bare_fallbacks() {
    // The bare names sit last in the chain, so a framework emitting both must still
    // resolve to the gen_ai.usage.* values.
    let attrs = make_attrs(&[
        ("gen_ai.usage.input_tokens", "100"),
        ("gen_ai.usage.output_tokens", "200"),
        ("input_tokens", "1"),
        ("output_tokens", "2"),
    ]);
    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "chat");

    assert_eq!(span.gen_ai_usage_input_tokens, 100);
    assert_eq!(span.gen_ai_usage_output_tokens, 200);
}

#[test]
fn test_claude_agent_sdk_framework_detection_via_span_names() {
    let empty_attrs = HashMap::new();
    let resource_attrs = HashMap::new();

    // Every span the Claude Code CLI emits carries the claude_code. prefix
    for span_name in &[
        "claude_code.interaction",
        "claude_code.llm_request",
        "claude_code.tool",
        "claude_code.tool.execution",
        "claude_code.tool.blocked_on_user",
        "claude_code.hook",
    ] {
        assert_eq!(
            detect_framework(span_name, &empty_attrs, &resource_attrs),
            Framework::ClaudeAgentSdk.as_str(),
            "span name '{span_name}' should detect Claude Agent SDK"
        );
    }
}

#[test]
fn test_claude_agent_sdk_framework_detection_via_service_name() {
    let empty_attrs = HashMap::new();

    // CLI default, host-process default, plus an overridden one (matched by `contains`)
    for service_name in &["claude-code", "claude-agent-sdk", "claude-code-sample"] {
        let resource_attrs = make_attrs(&[("service.name", service_name)]);
        assert_eq!(
            detect_framework("chat", &empty_attrs, &resource_attrs),
            Framework::ClaudeAgentSdk.as_str(),
            "service.name='{service_name}' should detect Claude Agent SDK"
        );
    }
}

#[test]
fn test_claude_agent_sdk_does_not_shadow_other_frameworks() {
    let empty_attrs = HashMap::new();
    let resource_attrs = HashMap::new();

    // A bare "claude_code" without the dot separator is not a CLI span name
    assert_ne!(
        detect_framework("claude_code", &empty_attrs, &resource_attrs),
        Framework::ClaudeAgentSdk.as_str(),
        "bare 'claude_code' should not match"
    );

    // The rule sits before the Strands service-name fallback but must not steal its spans
    let strands_attrs = make_attrs(&[("gen_ai.system", "strands-agents")]);
    assert_eq!(
        detect_framework("chat", &strands_attrs, &resource_attrs),
        Framework::StrandsAgents.as_str(),
        "Strands spans should still detect as Strands"
    );
}

#[test]
fn test_strands_agents_framework_detection() {
    let span_attrs = make_attrs(&[("gen_ai.system", "strands-agents")]);
    let resource_attrs = HashMap::new();
    assert_eq!(
        detect_framework("chat", &span_attrs, &resource_attrs),
        Framework::StrandsAgents.as_str()
    );

    let span_attrs2 = HashMap::new();
    let resource_attrs2 = make_attrs(&[("service.name", "strands-agents")]);
    assert_eq!(
        detect_framework("chat", &span_attrs2, &resource_attrs2),
        Framework::StrandsAgents.as_str()
    );
}

#[test]
fn test_strands_agents_framework_detection_new_convention() {
    // Strands new convention uses gen_ai.provider.name instead of gen_ai.system
    let span_attrs = make_attrs(&[("gen_ai.provider.name", "strands-agents")]);
    let resource_attrs = HashMap::new();
    assert_eq!(
        detect_framework("chat", &span_attrs, &resource_attrs),
        Framework::StrandsAgents.as_str(),
        "Should detect Strands from gen_ai.provider.name"
    );
}
