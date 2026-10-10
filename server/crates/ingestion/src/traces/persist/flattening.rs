use super::*;

/// Flatten raw messages, tool definitions, tool names, and enrichments into DB-ready format.
///
/// Iterates request in same order as normalize_batch to match spans with OTLP data.
/// Builds raw span JSON directly from request (no lookup needed).
///
/// When `files_enabled`, extracts base64 files from the tool definitions and the metadata **in-memory before
/// serialization**, which avoids a serialize-deserialize-re-serialize round trip.
///
/// # Panics
/// Debug assertion fails if span counts don't match (indicates pipeline bug).
#[allow(clippy::too_many_arguments)]
pub(super) fn flatten(
    request: &ExportTraceServiceRequest,
    span_data: Vec<SpanData>,
    messages: Vec<Vec<RawMessage>>,
    tool_definitions: Vec<Vec<RawToolDefinition>>,
    tool_names: Vec<Vec<RawToolNames>>,
    enrichments: Vec<SpanEnrichment>,
    files_enabled: bool,
    file_cache: Option<&FileExtractionCache>,
) -> (Vec<NormalizedSpan>, Vec<PendingFileWrite>) {
    let span_count = span_data.len();
    let mut result = Vec::with_capacity(span_count);
    let mut pending_files: Vec<PendingFileWrite> = Vec::new();
    let mut iter = span_data
        .into_iter()
        .zip(messages)
        .zip(tool_definitions)
        .zip(tool_names)
        .zip(enrichments);

    let extract_fn = |json: &mut JsonValue| match file_cache {
        Some(c) => extract_and_replace_files_cached(json, c),
        None => extract_and_replace_files(json),
    };

    // Iterate request in same order as normalize_batch
    for resource_spans in &request.resource_spans {
        for scope_spans in &resource_spans.scope_spans {
            for otlp_span in &scope_spans.spans {
                if let Some(((((mut span, msgs), tools), tnames), enrichment)) = iter.next() {
                    let content_digest =
                        span_content_digest(resource_spans, scope_spans, otlp_span);
                    let messages_str =
                        Some(serde_json::to_string(&msgs).expect("JsonValue is always valid JSON"));

                    let mut tool_definitions_json = flatten_tool_definitions(&tools);
                    let tool_names_json = flatten_tool_names(&tnames);
                    let tool_names_str = Some(
                        serde_json::to_string(&tool_names_json)
                            .expect("JsonValue is always valid JSON"),
                    );

                    // Extract files from JSON values in-memory BEFORE serialization.
                    // This avoids the costly serialize→deserialize→re-serialize round-trip.
                    //
                    // The span's own OTLP JSON is not among them: it is no longer stored, and the media it
                    // carries is cut out of the raw record (a longer reach than this extraction's, down to a
                    // 256-character run), owned by the same traces and kept by the same survivor
                    // reconciliation. `raw_views` renders the JSON, and these references, when a reader asks.
                    if files_enabled {
                        let project_id = span
                            .project_id
                            .as_deref()
                            .unwrap_or(DEFAULT_PROJECT_ID)
                            .to_string();
                        let trace_id = &span.trace_id;

                        // tool_definitions
                        pending_files.extend(to_pending_files(
                            extract_fn(&mut tool_definitions_json).files,
                            &project_id,
                            trace_id,
                        ));

                        // metadata (owned, mutate in place before serialization)
                        pending_files.extend(to_pending_files(
                            extract_fn(&mut span.metadata).files,
                            &project_id,
                            trace_id,
                        ));
                    }

                    // Serialize to strings ONCE (after file extraction)
                    let tool_definitions_str = Some(
                        serde_json::to_string(&tool_definitions_json)
                            .expect("JsonValue is always valid JSON"),
                    );
                    result.push(to_normalized_span(
                        span,
                        &enrichment,
                        messages_str,
                        tool_definitions_str,
                        tool_names_str,
                        content_digest,
                        otlp_span,
                    ));
                }
            }
        }
    }

    debug_assert_eq!(
        result.len(),
        span_count,
        "Span count mismatch: expected {}, got {}",
        span_count,
        result.len()
    );

    (result, pending_files)
}

/// Flatten tool definitions: extract content from each RawToolDefinition and merge arrays.
/// Input: `[{source: {...}, content: [def1, def2]}, {source: {...}, content: [def3]}]`
/// Output: `[def1, def2, def3]`
fn flatten_tool_definitions(tools: &[RawToolDefinition]) -> JsonValue {
    let mut result: Vec<JsonValue> = Vec::new();
    for tool in tools {
        if let Some(arr) = tool.content.as_array() {
            result.extend(arr.iter().cloned());
        } else {
            result.push(tool.content.clone());
        }
    }
    JsonValue::Array(result)
}

/// Flatten tool names: extract content from each RawToolNames and merge into flat string list.
/// Input: `[{source: {...}, content: ["tool1", "tool2"]}, {source: {...}, content: ["tool3"]}]`
/// Output: `["tool1", "tool2", "tool3"]`
fn flatten_tool_names(tnames: &[RawToolNames]) -> JsonValue {
    let mut result: Vec<JsonValue> = Vec::new();
    for tname in tnames {
        if let Some(arr) = tname.content.as_array() {
            result.extend(arr.iter().cloned());
        } else {
            result.push(tname.content.clone());
        }
    }
    JsonValue::Array(result)
}

/// Convert SpanData to NormalizedSpan and apply enrichment.
fn to_normalized_span(
    span: SpanData,
    enrichment: &SpanEnrichment,
    messages: Option<String>,
    tool_definitions: Option<String>,
    tool_names: Option<String>,
    content_digest: String,
    // The OTLP span this row was extracted from, for its event and link counts only: the events and links
    // themselves are rendered from the raw record.
    otlp: &Span,
) -> NormalizedSpan {
    let mut normalized = NormalizedSpan {
        // Identity
        //
        // Defaulted here rather than left as it arrived, so the stored value is the one every other
        // path uses. `NULL` was a third state nothing agreed on: project-scoped reads (`WHERE
        // project_id = ?`) could not see such a row, `delete_project_data` could not delete it, and the
        // write fence checked it as `default` - so a span with no project id was stored where nothing
        // could read it and nothing could remove it.
        project_id: Some(
            span.project_id
                .unwrap_or_else(|| DEFAULT_PROJECT_ID.to_string()),
        ),
        trace_id: span.trace_id,
        span_id: span.span_id,
        content_digest,
        parent_span_id: span.parent_span_id,
        trace_state: span.trace_state,

        // Session and user
        session_id: span.session_id,
        user_id: span.user_id,

        // Naming and classification
        span_name: span.span_name,
        span_kind: span.span_kind,
        span_category: span.span_category,
        observation_type: span.observation_type,
        framework: span.framework,
        request_thread: span.request_thread,
        request_frame: span.request_frame,
        span_marks: span.span_marks,
        scope_name: span.scope_name.clone(),
        scope_version: span.scope_version.clone(),
        status_code: span.status_code,
        status_message: span.status_message,
        exception_type: span.exception_type,
        exception_message: span.exception_message,
        exception_stacktrace: span.exception_stacktrace,

        // Time
        timestamp_start: span.timestamp_start,
        timestamp_end: span.timestamp_end,
        duration_ms: span.duration_ms,

        // Environment
        environment: span.environment,

        // GenAI core fields
        gen_ai_system: span.gen_ai_system,
        gen_ai_operation_name: span.gen_ai_operation_name,
        gen_ai_request_model: span.gen_ai_request_model,
        gen_ai_response_model: span.gen_ai_response_model,
        gen_ai_response_id: span.gen_ai_response_id,

        // GenAI request parameters
        gen_ai_temperature: span.gen_ai_temperature,
        gen_ai_top_p: span.gen_ai_top_p,
        gen_ai_top_k: span.gen_ai_top_k,
        gen_ai_max_tokens: span.gen_ai_max_tokens,
        gen_ai_frequency_penalty: span.gen_ai_frequency_penalty,
        gen_ai_presence_penalty: span.gen_ai_presence_penalty,
        gen_ai_stop_sequences: span.gen_ai_stop_sequences,

        // GenAI response
        gen_ai_finish_reasons: span.gen_ai_finish_reasons,

        // GenAI agent fields
        gen_ai_agent_id: span.gen_ai_agent_id,
        gen_ai_agent_name: span.gen_ai_agent_name,

        // GenAI tool fields
        gen_ai_tool_name: span.gen_ai_tool_name,
        gen_ai_tool_call_id: span.gen_ai_tool_call_id,

        // GenAI performance metrics
        gen_ai_server_ttft_ms: span.gen_ai_server_ttft_ms,
        gen_ai_server_request_duration_ms: span.gen_ai_server_request_duration_ms,

        // Token usage
        gen_ai_usage_input_tokens: span.gen_ai_usage_input_tokens,
        gen_ai_usage_output_tokens: span.gen_ai_usage_output_tokens,
        // The extractor's total, unless pricing resolved a provider and so knows which convention applies -
        // see `enrich::corrected_total_tokens`. Without this the charge and the total could describe
        // different calls whenever `gen_ai.system` was absent or spelled in a way the mapper did not know.
        gen_ai_usage_total_tokens: enrichment
            .total_tokens
            .unwrap_or(span.gen_ai_usage_total_tokens),
        gen_ai_usage_cache_read_tokens: span.gen_ai_usage_cache_read_tokens,
        gen_ai_usage_cache_write_tokens: span.gen_ai_usage_cache_write_tokens,
        gen_ai_usage_reasoning_tokens: span.gen_ai_usage_reasoning_tokens,
        gen_ai_usage_details: json_to_pre_serialized(&span.gen_ai_usage_details),

        // Enrichment data (costs)
        gen_ai_cost_input: enrichment.input_cost,
        gen_ai_cost_output: enrichment.output_cost,
        gen_ai_cost_cache_read: enrichment.cache_read_cost,
        gen_ai_cost_cache_write: enrichment.cache_write_cost,
        gen_ai_cost_reasoning: enrichment.reasoning_cost,
        gen_ai_cost_total: enrichment.total_cost,

        // Enrichment data (previews)
        input_preview: enrichment.input_preview.clone(),
        output_preview: enrichment.output_preview.clone(),

        // External services
        http_method: span.http_method,
        http_url: span.http_url,
        http_status_code: span.http_status_code,

        db_system: span.db_system,
        db_name: span.db_name,
        db_operation: span.db_operation,
        db_statement: span.db_statement,

        storage_system: span.storage_system,
        storage_bucket: span.storage_bucket,
        storage_object: span.storage_object,

        messaging_system: span.messaging_system,
        messaging_destination: span.messaging_destination,

        // Tags and metadata
        tags: span.tags,
        metadata: json_to_pre_serialized(&span.metadata),

        // Raw messages (converted to SideML on query)
        messages,

        // Raw tool definitions (separate from conversation messages)
        tool_definitions,

        // Raw tool names (list of tool names, separate from full definitions)
        tool_names,

        // Ingestion time (populated by DB default, not set during span creation)
        ingested_at: None,
        hold_until: None,
        logical_bytes: 0,
        search: Default::default(),
        // Stamped by the pipeline once the request's raw record exists.
        raw_id: None,
        event_count: u32::try_from(otlp.events.len()).unwrap_or(u32::MAX),
        link_count: u32::try_from(otlp.links.len()).unwrap_or(u32::MAX),
        // Set by the batch that ingests the span; a span outside a batch is its own export.
        batch_slot: 0,
    };
    normalized.logical_bytes = crate::accounting::span_logical_bytes(&mut normalized);
    normalized
}

/// Digest the producer-owned resource, scope and span exactly as OTLP represents them.
///
/// The transport injects `sideseat.project_id` before extraction. It is removed from the cloned
/// resource so a byte-identical retry has the same digest and a system-managed tenant field can never
/// masquerade as producer content. Receipt time, legal hold and accounting fields do not exist in these
/// protobuf messages and therefore cannot enter the digest.
pub(crate) fn span_content_digest(
    resource_spans: &ResourceSpans,
    scope_spans: &ScopeSpans,
    span: &Span,
) -> String {
    let mut resource = resource_spans.resource.clone();
    if let Some(resource) = &mut resource {
        resource
            .attributes
            .retain(|attribute| attribute.key != crate::otlp::PROJECT_ID_ATTR);
    }

    let mut hasher = blake3::Hasher::new();
    for bytes in [
        resource
            .as_ref()
            .map(Message::encode_to_vec)
            .unwrap_or_default(),
        resource_spans.schema_url.as_bytes().to_vec(),
        scope_spans
            .scope
            .as_ref()
            .map(Message::encode_to_vec)
            .unwrap_or_default(),
        scope_spans.schema_url.as_bytes().to_vec(),
        span.encode_to_vec(),
    ] {
        hasher.update(&(bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    hasher.finalize().to_hex().to_string()
}

// ============================================================================
// RAW SPAN JSON BUILDING
// ============================================================================

/// Build a raw JSON representation of an OTLP span for archival.
/// Uses ordered map for better readability (identity -> timing -> content -> metadata).
pub(in crate::traces) fn build_raw_span_json(
    span: &Span,
    resource_attrs: &HashMap<String, String>,
) -> JsonValue {
    let mut map = serde_json::Map::new();

    // Identity fields first
    map.insert("trace_id".into(), json!(hex::encode(&span.trace_id)));
    map.insert("span_id".into(), json!(hex::encode(&span.span_id)));
    map.insert(
        "parent_span_id".into(),
        if span.parent_span_id.is_empty() {
            JsonValue::Null
        } else {
            json!(hex::encode(&span.parent_span_id))
        },
    );
    map.insert("name".into(), json!(&span.name));
    map.insert("kind".into(), json!(span.kind));

    // Timing
    map.insert(
        "start_time_unix_nano".into(),
        json!(span.start_time_unix_nano),
    );
    map.insert("end_time_unix_nano".into(), json!(span.end_time_unix_nano));

    // Status
    map.insert(
        "status".into(),
        span.status
            .as_ref()
            .map(|s| {
                let mut status_map = serde_json::Map::new();
                status_map.insert("code".into(), json!(s.code));
                status_map.insert("message".into(), json!(&s.message));
                JsonValue::Object(status_map)
            })
            .unwrap_or(JsonValue::Null),
    );

    // Attributes
    map.insert("attributes".into(), build_attributes_json(&span.attributes));

    // Events (with ordered fields)
    let events: Vec<JsonValue> = span
        .events
        .iter()
        .map(|e| {
            let mut event_map = serde_json::Map::new();
            event_map.insert("name".into(), json!(&e.name));
            event_map.insert("timestamp".into(), json!(nanos_to_iso(e.time_unix_nano)));
            event_map.insert("attributes".into(), build_attributes_json(&e.attributes));
            event_map.insert(
                "dropped_attributes_count".into(),
                json!(e.dropped_attributes_count),
            );
            JsonValue::Object(event_map)
        })
        .collect();
    map.insert("events".into(), json!(events));

    // Links (with ordered fields)
    let links: Vec<JsonValue> = span
        .links
        .iter()
        .map(|l| {
            let mut link_map = serde_json::Map::new();
            link_map.insert("trace_id".into(), json!(hex::encode(&l.trace_id)));
            link_map.insert("span_id".into(), json!(hex::encode(&l.span_id)));
            link_map.insert("trace_state".into(), json!(&l.trace_state));
            link_map.insert("attributes".into(), build_attributes_json(&l.attributes));
            link_map.insert("flags".into(), json!(l.flags));
            link_map.insert(
                "dropped_attributes_count".into(),
                json!(l.dropped_attributes_count),
            );
            JsonValue::Object(link_map)
        })
        .collect();
    map.insert("links".into(), json!(links));

    // Resource
    let mut resource_map = serde_json::Map::new();
    resource_map.insert(
        "attributes".into(),
        build_resource_attributes(resource_attrs),
    );
    map.insert("resource".into(), JsonValue::Object(resource_map));

    // Metadata (less important, at the end)
    map.insert(
        "trace_state".into(),
        if span.trace_state.is_empty() {
            JsonValue::Null
        } else {
            json!(&span.trace_state)
        },
    );
    map.insert("flags".into(), json!(span.flags));
    map.insert(
        "dropped_attributes_count".into(),
        json!(span.dropped_attributes_count),
    );
    map.insert(
        "dropped_events_count".into(),
        json!(span.dropped_events_count),
    );
    map.insert(
        "dropped_links_count".into(),
        json!(span.dropped_links_count),
    );

    JsonValue::Object(map)
}

/// Build JSON from attributes HashMap (for resource attributes)
/// The resource's attributes, by key.
///
/// Sorted, because the source is a `HashMap` and `serde_json` preserves insertion order: an unsorted copy
/// serialized its keys in a different order on every call, so the same span rendered twice was two different
/// strings. A derived view has to be a function of the raw telemetry, or re-deriving it is not a no-op.
fn build_resource_attributes(attrs: &HashMap<String, String>) -> JsonValue {
    let mut keys: Vec<&String> = attrs.keys().collect();
    keys.sort_unstable();
    JsonValue::Object(
        keys.into_iter()
            .map(|key| (key.clone(), json!(attrs[key])))
            .collect(),
    )
}
