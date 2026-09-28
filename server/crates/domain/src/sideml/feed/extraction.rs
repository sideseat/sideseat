use super::*;

// ============================================================================
// INTERNAL: PARSING
// ============================================================================

/// Parse span rows into parsed messages.
pub(in crate::sideml::feed) fn parse_span_rows(rows: &[MessageSpanRow]) -> Vec<ParsedMessage> {
    let mut messages: Vec<ParsedMessage> = Vec::with_capacity(rows.len() * 4);
    let mut parsed_bodies: HashMap<&str, Option<Arc<Vec<RawMessage>>>> = HashMap::new();

    for row in rows {
        // Determine if this is a tool execution span
        let is_tool_span = row.observation_type.as_deref() == Some(obs_type::TOOL);

        // Parse each distinct body once. Replaying frameworks put the same growing conversation body on
        // many spans; content addressing makes equality explicit, and this per-reconstruction memo makes
        // that equality useful instead of paying the JSON parser once per reference.
        let raw_msgs = if let Some(cached) = parsed_bodies.get(row.messages_json.as_str()) {
            cached.clone()
        } else {
            let parsed = match serde_json::from_str::<Vec<RawMessage>>(&row.messages_json) {
                Ok(raw_msgs) => Some(Arc::new(raw_msgs)),
                Err(error) => {
                    tracing::debug!(
                        span_id = %row.span_id,
                        %error,
                        "Failed to parse messages JSON"
                    );
                    None
                }
            };
            parsed_bodies.insert(row.messages_json.as_str(), parsed.clone());
            parsed
        };

        if let Some(raw_msgs) = raw_msgs {
            // Producer-owned projection rules may identify stored bookkeeping that is not another
            // conversation. Extraction remains lossless: this only omits it from the SideML view.
            let successful = row.status_code.as_deref() != Some(status::ERROR)
                && row.exception_type.is_none()
                && row.exception_message.is_none()
                && row.exception_stacktrace.is_none();
            if crate::rules::ruleset()
                .message_projection
                .suppresses_messages(
                    &crate::rules::message_projection::MessageProjectionContext {
                        scope_name: row.scope_name.as_deref(),
                        scope_version: row.scope_version.as_deref(),
                        span_name: row.span_name.as_deref(),
                        successful,
                        messages: &raw_msgs,
                    },
                )
            {
                continue;
            }

            // Debug: Log raw message count
            tracing::trace!(
                span_id = %row.span_id,
                raw_msg_count = raw_msgs.len(),
                "parse_span_rows: raw messages parsed"
            );
            let sideml_msgs = to_sideml_with_context(&raw_msgs, is_tool_span);
            tracing::trace!(
                span_id = %row.span_id,
                sideml_msg_count = sideml_msgs.len(),
                "parse_span_rows: SideML conversion done"
            );
            for (index, msg) in sideml_msgs.into_iter().enumerate() {
                let timestamp = msg.timestamp;
                messages.push(ParsedMessage {
                    position: msg.position.clone(),
                    trace_id: row.trace_id.clone(),
                    span_id: row.span_id.clone(),
                    parent_span_id: row.parent_span_id.clone(),
                    session_id: row.session_id.clone(),
                    message_index: index as i32,
                    timestamp,
                    source: msg.source,
                    message: msg.sideml,
                    category: msg.category,
                    model: row.model.clone(),
                    provider: row.provider.clone(),
                    status_code: row.status_code.clone(),
                    total_tokens: row.total_tokens,
                    cost_total: row.cost_total,
                    observation_type: row.observation_type.clone(),
                    span_name: row.span_name.clone(),
                    scope_name: row.scope_name.clone(),
                    scope_version: row.scope_version.clone(),
                });
            }
        }
    }

    messages
}

/// Extract tool definitions and names from span rows.
///
/// Standalone function decoupled from message parsing so handlers can
/// scope tool extraction to specific rows (e.g., a single trace).
pub fn extract_tools_from_rows<'a>(
    rows: impl IntoIterator<Item = &'a MessageSpanRow>,
) -> ExtractedTools {
    let mut tool_defs: Vec<JsonValue> = Vec::new();
    let mut tool_names_raw: Vec<String> = Vec::new();

    for row in rows {
        match serde_json::from_str::<Vec<JsonValue>>(&row.tool_definitions_json) {
            Ok(defs) => tool_defs.extend(defs),
            Err(e) => {
                tracing::debug!(
                    span_id = %row.span_id,
                    error = %e,
                    "Failed to parse tool definitions JSON"
                );
            }
        }

        // **Item by item.** Read as `Vec<String>` this failed whole: one stored non-string - `["search", 7]`
        // from a producer that wrote a number - took the valid `"search"` with it, so a malformed item poisoned
        // its siblings at the last possible moment, after storage had already accepted it. The emission path
        // now keeps a non-string out, and this keeps the ones already stored from costing their neighbours.
        match serde_json::from_str::<Vec<JsonValue>>(&row.tool_names_json) {
            Ok(items) => {
                let stored = items.len();
                let usable: Vec<String> = items
                    .iter()
                    .filter_map(|item| item.as_str())
                    .filter(|name| !name.trim().is_empty())
                    .map(ToString::to_string)
                    .collect();
                if usable.len() < stored {
                    tracing::debug!(
                        span_id = %row.span_id,
                        stored,
                        kept = usable.len(),
                        "stored tool names include items that are not non-blank strings; the rest are kept"
                    );
                }
                tool_names_raw.extend(usable);
            }
            Err(e) => {
                tracing::debug!(
                    span_id = %row.span_id,
                    error = %e,
                    "Failed to parse tool names JSON"
                );
            }
        }
    }

    ExtractedTools {
        tool_definitions: deduplicate_tools(tool_defs),
        tool_names: deduplicate_names(tool_names_raw),
    }
}

/// Compose error display text from structured exception fields.
/// Presentation logic at query time — raw data preserved in DB columns.
pub(in crate::sideml::feed) fn compose_error_text(
    exception_type: Option<&str>,
    exception_message: Option<&str>,
    exception_stacktrace: Option<&str>,
) -> Option<String> {
    let header = match (exception_type, exception_message) {
        (Some(t), Some(m)) if !t.is_empty() && !m.is_empty() => Some(format!("{t}: {m}")),
        (_, Some(m)) if !m.is_empty() => Some(m.to_string()),
        (Some(t), _) if !t.is_empty() => Some(t.to_string()),
        _ => None,
    };

    let stacktrace = exception_stacktrace.filter(|s| !s.is_empty());

    match (header, stacktrace) {
        (Some(h), Some(st)) => Some(format!("{h}\n\n```\n{st}\n```")),
        (Some(h), None) => Some(h),
        (None, Some(st)) => Some(format!("```\n{st}\n```")),
        (None, None) => None,
    }
}

/// Append error messages from leaf error spans.
///
/// Creates ParsedMessage objects from exception fields of ERROR spans.
/// These flow through flatten_to_blocks -> classify -> dedup naturally.
/// Only leaf error spans get messages (deepest ERROR in hierarchy).
///
/// Leaf detection is scoped by trace_id to prevent cross-trace collisions
/// when process_feed groups multiple traces into one session.
pub(in crate::sideml::feed) fn append_error_messages(
    messages: &mut Vec<ParsedMessage>,
    rows: &[MessageSpanRow],
) {
    // A child suppresses its parent only when the child can actually *say* what went wrong.
    //
    // Testing `status == ERROR` alone deferred to a child that had nothing to render, and then rendered
    // nothing: in `strands/error` the innermost span is a failed `chat` carrying ERROR status and no
    // exception fields, so it silenced its parent and contributed no message, and the same applied one
    // level further up. The trace of a failed run showed `system, user` and **no error at all**, while
    // three separate span views each displayed the `ValidationException`. Deferring to a child is only
    // sound if the child will report.
    let spans_with_error_children: HashSet<(&str, &str)> = rows
        .iter()
        .filter(|r| {
            r.status_code.as_deref() == Some(status::ERROR)
                && r.parent_span_id.is_some()
                && compose_error_text(
                    r.exception_type.as_deref(),
                    r.exception_message.as_deref(),
                    r.exception_stacktrace.as_deref(),
                )
                .is_some()
        })
        .filter_map(|r| {
            r.parent_span_id
                .as_deref()
                .map(|p| (r.trace_id.as_str(), p))
        })
        .collect();

    for row in rows {
        if row.status_code.as_deref() != Some(status::ERROR) {
            continue;
        }
        let error_msg = match compose_error_text(
            row.exception_type.as_deref(),
            row.exception_message.as_deref(),
            row.exception_stacktrace.as_deref(),
        ) {
            Some(m) => m,
            None => continue,
        };
        // Skip non-leaf: this span has an ERROR child within the same trace
        if spans_with_error_children.contains(&(row.trace_id.as_str(), row.span_id.as_str())) {
            continue;
        }

        let timestamp = row.span_end_timestamp.unwrap_or(row.span_timestamp);
        let max_msg_idx = messages
            .iter()
            .filter(|m| m.span_id == row.span_id)
            .map(|m| m.message_index)
            .max()
            .unwrap_or(-1);

        messages.push(ParsedMessage {
            // Composed from the span's exception fields rather than read out of a payload, so there
            // is no position to record. `is_empty()` is how a consumer tells the two apart.
            position: PositionPath::default(),
            trace_id: row.trace_id.clone(),
            span_id: row.span_id.clone(),
            parent_span_id: row.parent_span_id.clone(),
            session_id: row.session_id.clone(),
            message_index: max_msg_idx + 1,
            timestamp,
            source: MessageSource::Attribute {
                key: "exception".to_string(),
                time: timestamp,
            },
            message: crate::sideml::types::ChatMessage {
                role: crate::sideml::types::ChatRole::Assistant,
                content: vec![ContentBlock::Text { text: error_msg }],
                finish_reason: Some(crate::sideml::types::FinishReason::Error),
                ..Default::default()
            },
            category: MessageCategory::Exception,
            model: row.model.clone(),
            provider: row.provider.clone(),
            status_code: row.status_code.clone(),
            total_tokens: 0,
            cost_total: 0.0,
            observation_type: row.observation_type.clone(),
            span_name: row.span_name.clone(),
            scope_name: row.scope_name.clone(),
            scope_version: row.scope_version.clone(),
        });
    }
}

// ============================================================================
// INTERNAL: FLATTENING
// ============================================================================

/// Build span hierarchy map for span_path computation.
///
/// Includes cycle detection to prevent infinite loops from malformed data.
pub(in crate::sideml::feed) fn build_span_hierarchy(
    span_rows: &[MessageSpanRow],
) -> HashMap<String, Vec<String>> {
    let parent_map: HashMap<_, _> = span_rows
        .iter()
        .filter_map(|s| {
            s.parent_span_id
                .as_ref()
                .map(|p| (s.span_id.clone(), p.clone()))
        })
        .collect();

    let mut paths = HashMap::new();
    let max_depth = span_rows.len().max(256); // Floor for partial views (single-span queries)

    for span in span_rows {
        let mut path = vec![span.span_id.clone()];
        let mut current = span.span_id.clone();
        let mut visited = HashSet::with_capacity(max_depth.min(32));
        visited.insert(current.clone());

        while let Some(parent) = parent_map.get(&current) {
            // Cycle detection: stop if we've seen this parent before
            if !visited.insert(parent.clone()) {
                tracing::warn!(
                    span_id = %span.span_id,
                    cycle_at = %parent,
                    "Cycle detected in span hierarchy, truncating path"
                );
                break;
            }

            // Depth limit: prevent runaway in malformed data
            if path.len() >= max_depth {
                tracing::warn!(
                    span_id = %span.span_id,
                    depth = path.len(),
                    "Span hierarchy depth exceeded limit, truncating path"
                );
                break;
            }

            path.push(parent.clone());
            current = parent.clone();
        }

        path.reverse(); // [root, ..., current]
        paths.insert(span.span_id.clone(), path);
    }

    paths
}

/// Build span timestamps map for birth time computation.
pub(in crate::sideml::feed) fn build_span_timestamps(
    span_rows: &[MessageSpanRow],
) -> HashMap<String, SpanTimestamps> {
    span_rows
        .iter()
        .map(|row| {
            (
                row.span_id.clone(),
                SpanTimestamps {
                    span_start: row.span_timestamp,
                    span_end: row.span_end_timestamp,
                },
            )
        })
        .collect()
}

/// Derive role from content block type, overriding raw message role when needed.
///
/// This handles provider-specific message formats where tool-related content
/// may come with unexpected roles:
/// - ADK/Gemini: ToolResult in "user" role messages (Gemini protocol)
/// - All: ToolUse should always be "assistant" (LLM decided to call)
///
/// For regular content types (text, image, etc.), the original role is preserved.
fn derive_role_from_content(
    block: &ContentBlock,
    original_role: crate::sideml::types::ChatRole,
) -> crate::sideml::types::ChatRole {
    match block {
        // Tool results MUST be "tool" role, regardless of raw message
        // Gemini stores these in user messages, but semantically they're tool outputs
        ContentBlock::ToolResult { .. } => crate::sideml::types::ChatRole::Tool,
        // Tool calls MUST be "assistant" role (LLM decided to call a tool)
        ContentBlock::ToolUse { .. } => crate::sideml::types::ChatRole::Assistant,
        // All other content types preserve original role
        _ => original_role,
    }
}

/// Flatten parsed messages into individual content blocks.
///
/// All blocks start with `is_history = false`. History detection is done
/// separately by `mark_history()` based on actual
/// content duplication across spans.
///
/// Deliberately unfiltered: every block the spans contain reaches the later stages, because
/// correlation, history detection and dedup all read blocks they do not return. See
/// [`apply_role_filter`].
pub(in crate::sideml::feed) fn flatten_to_blocks(
    messages: Vec<ParsedMessage>,
    span_hierarchy: &HashMap<String, Vec<String>>,
) -> Vec<BlockEntry> {
    let mut blocks = Vec::new();

    for msg in messages {
        // Skip empty messages
        if msg.message.content.is_empty() {
            tracing::trace!(
                span_id = %msg.span_id,
                role = ?msg.message.role,
                "flatten_to_blocks: skipping empty message"
            );
            continue;
        }

        // Skip spurious tool input JSON blocks from tool spans
        // These are tool invocation parameters that shouldn't appear as messages.
        // Exception: output.value attributes may contain legitimate structured output.
        let is_tool_span = msg.observation_type.as_deref() == Some(obs_type::TOOL);
        let is_output_attr = matches!(
            &msg.source,
            MessageSource::Attribute { key, .. } if key == "output.value" || key.starts_with("output.")
        );
        if is_tool_span
            && !is_output_attr
            && msg.message.content.len() == 1
            && matches!(msg.message.content.first(), Some(ContentBlock::Json { .. }))
        {
            continue;
        }

        let span_path = span_hierarchy
            .get(&msg.span_id)
            .cloned()
            .unwrap_or_default();

        // Source type, event name, and attribute key
        let (src_type, event_name, source_attribute) = match &msg.source {
            MessageSource::Event { name, .. } => (source_type::EVENT, Some(name.clone()), None),
            MessageSource::Attribute { key, .. } => {
                (source_type::ATTRIBUTE, None, Some(key.clone()))
            }
        };

        // Flatten each content block into its own BlockEntry
        // is_history starts as false; will be set by mark_history()
        for (entry_index, block) in msg.message.content.iter().enumerate() {
            let entry_type = block.block_type().to_string();
            let tool_use_id =
                extract_tool_use_id_from_block(block).or_else(|| msg.message.tool_use_id.clone());
            let tool_name = extract_tool_name_from_block(block);
            let content_hash = compute_block_hash(block);
            let is_semantic = block.is_semantic();

            // Derive role from content type, not raw message role.
            // This is critical for frameworks like ADK/Gemini where:
            // - ToolResult comes in "user" role messages (Gemini protocol)
            // - ToolUse should always be "assistant" (LLM decided to call tool)
            let role = derive_role_from_content(block, msg.message.role);

            blocks.push(BlockEntry {
                // The block's own position: the message's path plus which content block this is. Two
                // blocks of one message therefore differ, and so do two identical calls a model made
                // in one response - the thing content alone cannot tell apart.
                position: msg.position.child_index(entry_index),
                entry_type,
                content: block.clone(),
                role,

                trace_id: msg.trace_id.clone(),
                span_id: msg.span_id.clone(),
                session_id: msg.session_id.clone(),
                message_index: msg.message_index,
                entry_index: entry_index as i32,

                parent_span_id: msg.parent_span_id.clone(),
                span_path: span_path.clone(),

                timestamp: msg.timestamp,
                order_time: msg.timestamp,

                observation_type: msg.observation_type.clone(),
                span_name: msg.span_name.clone(),
                scope_name: msg.scope_name.clone(),
                scope_version: msg.scope_version.clone(),

                model: msg.model.clone(),
                provider: msg.provider.clone(),

                name: msg.message.name.clone(),
                finish_reason: msg.message.finish_reason,

                tool_use_id,
                tool_name,

                tokens: Some(msg.total_tokens),
                cost: Some(msg.cost_total),

                status_code: msg.status_code.clone(),
                is_error: msg.status_code.as_deref() == Some(status::ERROR),

                source_type: src_type.to_string(),
                event_name: event_name.clone(),
                source_attribute: source_attribute.clone(),
                category: msg.category,

                content_hash: format!("{:016x}", content_hash),
                is_semantic,
                uses_span_end: false, // Will be set by classify_blocks()
                is_history: false,    // Will be set by classify_blocks()
                tool_use_id_correlated: false, // Will be set by correlate_tool_results()
                promoted_to_span_output: false, // Will be set by classify_blocks()
            });
        }
    }

    blocks
}

// ============================================================================
// BLOCK CLASSIFICATION
// ============================================================================

/// Classify blocks and detect history.
///
/// This function performs two key operations:
///
/// 1. **Timestamp classification** (`uses_span_end`): Determines whether each block
///    uses span_end or event_time for ordering. See `classify` module.
///
/// 2. **History detection** (`is_history`): Marks blocks that should be filtered
///    (context copies, intermediate output, duplicates). See `history` module.
///
/// # Pipeline Position
///
/// This runs after flattening and before dedup/sort:
/// ```text
/// Parse → Flatten → [CLASSIFY] → Dedup → Sort
/// ```
pub(in crate::sideml::feed) fn classify_blocks(
    blocks: &mut [BlockEntry],
    span_timestamps: &HashMap<String, SpanTimestamps>,
) {
    // Step 1: Classify timestamp strategy for each block
    let mut output_count = 0;
    for block in blocks.iter_mut() {
        block.uses_span_end = uses_span_end(block);
        if block.uses_span_end {
            output_count += 1;
        }
    }

    // Step 1b: Promote assistant messages in choiceless generation spans.
    // Logfire/OpenAI Agents store LLM output as gen_ai.assistant.message (not gen_ai.choice).
    // Without promotion, these sort by array index alongside input events → wrong order.
    // Promoting to uses_span_end + GenAIChoice category fixes ordering and history protection.
    //
    // Check at TRACE level: if any span in the trace has gen_ai.choice, skip promotion
    // for the entire trace. This prevents promoting intermediate assistant text in
    // frameworks like Strands where gen_ai.choice lives in a parent/sibling span.
    let traces_with_choice: HashSet<String> = blocks
        .iter()
        .filter(|b| b.is_output_event())
        .map(|b| b.trace_id.clone())
        .collect();

    let mut promoted = 0;
    for block in blocks.iter_mut() {
        if block.is_generation_span()
            && !block.is_tool_use()
            && !traces_with_choice.contains(&block.trace_id)
            && block.event_name.as_deref() == Some("gen_ai.assistant.message")
        {
            block.uses_span_end = true;
            block.category = MessageCategory::GenAIChoice;
            // Effective direction, in one place: the order resolver reads this to know the span
            // produced the block, which its carrier does not say.
            block.promoted_to_span_output = true;
            // Update timestamp to span_end so the block exits the same-batch group
            // (Logfire emits all events at span_start, so without this the sort
            // would preserve array index order instead of using birth_time).
            if let Some(ts) = span_timestamps.get(&block.span_id)
                && let Some(end) = ts.span_end
            {
                block.timestamp = end;
            }
            output_count += 1;
            promoted += 1;
        }
    }

    tracing::trace!(
        total = blocks.len(),
        output_count,
        promoted,
        "timestamp classification complete"
    );

    // Step 2: Detect and mark history blocks
    let stats = mark_history(blocks, span_timestamps);

    tracing::trace!(
        total_history = stats.total_history(),
        "history detection complete"
    );
}

/// Extract tool_use_id from a content block if applicable.
fn extract_tool_use_id_from_block(block: &ContentBlock) -> Option<String> {
    match block {
        ContentBlock::ToolUse { id, .. } => id.clone(),
        ContentBlock::ToolResult { tool_use_id, .. } => tool_use_id.clone(),
        _ => None,
    }
}

/// Extract tool name from a content block if applicable.
fn extract_tool_name_from_block(block: &ContentBlock) -> Option<String> {
    match block {
        ContentBlock::ToolUse { name, .. } => Some(name.clone()),
        _ => None,
    }
}

/// Hash binary content for deduplication - all of it.
///
/// This used to hash the length plus the first and last 128 bytes, which is not a digest but a
/// sample: two assets of the same size whose first and last 128 bytes agree are *deterministically*
/// identical to dedup, so one of them silently disappears from the feed. Three images generated from
/// one prompt in one turn are exactly that shape - same encoder, same dimensions, same header and
/// trailer - and every invariant in the suite still passes, because one image vanishing is
/// indistinguishable from a framework that only reported two.
///
/// Hashing the whole payload is what makes identity mean identity. It is affordable because nothing
/// here allocates: the bytes go straight into the hasher, at gigabytes per second, and a content hash
/// is computed once per block rather than once per comparison.
#[inline]
fn hash_binary_content<H: std::hash::Hasher>(data: &[u8], hasher: &mut H) {
    use std::hash::Hash;

    // Length first, so that appending to a payload cannot collide with the payload itself.
    data.len().hash(hasher);
    hasher.write(data);
}

/// Compute a hash for a content block.
pub(in crate::sideml::feed) fn compute_block_hash(block: &ContentBlock) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();

    // Hash based on block type and key content
    match block {
        ContentBlock::Text { text } => {
            "text".hash(&mut hasher);
            text.trim().hash(&mut hasher); // Normalize whitespace
        }
        ContentBlock::ToolUse { name, input, .. } => {
            // Hash by name + normalized input only (not id)
            "tool_use".hash(&mut hasher);
            name.hash(&mut hasher);
            hash_json_into(input, &mut hasher);
        }
        ContentBlock::ToolResult {
            name,
            content,
            is_error,
            ..
        } => {
            // Hash by tool name, error flag and normalized content - not tool_use_id, which a
            // history re-send regenerates.
            //
            // Content alone made every "ok" the same message: two tools both reporting success,
            // or a success and a failure whose text happens to match, collapsed into one wherever
            // this hash is the identity - which is the case for a result with no id.
            "tool_result".hash(&mut hasher);
            name.hash(&mut hasher);
            is_error.hash(&mut hasher);
            hash_tool_result_content_into(content, &mut hasher);
        }
        ContentBlock::Thinking { text, .. } => {
            "thinking".hash(&mut hasher);
            text.trim().hash(&mut hasher); // Normalize whitespace
        }
        ContentBlock::RedactedThinking { data } => {
            "redacted_thinking".hash(&mut hasher);
            data.hash(&mut hasher);
        }
        ContentBlock::Image { source, data, .. } => {
            "image".hash(&mut hasher);
            source.hash(&mut hasher);
            hash_binary_content(data.as_bytes(), &mut hasher);
        }
        ContentBlock::Audio { source, data, .. } => {
            "audio".hash(&mut hasher);
            source.hash(&mut hasher);
            hash_binary_content(data.as_bytes(), &mut hasher);
        }
        ContentBlock::Video { source, data, .. } => {
            "video".hash(&mut hasher);
            source.hash(&mut hasher);
            hash_binary_content(data.as_bytes(), &mut hasher);
        }
        ContentBlock::Document {
            source, data, name, ..
        } => {
            "document".hash(&mut hasher);
            source.hash(&mut hasher);
            name.hash(&mut hasher);
            hash_binary_content(data.as_bytes(), &mut hasher);
        }
        ContentBlock::File {
            source, data, name, ..
        } => {
            "file".hash(&mut hasher);
            source.hash(&mut hasher);
            name.hash(&mut hasher);
            hash_binary_content(data.as_bytes(), &mut hasher);
        }
        ContentBlock::ToolDefinitions { tools, .. } => {
            "tool_definitions".hash(&mut hasher);
            tools.len().hash(&mut hasher);
        }
        ContentBlock::Context { data, context_type } => {
            "context".hash(&mut hasher);
            context_type.hash(&mut hasher);
            hash_json_into(data, &mut hasher); // canonical key order
        }
        ContentBlock::Refusal { message } => {
            "refusal".hash(&mut hasher);
            message.hash(&mut hasher);
        }
        ContentBlock::Json { data } => {
            "json".hash(&mut hasher);
            // Structured normalization: a schema-filled answer and the model's raw one are the
            // same answer. See normalize_structured_json_for_hash.
            hash_structured_json_into(data, &mut hasher);
        }
        ContentBlock::Unknown { raw } => {
            "unknown".hash(&mut hasher);
            hash_json_into(raw, &mut hasher); // canonical key order
        }
    }

    hasher.finish()
}
