//! Test-only equivalence oracle for semantic field extraction.

use std::collections::HashMap;

use serde_json::Value as JsonValue;

use crate::traces::extract::{extract_json, keys};

use super::{SpanData, get_first, merge_tags};

/// The chains the declared resolvers replaced, kept as the equivalence oracle.
///
/// `the_field_rules_reproduce_the_chains_they_replaced` runs both over every span of the corpus and requires
/// the same answer, which is what makes the migration a provable no-op rather than a hope.
pub(in crate::traces::extract) fn extract_semantic_legacy(
    span: &mut SpanData,
    attrs: &HashMap<String, String>,
) {
    let metadata: Option<JsonValue> = extract_json(attrs, keys::METADATA);

    // Session ID with framework fallbacks (including Vercel AI telemetry metadata)
    span.session_id = get_first(
        attrs,
        &[
            keys::SESSION_ID,
            // Standard semconv 1.37 conversation id - every compliant emitter sets this,
            // and without it their spans do not group into sessions.
            "gen_ai.conversation.id",
            keys::LANGSMITH_SESSION_ID,
            keys::LANGSMITH_TRACE_SESSION_ID, // LangSmith OTEL exporter
            keys::GCP_VERTEX_SESSION_ID,
            keys::AI_TELEMETRY_SESSION_ID, // Vercel AI SDK
            keys::LANGGRAPH_THREAD_ID,     // LangGraph
            keys::MLFLOW_TRACE_SESSION,    // MLflow
        ],
    )
    .or_else(|| {
        // Try thread_id or langgraph_thread_id from metadata
        metadata.as_ref().and_then(|m| {
            m.get("thread_id")
                .or_else(|| m.get("langgraph_thread_id"))
                .and_then(|v| v.as_str())
                .map(String::from)
        })
    });

    // User ID (including Vercel AI telemetry metadata)
    span.user_id = get_first(
        attrs,
        &[
            keys::USER_ID,
            keys::ENDUSER_ID,
            keys::AI_TELEMETRY_USER_ID, // Vercel AI SDK
            keys::MLFLOW_TRACE_USER,    // MLflow
        ],
    )
    .or_else(|| {
        metadata
            .as_ref()?
            .get("user_id")?
            .as_str()
            .map(String::from)
    });

    // HTTP
    span.http_method = get_first(attrs, &[keys::HTTP_METHOD, keys::HTTP_REQUEST_METHOD]);
    span.http_url = get_first(attrs, &[keys::HTTP_URL, keys::URL_FULL]);
    span.http_status_code = get_first(
        attrs,
        &[keys::HTTP_STATUS_CODE, keys::HTTP_RESPONSE_STATUS_CODE],
    )
    .and_then(|value| value.parse().ok());

    // Database
    span.db_system = attrs.get(keys::DB_SYSTEM).cloned();
    span.db_name = attrs.get(keys::DB_NAME).cloned();
    span.db_operation = attrs.get(keys::DB_OPERATION).cloned();
    span.db_statement = attrs.get(keys::DB_STATEMENT).cloned();

    // Storage
    span.storage_system = attrs.get(keys::CLOUD_PROVIDER).cloned();
    span.storage_bucket = get_first(attrs, &[keys::AWS_S3_BUCKET, keys::GCP_GCS_BUCKET]);
    span.storage_object = get_first(attrs, &[keys::AWS_S3_KEY, keys::GCP_GCS_OBJECT]);

    // Messaging
    span.messaging_system = attrs.get(keys::MESSAGING_SYSTEM).cloned();
    span.messaging_destination = get_first(
        attrs,
        &[
            keys::MESSAGING_DESTINATION,
            keys::MESSAGING_DESTINATION_NAME,
        ],
    );

    // Tags (merge and dedupe from multiple sources)
    span.tags = merge_tags(attrs, &[keys::TAGS, keys::LANGSMITH_TAGS, keys::TAG_TAGS]);
}
