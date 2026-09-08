//! Trace Extraction and Normalization (Stage 1)
//!
//! Parses OTLP protobuf and extracts GenAI attributes and raw messages.
//! Raw message content is preserved literally for normalization in sideml (Stage 2).
//!
//! **Important**: This module only extracts raw data. It does NOT deduplicate, filter,
//! or modify messages. Deduplication happens downstream in the messages repository.
//!
//! ## Extraction Priority (Messages)
//!
//! 1. OTEL Events: `gen_ai.*.message`, `gen_ai.choice`
//! 2. Gen AI Attrs: `gen_ai.prompt.N.*`, `gen_ai.completion.N.*`
//! 3. OpenInference: `llm.input_messages.N.*`, `llm.output_messages.N.*`
//! 4. Logfire: `events` JSON array
//! 5. Framework-specific: Vercel AI, Google ADK, AutoGen, CrewAI
//! 6. Raw I/O: `input.value`, `output.value`, `raw_input`, `response`
//!
//! ## Raw Message Format
//!
//! All extractors preserve literal content from the source (no metadata added).
//! The `RawMessage.source` field tracks where the message came from.
//!
//! ## Architecture
//!
//! - `SpanData`: Extracted span data (pipeline intermediate)
//! - `RawMessage`: Pre-normalized message with source tracking
//! - `RawToolDefinition`: Pre-normalized tool definition with source tracking
//! - `RawToolNames`: Pre-normalized tool names list with source tracking
//! - `AttributeExtractor`: Extracts span attributes (GenAI, semantic, classification)
//! - `MessageExtractor`: Extracts messages and tool definitions from events and attributes

#![allow(clippy::collapsible_if)]

#[cfg_attr(test, allow(unreachable_pub))]
pub(crate) mod attributes;
pub mod files;
pub(crate) mod messages;

use std::collections::HashMap;

use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use serde_json::Value as JsonValue;

use crate::core::constants;
use crate::utils::otlp::extract_attributes;

// ============================================================================
// SHARED HELPER FUNCTIONS
// ============================================================================

/// Truncate a string to at most `max` bytes on a UTF-8 char boundary.
fn truncate_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Parse JSON from an attribute value.
pub(super) fn extract_json<T: serde::de::DeserializeOwned>(
    attrs: &HashMap<String, String>,
    key: &str,
) -> Option<T> {
    attrs.get(key).and_then(|s| serde_json::from_str(s).ok())
}

// Re-export public types
pub use self::attributes::SpanData;
pub use self::messages::{MessageSource, RawMessage, RawToolDefinition, RawToolNames};
pub(crate) use messages::ExtractionMode;

// ============================================================================
// ATTRIBUTE KEYS
// ============================================================================

pub(super) mod keys {
    // Resource
    pub const PROJECT_ID: &str = "sideseat.project_id";
    pub const DEPLOYMENT_ENV: &str = "deployment.environment";
    pub const DEPLOYMENT_ENV_NAME: &str = "deployment.environment.name";
    // Reachable only from the detection oracle now. The runtime home of "which resource attribute is
    // the service name" is `domain::rules`, because that is where the dimension is defined; the values
    // that identify a producer through it are asset data.
    #[cfg(test)]
    pub const SERVICE_NAME: &str = "service.name";
    #[cfg(test)]
    pub const TELEMETRY_SDK_NAME: &str = "telemetry.sdk.name";
    /// The framework a SideSeat SDK declares it was configured for, as a resource attribute.
    ///
    /// Read as a **fallback** for detection: the current OTel GenAI conventions are deliberately
    /// framework-neutral, so a producer that follows them emits nothing to sniff. The Vercel AI SDK's
    /// current OpenTelemetry integration is exactly that - pure `gen_ai.*`, no `ai.*` at all - and its spans
    /// therefore arrived as `Unknown` however carefully the rules were written. A declaration by our own SDK
    /// is the only honest source for that, and it cannot override per-span evidence.
    pub const SIDESEAT_FRAMEWORK: &str = "sideseat.framework";

    // Session/User
    #[cfg(test)]
    pub const SESSION_ID: &str = "session.id";
    #[cfg(test)]
    pub const USER_ID: &str = "user.id";
    #[cfg(test)]
    pub const ENDUSER_ID: &str = "enduser.id";

    // GenAI Core
    pub const GEN_AI_SYSTEM: &str = "gen_ai.system";
    #[cfg(test)]
    pub const GEN_AI_PROVIDER_NAME: &str = "gen_ai.provider.name"; // New OTEL semconv
    pub const GEN_AI_OPERATION_NAME: &str = "gen_ai.operation.name";
    pub const GEN_AI_REQUEST_MODEL: &str = "gen_ai.request.model";
    pub const GEN_AI_RESPONSE_MODEL: &str = "gen_ai.response.model";
    #[cfg(test)]
    pub const GEN_AI_RESPONSE_ID: &str = "gen_ai.response.id";

    // GenAI Request Parameters
    #[cfg(test)]
    pub const GEN_AI_TEMPERATURE: &str = "gen_ai.request.temperature";
    #[cfg(test)]
    pub const GEN_AI_TOP_P: &str = "gen_ai.request.top_p";
    #[cfg(test)]
    pub const GEN_AI_TOP_K: &str = "gen_ai.request.top_k";
    #[cfg(test)]
    pub const GEN_AI_MAX_TOKENS: &str = "gen_ai.request.max_tokens";
    #[cfg(test)]
    pub const GEN_AI_FREQUENCY_PENALTY: &str = "gen_ai.request.frequency_penalty";
    #[cfg(test)]
    pub const GEN_AI_PRESENCE_PENALTY: &str = "gen_ai.request.presence_penalty";
    #[cfg(test)]
    pub const GEN_AI_STOP_SEQUENCES: &str = "gen_ai.request.stop_sequences";
    #[cfg(test)]
    pub const GEN_AI_FINISH_REASONS: &str = "gen_ai.response.finish_reasons";

    // GenAI Agent/Tool
    pub const GEN_AI_AGENT_ID: &str = "gen_ai.agent.id";
    pub const GEN_AI_AGENT_NAME: &str = "gen_ai.agent.name";
    pub const GEN_AI_TOOL_NAME: &str = "gen_ai.tool.name";
    pub const GEN_AI_TOOL_CALL_ID: &str = "gen_ai.tool.call.id";
    pub const GEN_AI_TOOL_STATUS: &str = "gen_ai.tool.status";

    // GenAI Performance
    #[cfg(test)]
    pub const GEN_AI_TTFT: &str = "gen_ai.server.time_to_first_token";
    #[cfg(test)]
    pub const GEN_AI_REQUEST_DURATION: &str = "gen_ai.server.request_duration";

    // Framework Session IDs
    #[cfg(test)]
    pub const LANGSMITH_SESSION_ID: &str = "langsmith.session.id";
    #[cfg(test)]
    pub const LANGSMITH_TRACE_SESSION_ID: &str = "langsmith.trace.session_id";
    #[cfg(test)]
    pub const GCP_VERTEX_SESSION_ID: &str = "gcp.vertex.agent.session_id";

    // LangSmith
    #[cfg(test)]
    pub const LANGSMITH_TRACE_NAME: &str = "langsmith.trace.name";

    // LangGraph
    #[cfg(test)]
    pub const LANGGRAPH_CHECKPOINT_NS: &str = "langgraph.checkpoint_ns";
    #[cfg(test)]
    pub const LANGGRAPH_NODE: &str = "langgraph.node";
    #[cfg(test)]
    pub const LANGGRAPH_THREAD_ID: &str = "langgraph.thread_id";

    // Span Kind Attributes
    pub const OPENINFERENCE_SPAN_KIND: &str = "openinference.span.kind";
    pub const LANGSMITH_SPAN_KIND: &str = "langsmith.span.kind";

    // OpenInference LLM attributes
    #[cfg(test)]
    pub const LLM_INVOCATION_PARAMETERS: &str = "llm.invocation_parameters";

    // OpenInference Tool attributes (single tool per span)

    // OpenInference Cost Tracking
    pub const LLM_COST_TOTAL: &str = "llm.cost.total";
    pub const LLM_COST_PROMPT: &str = "llm.cost.prompt";
    pub const LLM_COST_COMPLETION: &str = "llm.cost.completion";

    // OpenInference Embedding attributes
    /// Named by the OpenInference asset now; kept for the equivalence oracle's reference.
    #[cfg(test)]
    pub const EMBEDDING_TEXT: &str = "embedding.text";
    #[cfg(test)]
    pub const EMBEDDING_MODEL_NAME: &str = "embedding.model_name";

    // OpenInference Reranker attributes
    /// Named by the OpenInference asset now; kept for the equivalence oracle's reference.
    #[cfg(test)]
    pub const RERANKER_QUERY: &str = "reranker.query";
    #[cfg(test)]
    pub const RERANKER_MODEL_NAME: &str = "reranker.model_name";

    // HTTP
    pub const HTTP_METHOD: &str = "http.method";
    pub const HTTP_REQUEST_METHOD: &str = "http.request.method";
    #[cfg(test)]
    pub const HTTP_URL: &str = "http.url";
    #[cfg(test)]
    pub const URL_FULL: &str = "url.full";
    #[cfg(test)]
    pub const HTTP_STATUS_CODE: &str = "http.status_code";
    #[cfg(test)]
    pub const HTTP_RESPONSE_STATUS_CODE: &str = "http.response.status_code";

    // RPC
    pub const RPC_SYSTEM: &str = "rpc.system";

    // Database
    pub const DB_SYSTEM: &str = "db.system";
    #[cfg(test)]
    pub const DB_NAME: &str = "db.name";
    #[cfg(test)]
    pub const DB_OPERATION: &str = "db.operation";
    #[cfg(test)]
    pub const DB_STATEMENT: &str = "db.statement";

    // Storage
    #[cfg(test)]
    pub const CLOUD_PROVIDER: &str = "cloud.provider";
    #[cfg(test)]
    pub const AWS_S3_BUCKET: &str = "aws.s3.bucket";
    #[cfg(test)]
    pub const AWS_S3_KEY: &str = "aws.s3.key";
    #[cfg(test)]
    pub const GCP_GCS_BUCKET: &str = "gcp.gcs.bucket";
    #[cfg(test)]
    pub const GCP_GCS_OBJECT: &str = "gcp.gcs.object";

    // Messaging
    pub const MESSAGING_SYSTEM: &str = "messaging.system";
    #[cfg(test)]
    pub const MESSAGING_DESTINATION: &str = "messaging.destination";
    #[cfg(test)]
    pub const MESSAGING_DESTINATION_NAME: &str = "messaging.destination.name";

    // Tags/Metadata
    #[cfg(test)]
    pub const TAGS: &str = "tags";
    #[cfg(test)]
    pub const LANGSMITH_TAGS: &str = "langsmith.tags";
    #[cfg(test)]
    pub const TAG_TAGS: &str = "tag.tags";
    pub const METADATA: &str = "metadata";

    // I/O Attributes
    /// Named by the assets now; kept for the equivalence oracles.
    #[cfg(test)]
    pub const INPUT_VALUE: &str = "input.value";
    pub const OUTPUT_VALUE: &str = "output.value";
    /// Named only by the AutoGen asset now; kept for the equivalence oracle's reference implementation.
    #[cfg(test)]
    pub const MESSAGE: &str = "message";
    #[cfg(test)]
    pub const EVENTS: &str = "events";

    // Claude Code CLI (Claude Agent SDK). Content attributes require detailed beta
    // tracing: ENABLE_BETA_TRACING_DETAILED=1 plus BETA_TRACING_ENDPOINT.
    #[cfg(test)]
    pub const CLAUDE_CODE_NEW_CONTEXT: &str = "new_context";
    #[cfg(test)]
    pub const CLAUDE_CODE_MODEL_OUTPUT: &str = "response.model_output";
    #[cfg(test)]
    pub const CLAUDE_CODE_USER_SYSTEM_PROMPT: &str = "user_system_prompt";
    #[cfg(test)]
    pub const CLAUDE_CODE_TOOL_NAME: &str = "tool_name";
    #[cfg(test)]
    pub const CLAUDE_CODE_TOOL_INPUT: &str = "tool_input";
    #[cfg(test)]
    pub const CLAUDE_CODE_TOOL_USE_ID: &str = "tool_use_id";

    // Logfire
    #[cfg(test)]
    pub const PROMPT: &str = "prompt";
    #[cfg(test)]
    pub const ALL_MESSAGES_EVENTS: &str = "all_messages_events";
    #[cfg(test)]
    pub const REQUEST_DATA: &str = "request_data";
    pub const RESPONSE_DATA: &str = "response_data";
    #[cfg(test)]
    pub const LOGFIRE_MSG_TEMPLATE: &str = "logfire.msg_template";
    #[cfg(test)]
    pub const LOGFIRE_MSG: &str = "logfire.msg";

    // Pydantic AI (via Logfire)
    #[cfg(test)]
    pub const TOOL_ARGUMENTS: &str = "tool_arguments";
    #[cfg(test)]
    pub const TOOL_RESPONSE: &str = "tool_response";
    #[cfg(test)]
    pub const PYDANTIC_AI_ALL_MESSAGES: &str = "pydantic_ai.all_messages";
    #[cfg(test)]
    pub const GEN_AI_SYSTEM_INSTRUCTIONS: &str = "gen_ai.system_instructions";

    // OTEL Standard GenAI Messages
    /// Named by the assets now; kept for the equivalence oracles.
    #[cfg(test)]
    pub const GEN_AI_INPUT_MESSAGES: &str = "gen_ai.input.messages";
    pub const GEN_AI_OUTPUT_MESSAGES: &str = "gen_ai.output.messages";
    #[cfg(test)]
    pub const GEN_AI_TOOL_CALL_ARGUMENTS: &str = "gen_ai.tool.call.arguments";
    #[cfg(test)]
    pub const GEN_AI_TOOL_CALL_RESULT: &str = "gen_ai.tool.call.result";

    // LangSmith OTEL Exporter
    #[cfg(test)]
    pub const GEN_AI_PROMPT: &str = "gen_ai.prompt";
    pub const GEN_AI_COMPLETION: &str = "gen_ai.completion";

    // LiveKit
    #[cfg(test)]
    pub const LK_INPUT_TEXT: &str = "lk.input_text";
    #[cfg(test)]
    pub const LK_USER_INPUT: &str = "lk.user_input";
    #[cfg(test)]
    pub const LK_INSTRUCTIONS: &str = "lk.instructions";
    #[cfg(test)]
    pub const LK_CHAT_CTX: &str = "lk.chat_ctx";
    #[cfg(test)]
    pub const LK_FUNCTION_TOOLS: &str = "lk.function_tools";
    #[cfg(test)]
    pub const LK_RESPONSE_TEXT: &str = "lk.response.text";
    #[cfg(test)]
    pub const LK_RESPONSE_FUNCTION_CALLS: &str = "lk.response.function_calls";
    #[cfg(test)]
    pub const LK_FUNCTION_TOOL_ID: &str = "lk.function_tool.id";
    #[cfg(test)]
    pub const LK_FUNCTION_TOOL_NAME: &str = "lk.function_tool.name";
    #[cfg(test)]
    pub const LK_FUNCTION_TOOL_ARGS: &str = "lk.function_tool.arguments";
    #[cfg(test)]
    pub const LK_FUNCTION_TOOL_OUTPUT: &str = "lk.function_tool.output";
    #[cfg(test)]
    pub const LK_FUNCTION_TOOL_IS_ERROR: &str = "lk.function_tool.is_error";

    // MLflow
    #[cfg(test)]
    pub const MLFLOW_SPAN_INPUTS: &str = "mlflow.spanInputs";
    #[cfg(test)]
    pub const MLFLOW_SPAN_OUTPUTS: &str = "mlflow.spanOutputs";
    #[cfg(test)]
    pub const MLFLOW_CHAT_TOOLS: &str = "mlflow.chat.tools";
    pub const MLFLOW_CHAT_TOKEN_USAGE: &str = "mlflow.chat.tokenUsage";
    #[cfg(test)]
    pub const MLFLOW_TRACE_SESSION: &str = "mlflow.trace.session";
    #[cfg(test)]
    pub const MLFLOW_TRACE_USER: &str = "mlflow.trace.user";

    // TraceLoop
    #[cfg(test)]
    pub const TRACELOOP_ENTITY_INPUT: &str = "traceloop.entity.input";
    #[cfg(test)]
    pub const TRACELOOP_ENTITY_OUTPUT: &str = "traceloop.entity.output";

    // Vercel AI SDK.
    //
    // Named by the assets in production; these are kept for the equivalence oracles' reference
    // implementations, which are the retired extractors and legitimately name every dialect.
    #[cfg(test)]
    pub const AI_PROMPT: &str = "ai.prompt";
    #[cfg(test)]
    pub const AI_PROMPT_MESSAGES: &str = "ai.prompt.messages";
    #[cfg(test)]
    pub const AI_TOOLCALL_NAME: &str = "ai.toolCall.name";
    #[cfg(test)]
    pub const AI_TOOLCALL_ID: &str = "ai.toolCall.id";
    #[cfg(test)]
    pub const GCP_VERTEX_TOOL_RESPONSE: &str = "gcp.vertex.agent.tool_response";
    pub const AI_MODEL_ID: &str = "ai.model.id";
    pub const AI_MODEL_PROVIDER: &str = "ai.model.provider";
    pub const AI_OPERATION_ID: &str = "ai.operationId";
    #[cfg(test)]
    pub const AI_RESULT_TEXT: &str = "ai.result.text";
    #[cfg(test)]
    pub const AI_RESULT_OBJECT: &str = "ai.result.object";
    #[cfg(test)]
    pub const AI_RESULT_TOOL_CALLS: &str = "ai.result.toolCalls";
    #[cfg(test)]
    pub const AI_TOOLCALL_ARGS: &str = "ai.toolCall.args";
    #[cfg(test)]
    pub const AI_TOOLCALL_RESULT: &str = "ai.toolCall.result";
    #[cfg(test)]
    pub const AI_TELEMETRY_SESSION_ID: &str = "ai.telemetry.metadata.sessionId";
    #[cfg(test)]
    pub const AI_TELEMETRY_USER_ID: &str = "ai.telemetry.metadata.userId";

    // Google ADK
    #[cfg(test)]
    pub const GCP_VERTEX_LLM_REQUEST: &str = "gcp.vertex.agent.llm_request";
    pub const GCP_VERTEX_LLM_RESPONSE: &str = "gcp.vertex.agent.llm_response";
    #[cfg(test)]
    pub const GCP_VERTEX_TOOL_CALL_ARGS: &str = "gcp.vertex.agent.tool_call_args";
    #[cfg(test)]
    pub const GCP_VERTEX_DATA: &str = "gcp.vertex.agent.data";

    // AWS Bedrock
    #[cfg(test)]
    pub const AWS_BEDROCK_AGENT_ID: &str = "aws.bedrock.agent.id";

    // OTEL event names. Which events carry messages is declared (`message_events` in the assets); these
    // remain for the equivalence oracles' reference implementations.
    #[cfg(test)]
    pub const EVENT_USER_MESSAGE: &str = "gen_ai.user.message";
    #[cfg(test)]
    pub const EVENT_ASSISTANT_MESSAGE: &str = "gen_ai.assistant.message";
    pub const EVENT_TOOL_MESSAGE: &str = "gen_ai.tool.message";
    pub const EVENT_CHOICE: &str = "gen_ai.choice";
    pub const EVENT_CONTENT_COMPLETION: &str = "gen_ai.content.completion";
}

// ============================================================================
// PIPELINE STEP 1a: ATTRIBUTE EXTRACTION
// ============================================================================

/// Extract span attributes from an OTLP trace request.
///
/// Pipeline Step 1a: Parses protobuf, extracts GenAI attributes, and classifies spans.
pub(super) fn extract_attributes_batch(request: &ExportTraceServiceRequest) -> Vec<SpanData> {
    let mut spans = Vec::new();

    for resource_spans in &request.resource_spans {
        let resource_attrs = resource_spans
            .resource
            .as_ref()
            .map(|r| extract_attributes(&r.attributes))
            .unwrap_or_default();

        for scope_spans in &resource_spans.scope_spans {
            // The scope is per ScopeSpans group, read once: empty strings are absences, the same
            // filtering the metrics path applies.
            let (scope_name, scope_version) = scope_spans
                .scope
                .as_ref()
                .map(|scope| {
                    (
                        Some(scope.name.clone()).filter(|s| !s.is_empty()),
                        Some(scope.version.clone()).filter(|s| !s.is_empty()),
                    )
                })
                .unwrap_or((None, None));
            for otlp_span in &scope_spans.spans {
                let span_attrs = extract_attributes(&otlp_span.attributes);
                let mut span = SpanData {
                    scope_name: scope_name.clone(),
                    scope_version: scope_version.clone(),
                    ..SpanData::default()
                };

                // Core OTLP fields
                attributes::set_core_fields(&mut span, otlp_span);

                // The display name is a declared field now, resolved with all the others below.

                // Extract resource attributes
                span.project_id = resource_attrs.get(keys::PROJECT_ID).cloned();
                span.environment = resource_attrs
                    .get(keys::DEPLOYMENT_ENV)
                    .or_else(|| resource_attrs.get(keys::DEPLOYMENT_ENV_NAME))
                    .cloned();

                // Every declared span field, resolved once.
                attributes::apply_span_fields(&mut span, &otlp_span.name, &span_attrs);

                // Extract GenAI attributes
                attributes::extract_genai(&mut span, &span_attrs, &otlp_span.name);

                // Extract finish_reason from various sources if not already set
                if span.gen_ai_finish_reasons.is_empty() {
                    // 1. Try gen_ai.choice event (Strands, OpenTelemetry GenAI)
                    for event in &otlp_span.events {
                        if event.name == "gen_ai.choice" {
                            let event_attrs = extract_attributes(&event.attributes);
                            if let Some(reason) = event_attrs.get("finish_reason") {
                                span.gen_ai_finish_reasons = vec![reason.clone()];
                                break;
                            }
                        }
                    }
                }

                if span.gen_ai_finish_reasons.is_empty() {
                    // 2. Try gen_ai.completion JSON (OpenLLMetry, LangSmith)
                    if let Some(completion) = span_attrs.get(keys::GEN_AI_COMPLETION) {
                        if let Ok(json) = serde_json::from_str::<JsonValue>(completion) {
                            if let Some(choices) = json.get("choices").and_then(|c| c.as_array()) {
                                for choice in choices {
                                    if let Some(reason) =
                                        choice.get("finish_reason").and_then(|r| r.as_str())
                                    {
                                        span.gen_ai_finish_reasons.push(reason.to_string());
                                    }
                                }
                            }
                        }
                    }
                }

                if span.gen_ai_finish_reasons.is_empty() {
                    // 3. Try gen_ai.output.messages JSON (PydanticAI)
                    if let Some(output_msgs) = span_attrs.get(keys::GEN_AI_OUTPUT_MESSAGES) {
                        if let Ok(msgs) = serde_json::from_str::<Vec<JsonValue>>(output_msgs) {
                            for msg in &msgs {
                                if let Some(reason) =
                                    msg.get("finish_reason").and_then(|r| r.as_str())
                                {
                                    span.gen_ai_finish_reasons.push(reason.to_string());
                                    break;
                                }
                            }
                        }
                    }
                }

                if span.gen_ai_finish_reasons.is_empty() {
                    // 4. Try ADK/Vertex response (gcp.vertex.llm.response)
                    if let Some(response) = span_attrs.get(keys::GCP_VERTEX_LLM_RESPONSE) {
                        if let Ok(json) = serde_json::from_str::<JsonValue>(response) {
                            if let Some(reason) = json.get("finish_reason").and_then(|r| r.as_str())
                            {
                                span.gen_ai_finish_reasons = vec![reason.to_lowercase()];
                            }
                        }
                    }
                }

                if span.gen_ai_finish_reasons.is_empty() {
                    // 5. Try logfire response_data (Text Completions: {finish_reason, text, usage})
                    if let Some(response) = span_attrs.get(keys::RESPONSE_DATA) {
                        if let Ok(json) = serde_json::from_str::<JsonValue>(response) {
                            if let Some(reason) = json.get("finish_reason").and_then(|r| r.as_str())
                            {
                                span.gen_ai_finish_reasons = vec![reason.to_string()];
                            }
                        }
                    }
                }

                // Classify span
                span.framework = Some(attributes::detect_framework(
                    &otlp_span.name,
                    &span_attrs,
                    &resource_attrs,
                ));
                span.observation_type = Some(attributes::detect_observation_type(
                    &otlp_span.name,
                    &span_attrs,
                ));
                span.span_category =
                    Some(attributes::categorize_span(&otlp_span.name, &span_attrs));

                // Enhance status from gen_ai.tool.status if OTEL status is not ERROR
                if span.status_code.as_deref() != Some("ERROR") {
                    if let Some(tool_status) = span_attrs.get(keys::GEN_AI_TOOL_STATUS) {
                        if tool_status.eq_ignore_ascii_case("error")
                            || tool_status.eq_ignore_ascii_case("failed")
                        {
                            span.status_code = Some("ERROR".to_string());
                            let msg = "Tool execution failed".to_string();
                            if span.status_message.is_none() {
                                span.status_message = Some(msg.clone());
                            }
                            if span.exception_message.is_none() {
                                span.exception_message = Some(msg);
                            }
                        }
                    }
                }

                // Extract exception data into separate fields (raw preservation)
                if span.status_code.as_deref() == Some("ERROR") {
                    for event in &otlp_span.events {
                        if event.name == "exception" {
                            let event_attrs = extract_attributes(&event.attributes);

                            if let Some(t) =
                                event_attrs.get("exception.type").filter(|s| !s.is_empty())
                            {
                                span.exception_type = Some(
                                    truncate_bytes(t, constants::ERROR_MESSAGE_MAX_LEN).to_string(),
                                );
                            }
                            if let Some(m) = event_attrs
                                .get("exception.message")
                                .filter(|s| !s.is_empty())
                            {
                                span.exception_message = Some(
                                    truncate_bytes(m, constants::ERROR_MESSAGE_MAX_LEN).to_string(),
                                );
                            }
                            if let Some(st) = event_attrs
                                .get("exception.stacktrace")
                                .filter(|s| !s.is_empty())
                            {
                                span.exception_stacktrace = Some(
                                    truncate_bytes(st, constants::ERROR_STACKTRACE_MAX_LEN)
                                        .to_string(),
                                );
                            }

                            // Enrich status_message for Raw tab display
                            if span.status_message.is_none() {
                                span.status_message = match (
                                    span.exception_type.as_deref(),
                                    span.exception_message.as_deref(),
                                ) {
                                    (Some(t), Some(m)) => Some(format!("{t}: {m}")),
                                    (_, Some(m)) => Some(m.to_string()),
                                    (Some(t), _) => Some(t.to_string()),
                                    _ => None,
                                };
                            }

                            break;
                        }
                    }

                    // No fallback from status_message → exception_message:
                    // OTEL SDKs propagate error status up the span tree, so every
                    // ancestor gets status_message. Only exception events and
                    // gen_ai.tool.status carry real error details for feed display.
                }

                // Metadata
                span.metadata = span_attrs
                    .get(keys::METADATA)
                    .and_then(|m| serde_json::from_str(m).ok())
                    .unwrap_or(JsonValue::Null);

                spans.push(span);
            }
        }
    }

    spans
}

// ============================================================================
// PIPELINE STEP 1b: MESSAGE EXTRACTION
// ============================================================================

/// Extract messages and tool definitions from an OTLP trace request.
///
/// Pipeline Step 1b: Extracts raw messages and tool definitions from OTEL events and span attributes.
/// Should be called after `extract_attributes_batch` with the corresponding spans.
///
/// Returns a tuple of (messages, tool_definitions, tool_names) where each inner Vec corresponds to a span.
#[allow(clippy::type_complexity)]
pub(super) fn extract_messages_batch(
    request: &ExportTraceServiceRequest,
    spans: &[SpanData],
    mode: messages::ExtractionMode,
) -> (
    Vec<Vec<RawMessage>>,
    Vec<Vec<RawToolDefinition>>,
    Vec<Vec<RawToolNames>>,
) {
    let mut all_messages = Vec::new();
    let mut all_tool_definitions = Vec::new();
    let mut all_tool_names = Vec::new();
    let mut span_idx = 0;

    for resource_spans in &request.resource_spans {
        for scope_spans in &resource_spans.scope_spans {
            for otlp_span in &scope_spans.spans {
                let span_attrs = extract_attributes(&otlp_span.attributes);
                let span = &spans[span_idx];
                span_idx += 1;

                let (raw_messages, tool_definitions, tool_names) =
                    messages::extract_messages_for_span(
                        otlp_span,
                        &span_attrs,
                        span.timestamp_start,
                        mode,
                    );
                all_messages.push(raw_messages);
                all_tool_definitions.push(tool_definitions);
                all_tool_names.push(tool_names);
            }
        }
    }

    (all_messages, all_tool_definitions, all_tool_names)
}
