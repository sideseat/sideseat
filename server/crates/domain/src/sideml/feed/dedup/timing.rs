use super::*;

// ============================================================================
// BIRTH TIME MAP
// ============================================================================

/// Combine trace, occurrence rank, and content hash into one lookup key.
/// Avoids String allocation on HashMap lookups.
#[inline]
fn make_key(trace_id: &str, ordinal: u32, hash: u64) -> u128 {
    let mut hasher = DefaultHasher::new();
    trace_id.hash(&mut hasher);
    ordinal.hash(&mut hasher);
    let trace_hash = hasher.finish();
    ((trace_hash as u128) << 64) | (hash as u128)
}

/// Combine trace, role, occurrence rank, and semantic hash into one lookup key.
#[inline]
fn make_regular_key(trace_id: &str, role: ChatRole, ordinal: u32, semantic_hash: u64) -> u128 {
    let mut hasher = DefaultHasher::new();
    trace_id.hash(&mut hasher);
    role.hash(&mut hasher);
    ordinal.hash(&mut hasher);
    let combined = hasher.finish();
    ((combined as u128) << 64) | (semantic_hash as u128)
}

/// Maps message identities to their "birth time" (earliest occurrence).
///
/// This is the key data structure for deduplication:
/// - INPUT messages: birth_time = min(all occurrences)
/// - OUTPUT messages: birth_time = their own effective timestamp
///
/// Uses u128 combined hash keys to avoid String allocation on lookups.
#[derive(Debug, Default)]
pub(super) struct BirthTimeMap {
    /// Regular message identity → earliest effective timestamp
    regular_times: HashMap<u128, DateTime<Utc>>,
    /// Tool call content hash → earliest timestamp
    tool_call_times: HashMap<u128, DateTime<Utc>>,
    /// Tool result identity hash → earliest timestamp
    tool_result_times: HashMap<u128, DateTime<Utc>>,
}

impl BirthTimeMap {
    /// Record a timestamp for a regular message identity (keeps minimum).
    fn record_regular(
        &mut self,
        trace_id: &str,
        role: ChatRole,
        ordinal: u32,
        semantic_hash: u64,
        timestamp: DateTime<Utc>,
    ) {
        let key = make_regular_key(trace_id, role, ordinal, semantic_hash);
        self.regular_times
            .entry(key)
            .and_modify(|t| {
                if timestamp < *t {
                    *t = timestamp;
                }
            })
            .or_insert(timestamp);
    }

    /// Record a timestamp for a tool call (keeps minimum for dedup).
    fn record_tool_call(
        &mut self,
        trace_id: &str,
        ordinal: u32,
        content_hash: u64,
        timestamp: DateTime<Utc>,
    ) {
        let key = make_key(trace_id, ordinal, content_hash);
        self.tool_call_times
            .entry(key)
            .and_modify(|t| {
                if timestamp < *t {
                    *t = timestamp;
                }
            })
            .or_insert(timestamp);
    }

    /// Record a timestamp for a tool result (keeps minimum for dedup).
    fn record_tool_result(
        &mut self,
        trace_id: &str,
        ordinal: u32,
        identity_hash: u64,
        timestamp: DateTime<Utc>,
    ) {
        let key = make_key(trace_id, ordinal, identity_hash);
        self.tool_result_times
            .entry(key)
            .and_modify(|t| {
                if timestamp < *t {
                    *t = timestamp;
                }
            })
            .or_insert(timestamp);
    }

    /// Get birth time for a regular message.
    #[inline]
    fn get_regular(
        &self,
        trace_id: &str,
        role: ChatRole,
        ordinal: u32,
        semantic_hash: u64,
    ) -> Option<DateTime<Utc>> {
        let key = make_regular_key(trace_id, role, ordinal, semantic_hash);
        self.regular_times.get(&key).copied()
    }

    /// Get birth time for a tool call.
    #[inline]
    fn get_tool_call(
        &self,
        trace_id: &str,
        ordinal: u32,
        content_hash: u64,
    ) -> Option<DateTime<Utc>> {
        let key = make_key(trace_id, ordinal, content_hash);
        self.tool_call_times.get(&key).copied()
    }

    /// Get birth time for a tool result.
    #[inline]
    fn get_tool_result(
        &self,
        trace_id: &str,
        ordinal: u32,
        identity_hash: u64,
    ) -> Option<DateTime<Utc>> {
        let key = make_key(trace_id, ordinal, identity_hash);
        self.tool_result_times.get(&key).copied()
    }
}

// ============================================================================
// EFFECTIVE TIMESTAMP
// ============================================================================

/// Context needed for timestamp computation.
#[derive(Debug, Clone)]
pub struct SpanTimestamps {
    pub span_start: DateTime<Utc>,
    pub span_end: Option<DateTime<Utc>>,
}

/// Compute the effective timestamp for ordering.
///
/// The `uses_span_end` field determines timestamp strategy:
/// - `uses_span_end=true`: Use span_end (block represents COMPLETION of an operation)
/// - `uses_span_end=false`: Use event_time (block is intermediate or input)
///
/// # What uses_span_end Really Means
///
/// `uses_span_end=true` means "this block represents a COMPLETION event":
/// - `gen_ai.choice` events (generation completed)
/// - `gen_ai.content.completion` events
/// - Blocks with `finish_reason` (explicit completion marker)
/// - ToolResult from tool spans (tool execution completed)
///
/// `uses_span_end=false` means "this block is intermediate or input":
/// - ToolUse (decision made DURING generation, not at completion)
/// - Assistant text without finish_reason (intermediate streaming)
/// - User/System messages (input)
/// - Tool messages from non-tool spans (history copies)
///
/// # Why This Matters for Ordering
///
/// Consider a generation span producing: ToolUse → ToolResult → FinalText
/// - ToolUse event_time: T=100 (mid-generation)
/// - ToolResult event_time: T=200 (after tool execution)
/// - FinalText event_time: T=300 (generation complete)
/// - span_end: T=300
///
/// If ToolUse used span_end, it would have effective_time=300, sorting AFTER
/// ToolResult (effective_time=200). This would be wrong.
///
/// By using event_time for ToolUse, we get: 100 < 200 < 300 (correct order).
pub fn effective_timestamp(
    block: &BlockEntry,
    span_timestamps: &HashMap<String, SpanTimestamps>,
) -> DateTime<Utc> {
    let timestamps = span_timestamps.get(&block.span_id);

    if block.uses_span_end {
        // COMPLETION: use span_end (when operation finished)
        // Fallback chain: span_end → event_time
        // Safety: .max(event_time) handles malformed data where span_end < event_time
        timestamps
            .and_then(|t| t.span_end)
            .unwrap_or(block.timestamp)
            .max(block.timestamp)
    } else {
        // INTERMEDIATE/INPUT: use event_time (when event was recorded)
        // Safety: .max(span_start) ensures events aren't placed before their span
        let span_start = timestamps.map(|t| t.span_start).unwrap_or(block.timestamp);
        block.timestamp.max(span_start)
    }
}

// ============================================================================
// BIRTH TIME COMPUTATION
// ============================================================================

/// Build birth time map from blocks.
///
/// Pass 1: Record all timestamps to find birth times.
///
/// IMPORTANT: Only non-history blocks contribute to birth times.
/// History copies often have earlier timestamps (when context was assembled)
/// but shouldn't affect the ordering of actual message occurrences.
pub(super) fn build_birth_times(
    blocks: &[BlockEntry],
    ordinals: &[u32],
    span_timestamps: &HashMap<String, SpanTimestamps>,
) -> BirthTimeMap {
    let mut map = BirthTimeMap::default();

    for (index, block) in blocks.iter().enumerate() {
        // Skip history blocks - they shouldn't affect birth time calculation
        // History copies have misleading timestamps (when context was assembled)
        if block.is_history {
            continue;
        }

        let effective = effective_timestamp(block, span_timestamps);
        let identity = MessageIdentity::from_block(block);
        let ordinal = ordinals.get(index).copied().unwrap_or(0);

        // Debug: log tool block registration
        if block.is_tool_use() || block.is_tool_result() {
            tracing::trace!(
                entry_type = %block.entry_type,
                span_id = %block.span_id,
                uses_span_end = block.uses_span_end,
                is_history = block.is_history,
                event_time = %block.timestamp,
                effective_time = %effective,
                tool_name = ?block.tool_name,
                "build_birth_times: registering tool block"
            );
        }

        match identity {
            MessageIdentity::ToolCall {
                ref trace_id,
                content_hash,
            } => {
                // Record tool call timestamp (earliest occurrence)
                map.record_tool_call(trace_id, ordinal, content_hash, effective);
            }
            MessageIdentity::ToolResult {
                ref trace_id,
                identity_hash,
            } => {
                // Record tool result timestamp (earliest occurrence)
                map.record_tool_result(trace_id, ordinal, identity_hash, effective);
            }
            MessageIdentity::Regular {
                ref trace_id,
                role,
                semantic_hash,
            } => {
                // Record regular message timestamp (earliest occurrence)
                map.record_regular(trace_id, role, ordinal, semantic_hash, effective);
            }
        }
    }

    map
}

/// Get birth time for a block.
pub(super) fn get_birth_time(
    block: &BlockEntry,
    ordinal: u32,
    birth_map: &BirthTimeMap,
    span_timestamps: &HashMap<String, SpanTimestamps>,
) -> DateTime<Utc> {
    let effective = effective_timestamp(block, span_timestamps);
    let identity = MessageIdentity::from_block(block);

    match identity {
        MessageIdentity::ToolCall {
            ref trace_id,
            content_hash,
        } => {
            // Tool calls: look up birth time
            birth_map
                .get_tool_call(trace_id, ordinal, content_hash)
                .unwrap_or(effective)
        }
        MessageIdentity::ToolResult {
            ref trace_id,
            identity_hash,
        } => {
            // Tool results: look up birth time
            birth_map
                .get_tool_result(trace_id, ordinal, identity_hash)
                .unwrap_or(effective)
        }
        MessageIdentity::Regular {
            ref trace_id,
            role,
            semantic_hash,
        } => {
            // Look up birth time
            birth_map
                .get_regular(trace_id, role, ordinal, semantic_hash)
                .unwrap_or(effective)
        }
    }
}
