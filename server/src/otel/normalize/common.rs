//! Cross-framework attribute normalization and helper functions
//!
//! This module provides:
//! 1. Helper functions for extracting OTLP span attributes (get_string_attr, get_i64_attr, etc.)
//! 2. Cross-framework normalization that maps vendor-specific attributes to common field names
//!
//! ## Normalized fields (mapped to common names):
//! - session_id: langfuse.session.id, session.id, aws.bedrock.agent.session_id
//! - user_id: langfuse.user.id, user.id, enduser.id
//! - tags: langfuse.trace.tags, tags
//! - environment: deployment.environment, langfuse.trace.environment
//! - gen_ai_*: Various GenAI semantic convention fields
//! - usage_*_tokens: Token usage from multiple sources
//!
//! ## Service-specific fields (stored in attributes_json as-is):
//! - aws.bedrock.guardrail.id, aws.bedrock.knowledge_base.id, aws.bedrock.agent.alias_id
//! - langfuse.observation.id, langfuse.parent.observation.id, langfuse.trace.metadata.*
//! - langfuse.prompt.name, langfuse.prompt.version

use opentelemetry_proto::tonic::common::v1::any_value;
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::Span as OtlpSpan;

use super::NormalizedSpan;

// ============================================================================
// Attribute Helper Functions
// ============================================================================

/// Check if span has a specific attribute
pub fn has_attribute(span: &OtlpSpan, key: &str) -> bool {
    span.attributes.iter().any(|a| a.key == key)
}

/// Get string attribute from span
pub fn get_string_attr(span: &OtlpSpan, key: &str) -> Option<String> {
    span.attributes
        .iter()
        .find(|a| a.key == key)
        .and_then(|a| a.value.as_ref())
        .and_then(|v| v.value.as_ref())
        .and_then(|v| match v {
            any_value::Value::StringValue(s) => Some(s.clone()),
            _ => None,
        })
}

/// Get i64 attribute from span
pub fn get_i64_attr(span: &OtlpSpan, key: &str) -> Option<i64> {
    span.attributes
        .iter()
        .find(|a| a.key == key)
        .and_then(|a| a.value.as_ref())
        .and_then(|v| v.value.as_ref())
        .and_then(|v| match v {
            any_value::Value::IntValue(i) => Some(*i),
            _ => None,
        })
}

/// Get f64 attribute from span
pub fn get_f64_attr(span: &OtlpSpan, key: &str) -> Option<f64> {
    span.attributes
        .iter()
        .find(|a| a.key == key)
        .and_then(|a| a.value.as_ref())
        .and_then(|v| v.value.as_ref())
        .and_then(|v| match v {
            any_value::Value::DoubleValue(d) => Some(*d),
            _ => None,
        })
}

/// Get string array attribute from span (returns JSON array string)
pub fn get_string_array_attr(span: &OtlpSpan, key: &str) -> Option<String> {
    span.attributes
        .iter()
        .find(|a| a.key == key)
        .and_then(|a| a.value.as_ref())
        .and_then(|v| v.value.as_ref())
        .and_then(|v| match v {
            any_value::Value::ArrayValue(arr) => {
                let strings: Vec<String> = arr
                    .values
                    .iter()
                    .filter_map(|v| match v.value.as_ref()? {
                        any_value::Value::StringValue(s) => Some(s.clone()),
                        _ => None,
                    })
                    .collect();
                serde_json::to_string(&strings).ok()
            }
            _ => None,
        })
}

/// Get string attribute from resource
fn get_resource_string_attr(resource: &Resource, key: &str) -> Option<String> {
    resource
        .attributes
        .iter()
        .find(|a| a.key == key)
        .and_then(|a| a.value.as_ref())
        .and_then(|v| v.value.as_ref())
        .and_then(|v| match v {
            any_value::Value::StringValue(s) => Some(s.clone()),
            _ => None,
        })
}

// ============================================================================
// Cross-Framework Normalization
// ============================================================================

/// Extract common cross-framework fields from span and resource attributes
///
/// This normalizes various vendor-specific attribute names to common field names.
/// Service-specific fields (like aws.bedrock.guardrail.id) remain in attributes_json.
pub fn extract_common_fields(
    span: &OtlpSpan,
    resource: &Resource,
    normalized: &mut NormalizedSpan,
) {
    // === Identity & Session Fields ===
    normalize_session_fields(span, normalized);
    normalize_user_fields(span, normalized);
    normalize_tags_and_environment(span, resource, normalized);

    // === GenAI Fields ===
    // Note: model must be extracted before identity because provider can be inferred from model
    normalize_genai_model(span, normalized);
    normalize_genai_identity(span, normalized);
    normalize_genai_operation(span, normalized);
    normalize_genai_request_params(span, normalized);
    normalize_genai_response(span, normalized);
    normalize_token_usage(span, normalized);
    normalize_tool_fields(span, normalized);
    normalize_performance_metrics(span, normalized);
}

/// Normalize GenAI performance metrics (TTFT, request duration)
fn normalize_performance_metrics(span: &OtlpSpan, normalized: &mut NormalizedSpan) {
    // Time to First Token (milliseconds)
    if normalized.time_to_first_token_ms.is_none() {
        normalized.time_to_first_token_ms = None
            .or_else(|| get_i64_attr(span, "gen_ai.server.time_to_first_token"))
            .or_else(|| get_i64_attr(span, "llm.time_to_first_token"))
            .or_else(|| get_i64_attr(span, "time_to_first_token_ms"))
            .or_else(|| get_i64_attr(span, "ttft_ms"));
    }

    // Request Duration (milliseconds)
    if normalized.request_duration_ms.is_none() {
        normalized.request_duration_ms = None
            .or_else(|| get_i64_attr(span, "gen_ai.server.request.duration"))
            .or_else(|| get_i64_attr(span, "llm.request.duration"))
            .or_else(|| get_i64_attr(span, "request_duration_ms"))
            // AWS Bedrock
            .or_else(|| get_i64_attr(span, "aws.bedrock.invocation.latency"));
    }
}

/// Normalize session ID from various sources
fn normalize_session_fields(span: &OtlpSpan, normalized: &mut NormalizedSpan) {
    if normalized.session_id.is_none() {
        normalized.session_id = None
            // OpenInference / Generic
            .or_else(|| get_string_attr(span, "session.id"))
            // Langfuse
            .or_else(|| get_string_attr(span, "langfuse.session.id"))
            .or_else(|| get_string_attr(span, "langfuse_session_id"))
            // AWS Bedrock AgentCore
            .or_else(|| get_string_attr(span, "aws.bedrock.agent.session_id"))
            .or_else(|| get_string_attr(span, "aws.bedrock.session.id"))
            .or_else(|| get_string_attr(span, "aws.bedrock.invocation.session_id"))
            // LangSmith
            .or_else(|| get_string_attr(span, "langsmith.trace.session_id"))
            // Strands
            .or_else(|| get_string_attr(span, "strands.session.id"))
            // Generic conversation/thread
            .or_else(|| get_string_attr(span, "conversation.id"))
            .or_else(|| get_string_attr(span, "thread.id"))
            .or_else(|| get_string_attr(span, "gen_ai.conversation.id"));
    }

    // Also map conversation_id if session is found but conversation isn't
    if normalized.gen_ai_conversation_id.is_none() {
        normalized.gen_ai_conversation_id = normalized
            .session_id
            .clone()
            .or_else(|| get_string_attr(span, "gen_ai.conversation.id"))
            .or_else(|| get_string_attr(span, "conversation_id"));
    }
}

/// Normalize user ID from various sources
fn normalize_user_fields(span: &OtlpSpan, normalized: &mut NormalizedSpan) {
    if normalized.user_id.is_none() {
        normalized.user_id = None
            // OpenInference / Generic
            .or_else(|| get_string_attr(span, "user.id"))
            // Langfuse
            .or_else(|| get_string_attr(span, "langfuse.user.id"))
            .or_else(|| get_string_attr(span, "langfuse_user_id"))
            // OpenTelemetry semantic conventions
            .or_else(|| get_string_attr(span, "enduser.id"))
            // AWS Bedrock
            .or_else(|| get_string_attr(span, "aws.bedrock.user.id"))
            // Generic variations
            .or_else(|| get_string_attr(span, "user_id"))
            .or_else(|| get_string_attr(span, "userId"))
            .or_else(|| get_string_attr(span, "gen_ai.user.id"));
    }
}

/// Normalize tags and environment
fn normalize_tags_and_environment(
    span: &OtlpSpan,
    resource: &Resource,
    normalized: &mut NormalizedSpan,
) {
    // Tags - stored as JSON array string
    if normalized.tags.is_none() {
        normalized.tags = None
            // Langfuse
            .or_else(|| get_string_array_attr(span, "langfuse.trace.tags"))
            // LangSmith
            .or_else(|| get_string_array_attr(span, "langsmith.trace.tags"))
            // Generic
            .or_else(|| get_string_array_attr(span, "tags"))
            .or_else(|| get_string_array_attr(span, "labels"))
            // Single tag value - wrap in array
            .or_else(|| get_string_attr(span, "tag").map(|t| format!("[\"{}\"]", t)));
    }

    // Environment - span attributes take precedence over resource
    if normalized.environment.is_none() {
        normalized.environment = None
            // Standard OTEL
            .or_else(|| get_string_attr(span, "deployment.environment"))
            // Langfuse
            .or_else(|| get_string_attr(span, "langfuse.trace.environment"))
            // Generic
            .or_else(|| get_string_attr(span, "environment"))
            .or_else(|| get_string_attr(span, "env"))
            // Resource attributes (fallback)
            .or_else(|| get_resource_string_attr(resource, "deployment.environment"))
            .or_else(|| get_resource_string_attr(resource, "service.environment"))
            .or_else(|| get_resource_string_attr(resource, "environment"));
    }
}

/// Normalize GenAI identity fields (agent, system, provider)
fn normalize_genai_identity(span: &OtlpSpan, normalized: &mut NormalizedSpan) {
    // Agent ID
    if normalized.gen_ai_agent_id.is_none() {
        normalized.gen_ai_agent_id = None
            .or_else(|| get_string_attr(span, "gen_ai.agent.id"))
            // AWS Bedrock
            .or_else(|| get_string_attr(span, "aws.bedrock.agent.id"))
            .or_else(|| get_string_attr(span, "aws.bedrock.agent_id"))
            // Generic
            .or_else(|| get_string_attr(span, "agent.id"))
            .or_else(|| get_string_attr(span, "agentId"))
            .or_else(|| get_string_attr(span, "agent_id"));
    }

    // Agent Name
    if normalized.gen_ai_agent_name.is_none() {
        normalized.gen_ai_agent_name = None
            .or_else(|| get_string_attr(span, "gen_ai.agent.name"))
            // AWS Bedrock
            .or_else(|| get_string_attr(span, "aws.bedrock.agent.name"))
            .or_else(|| get_string_attr(span, "aws.bedrock.agent_name"))
            // Strands
            .or_else(|| get_string_attr(span, "strands.agent.name"))
            // Generic
            .or_else(|| get_string_attr(span, "agent.name"))
            .or_else(|| get_string_attr(span, "agentName"))
            .or_else(|| get_string_attr(span, "agent_name"));
    }

    // System/Provider
    if normalized.gen_ai_system.is_none() {
        normalized.gen_ai_system = None
            .or_else(|| get_string_attr(span, "gen_ai.system"))
            // Langfuse
            .or_else(|| get_string_attr(span, "langfuse.system"))
            // Generic
            .or_else(|| get_string_attr(span, "llm.system"))
            .or_else(|| get_string_attr(span, "llm.provider"))
            .or_else(|| get_string_attr(span, "ai.system"))
            .or_else(|| get_string_attr(span, "ai.provider"))
            // Infer from model name
            .or_else(|| infer_system_from_model(normalized.gen_ai_request_model.as_deref()));
    }

    // Provider Name (separate from system)
    if normalized.gen_ai_provider_name.is_none() {
        normalized.gen_ai_provider_name = None
            .or_else(|| get_string_attr(span, "gen_ai.provider.name"))
            .or_else(|| get_string_attr(span, "llm.provider.name"))
            // AWS Bedrock uses the provider in the model ID
            .or_else(|| {
                normalized.gen_ai_request_model.as_ref().and_then(|m| {
                    if m.contains('.') { m.split('.').next().map(|s| s.to_string()) } else { None }
                })
            });
    }
}

/// Normalize GenAI model fields
fn normalize_genai_model(span: &OtlpSpan, normalized: &mut NormalizedSpan) {
    // Request Model
    if normalized.gen_ai_request_model.is_none() {
        normalized.gen_ai_request_model = None
            .or_else(|| get_string_attr(span, "gen_ai.request.model"))
            // AWS Bedrock
            .or_else(|| get_string_attr(span, "aws.bedrock.model_id"))
            .or_else(|| get_string_attr(span, "aws.bedrock.model.id"))
            .or_else(|| get_string_attr(span, "aws.bedrock.invocation.model_id"))
            // Langfuse
            .or_else(|| get_string_attr(span, "langfuse.model"))
            .or_else(|| get_string_attr(span, "langfuse.model_id"))
            // OpenInference / LlamaIndex
            .or_else(|| get_string_attr(span, "llm.model_name"))
            .or_else(|| get_string_attr(span, "llm.model"))
            // Generic
            .or_else(|| get_string_attr(span, "model"))
            .or_else(|| get_string_attr(span, "modelId"))
            .or_else(|| get_string_attr(span, "model_id"))
            .or_else(|| get_string_attr(span, "model_name"));
    }

    // Response Model (may differ from request model)
    if normalized.gen_ai_response_model.is_none() {
        normalized.gen_ai_response_model = None
            .or_else(|| get_string_attr(span, "gen_ai.response.model"))
            .or_else(|| get_string_attr(span, "llm.response.model"))
            // Often same as request model
            .or_else(|| normalized.gen_ai_request_model.clone());
    }
}

/// Normalize GenAI operation fields
fn normalize_genai_operation(span: &OtlpSpan, normalized: &mut NormalizedSpan) {
    // Operation Name
    if normalized.gen_ai_operation_name.is_none() {
        normalized.gen_ai_operation_name = None
            .or_else(|| get_string_attr(span, "gen_ai.operation.name"))
            // AWS Bedrock
            .or_else(|| get_string_attr(span, "aws.bedrock.operation"))
            .or_else(|| get_string_attr(span, "aws.bedrock.invocation.operation"))
            // Generic
            .or_else(|| get_string_attr(span, "llm.operation"))
            .or_else(|| get_string_attr(span, "operation.name"));
    }

    // Response ID
    if normalized.gen_ai_response_id.is_none() {
        normalized.gen_ai_response_id = None
            .or_else(|| get_string_attr(span, "gen_ai.response.id"))
            // AWS Bedrock
            .or_else(|| get_string_attr(span, "aws.bedrock.invocation.id"))
            .or_else(|| get_string_attr(span, "aws.bedrock.request_id"))
            // Langfuse
            .or_else(|| get_string_attr(span, "langfuse.generation.id"))
            // Generic
            .or_else(|| get_string_attr(span, "response.id"))
            .or_else(|| get_string_attr(span, "completion.id"));
    }
}

/// Normalize GenAI request parameters
fn normalize_genai_request_params(span: &OtlpSpan, normalized: &mut NormalizedSpan) {
    // Temperature
    if normalized.gen_ai_request_temperature.is_none() {
        normalized.gen_ai_request_temperature = None
            .or_else(|| get_f64_attr(span, "gen_ai.request.temperature"))
            .or_else(|| get_f64_attr(span, "llm.temperature"))
            .or_else(|| get_f64_attr(span, "temperature"))
            // Langfuse
            .or_else(|| get_f64_attr(span, "langfuse.temperature"));
    }

    // Top P
    if normalized.gen_ai_request_top_p.is_none() {
        normalized.gen_ai_request_top_p = None
            .or_else(|| get_f64_attr(span, "gen_ai.request.top_p"))
            .or_else(|| get_f64_attr(span, "llm.top_p"))
            .or_else(|| get_f64_attr(span, "top_p"))
            .or_else(|| get_f64_attr(span, "topP"));
    }

    // Top K
    if normalized.gen_ai_request_top_k.is_none() {
        normalized.gen_ai_request_top_k = None
            .or_else(|| get_i64_attr(span, "gen_ai.request.top_k"))
            .or_else(|| get_i64_attr(span, "llm.top_k"))
            .or_else(|| get_i64_attr(span, "top_k"))
            .or_else(|| get_i64_attr(span, "topK"));
    }

    // Max Tokens
    if normalized.gen_ai_request_max_tokens.is_none() {
        normalized.gen_ai_request_max_tokens = None
            .or_else(|| get_i64_attr(span, "gen_ai.request.max_tokens"))
            .or_else(|| get_i64_attr(span, "llm.max_tokens"))
            .or_else(|| get_i64_attr(span, "max_tokens"))
            .or_else(|| get_i64_attr(span, "maxTokens"))
            // Langfuse
            .or_else(|| get_i64_attr(span, "langfuse.max_tokens"));
    }

    // Frequency Penalty
    if normalized.gen_ai_request_frequency_penalty.is_none() {
        normalized.gen_ai_request_frequency_penalty = None
            .or_else(|| get_f64_attr(span, "gen_ai.request.frequency_penalty"))
            .or_else(|| get_f64_attr(span, "llm.frequency_penalty"))
            .or_else(|| get_f64_attr(span, "frequency_penalty"))
            .or_else(|| get_f64_attr(span, "frequencyPenalty"));
    }

    // Presence Penalty
    if normalized.gen_ai_request_presence_penalty.is_none() {
        normalized.gen_ai_request_presence_penalty = None
            .or_else(|| get_f64_attr(span, "gen_ai.request.presence_penalty"))
            .or_else(|| get_f64_attr(span, "llm.presence_penalty"))
            .or_else(|| get_f64_attr(span, "presence_penalty"))
            .or_else(|| get_f64_attr(span, "presencePenalty"));
    }

    // Stop Sequences
    if normalized.gen_ai_request_stop_sequences.is_none() {
        normalized.gen_ai_request_stop_sequences = None
            .or_else(|| get_string_array_attr(span, "gen_ai.request.stop_sequences"))
            .or_else(|| get_string_array_attr(span, "llm.stop_sequences"))
            .or_else(|| get_string_array_attr(span, "stop_sequences"))
            .or_else(|| get_string_array_attr(span, "stop"));
    }
}

/// Normalize GenAI response fields
fn normalize_genai_response(span: &OtlpSpan, normalized: &mut NormalizedSpan) {
    // Finish Reasons
    if normalized.gen_ai_response_finish_reasons.is_none() {
        normalized.gen_ai_response_finish_reasons = None
            .or_else(|| get_string_array_attr(span, "gen_ai.response.finish_reasons"))
            .or_else(|| get_string_array_attr(span, "llm.finish_reasons"))
            // Single value
            .or_else(|| get_string_attr(span, "finish_reason").map(|r| format!("[\"{}\"]", r)))
            .or_else(|| get_string_attr(span, "stop_reason").map(|r| format!("[\"{}\"]", r)));
    }
}

/// Normalize token usage from various attribute naming conventions
fn normalize_token_usage(span: &OtlpSpan, normalized: &mut NormalizedSpan) {
    // Input tokens
    if normalized.usage_input_tokens.is_none() {
        normalized.usage_input_tokens = None
            .or_else(|| get_i64_attr(span, "gen_ai.usage.input_tokens"))
            .or_else(|| get_i64_attr(span, "gen_ai.usage.prompt_tokens"))
            // OpenInference / LlamaIndex
            .or_else(|| get_i64_attr(span, "llm.token_count.prompt"))
            .or_else(|| get_i64_attr(span, "llm.usage.prompt_tokens"))
            // Langfuse
            .or_else(|| get_i64_attr(span, "langfuse.usage.input"))
            .or_else(|| get_i64_attr(span, "langfuse.usage.promptTokens"))
            // AWS Bedrock
            .or_else(|| get_i64_attr(span, "aws.bedrock.usage.input_tokens"))
            .or_else(|| get_i64_attr(span, "aws.bedrock.invocation.input_tokens"))
            // Generic
            .or_else(|| get_i64_attr(span, "input_tokens"))
            .or_else(|| get_i64_attr(span, "promptTokens"))
            .or_else(|| get_i64_attr(span, "prompt_tokens"));
    }

    // Output tokens
    if normalized.usage_output_tokens.is_none() {
        normalized.usage_output_tokens = None
            .or_else(|| get_i64_attr(span, "gen_ai.usage.output_tokens"))
            .or_else(|| get_i64_attr(span, "gen_ai.usage.completion_tokens"))
            // OpenInference / LlamaIndex
            .or_else(|| get_i64_attr(span, "llm.token_count.completion"))
            .or_else(|| get_i64_attr(span, "llm.usage.completion_tokens"))
            // Langfuse
            .or_else(|| get_i64_attr(span, "langfuse.usage.output"))
            .or_else(|| get_i64_attr(span, "langfuse.usage.completionTokens"))
            // AWS Bedrock
            .or_else(|| get_i64_attr(span, "aws.bedrock.usage.output_tokens"))
            .or_else(|| get_i64_attr(span, "aws.bedrock.invocation.output_tokens"))
            // Generic
            .or_else(|| get_i64_attr(span, "output_tokens"))
            .or_else(|| get_i64_attr(span, "completionTokens"))
            .or_else(|| get_i64_attr(span, "completion_tokens"));
    }

    // Total tokens - compute if not provided
    if normalized.usage_total_tokens.is_none() {
        normalized.usage_total_tokens = None
            .or_else(|| get_i64_attr(span, "gen_ai.usage.total_tokens"))
            .or_else(|| get_i64_attr(span, "llm.token_count.total"))
            // Langfuse
            .or_else(|| get_i64_attr(span, "langfuse.usage.total"))
            .or_else(|| get_i64_attr(span, "langfuse.usage.totalTokens"))
            // AWS Bedrock
            .or_else(|| get_i64_attr(span, "aws.bedrock.usage.total_tokens"))
            // Generic
            .or_else(|| get_i64_attr(span, "total_tokens"))
            .or_else(|| get_i64_attr(span, "totalTokens"))
            // Compute from input + output
            .or_else(|| match (normalized.usage_input_tokens, normalized.usage_output_tokens) {
                (Some(i), Some(o)) => Some(i + o),
                _ => None,
            });
    }

    // Cache read tokens (Anthropic)
    if normalized.usage_cache_read_tokens.is_none() {
        normalized.usage_cache_read_tokens = None
            .or_else(|| get_i64_attr(span, "gen_ai.usage.cache_read_tokens"))
            .or_else(|| get_i64_attr(span, "cache_read_input_tokens"))
            .or_else(|| get_i64_attr(span, "cacheReadInputTokens"));
    }

    // Cache write tokens (Anthropic)
    if normalized.usage_cache_write_tokens.is_none() {
        normalized.usage_cache_write_tokens = None
            .or_else(|| get_i64_attr(span, "gen_ai.usage.cache_write_tokens"))
            .or_else(|| get_i64_attr(span, "cache_creation_input_tokens"))
            .or_else(|| get_i64_attr(span, "cacheCreationInputTokens"));
    }
}

/// Normalize tool-related fields
fn normalize_tool_fields(span: &OtlpSpan, normalized: &mut NormalizedSpan) {
    // Tool Name
    if normalized.gen_ai_tool_name.is_none() {
        normalized.gen_ai_tool_name = None
            .or_else(|| get_string_attr(span, "gen_ai.tool.name"))
            // AWS Bedrock
            .or_else(|| get_string_attr(span, "aws.bedrock.tool.name"))
            .or_else(|| get_string_attr(span, "aws.bedrock.action_group.name"))
            // Generic
            .or_else(|| get_string_attr(span, "tool.name"))
            .or_else(|| get_string_attr(span, "function.name"))
            .or_else(|| get_string_attr(span, "toolName"));
    }

    // Tool Call ID
    if normalized.gen_ai_tool_call_id.is_none() {
        normalized.gen_ai_tool_call_id = None
            .or_else(|| get_string_attr(span, "gen_ai.tool.call.id"))
            // Generic
            .or_else(|| get_string_attr(span, "tool.call.id"))
            .or_else(|| get_string_attr(span, "tool_call_id"))
            .or_else(|| get_string_attr(span, "toolCallId"));
    }

    // Tool Status
    if normalized.tool_status.is_none() {
        normalized.tool_status = None
            .or_else(|| get_string_attr(span, "tool.status"))
            .or_else(|| get_string_attr(span, "tool_status"))
            .or_else(|| get_string_attr(span, "function.status"));
    }
}

/// Infer GenAI system from model name patterns
fn infer_system_from_model(model: Option<&str>) -> Option<String> {
    let model = model?;
    let model_lower = model.to_lowercase();

    if model_lower.contains("gpt") || model_lower.contains("o1") || model_lower.contains("davinci")
    {
        Some("openai".to_string())
    } else if model_lower.contains("claude") {
        Some("anthropic".to_string())
    } else if model_lower.contains("gemini") || model_lower.contains("palm") {
        Some("google".to_string())
    } else if model_lower.contains("llama") || model_lower.contains("meta") {
        Some("meta".to_string())
    } else if model_lower.contains("mistral") || model_lower.contains("mixtral") {
        Some("mistral".to_string())
    } else if model_lower.contains("titan") || model_lower.contains("amazon") {
        Some("aws".to_string())
    } else if model_lower.contains("cohere") || model_lower.contains("command") {
        Some("cohere".to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::common::v1::{AnyValue, ArrayValue, KeyValue};

    // ========================================================================
    // Test Helpers
    // ========================================================================

    fn make_span_with_attr(key: &str, value: any_value::Value) -> OtlpSpan {
        OtlpSpan {
            attributes: vec![KeyValue {
                key: key.to_string(),
                value: Some(AnyValue { value: Some(value) }),
            }],
            ..Default::default()
        }
    }

    fn make_span_with_string_attrs(attrs: Vec<(&str, &str)>) -> OtlpSpan {
        let attributes: Vec<KeyValue> = attrs
            .into_iter()
            .map(|(k, v)| KeyValue {
                key: k.to_string(),
                value: Some(AnyValue { value: Some(any_value::Value::StringValue(v.to_string())) }),
            })
            .collect();
        OtlpSpan { attributes, ..Default::default() }
    }

    fn make_resource_with_attrs(attrs: Vec<(&str, &str)>) -> Resource {
        let attributes: Vec<KeyValue> = attrs
            .into_iter()
            .map(|(k, v)| KeyValue {
                key: k.to_string(),
                value: Some(AnyValue { value: Some(any_value::Value::StringValue(v.to_string())) }),
            })
            .collect();
        Resource { attributes, ..Default::default() }
    }

    fn make_empty_span() -> OtlpSpan {
        OtlpSpan::default()
    }

    // ========================================================================
    // Helper Function Tests
    // ========================================================================

    #[test]
    fn test_has_attribute_true() {
        let span = make_span_with_attr(
            "gen_ai.system",
            any_value::Value::StringValue("openai".to_string()),
        );
        assert!(has_attribute(&span, "gen_ai.system"));
    }

    #[test]
    fn test_has_attribute_false() {
        let span = make_empty_span();
        assert!(!has_attribute(&span, "gen_ai.system"));
    }

    #[test]
    fn test_get_string_attr_found() {
        let span = make_span_with_attr(
            "gen_ai.system",
            any_value::Value::StringValue("openai".to_string()),
        );
        assert_eq!(get_string_attr(&span, "gen_ai.system"), Some("openai".to_string()));
    }

    #[test]
    fn test_get_string_attr_not_found() {
        let span = make_empty_span();
        assert_eq!(get_string_attr(&span, "gen_ai.system"), None);
    }

    #[test]
    fn test_get_string_attr_wrong_type() {
        let span = make_span_with_attr("count", any_value::Value::IntValue(42));
        assert_eq!(get_string_attr(&span, "count"), None);
    }

    #[test]
    fn test_get_i64_attr_found() {
        let span =
            make_span_with_attr("gen_ai.usage.input_tokens", any_value::Value::IntValue(100));
        assert_eq!(get_i64_attr(&span, "gen_ai.usage.input_tokens"), Some(100));
    }

    #[test]
    fn test_get_i64_attr_not_found() {
        let span = make_empty_span();
        assert_eq!(get_i64_attr(&span, "gen_ai.usage.input_tokens"), None);
    }

    #[test]
    fn test_get_i64_attr_wrong_type() {
        let span = make_span_with_attr("count", any_value::Value::StringValue("100".to_string()));
        assert_eq!(get_i64_attr(&span, "count"), None);
    }

    #[test]
    fn test_get_f64_attr_found() {
        let span =
            make_span_with_attr("gen_ai.request.temperature", any_value::Value::DoubleValue(0.7));
        assert_eq!(get_f64_attr(&span, "gen_ai.request.temperature"), Some(0.7));
    }

    #[test]
    fn test_get_f64_attr_not_found() {
        let span = make_empty_span();
        assert_eq!(get_f64_attr(&span, "gen_ai.request.temperature"), None);
    }

    #[test]
    fn test_get_f64_attr_wrong_type() {
        let span = make_span_with_attr("temp", any_value::Value::IntValue(1));
        assert_eq!(get_f64_attr(&span, "temp"), None);
    }

    #[test]
    fn test_get_string_array_attr_found() {
        let arr = ArrayValue {
            values: vec![
                AnyValue { value: Some(any_value::Value::StringValue("stop".to_string())) },
                AnyValue { value: Some(any_value::Value::StringValue("end".to_string())) },
            ],
        };
        let span =
            make_span_with_attr("gen_ai.request.stop_sequences", any_value::Value::ArrayValue(arr));
        let result = get_string_array_attr(&span, "gen_ai.request.stop_sequences");
        assert!(result.is_some());
        let parsed: Vec<String> = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(parsed, vec!["stop", "end"]);
    }

    #[test]
    fn test_get_string_array_attr_not_found() {
        let span = make_empty_span();
        assert_eq!(get_string_array_attr(&span, "gen_ai.request.stop_sequences"), None);
    }

    #[test]
    fn test_get_string_array_attr_wrong_type() {
        let span =
            make_span_with_attr("arr", any_value::Value::StringValue("not array".to_string()));
        assert_eq!(get_string_array_attr(&span, "arr"), None);
    }

    #[test]
    fn test_get_string_array_attr_filters_non_strings() {
        let arr = ArrayValue {
            values: vec![
                AnyValue { value: Some(any_value::Value::StringValue("valid".to_string())) },
                AnyValue { value: Some(any_value::Value::IntValue(123)) },
                AnyValue { value: Some(any_value::Value::StringValue("also_valid".to_string())) },
            ],
        };
        let span = make_span_with_attr("mixed", any_value::Value::ArrayValue(arr));
        let result = get_string_array_attr(&span, "mixed");
        assert!(result.is_some());
        let parsed: Vec<String> = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(parsed, vec!["valid", "also_valid"]);
    }

    // ========================================================================
    // Session ID Normalization Tests
    // ========================================================================

    #[test]
    fn test_session_id_from_openinference() {
        let span = make_span_with_string_attrs(vec![("session.id", "oi-session-456")]);
        let mut normalized = NormalizedSpan::default();
        extract_common_fields(&span, &Resource::default(), &mut normalized);
        assert_eq!(normalized.session_id, Some("oi-session-456".to_string()));
    }

    #[test]
    fn test_session_id_from_langfuse() {
        let span = make_span_with_string_attrs(vec![("langfuse.session.id", "lf-session-123")]);
        let mut normalized = NormalizedSpan::default();
        extract_common_fields(&span, &Resource::default(), &mut normalized);
        assert_eq!(normalized.session_id, Some("lf-session-123".to_string()));
    }

    #[test]
    fn test_session_id_from_bedrock() {
        let span =
            make_span_with_string_attrs(vec![("aws.bedrock.agent.session_id", "bedrock-session")]);
        let mut normalized = NormalizedSpan::default();
        extract_common_fields(&span, &Resource::default(), &mut normalized);
        assert_eq!(normalized.session_id, Some("bedrock-session".to_string()));
    }

    #[test]
    fn test_session_id_from_langsmith() {
        let span = make_span_with_string_attrs(vec![("langsmith.trace.session_id", "ls-session")]);
        let mut normalized = NormalizedSpan::default();
        extract_common_fields(&span, &Resource::default(), &mut normalized);
        assert_eq!(normalized.session_id, Some("ls-session".to_string()));
    }

    #[test]
    fn test_session_id_from_conversation() {
        let span = make_span_with_string_attrs(vec![("conversation.id", "conv-123")]);
        let mut normalized = NormalizedSpan::default();
        extract_common_fields(&span, &Resource::default(), &mut normalized);
        assert_eq!(normalized.session_id, Some("conv-123".to_string()));
    }

    // ========================================================================
    // User ID Normalization Tests
    // ========================================================================

    #[test]
    fn test_user_id_from_user_id() {
        let span = make_span_with_string_attrs(vec![("user.id", "user-123")]);
        let mut normalized = NormalizedSpan::default();
        extract_common_fields(&span, &Resource::default(), &mut normalized);
        assert_eq!(normalized.user_id, Some("user-123".to_string()));
    }

    #[test]
    fn test_user_id_from_langfuse() {
        let span = make_span_with_string_attrs(vec![("langfuse.user.id", "lf-user-abc")]);
        let mut normalized = NormalizedSpan::default();
        extract_common_fields(&span, &Resource::default(), &mut normalized);
        assert_eq!(normalized.user_id, Some("lf-user-abc".to_string()));
    }

    #[test]
    fn test_user_id_from_enduser() {
        let span = make_span_with_string_attrs(vec![("enduser.id", "end-user-123")]);
        let mut normalized = NormalizedSpan::default();
        extract_common_fields(&span, &Resource::default(), &mut normalized);
        assert_eq!(normalized.user_id, Some("end-user-123".to_string()));
    }

    // ========================================================================
    // Environment Normalization Tests
    // ========================================================================

    #[test]
    fn test_environment_from_span() {
        let span = make_span_with_string_attrs(vec![("deployment.environment", "staging")]);
        let mut normalized = NormalizedSpan::default();
        extract_common_fields(&span, &Resource::default(), &mut normalized);
        assert_eq!(normalized.environment, Some("staging".to_string()));
    }

    #[test]
    fn test_environment_from_resource() {
        let span = make_empty_span();
        let resource = make_resource_with_attrs(vec![("deployment.environment", "production")]);
        let mut normalized = NormalizedSpan::default();
        extract_common_fields(&span, &resource, &mut normalized);
        assert_eq!(normalized.environment, Some("production".to_string()));
    }

    #[test]
    fn test_environment_span_takes_precedence() {
        let span = make_span_with_string_attrs(vec![("deployment.environment", "staging")]);
        let resource = make_resource_with_attrs(vec![("deployment.environment", "production")]);
        let mut normalized = NormalizedSpan::default();
        extract_common_fields(&span, &resource, &mut normalized);
        assert_eq!(normalized.environment, Some("staging".to_string()));
    }

    #[test]
    fn test_environment_from_langfuse() {
        let span = make_span_with_string_attrs(vec![("langfuse.trace.environment", "dev")]);
        let mut normalized = NormalizedSpan::default();
        extract_common_fields(&span, &Resource::default(), &mut normalized);
        assert_eq!(normalized.environment, Some("dev".to_string()));
    }

    // ========================================================================
    // Model Normalization Tests
    // ========================================================================

    #[test]
    fn test_model_from_gen_ai() {
        let span = make_span_with_string_attrs(vec![("gen_ai.request.model", "gpt-4-turbo")]);
        let mut normalized = NormalizedSpan::default();
        extract_common_fields(&span, &Resource::default(), &mut normalized);
        assert_eq!(normalized.gen_ai_request_model, Some("gpt-4-turbo".to_string()));
    }

    #[test]
    fn test_model_from_bedrock() {
        let span =
            make_span_with_string_attrs(vec![("aws.bedrock.model_id", "anthropic.claude-v2")]);
        let mut normalized = NormalizedSpan::default();
        extract_common_fields(&span, &Resource::default(), &mut normalized);
        assert_eq!(normalized.gen_ai_request_model, Some("anthropic.claude-v2".to_string()));
    }

    #[test]
    fn test_model_from_langfuse() {
        let span = make_span_with_string_attrs(vec![("langfuse.model", "claude-3-opus")]);
        let mut normalized = NormalizedSpan::default();
        extract_common_fields(&span, &Resource::default(), &mut normalized);
        assert_eq!(normalized.gen_ai_request_model, Some("claude-3-opus".to_string()));
    }

    #[test]
    fn test_model_from_llm() {
        let span = make_span_with_string_attrs(vec![("llm.model_name", "gpt-3.5-turbo")]);
        let mut normalized = NormalizedSpan::default();
        extract_common_fields(&span, &Resource::default(), &mut normalized);
        assert_eq!(normalized.gen_ai_request_model, Some("gpt-3.5-turbo".to_string()));
    }

    // ========================================================================
    // System Inference Tests
    // ========================================================================

    #[test]
    fn test_infer_system_openai() {
        assert_eq!(infer_system_from_model(Some("gpt-4")), Some("openai".to_string()));
        assert_eq!(infer_system_from_model(Some("gpt-3.5-turbo")), Some("openai".to_string()));
        assert_eq!(infer_system_from_model(Some("o1-preview")), Some("openai".to_string()));
    }

    #[test]
    fn test_infer_system_anthropic() {
        assert_eq!(infer_system_from_model(Some("claude-3-opus")), Some("anthropic".to_string()));
        assert_eq!(infer_system_from_model(Some("claude-2")), Some("anthropic".to_string()));
    }

    #[test]
    fn test_infer_system_google() {
        assert_eq!(infer_system_from_model(Some("gemini-pro")), Some("google".to_string()));
        assert_eq!(infer_system_from_model(Some("gemini-1.5-flash")), Some("google".to_string()));
    }

    #[test]
    fn test_infer_system_meta() {
        assert_eq!(infer_system_from_model(Some("llama-3")), Some("meta".to_string()));
        assert_eq!(infer_system_from_model(Some("meta-llama")), Some("meta".to_string()));
    }

    #[test]
    fn test_infer_system_mistral() {
        assert_eq!(infer_system_from_model(Some("mistral-7b")), Some("mistral".to_string()));
        assert_eq!(infer_system_from_model(Some("mixtral-8x7b")), Some("mistral".to_string()));
    }

    #[test]
    fn test_infer_system_unknown() {
        assert_eq!(infer_system_from_model(Some("custom-model")), None);
        assert_eq!(infer_system_from_model(None), None);
    }

    // ========================================================================
    // Agent ID Normalization Tests
    // ========================================================================

    #[test]
    fn test_agent_id_from_gen_ai() {
        let span = make_span_with_string_attrs(vec![("gen_ai.agent.id", "agent-123")]);
        let mut normalized = NormalizedSpan::default();
        extract_common_fields(&span, &Resource::default(), &mut normalized);
        assert_eq!(normalized.gen_ai_agent_id, Some("agent-123".to_string()));
    }

    #[test]
    fn test_agent_id_from_bedrock() {
        let span = make_span_with_string_attrs(vec![("aws.bedrock.agent.id", "bedrock-agent-456")]);
        let mut normalized = NormalizedSpan::default();
        extract_common_fields(&span, &Resource::default(), &mut normalized);
        assert_eq!(normalized.gen_ai_agent_id, Some("bedrock-agent-456".to_string()));
    }

    #[test]
    fn test_agent_name_from_strands() {
        let span = make_span_with_string_attrs(vec![("strands.agent.name", "my-strands-agent")]);
        let mut normalized = NormalizedSpan::default();
        extract_common_fields(&span, &Resource::default(), &mut normalized);
        assert_eq!(normalized.gen_ai_agent_name, Some("my-strands-agent".to_string()));
    }

    // ========================================================================
    // Token Usage Normalization Tests
    // ========================================================================

    #[test]
    fn test_tokens_from_gen_ai() {
        let span = OtlpSpan {
            attributes: vec![
                KeyValue {
                    key: "gen_ai.usage.input_tokens".to_string(),
                    value: Some(AnyValue { value: Some(any_value::Value::IntValue(100)) }),
                },
                KeyValue {
                    key: "gen_ai.usage.output_tokens".to_string(),
                    value: Some(AnyValue { value: Some(any_value::Value::IntValue(50)) }),
                },
            ],
            ..Default::default()
        };
        let mut normalized = NormalizedSpan::default();
        extract_common_fields(&span, &Resource::default(), &mut normalized);
        assert_eq!(normalized.usage_input_tokens, Some(100));
        assert_eq!(normalized.usage_output_tokens, Some(50));
        assert_eq!(normalized.usage_total_tokens, Some(150));
    }

    #[test]
    fn test_tokens_from_langfuse() {
        let span = OtlpSpan {
            attributes: vec![
                KeyValue {
                    key: "langfuse.usage.input".to_string(),
                    value: Some(AnyValue { value: Some(any_value::Value::IntValue(200)) }),
                },
                KeyValue {
                    key: "langfuse.usage.output".to_string(),
                    value: Some(AnyValue { value: Some(any_value::Value::IntValue(75)) }),
                },
            ],
            ..Default::default()
        };
        let mut normalized = NormalizedSpan::default();
        extract_common_fields(&span, &Resource::default(), &mut normalized);
        assert_eq!(normalized.usage_input_tokens, Some(200));
        assert_eq!(normalized.usage_output_tokens, Some(75));
    }

    #[test]
    fn test_tokens_from_bedrock() {
        let span = OtlpSpan {
            attributes: vec![
                KeyValue {
                    key: "aws.bedrock.usage.input_tokens".to_string(),
                    value: Some(AnyValue { value: Some(any_value::Value::IntValue(150)) }),
                },
                KeyValue {
                    key: "aws.bedrock.usage.output_tokens".to_string(),
                    value: Some(AnyValue { value: Some(any_value::Value::IntValue(80)) }),
                },
            ],
            ..Default::default()
        };
        let mut normalized = NormalizedSpan::default();
        extract_common_fields(&span, &Resource::default(), &mut normalized);
        assert_eq!(normalized.usage_input_tokens, Some(150));
        assert_eq!(normalized.usage_output_tokens, Some(80));
    }

    // ========================================================================
    // Request Parameters Normalization Tests
    // ========================================================================

    #[test]
    fn test_temperature_from_gen_ai() {
        let span =
            make_span_with_attr("gen_ai.request.temperature", any_value::Value::DoubleValue(0.8));
        let mut normalized = NormalizedSpan::default();
        extract_common_fields(&span, &Resource::default(), &mut normalized);
        assert_eq!(normalized.gen_ai_request_temperature, Some(0.8));
    }

    #[test]
    fn test_max_tokens_from_gen_ai() {
        let span =
            make_span_with_attr("gen_ai.request.max_tokens", any_value::Value::IntValue(2048));
        let mut normalized = NormalizedSpan::default();
        extract_common_fields(&span, &Resource::default(), &mut normalized);
        assert_eq!(normalized.gen_ai_request_max_tokens, Some(2048));
    }

    // ========================================================================
    // Existing Values Not Overwritten Tests
    // ========================================================================

    #[test]
    fn test_existing_session_id_not_overwritten() {
        let span = make_span_with_string_attrs(vec![("session.id", "new-session")]);
        let mut normalized = NormalizedSpan {
            session_id: Some("existing-session".to_string()),
            ..Default::default()
        };
        extract_common_fields(&span, &Resource::default(), &mut normalized);
        assert_eq!(normalized.session_id, Some("existing-session".to_string()));
    }

    #[test]
    fn test_existing_model_not_overwritten() {
        let span = make_span_with_string_attrs(vec![("gen_ai.request.model", "new-model")]);
        let mut normalized = NormalizedSpan {
            gen_ai_request_model: Some("existing-model".to_string()),
            ..Default::default()
        };
        extract_common_fields(&span, &Resource::default(), &mut normalized);
        assert_eq!(normalized.gen_ai_request_model, Some("existing-model".to_string()));
    }

    // ========================================================================
    // Tool Fields Normalization Tests
    // ========================================================================

    #[test]
    fn test_tool_name_from_gen_ai() {
        let span = make_span_with_string_attrs(vec![("gen_ai.tool.name", "search_web")]);
        let mut normalized = NormalizedSpan::default();
        extract_common_fields(&span, &Resource::default(), &mut normalized);
        assert_eq!(normalized.gen_ai_tool_name, Some("search_web".to_string()));
    }

    #[test]
    fn test_tool_name_from_bedrock() {
        let span =
            make_span_with_string_attrs(vec![("aws.bedrock.action_group.name", "my-action-group")]);
        let mut normalized = NormalizedSpan::default();
        extract_common_fields(&span, &Resource::default(), &mut normalized);
        assert_eq!(normalized.gen_ai_tool_name, Some("my-action-group".to_string()));
    }

    #[test]
    fn test_tool_call_id() {
        let span = make_span_with_string_attrs(vec![("gen_ai.tool.call.id", "call-123")]);
        let mut normalized = NormalizedSpan::default();
        extract_common_fields(&span, &Resource::default(), &mut normalized);
        assert_eq!(normalized.gen_ai_tool_call_id, Some("call-123".to_string()));
    }

    // ========================================================================
    // Provider Extraction Tests
    // ========================================================================

    #[test]
    fn test_provider_from_bedrock_model_id() {
        let span =
            make_span_with_string_attrs(vec![("aws.bedrock.model_id", "anthropic.claude-v2")]);
        let mut normalized = NormalizedSpan::default();
        extract_common_fields(&span, &Resource::default(), &mut normalized);
        assert_eq!(normalized.gen_ai_provider_name, Some("anthropic".to_string()));
    }
}
