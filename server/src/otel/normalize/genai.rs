//! Common GenAI field extraction

use opentelemetry_proto::tonic::common::v1::any_value;
use opentelemetry_proto::tonic::trace::v1::Span as OtlpSpan;

use super::NormalizedSpan;

/// Extract common GenAI semantic convention fields
pub fn extract_common_genai_fields(span: &OtlpSpan, normalized: &mut NormalizedSpan) {
    // Provider and Model
    normalized.gen_ai_system =
        normalized.gen_ai_system.take().or_else(|| get_string_attr(span, "gen_ai.system"));
    normalized.gen_ai_provider_name = get_string_attr(span, "gen_ai.provider.name");
    normalized.gen_ai_request_model = normalized
        .gen_ai_request_model
        .take()
        .or_else(|| get_string_attr(span, "gen_ai.request.model"));
    normalized.gen_ai_response_model = get_string_attr(span, "gen_ai.response.model");

    // Operation and Conversation
    normalized.gen_ai_operation_name = get_string_attr(span, "gen_ai.operation.name");
    normalized.gen_ai_conversation_id = get_string_attr(span, "gen_ai.conversation.id");
    normalized.gen_ai_response_id = get_string_attr(span, "gen_ai.response.id");

    // Agent Metadata
    normalized.gen_ai_agent_id =
        normalized.gen_ai_agent_id.take().or_else(|| get_string_attr(span, "gen_ai.agent.id"));
    normalized.gen_ai_agent_name =
        normalized.gen_ai_agent_name.take().or_else(|| get_string_attr(span, "gen_ai.agent.name"));

    // Response Metadata
    normalized.gen_ai_response_finish_reasons =
        get_string_array_attr(span, "gen_ai.response.finish_reasons");

    // Token Usage (try multiple attribute names)
    normalized.usage_input_tokens = normalized
        .usage_input_tokens
        .or_else(|| get_i64_attr(span, "gen_ai.usage.input_tokens"))
        .or_else(|| get_i64_attr(span, "gen_ai.usage.prompt_tokens"));
    normalized.usage_output_tokens = normalized
        .usage_output_tokens
        .or_else(|| get_i64_attr(span, "gen_ai.usage.output_tokens"))
        .or_else(|| get_i64_attr(span, "gen_ai.usage.completion_tokens"));
    normalized.usage_total_tokens = normalized
        .usage_total_tokens
        .or_else(|| get_i64_attr(span, "gen_ai.usage.total_tokens"))
        .or_else(|| {
            // Compute total if not provided
            match (normalized.usage_input_tokens, normalized.usage_output_tokens) {
                (Some(i), Some(o)) => Some(i + o),
                _ => None,
            }
        });

    // Request Parameters
    normalized.gen_ai_request_temperature = get_f64_attr(span, "gen_ai.request.temperature");
    normalized.gen_ai_request_top_p = get_f64_attr(span, "gen_ai.request.top_p");
    normalized.gen_ai_request_top_k = get_i64_attr(span, "gen_ai.request.top_k");
    normalized.gen_ai_request_max_tokens = get_i64_attr(span, "gen_ai.request.max_tokens");
    normalized.gen_ai_request_frequency_penalty =
        get_f64_attr(span, "gen_ai.request.frequency_penalty");
    normalized.gen_ai_request_presence_penalty =
        get_f64_attr(span, "gen_ai.request.presence_penalty");
    normalized.gen_ai_request_stop_sequences =
        get_string_array_attr(span, "gen_ai.request.stop_sequences");

    // Tool Fields
    normalized.gen_ai_tool_name =
        normalized.gen_ai_tool_name.take().or_else(|| get_string_attr(span, "gen_ai.tool.name"));
    normalized.gen_ai_tool_call_id = get_string_attr(span, "gen_ai.tool.call.id");
}

// Helper functions for attribute extraction
pub fn has_attribute(span: &OtlpSpan, key: &str) -> bool {
    span.attributes.iter().any(|a| a.key == key)
}

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
