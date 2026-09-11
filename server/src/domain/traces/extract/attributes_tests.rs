//! Tests for attribute extraction

use std::collections::HashMap;

use crate::data::types::{Framework, ObservationType, SpanCategory};

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

/// And a reported zero is a fact the fallback must not overwrite.
#[test]
fn a_reported_zero_survives_a_fallback() {
    let mut attrs = HashMap::new();
    attrs.insert("crew_key".to_string(), "crew-1".to_string());
    attrs.insert("gen_ai.usage.input_tokens".to_string(), "0".to_string());
    attrs.insert("gen_ai.usage.output_tokens".to_string(), "0".to_string());
    attrs.insert(
        "output.value".to_string(),
        serde_json::json!({
            "token_usage": {"prompt_tokens": 999, "completion_tokens": 999}
        })
        .to_string(),
    );

    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "crew");

    assert_eq!(
        (
            span.gen_ai_usage_input_tokens,
            span.gen_ai_usage_output_tokens
        ),
        (0, 0),
        "the provider stated zero; an aggregate elsewhere in the payload does not override it"
    );
}

/// A reported cache counter of zero survives a framework fallback, like the two sides do.
///
/// The per-side gate fixed input and output but left the cache counter testing its *value*, so an explicit
/// `cache_read=0` was replaced by whatever the embedded payload reported - inflating stored usage, and the
/// cost with it.
#[test]
fn a_reported_zero_cache_counter_survives_a_fallback() {
    let mut attrs = HashMap::new();
    attrs.insert("crew_key".to_string(), "crew-1".to_string());
    attrs.insert("gen_ai.usage.input_tokens".to_string(), "10".to_string());
    attrs.insert(
        "gen_ai.usage.cache_read_input_tokens".to_string(),
        "0".to_string(),
    );
    attrs.insert(
        "output.value".to_string(),
        serde_json::json!({
            "token_usage": {"completion_tokens": 5, "cached_prompt_tokens": 100}
        })
        .to_string(),
    );

    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "crew");

    assert_eq!(span.gen_ai_usage_input_tokens, 10);
    assert_eq!(
        span.gen_ai_usage_output_tokens, 5,
        "the output side was absent, so the fallback supplies it"
    );
    assert_eq!(
        span.gen_ai_usage_cache_read_tokens, 0,
        "the provider stated zero cache reads; the embedded payload does not override that"
    );
}

/// An imported total must describe the parts that were imported with it.
///
/// With a flat `input_tokens=0` and no output attribute, the output came from the embedded payload while the
/// input kept its reported zero - and taking the embedded *total* regardless left the row claiming 1,099
/// tokens for 0 + 100.
#[test]
fn an_imported_total_is_only_used_when_it_describes_the_imported_parts() {
    let mut attrs = HashMap::new();
    attrs.insert("crew_key".to_string(), "crew-1".to_string());
    attrs.insert("gen_ai.usage.input_tokens".to_string(), "0".to_string());
    attrs.insert(
        "output.value".to_string(),
        serde_json::json!({
            "token_usage": {"prompt_tokens": 999, "completion_tokens": 100, "total_tokens": 1099}
        })
        .to_string(),
    );

    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "crew");

    assert_eq!(
        (
            span.gen_ai_usage_input_tokens,
            span.gen_ai_usage_output_tokens
        ),
        (0, 100),
        "the reported zero stands; only the absent side is filled"
    );
    assert_eq!(
        span.gen_ai_usage_total_tokens, 100,
        "the total must account for the input and output actually stored"
    );
}

/// The embedded cache counter and total are reachable even when both flat sides were reported.
///
/// The CrewAI block was gated on a *side* being missing, so a payload reporting both sides flatly could
/// never contribute its cache counter or its total: `{prompt:500, completion:600, total:2000, cached:100}`
/// with flat `500/600` stored a total of 1,100 and no cache at all.
#[test]
fn an_embedded_cache_counter_is_read_even_when_both_sides_were_reported() {
    let mut attrs = HashMap::new();
    attrs.insert("crew_key".to_string(), "crew-1".to_string());
    attrs.insert("gen_ai.usage.input_tokens".to_string(), "500".to_string());
    attrs.insert("gen_ai.usage.output_tokens".to_string(), "600".to_string());
    attrs.insert(
        "output.value".to_string(),
        serde_json::json!({
            "token_usage": {
                "prompt_tokens": 500,
                "completion_tokens": 600,
                "total_tokens": 2000,
                "cached_prompt_tokens": 100
            }
        })
        .to_string(),
    );

    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "crew");

    assert_eq!(span.gen_ai_usage_cache_read_tokens, 100);
    assert_eq!(
        span.gen_ai_usage_total_tokens, 2_000,
        "the payload's parts match what is stored, so its reported total applies"
    );
}

/// A later source does not overwrite a counter an earlier one supplied.
///
/// `cache_read_supplied` recorded only whether a *flat attribute* existed, so a value the Logfire
/// `response_data` path filled in was not protected: CrewAI's `cached_prompt_tokens` overwrote it, and the
/// cache charge followed the wrong number. The flag describes the span, not one source's view of it.
#[test]
fn a_framework_fallback_does_not_overwrite_an_earlier_fallback_s_cache_count() {
    let mut attrs = HashMap::new();
    attrs.insert("crew_key".to_string(), "crew-1".to_string());
    attrs.insert(
        "response_data".to_string(),
        serde_json::json!({ "usage": { "cache_read_input_tokens": 17 } }).to_string(),
    );
    attrs.insert(
        "output.value".to_string(),
        serde_json::json!({ "token_usage": { "cached_prompt_tokens": 100 } }).to_string(),
    );

    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "crew");

    assert_eq!(
        span.gen_ai_usage_cache_read_tokens, 17,
        "the first source to supply the counter owns it"
    );
}

/// A framework's embedded total does not raise a total the provider stated itself.
///
/// The embedded total is taken through `max`, which against an explicit flat total can only replace the
/// provider's own statement with the framework's: flat `500/600` with a flat total of 1,100 became 2,000.
#[test]
fn an_embedded_total_does_not_override_an_explicit_flat_total() {
    let mut attrs = HashMap::new();
    attrs.insert("crew_key".to_string(), "crew-1".to_string());
    attrs.insert("gen_ai.usage.input_tokens".to_string(), "500".to_string());
    attrs.insert("gen_ai.usage.output_tokens".to_string(), "600".to_string());
    attrs.insert("gen_ai.usage.total_tokens".to_string(), "1100".to_string());
    attrs.insert(
        "output.value".to_string(),
        serde_json::json!({
            "token_usage": {
                "prompt_tokens": 500,
                "completion_tokens": 600,
                "total_tokens": 2000
            }
        })
        .to_string(),
    );

    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "crew");

    assert_eq!(
        span.gen_ai_usage_total_tokens, 1_100,
        "the provider stated the total; the framework's does not raise it"
    );
}

/// A declared framework fills the gap the conventions leave, and never overrides span evidence.
///
/// The current OTel GenAI conventions are framework-neutral on purpose, so a producer that follows them -
/// the Vercel AI SDK's current integration is pure `gen_ai.*`, with no `ai.*` at all - offers nothing to
/// sniff and arrived as `Unknown`. The SDKs declare what they were configured for, and that is consulted
/// **after** every rule: a process configured for one framework can still emit another's spans from a nested
/// library, and those carry their own attributes for a rule to find.
#[test]
fn a_declared_framework_is_a_fallback_and_not_an_override() {
    let declared = |slug: &str| {
        let mut resource = HashMap::new();
        resource.insert("sideseat.framework".to_string(), slug.to_string());
        resource
    };

    // Pure semantic conventions: no rule matches, so the declaration answers.
    let mut semconv = HashMap::new();
    semconv.insert("gen_ai.operation.name".to_string(), "chat".to_string());
    semconv.insert("gen_ai.provider.name".to_string(), "anthropic".to_string());
    semconv.insert("gen_ai.request.model".to_string(), "claude".to_string());
    assert_eq!(
        detect_framework("chat claude", &semconv, &declared("vercel-ai")),
        Framework::VercelAISdk.as_str(),
        "a framework-neutral span is attributed by what the SDK declared"
    );
    assert_eq!(
        detect_framework("chat claude", &semconv, &HashMap::new()),
        Framework::Unknown.as_str(),
        "and with no declaration it stays unknown rather than guessing"
    );

    // Span evidence wins: a nested library's spans keep their own framework.
    let mut langchain = semconv.clone();
    langchain.insert("langchain.version".to_string(), "0.3".to_string());
    assert_eq!(
        detect_framework("RunnableSequence", &langchain, &declared("strands")),
        Framework::LangChain.as_str(),
        "the span says LangChain, so the process-level declaration must not relabel it"
    );

    // A provider slug is not a framework, so `[strands, bedrock]` still resolves to Strands...
    assert_eq!(
        detect_framework("chat claude", &semconv, &declared("strands,bedrock")),
        Framework::StrandsAgents.as_str()
    );
    // ...while two genuine frameworks resolve to nothing: two answers is not an answer.
    assert_eq!(
        detect_framework("chat claude", &semconv, &declared("strands,langgraph")),
        Framework::Unknown.as_str()
    );
    // An unknown slug claims nothing.
    assert_eq!(
        detect_framework("chat claude", &semconv, &declared("something-else")),
        Framework::Unknown.as_str()
    );
}

/// Attributes for the detection cases below.
fn detect_attrs(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Detection over one span, both ways: the table first, then the rules.
fn both(
    span_name: &str,
    span_attrs: &HashMap<String, String>,
    resource_attrs: &HashMap<String, String>,
) -> (String, String) {
    (
        legacy_detect_framework(span_name, span_attrs, resource_attrs).to_string(),
        detect_framework(span_name, span_attrs, resource_attrs),
    )
}

// ============================================================================
// DETECTION EQUIVALENCE: the rules against the table they replaced
// ============================================================================

/// Spans covering every rule, every dimension, and the orderings that matter.
///
/// Hand-written rather than derived from the assets: a case list generated from the thing under test
/// would stop covering a rule the moment that rule was deleted, and the equivalence claim would still
/// pass. The corpus-wide comparison in `message_goldens_tests` covers the real payloads; this covers the
/// shapes the corpus does not contain, which is most of the 28 rules.
#[test]
fn the_rules_reproduce_the_legacy_detection() {
    let sdk_default = detect_attrs(&[("service.name", "strands-agents")]);
    let empty = detect_attrs(&[]);

    /// One detection case: the span name, its attributes, and the resource's.
    type DetectCase = (
        &'static str,
        HashMap<String, String>,
        HashMap<String, String>,
    );

    let cases: Vec<DetectCase> = vec![
        // Every rule, by its own signal.
        ("autogen run", empty.clone(), empty.clone()),
        ("s", detect_attrs(&[("autogen.foo", "1")]), empty.clone()),
        (
            "s",
            detect_attrs(&[("gen_ai.system", "autogen")]),
            empty.clone(),
        ),
        (
            "s",
            detect_attrs(&[("gcp.vertex.agent.x", "1")]),
            empty.clone(),
        ),
        ("s", detect_attrs(&[("google.adk.x", "1")]), empty.clone()),
        ("s", detect_attrs(&[("crew_key", "k")]), empty.clone()),
        (
            "s",
            empty.clone(),
            detect_attrs(&[("service.name", "crewAI-telemetry")]),
        ),
        ("LangGraph", empty.clone(), empty.clone()),
        ("s", detect_attrs(&[("langgraph.step", "1")]), empty.clone()),
        (
            "s",
            detect_attrs(&[("metadata", "{\"langgraph_step\":1}")]),
            empty.clone(),
        ),
        ("s", detect_attrs(&[("langchain.x", "1")]), empty.clone()),
        ("s", detect_attrs(&[("langsmith.x", "1")]), empty.clone()),
        ("s", detect_attrs(&[("llama_index.x", "1")]), empty.clone()),
        ("s", detect_attrs(&[("agno.x", "1")]), empty.clone()),
        ("s", detect_attrs(&[("smolagents.x", "1")]), empty.clone()),
        ("s", detect_attrs(&[("agentscope.x", "1")]), empty.clone()),
        ("s", detect_attrs(&[("langflow.x", "1")]), empty.clone()),
        ("s", detect_attrs(&[("ag2.x", "1")]), empty.clone()),
        ("s", detect_attrs(&[("haystack.x", "1")]), empty.clone()),
        (
            "s",
            detect_attrs(&[("gen_ai.provider.name", "browser_use")]),
            empty.clone(),
        ),
        (
            "s",
            detect_attrs(&[("openinference.span.kind", "LLM")]),
            empty.clone(),
        ),
        (
            "s",
            detect_attrs(&[("semantic_kernel.x", "1")]),
            empty.clone(),
        ),
        (
            "s",
            detect_attrs(&[("gen_ai.system", "azure_openai")]),
            empty.clone(),
        ),
        (
            "s",
            detect_attrs(&[("gen_ai.system", "azure.openai")]),
            empty.clone(),
        ),
        ("s", detect_attrs(&[("azure.openai.x", "1")]), empty.clone()),
        ("s", detect_attrs(&[("az.ai.x", "1")]), empty.clone()),
        ("vertexai.generate", empty.clone(), empty.clone()),
        ("s", detect_attrs(&[("ai.operationId", "x")]), empty.clone()),
        (
            "s",
            detect_attrs(&[("ai.prompt.messages", "[]")]),
            empty.clone(),
        ),
        (
            "s",
            detect_attrs(&[("ai.usage.promptTokens", "1")]),
            empty.clone(),
        ),
        (
            "s",
            detect_attrs(&[("ai.finishReason", "stop")]),
            empty.clone(),
        ),
        ("s", detect_attrs(&[("logfire.msg", "x")]), empty.clone()),
        (
            "s",
            empty.clone(),
            detect_attrs(&[("telemetry.sdk.name", "logfire-python")]),
        ),
        ("s", detect_attrs(&[("mlflow.x", "1")]), empty.clone()),
        (
            "s",
            detect_attrs(&[("traceloop.entity.input", "x")]),
            empty.clone(),
        ),
        (
            "s",
            empty.clone(),
            detect_attrs(&[("telemetry.sdk.name", "traceloop-sdk")]),
        ),
        ("s", detect_attrs(&[("livekit.x", "1")]), empty.clone()),
        ("s", detect_attrs(&[("lk.x", "1")]), empty.clone()),
        (
            "s",
            detect_attrs(&[("openai.agents.x", "1")]),
            empty.clone(),
        ),
        (
            "s",
            empty.clone(),
            detect_attrs(&[("service.name", "openai-agents")]),
        ),
        (
            "s",
            detect_attrs(&[("gen_ai.provider.name", "microsoft.agent_framework")]),
            empty.clone(),
        ),
        (
            "s",
            empty.clone(),
            detect_attrs(&[("service.name", "agent-framework-core")]),
        ),
        ("s", detect_attrs(&[("aws.bedrock.x", "1")]), empty.clone()),
        (
            "s",
            detect_attrs(&[("gen_ai.system", "aws_bedrock")]),
            empty.clone(),
        ),
        ("claude_code.interaction", empty.clone(), empty.clone()),
        (
            "s",
            empty.clone(),
            detect_attrs(&[("service.name", "claude-code")]),
        ),
        (
            "s",
            detect_attrs(&[("gen_ai.system", "strands-agents")]),
            empty.clone(),
        ),
        ("Strands Agent loop", empty.clone(), empty.clone()),
        ("strands-agent", empty.clone(), empty.clone()),
        (
            "s",
            detect_attrs(&[("gen_ai.agent.name", "My STRANDS_AGENT")]),
            empty.clone(),
        ),
        // Nothing at all.
        ("plain span", empty.clone(), empty.clone()),
        // The declaration fallback, including a provider slug that must contribute nothing and two
        // genuine frameworks that must resolve to nothing.
        (
            "s",
            empty.clone(),
            detect_attrs(&[("sideseat.framework", "strands")]),
        ),
        (
            "s",
            empty.clone(),
            detect_attrs(&[("sideseat.framework", "strands,bedrock")]),
        ),
        (
            "s",
            empty.clone(),
            detect_attrs(&[("sideseat.framework", "strands,crewai")]),
        ),
        (
            "s",
            empty.clone(),
            detect_attrs(&[("sideseat.framework", "vercel-ai")]),
        ),
        (
            "s",
            empty.clone(),
            detect_attrs(&[("sideseat.framework", "openai")]),
        ),
        (
            "s",
            empty.clone(),
            detect_attrs(&[("sideseat.framework", "pydantic-ai")]),
        ),
        (
            "s",
            empty.clone(),
            detect_attrs(&[("sideseat.framework", "not-a-framework")]),
        ),
        // `dedup` runs on an *unsorted* list, so it removes only consecutive repeats - preserved from
        // the table this replaced. It cannot change an answer, and these cases are why: a repeat becomes
        // non-consecutive only when a different label sits between it, and two distinct labels already
        // resolve to nothing; a slug that resolves to nothing (a provider) is filtered out before the
        // dedup, so it cannot separate them either.
        (
            "s",
            empty.clone(),
            detect_attrs(&[("sideseat.framework", "strands,bedrock,strands")]),
        ),
        (
            "s",
            empty.clone(),
            detect_attrs(&[("sideseat.framework", "strands,crewai,strands")]),
        ),
        (
            "s",
            empty.clone(),
            detect_attrs(&[("sideseat.framework", "strands,crewai,crewai")]),
        ),
        (
            "s",
            empty.clone(),
            detect_attrs(&[("sideseat.framework", " strands , bedrock ")]),
        ),
        (
            "s",
            empty.clone(),
            detect_attrs(&[("sideseat.framework", "")]),
        ),
        // The ordering that matters most: the SDK's default service name must not claim a span whose own
        // attributes name a different framework.
        (
            "s",
            detect_attrs(&[("langgraph.step", "1")]),
            sdk_default.clone(),
        ),
        ("s", detect_attrs(&[("crew_key", "k")]), sdk_default.clone()),
        (
            "s",
            detect_attrs(&[("openinference.span.kind", "LLM")]),
            sdk_default.clone(),
        ),
        ("s", empty.clone(), sdk_default.clone()),
        // Overlaps the ranks exist to resolve.
        (
            "s",
            detect_attrs(&[("langgraph.step", "1"), ("langchain.x", "1")]),
            empty.clone(),
        ),
        (
            "s",
            detect_attrs(&[("agno.x", "1"), ("openinference.span.kind", "LLM")]),
            empty.clone(),
        ),
        (
            "s",
            detect_attrs(&[("azure.openai.x", "1"), ("az.ai.x", "1")]),
            empty.clone(),
        ),
    ];

    let mut disagreements = Vec::new();
    for (span_name, span_attrs, resource_attrs) in &cases {
        let (legacy, rules) = both(span_name, span_attrs, resource_attrs);
        if legacy != rules {
            disagreements.push(format!(
                "  span `{span_name}` attrs={span_attrs:?} resource={resource_attrs:?}: table said \
                 `{legacy}`, rules said `{rules}`"
            ));
        }
    }
    assert!(
        disagreements.is_empty(),
        "detection changed for {} case(s):\n{}",
        disagreements.len(),
        disagreements.join("\n")
    );
}

#[test]
fn a_span_nothing_claims_is_labelled_unclaimed() {
    let (legacy, rules) = both("plain", &detect_attrs(&[]), &detect_attrs(&[]));
    assert_eq!(rules, crate::domain::rules::UNCLAIMED_LABEL);
    assert_eq!(legacy, rules);
}

/// How far detection is from being order-independent, measured rather than assumed.
///
/// `legacy_rank` exists because the rules were transcribed from a first-match table whose order is
/// load-bearing. The target is that no span has two candidates, and this reports which of the corpus's
/// own shapes still do - so retiring the rank is a measurable job rather than a hope. Reported, not
/// asserted to be zero: making it zero means narrowing predicates, which is its own reviewed change.
#[test]
fn detection_overlaps_are_reported() {
    let plan = &crate::domain::rules::ruleset().detect;
    let empty = HashMap::new();

    // Shapes that really co-occur: a framework riding OpenInference, and any framework using the SDK
    // (whose default service name is one framework's own).
    let sdk_default = detect_attrs(&[("service.name", "strands-agents")]);
    /// One overlap probe: span name, its attributes, and the resource's.
    type OverlapProbe<'a> = (
        &'a str,
        HashMap<String, String>,
        &'a HashMap<String, String>,
    );

    let probes: Vec<OverlapProbe<'_>> = vec![
        (
            "s",
            detect_attrs(&[("agno.x", "1"), ("openinference.span.kind", "LLM")]),
            &empty,
        ),
        (
            "s",
            detect_attrs(&[("langgraph.step", "1"), ("langchain.x", "1")]),
            &empty,
        ),
        ("s", detect_attrs(&[("langgraph.step", "1")]), &sdk_default),
        ("s", detect_attrs(&[("crew_key", "k")]), &sdk_default),
        (
            "s",
            detect_attrs(&[("openinference.span.kind", "LLM")]),
            &sdk_default,
        ),
        (
            "s",
            detect_attrs(&[("azure.openai.x", "1"), ("az.ai.x", "1")]),
            &empty,
        ),
    ];

    let mut overlaps = Vec::new();
    for (span_name, span_attrs, resource_attrs) in &probes {
        let ctx = crate::domain::rules::DetectContext {
            span_name,
            span_attrs,
            resource_attrs,
        };
        let candidates = plan.overlapping_candidates(&ctx);
        if !candidates.is_empty() {
            let names: Vec<&str> = candidates.iter().map(|c| c.rule_id.as_str()).collect();
            overlaps.push(format!("  {span_attrs:?} -> {}", names.join(", ")));
        }
    }
    println!(
        "detection: {} of {} probed shapes have more than one candidate, so `legacy_rank` still \
         decides them:\n{}",
        overlaps.len(),
        probes.len(),
        overlaps.join("\n")
    );
    // The bound that must hold: whatever the overlaps, the *winner* is the one the table chose. That is
    // what `the_rules_reproduce_the_legacy_detection` asserts; this test exists to name the residue.
    assert!(
        overlaps.len() <= probes.len(),
        "sanity: cannot overlap on more shapes than were probed"
    );
}

/// The declared finish-reason chain answers exactly as the four Rust blocks it replaced did, and in their
/// order.
///
/// Four attribute sources moved (`ff0d3cdf`); only the `gen_ai.choice` **event** stayed, which field
/// resolution cannot see. Three things this pins that the goldens cannot, because no captured span carries
/// the shapes:
///
/// - **The order**, spelled out: the flat attribute, then a serialised completion's choices, then a
///   serialised output-message list, then one dialect's serialised response, then another's. A reordering of
///   the asset is a silent change in which producer's statement is believed.
/// - **The array requirement.** Two of the retired readers decoded a *list* and fell through when the payload
///   was not one. A JSONPath wildcard matches an object's members too, so `{"x": {"finish_reason": …}}`
///   answered where the retired chain moved on - and answered with a different producer's value.
/// - **Scalar strings only.** The retired readers took `as_str()`, so a member holding `["stop", "length"]`
///   was ignored; collected as a string *list* it contributed two reasons the producer never stated.
#[test]
fn the_declared_finish_reason_chain_reproduces_the_retired_blocks() {
    use crate::domain::rules::ruleset;
    use std::collections::HashMap;

    let resolve = |attrs: &HashMap<String, String>| -> Vec<String> {
        ruleset()
            .span_fields
            .resolve("some.span", attrs, &[])
            .into_iter()
            .find(|r| {
                matches!(
                    r.target,
                    crate::domain::rules::schema::FieldTarget::GenAiFinishReasons
                )
            })
            .and_then(|r| match r.reading {
                crate::domain::rules::span_fields::Reading::StringList(items) => Some(items),
                crate::domain::rules::span_fields::Reading::Text(text) => Some(vec![text]),
                _ => None,
            })
            .unwrap_or_default()
    };

    /// The four retired blocks, in their order, verbatim in behaviour.
    fn retired(attrs: &HashMap<String, String>) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        if let Some(flat) = attrs.get("gen_ai.response.finish_reasons") {
            // The flat attribute, read as the stored list form.
            if let Ok(items) = serde_json::from_str::<Vec<String>>(flat) {
                out = items;
            } else if !flat.is_empty() {
                out = vec![flat.clone()];
            }
        }
        if out.is_empty()
            && let Some(completion) = attrs.get("gen_ai.completion")
            && let Ok(json) = serde_json::from_str::<serde_json::Value>(completion)
            && let Some(choices) = json.get("choices").and_then(|c| c.as_array())
        {
            for choice in choices {
                if let Some(reason) = choice.get("finish_reason").and_then(|r| r.as_str()) {
                    out.push(reason.to_string());
                }
            }
        }
        if out.is_empty()
            && let Some(messages) = attrs.get("gen_ai.output.messages")
            && let Ok(list) = serde_json::from_str::<Vec<serde_json::Value>>(messages)
        {
            for message in &list {
                if let Some(reason) = message.get("finish_reason").and_then(|r| r.as_str()) {
                    out.push(reason.to_string());
                    break;
                }
            }
        }
        if out.is_empty()
            && let Some(response) = attrs.get("gcp.vertex.agent.llm_response")
            && let Ok(json) = serde_json::from_str::<serde_json::Value>(response)
            && let Some(reason) = json.get("finish_reason").and_then(|r| r.as_str())
        {
            out = vec![reason.to_lowercase()];
        }
        if out.is_empty()
            && let Some(response) = attrs.get("response_data")
            && let Ok(json) = serde_json::from_str::<serde_json::Value>(response)
            && let Some(reason) = json.get("finish_reason").and_then(|r| r.as_str())
        {
            out = vec![reason.to_string()];
        }
        out
    }

    // Each case names every attribute it sets, so the order is exercised by conflicting values rather than
    // asserted about. Every shape the retired readers refused is here, because a refusal *is* the order:
    // falling through is how the next producer's statement gets believed.
    let cases: &[&[(&str, &str)]] = &[
        // One source at a time.
        &[("gen_ai.response.finish_reasons", "tool_use")],
        &[(
            "gen_ai.completion",
            r#"{"choices":[{"finish_reason":"stop"}]}"#,
        )],
        &[(
            "gen_ai.completion",
            r#"{"choices":[{"finish_reason":"stop"},{"finish_reason":"length"}]}"#,
        )],
        &[(
            "gen_ai.output.messages",
            r#"[{"finish_reason":"stop"},{"finish_reason":"length"}]"#,
        )],
        &[(
            "gcp.vertex.agent.llm_response",
            r#"{"finish_reason":"STOP"}"#,
        )],
        &[("response_data", r#"{"finish_reason":"length"}"#)],
        // The order, by conflict: each earlier source must win.
        &[
            ("gen_ai.response.finish_reasons", "tool_use"),
            (
                "gen_ai.completion",
                r#"{"choices":[{"finish_reason":"stop"}]}"#,
            ),
            ("gen_ai.output.messages", r#"[{"finish_reason":"length"}]"#),
            (
                "gcp.vertex.agent.llm_response",
                r#"{"finish_reason":"STOP"}"#,
            ),
            ("response_data", r#"{"finish_reason":"content_filter"}"#),
        ],
        &[
            (
                "gen_ai.completion",
                r#"{"choices":[{"finish_reason":"stop"}]}"#,
            ),
            ("gen_ai.output.messages", r#"[{"finish_reason":"length"}]"#),
            ("response_data", r#"{"finish_reason":"content_filter"}"#),
        ],
        &[
            ("gen_ai.output.messages", r#"[{"finish_reason":"length"}]"#),
            (
                "gcp.vertex.agent.llm_response",
                r#"{"finish_reason":"STOP"}"#,
            ),
        ],
        &[
            (
                "gcp.vertex.agent.llm_response",
                r#"{"finish_reason":"STOP"}"#,
            ),
            ("response_data", r#"{"finish_reason":"content_filter"}"#),
        ],
        // The shapes a refusal must let through. An **object** where a list was required: the retired reader
        // decoded a `Vec` and fell through, and a JSONPath wildcard would have answered here instead.
        &[
            (
                "gen_ai.output.messages",
                r#"{"x":{"finish_reason":"stop"}}"#,
            ),
            ("response_data", r#"{"finish_reason":"length"}"#),
        ],
        &[
            (
                "gen_ai.completion",
                r#"{"choices":{"a":{"finish_reason":"stop"}}}"#,
            ),
            ("response_data", r#"{"finish_reason":"length"}"#),
        ],
        // A member that is not a scalar string, on **every** source. The retired readers all took `as_str()`,
        // so an array-valued member was ignored and the chain moved to the next producer - a different
        // statement about why the model stopped, not a formatting difference. `collect_all` fixed only the
        // completion source; the other three read the field's own *list* type and answered here instead.
        &[(
            "gen_ai.output.messages",
            r#"[{"finish_reason":["stop"]},{"finish_reason":"length"}]"#,
        )],
        &[
            ("gen_ai.output.messages", r#"[{"finish_reason":["stop"]}]"#),
            ("response_data", r#"{"finish_reason":"length"}"#),
        ],
        &[
            (
                "gcp.vertex.agent.llm_response",
                r#"{"finish_reason":["STOP"]}"#,
            ),
            ("response_data", r#"{"finish_reason":"length"}"#),
        ],
        &[("response_data", r#"{"finish_reason":["length"]}"#)],
        &[
            ("gen_ai.response.finish_reasons", "[]"),
            ("response_data", r#"{"finish_reason":"length"}"#),
        ],
        // A member that is not a scalar string. `as_str()` ignored it; collected as a list it became two.
        &[(
            "gen_ai.completion",
            r#"{"choices":[{"finish_reason":["stop","length"]}]}"#,
        )],
        &[
            (
                "gen_ai.completion",
                r#"{"choices":[{"finish_reason":["stop","length"]}]}"#,
            ),
            ("response_data", r#"{"finish_reason":"content_filter"}"#),
        ],
        // An **empty** reason is a value, and it ends the chain. `as_str()` returned `Some("")`, so the
        // retired reader pushed it and stopped looking - discarding it lost the value *and* let a later
        // producer's reason answer in its place.
        &[("gen_ai.completion", r#"{"choices":[{"finish_reason":""}]}"#)],
        &[
            ("gen_ai.completion", r#"{"choices":[{"finish_reason":""}]}"#),
            ("response_data", r#"{"finish_reason":"length"}"#),
        ],
        &[
            ("gen_ai.output.messages", r#"[{"finish_reason":""}]"#),
            ("response_data", r#"{"finish_reason":"length"}"#),
        ],
        &[
            ("gcp.vertex.agent.llm_response", r#"{"finish_reason":""}"#),
            ("response_data", r#"{"finish_reason":"length"}"#),
        ],
        &[("response_data", r#"{"finish_reason":""}"#)],
        // Nothing readable anywhere.
        &[("gen_ai.completion", "not json")],
        &[("response_data", r#"{"finish_reason":null}"#)],
        &[("gcp.vertex.agent.llm_response", "[]")],
        &[],
    ];

    for case in cases {
        let attrs: HashMap<String, String> = case
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        assert_eq!(
            resolve(&attrs),
            retired(&attrs),
            "the declared chain disagrees with the retired blocks on {case:?}"
        );
    }
}

/// The `gen_ai.choice` **event** is the *first* finish-reason source, which is where the retired chain had it.
///
/// This test asserted the opposite for one commit's worth of reasons, and both states were honest at the time.
/// When the four attribute sources moved into the declared chain (`ff0d3cdf`) the event could not follow -
/// `FieldSource` had no way to name an event - so it stayed as a hand-written scan in `extract/mod.rs` that
/// necessarily ran *after* resolution. That made the event last, which is a precedence **no producer states**:
/// it came from where the code could put it, not from what the telemetry means.
///
/// Cycle 9 added `event_attribute` to `FieldSource`, so the source is declared like every other and sits where
/// the retired order had it - first. A reader who believes the intermediate order would expect `length` here.
///
/// Still end to end through `extract_attributes_batch`, because that is what supplies the events.
#[test]
fn the_choice_event_is_the_first_finish_reason_source() {
    use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
    use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
    use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span, span::Event};

    let kv = |key: &str, value: &str| KeyValue {
        key: key.to_string(),
        value: Some(AnyValue {
            value: Some(any_value::Value::StringValue(value.to_string())),
        }),
    };

    let span_with = |attrs: Vec<KeyValue>, with_event: bool| {
        let events = if with_event {
            vec![Event {
                time_unix_nano: 1_700_000_000_000_000_000,
                name: "gen_ai.choice".to_string(),
                attributes: vec![kv("finish_reason", "stop")],
                dropped_attributes_count: 0,
            }]
        } else {
            Vec::new()
        };
        let request = ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                resource: None,
                scope_spans: vec![ScopeSpans {
                    scope: None,
                    spans: vec![Span {
                        trace_id: vec![1; 16],
                        span_id: vec![2; 8],
                        name: "chat".to_string(),
                        kind: 1,
                        start_time_unix_nano: 1_700_000_000_000_000_000,
                        end_time_unix_nano: 1_700_000_000_100_000_000,
                        attributes: attrs,
                        events,
                        ..Default::default()
                    }],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        };
        crate::domain::traces::extract::extract_attributes_batch(&request)
            .into_iter()
            .next()
            .expect("one span")
            .gen_ai_finish_reasons
    };

    // An attribute source and the event disagreeing: the **event** wins, because it is the first declared
    // source - the conventions' own spelling, ahead of four dialects' serialised payloads.
    assert_eq!(
        span_with(
            vec![kv("response_data", r#"{"finish_reason":"length"}"#)],
            true
        ),
        vec!["stop".to_string()],
        "the event is declared first, so the conventions' own spelling answers"
    );
    // The event alone answers, and an attribute alone answers - so neither is dead.
    assert_eq!(
        span_with(Vec::new(), true),
        vec!["stop".to_string()],
        "with no attribute source the event must answer"
    );
    assert_eq!(
        span_with(
            vec![kv("response_data", r#"{"finish_reason":"length"}"#)],
            false
        ),
        vec!["length".to_string()],
        "with no event the attribute chain must answer"
    );
    // And neither: no reason at all, rather than an empty string.
    assert!(span_with(Vec::new(), false).is_empty());
}

/// Every field target is listed in `FieldTarget::ALL`.
///
/// The list is hand-written, so it is the one thing a new variant can escape - and the two tests below need it to
/// be complete or they check a subset while claiming to check the ontology.
#[test]
fn every_field_target_is_listed() {
    let source = include_str!("../../rules/schema.rs");
    let start = source
        .find("pub enum FieldTarget {")
        .expect("the enum is declared here");
    let body = &source[start..];
    let end = body.find("\n}\n").expect("the enum body ends");
    let declared = body[..end]
        .lines()
        .filter(|line| {
            let trimmed = line.trim_end();
            trimmed.starts_with("    ")
                && trimmed.ends_with(',')
                && !trimmed.trim_start().starts_with("//")
                && trimmed
                    .trim_start()
                    .trim_end_matches(',')
                    .chars()
                    .next()
                    .is_some_and(char::is_uppercase)
                && trimmed
                    .trim_start()
                    .trim_end_matches(',')
                    .chars()
                    .all(char::is_alphanumeric)
        })
        .count();
    assert_eq!(
        crate::domain::rules::schema::FieldTarget::ALL.len(),
        declared,
        "`FieldTarget::ALL` lists {} of the {declared} declared variants",
        crate::domain::rules::schema::FieldTarget::ALL.len()
    );
}

/// Every target's declared **type** reaches the sink that writes it.
///
/// `apply_field` chooses a setter per target - `text()`, `integer()`, `float()`, `list()` - and each returns
/// `None`/empty when the reading is a different variant. So a target whose `field_type()` says `Float` while its
/// arm calls `integer()` resolves perfectly, converts perfectly, and writes **nothing**: the column stays unset
/// and no diagnostic says why. Nothing checked the correspondence, and it is exactly the kind of pair that drifts
/// when a column's type changes.
///
/// Asked by feeding each target a reading of its own declared type and requiring the write to be observable, in
/// either sink - the counters go to `TokenReadings` rather than to a column.
#[test]
fn every_target_writes_what_its_declared_type_produces() {
    use crate::domain::rules::schema::{FieldTarget, FieldType};
    use crate::domain::rules::span_fields::{Reading, Resolved};

    for target in FieldTarget::ALL {
        // A value of the target's own type that is inside whatever range it admits, so this measures the sink and
        // not the bound cycle 19 added.
        let reading = match target.field_type() {
            FieldType::Text => Reading::Text("probe".to_string()),
            FieldType::Integer => Reading::Integer(1),
            FieldType::Float => Reading::Float(0.5),
            FieldType::StringList => Reading::StringList(vec!["probe".to_string()]),
        };
        let resolved = Resolved {
            target: *target,
            reading,
            rule_id: "probe.rule".to_string(),
            evidence: None,
            refused: Vec::new(),
        };
        let mut span = SpanData::default();
        let mut tokens = crate::domain::traces::extract::attributes::TokenReadings::default();
        let before = format!("{span:?}{tokens:?}");
        crate::domain::traces::extract::attributes::apply_field_for_test(
            &mut span,
            &resolved,
            &mut tokens,
        );
        assert_ne!(
            format!("{span:?}{tokens:?}"),
            before,
            "`{target:?}` declares {:?} and its sink wrote nothing, so a value of its own type is silently lost",
            target.field_type()
        );
    }
}

/// **Every** span-field refusal fires, because none of them did.
///
/// Seventeen refusals, each a statement about a declaration that cannot mean what it says, and not one was
/// exercised anywhere - so each was a claim rather than a guard, and any of them could have been deleted or
/// narrowed with the suite still green. Several are one edit from being unreachable: the exclusivity check *counts*
/// the seven reader forms (it used to pattern-match a pair, which stopped covering the forms as they were added),
/// and a count that drifted to six would silently admit the form it forgot.
///
/// One probe per refusal, matched on the variant rather than on the message, so rewording a diagnostic does not
/// quietly stop testing it.
#[test]
fn every_span_field_refusal_fires() {
    use crate::domain::rules::span_fields::{FieldCompileError as E, compile};

    let compiled = |asset: &str| {
        compile(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            asset.as_bytes().to_vec(),
        )]))
    };
    /// A probe asset, and the refusal it must produce.
    type Case = (&'static str, &'static str, fn(&E) -> bool);
    let cases: Vec<Case> = vec![
        ("not JSON at all", "{", |e| matches!(e, E::Parse { .. })),
        (
            "a rule with no source",
            r#"{"id":"t","span_fields":[{"id":"f","target":"user_id","sources":[]}]}"#,
            |e| matches!(e, E::NoSources { .. }),
        ),
        (
            "a source naming nowhere to read from",
            r#"{"id":"t","span_fields":[{"id":"f","target":"user_id","sources":[{"id":"s"}]}]}"#,
            |e| matches!(e, E::SourceReadsNothing { .. }),
        ),
        (
            "a source naming two places, where the reader's branch order would decide",
            r#"{"id":"t","span_fields":[{"id":"f","target":"user_id",
               "sources":[{"id":"s","attribute":"k","raw_span_name":true}]}]}"#,
            |e| matches!(e, E::SourceReadsTwoThings { .. }),
        ),
        (
            "a literal with no gate, which answers on every span",
            r#"{"id":"t","span_fields":[{"id":"f","target":"user_id",
               "sources":[{"id":"s","value":"x"}]}]}"#,
            |e| matches!(e, E::UngatedLiteral { .. }),
        ),
        (
            "a JSON source naming neither a path nor a first-present group",
            r#"{"id":"t","span_fields":[{"id":"f","target":"user_id",
               "sources":[{"id":"s","json":{"attribute":"a"}}]}]}"#,
            |e| matches!(e, E::JsonNamesNoMember { .. }),
        ),
        (
            "a sum into a field that holds text",
            r#"{"id":"t","span_fields":[{"id":"f","target":"user_id",
               "sources":[{"id":"s","json":{"attribute":"a","path":"$.n","reduce":"sum"}}]}]}"#,
            |e| matches!(e, E::ReductionThatCannotYield { .. }),
        ),
        (
            "a reduction over a first-present group, which selects one path rather than combining matches",
            r#"{"id":"t","span_fields":[{"id":"f","target":"usage_input_tokens",
               "sources":[{"id":"s","json":{"attribute":"a","first_present_of":["$.a","$.b"],"reduce":"sum"}}]}]}"#,
            |e| matches!(e, E::ReductionWithoutAPath { .. }),
        ),
        (
            "folding a field that holds no text",
            r#"{"id":"t","span_fields":[{"id":"f","target":"usage_input_tokens",
               "sources":[{"id":"s","attribute":"k","lowercase":true}]}]}"#,
            |e| matches!(e, E::FoldWithoutText { .. }),
        ),
        (
            "`scalar_only` where it cannot apply",
            r#"{"id":"t","span_fields":[{"id":"f","target":"user_id",
               "sources":[{"id":"s","json":{"attribute":"a","first_present_of":["$.a"],"scalar_only":true}}]}]}"#,
            |e| matches!(e, E::ScalarOnlyWithoutAPath { .. }),
        ),
        (
            "an empty attribute name",
            r#"{"id":"t","span_fields":[{"id":"f","target":"user_id",
               "sources":[{"id":"s","attribute":""}]}]}"#,
            |e| matches!(e, E::EmptyAttribute { .. }),
        ),
        (
            "every occurrence of an event attribute into a field holding one value",
            r#"{"id":"t","span_fields":[{"id":"f","target":"user_id",
               "sources":[{"id":"s","event_attribute":{"event":"e","attribute":"a","occurrence":"every"}}]}]}"#,
            |e| matches!(e, E::EveryOccurrenceIntoOneValue { .. }),
        ),
        (
            "a merge into a field that holds one value",
            r#"{"id":"t","span_fields":[{"id":"f","target":"user_id","combine":"merge_all",
               "sources":[{"id":"s","attribute":"k"}]}]}"#,
            |e| matches!(e, E::MergeIntoScalar { .. }),
        ),
        (
            "two rules resolving one target",
            r#"{"id":"t","span_fields":[
               {"id":"f","target":"user_id","sources":[{"id":"s","attribute":"k"}]},
               {"id":"g","target":"user_id","sources":[{"id":"s","attribute":"j"}]}]}"#,
            |e| matches!(e, E::DuplicateTarget { .. }),
        ),
        (
            "two rules sharing an id",
            r#"{"id":"t","span_fields":[
               {"id":"f","target":"user_id","sources":[{"id":"s","attribute":"k"}]},
               {"id":"f","target":"http_method","sources":[{"id":"s","attribute":"j"}]}]}"#,
            |e| matches!(e, E::DuplicateId { .. }),
        ),
        (
            // Two *sources* sharing an id is a different rule, and it was enforced only where the whole ruleset
            // is built: `span_fields::compile` parsed the file itself and never asked `declaration_defect`, which
            // is the hole that function's own doc names - "a hole the moment anything else loads a file".
            "two sources of one rule sharing an id",
            r#"{"id":"t","span_fields":[{"id":"f","target":"user_id",
               "sources":[{"id":"s","attribute":"k"},{"id":"s","attribute":"j"}]}]}"#,
            |e| matches!(e, E::Parse { .. }),
        ),
        (
            "a gate that can never hold",
            r#"{"id":"t","span_fields":[{"id":"f","target":"user_id",
               "sources":[{"id":"s","attribute":"k","when":{"attr_prefix":[""]}}]}]}"#,
            |e| matches!(e, E::DeadGate { .. }),
        ),
        (
            // Each gate was validated on its own and neither validator asked about the other, so the pair
            // compiled as a source that is simply never consulted - which reads as a narrowing somebody chose.
            "a source admitted and skipped by the same condition",
            r#"{"id":"t","span_fields":[{"id":"f","target":"user_id",
               "sources":[{"id":"s","attribute":"k","when":{"attr_exists":["m"]},"unless":{"attr_exists":["m"]}}]}]}"#,
            |e| matches!(e, E::DeadGate { .. }),
        ),
        (
            "a gate naming a resource dimension field resolution is never given",
            r#"{"id":"t","span_fields":[{"id":"f","target":"user_id",
               "sources":[{"id":"s","attribute":"k","when":{"service_name":["x"]}}]}]}"#,
            |e| matches!(e, E::UnavailableGate { .. }),
        ),
    ];

    for (what, asset, expected) in cases {
        let error = compiled(asset)
            .err()
            .unwrap_or_else(|| panic!("should have been refused: {what}"));
        assert!(expected(&error), "wrong refusal for {what}: {error}");
    }

    // Two *different* gates on one source are fine - that is an admitted-unless pair, which several assets use.
    assert!(
        compiled(
            r#"{"id":"t","span_fields":[{"id":"f","target":"user_id",
               "sources":[{"id":"s","attribute":"k","when":{"attr_exists":["m"]},"unless":{"attr_exists":["n"]}}]}]}"#
        )
        .is_ok(),
        "two different conditions are an ordinary admitted-unless pair"
    );

    // And a rule stating one reader with nothing dead about it compiles, or the refusals are simply a ban.
    assert!(
        compiled(
            r#"{"id":"t","span_fields":[{"id":"f","target":"user_id",
               "sources":[{"id":"s","attribute":"k"},{"id":"t","json":{"attribute":"a","path":"$.u"}}]}]}"#
        )
        .is_ok()
    );
}
