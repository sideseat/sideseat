//! Span schema for Arrow/Parquet

use arrow::array::*;
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use std::sync::Arc;

use crate::otel::error::OtelResult;
use crate::otel::normalize::NormalizedSpan;

/// Arrow/Parquet schema for spans
pub struct SpanSchema;

impl SpanSchema {
    /// Get Arrow schema for spans
    pub fn arrow_schema() -> Arc<Schema> {
        Arc::new(Schema::new(vec![
            Field::new("trace_id", DataType::Utf8, false),
            Field::new("span_id", DataType::Utf8, false),
            Field::new("parent_span_id", DataType::Utf8, true),
            Field::new("start_time_unix_nano", DataType::Int64, false),
            Field::new("end_time_unix_nano", DataType::Int64, true),
            Field::new("duration_ns", DataType::Int64, true),
            Field::new("service_name", DataType::Utf8, false),
            Field::new("service_version", DataType::Utf8, true),
            Field::new("span_name", DataType::Utf8, false),
            Field::new("span_kind", DataType::Int8, false),
            Field::new("status_code", DataType::Int8, false),
            Field::new("status_message", DataType::Utf8, true),
            Field::new("detected_framework", DataType::Utf8, false),
            Field::new("detected_category", DataType::Utf8, true),
            Field::new("gen_ai_system", DataType::Utf8, true),
            Field::new("gen_ai_operation_name", DataType::Utf8, true),
            Field::new("gen_ai_agent_name", DataType::Utf8, true),
            Field::new("gen_ai_request_model", DataType::Utf8, true),
            Field::new("gen_ai_response_model", DataType::Utf8, true),
            Field::new("usage_input_tokens", DataType::Int64, true),
            Field::new("usage_output_tokens", DataType::Int64, true),
            Field::new("usage_total_tokens", DataType::Int64, true),
            Field::new("gen_ai_tool_name", DataType::Utf8, true),
            Field::new("attributes_json", DataType::Utf8, false),
            Field::new("resource_attributes_json", DataType::Utf8, true),
            Field::new("scope_name", DataType::Utf8, true),
            Field::new("scope_version", DataType::Utf8, true),
        ]))
    }

    /// Convert spans to Arrow RecordBatch
    pub fn to_record_batch(spans: &[NormalizedSpan]) -> OtelResult<RecordBatch> {
        let schema = Self::arrow_schema();

        // Build column arrays
        let trace_ids: StringArray = spans.iter().map(|s| Some(s.trace_id.as_str())).collect();
        let span_ids: StringArray = spans.iter().map(|s| Some(s.span_id.as_str())).collect();
        let parent_span_ids: StringArray =
            spans.iter().map(|s| s.parent_span_id.as_deref()).collect();
        let start_times: Int64Array = spans.iter().map(|s| Some(s.start_time_unix_nano)).collect();
        let end_times: Int64Array = spans.iter().map(|s| s.end_time_unix_nano).collect();
        let durations: Int64Array = spans.iter().map(|s| s.duration_ns).collect();
        let service_names: StringArray =
            spans.iter().map(|s| Some(s.service_name.as_str())).collect();
        let service_versions: StringArray =
            spans.iter().map(|s| s.service_version.as_deref()).collect();
        let span_names: StringArray = spans.iter().map(|s| Some(s.span_name.as_str())).collect();
        let span_kinds: Int8Array = spans.iter().map(|s| Some(s.span_kind)).collect();
        let status_codes: Int8Array = spans.iter().map(|s| Some(s.status_code)).collect();
        let status_messages: StringArray =
            spans.iter().map(|s| s.status_message.as_deref()).collect();
        let detected_frameworks: StringArray =
            spans.iter().map(|s| Some(s.detected_framework.as_str())).collect();
        let detected_categories: StringArray =
            spans.iter().map(|s| s.detected_category.as_deref()).collect();
        let gen_ai_systems: StringArray =
            spans.iter().map(|s| s.gen_ai_system.as_deref()).collect();
        let gen_ai_operations: StringArray =
            spans.iter().map(|s| s.gen_ai_operation_name.as_deref()).collect();
        let gen_ai_agent_names: StringArray =
            spans.iter().map(|s| s.gen_ai_agent_name.as_deref()).collect();
        let gen_ai_request_models: StringArray =
            spans.iter().map(|s| s.gen_ai_request_model.as_deref()).collect();
        let gen_ai_response_models: StringArray =
            spans.iter().map(|s| s.gen_ai_response_model.as_deref()).collect();
        let usage_input_tokens: Int64Array = spans.iter().map(|s| s.usage_input_tokens).collect();
        let usage_output_tokens: Int64Array = spans.iter().map(|s| s.usage_output_tokens).collect();
        let usage_total_tokens: Int64Array = spans.iter().map(|s| s.usage_total_tokens).collect();
        let gen_ai_tool_names: StringArray =
            spans.iter().map(|s| s.gen_ai_tool_name.as_deref()).collect();
        let attributes_jsons: StringArray =
            spans.iter().map(|s| Some(s.attributes_json.as_str())).collect();
        let resource_attributes_jsons: StringArray =
            spans.iter().map(|s| s.resource_attributes_json.as_deref()).collect();
        let scope_names: StringArray = spans.iter().map(|s| s.scope_name.as_deref()).collect();
        let scope_versions: StringArray =
            spans.iter().map(|s| s.scope_version.as_deref()).collect();

        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(trace_ids),
                Arc::new(span_ids),
                Arc::new(parent_span_ids),
                Arc::new(start_times),
                Arc::new(end_times),
                Arc::new(durations),
                Arc::new(service_names),
                Arc::new(service_versions),
                Arc::new(span_names),
                Arc::new(span_kinds),
                Arc::new(status_codes),
                Arc::new(status_messages),
                Arc::new(detected_frameworks),
                Arc::new(detected_categories),
                Arc::new(gen_ai_systems),
                Arc::new(gen_ai_operations),
                Arc::new(gen_ai_agent_names),
                Arc::new(gen_ai_request_models),
                Arc::new(gen_ai_response_models),
                Arc::new(usage_input_tokens),
                Arc::new(usage_output_tokens),
                Arc::new(usage_total_tokens),
                Arc::new(gen_ai_tool_names),
                Arc::new(attributes_jsons),
                Arc::new(resource_attributes_jsons),
                Arc::new(scope_names),
                Arc::new(scope_versions),
            ],
        )
        .map_err(crate::otel::error::OtelError::Arrow)
    }
}

/// Convert spans to Arrow RecordBatch (convenience function)
pub fn to_record_batch(spans: &[NormalizedSpan]) -> OtelResult<RecordBatch> {
    SpanSchema::to_record_batch(spans)
}

/// Convert span references to Arrow RecordBatch (for use with borrowed data)
pub fn to_record_batch_refs(spans: &[&NormalizedSpan]) -> OtelResult<RecordBatch> {
    let schema = SpanSchema::arrow_schema();

    // Build column arrays from references
    let trace_ids: StringArray = spans.iter().map(|s| Some(s.trace_id.as_str())).collect();
    let span_ids: StringArray = spans.iter().map(|s| Some(s.span_id.as_str())).collect();
    let parent_span_ids: StringArray = spans.iter().map(|s| s.parent_span_id.as_deref()).collect();
    let start_times: Int64Array = spans.iter().map(|s| Some(s.start_time_unix_nano)).collect();
    let end_times: Int64Array = spans.iter().map(|s| s.end_time_unix_nano).collect();
    let durations: Int64Array = spans.iter().map(|s| s.duration_ns).collect();
    let service_names: StringArray = spans.iter().map(|s| Some(s.service_name.as_str())).collect();
    let service_versions: StringArray =
        spans.iter().map(|s| s.service_version.as_deref()).collect();
    let span_names: StringArray = spans.iter().map(|s| Some(s.span_name.as_str())).collect();
    let span_kinds: Int8Array = spans.iter().map(|s| Some(s.span_kind)).collect();
    let status_codes: Int8Array = spans.iter().map(|s| Some(s.status_code)).collect();
    let status_messages: StringArray = spans.iter().map(|s| s.status_message.as_deref()).collect();
    let detected_frameworks: StringArray =
        spans.iter().map(|s| Some(s.detected_framework.as_str())).collect();
    let detected_categories: StringArray =
        spans.iter().map(|s| s.detected_category.as_deref()).collect();
    let gen_ai_systems: StringArray = spans.iter().map(|s| s.gen_ai_system.as_deref()).collect();
    let gen_ai_operations: StringArray =
        spans.iter().map(|s| s.gen_ai_operation_name.as_deref()).collect();
    let gen_ai_agent_names: StringArray =
        spans.iter().map(|s| s.gen_ai_agent_name.as_deref()).collect();
    let gen_ai_request_models: StringArray =
        spans.iter().map(|s| s.gen_ai_request_model.as_deref()).collect();
    let gen_ai_response_models: StringArray =
        spans.iter().map(|s| s.gen_ai_response_model.as_deref()).collect();
    let usage_input_tokens: Int64Array = spans.iter().map(|s| s.usage_input_tokens).collect();
    let usage_output_tokens: Int64Array = spans.iter().map(|s| s.usage_output_tokens).collect();
    let usage_total_tokens: Int64Array = spans.iter().map(|s| s.usage_total_tokens).collect();
    let gen_ai_tool_names: StringArray =
        spans.iter().map(|s| s.gen_ai_tool_name.as_deref()).collect();
    let attributes_jsons: StringArray =
        spans.iter().map(|s| Some(s.attributes_json.as_str())).collect();
    let resource_attributes_jsons: StringArray =
        spans.iter().map(|s| s.resource_attributes_json.as_deref()).collect();
    let scope_names: StringArray = spans.iter().map(|s| s.scope_name.as_deref()).collect();
    let scope_versions: StringArray = spans.iter().map(|s| s.scope_version.as_deref()).collect();

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(trace_ids),
            Arc::new(span_ids),
            Arc::new(parent_span_ids),
            Arc::new(start_times),
            Arc::new(end_times),
            Arc::new(durations),
            Arc::new(service_names),
            Arc::new(service_versions),
            Arc::new(span_names),
            Arc::new(span_kinds),
            Arc::new(status_codes),
            Arc::new(status_messages),
            Arc::new(detected_frameworks),
            Arc::new(detected_categories),
            Arc::new(gen_ai_systems),
            Arc::new(gen_ai_operations),
            Arc::new(gen_ai_agent_names),
            Arc::new(gen_ai_request_models),
            Arc::new(gen_ai_response_models),
            Arc::new(usage_input_tokens),
            Arc::new(usage_output_tokens),
            Arc::new(usage_total_tokens),
            Arc::new(gen_ai_tool_names),
            Arc::new(attributes_jsons),
            Arc::new(resource_attributes_jsons),
            Arc::new(scope_names),
            Arc::new(scope_versions),
        ],
    )
    .map_err(crate::otel::error::OtelError::Arrow)
}
