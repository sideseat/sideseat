//! SideML Message Deduplication and Ordering
//!
//! This module handles the complex task of reconstructing real conversation timelines
//! from OTEL spans that may contain duplicated messages (history duplication) and
//! need proper ordering (tool chains).
//!
//! # The Core Challenge
//!
//! OTEL traces often contain duplicate messages because:
//! 1. **History duplication**: Child spans re-send parent span messages as context
//! 2. **Streaming**: Same message sent in chunks with different timestamps
//! 3. **Tool chains**: ToolUse → Tool execution → ToolResult must maintain logical order
//!
//! # Solution: Birth Time Algorithm
//!
//! For each unique message identity:
//! - **Birth time** = earliest timestamp where this identity appeared
//! - This is when the message was REALLY sent/received
//!
//! History messages have `event_time=T+N` but `birth_time=T` (from first occurrence).
//!
//! # Message Types
//!
//! Blocks are pre-classified as OUTPUT or INPUT in the classification phase (mod.rs):
//! - **OUTPUT**: Uses span_end time, protected from history marking
//! - **INPUT**: Uses earliest occurrence time (birth time), can be marked as history
//!
//! The `uses_span_end` field is set on each block before this module processes them.
//!
//! # Quality Scoring
//!
//! When deduplicating, the highest-quality version is kept:
//! - Non-history block (+100) - strongly prefer current-turn messages
//! - Has finish_reason (+10) - complete response
//! - Enrichment content (+5) - thinking blocks
//! - Output source (+4) - vs input source copies
//! - Tool span (+3) - actual execution, not re-sent context
//! - Event source (+2) - vs attribute source
//! - Has model info (+1)
//!
//! # Ordering
//!
//! For messages with the same birth time, original message order is preserved
//! (by message_index, then entry_index). This maintains the order as it
//! appeared in the source data.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use chrono::{DateTime, Utc};

use super::types::BlockEntry;
use crate::domain::sideml::types::{ChatRole, ContentBlock};

// ============================================================================
// MESSAGE IDENTITY
// ============================================================================

/// Unique identity of a message for deduplication.
///
/// Two messages with the same identity are considered duplicates.
/// The identity is based on content, not position or timing.
///
/// # Tool Call Identity
///
/// Tool calls are identified by content hash (name + input), NOT by call_id.
/// This is because history re-sends often regenerate call IDs, causing the
/// same semantic tool call to appear with different IDs.
///
/// # Tool Result Identity
///
/// Tool results use `tool_use_id` as primary identity when present, falling back
/// to content hash. This is universal across frameworks:
/// - **Vercel AI SDK** (`toModelOutput`): Same `tool_use_id`, different content format.
///   Caught by `tool_use_id`-based identity here.
/// - **Strands** (history re-sends): Same content, regenerated `tool_use_id`.
///   Caught by `content_hash`-based Phase 7 in `history.rs` (independent signal).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum MessageIdentity {
    /// Regular message identified by trace, role, and content hash
    Regular {
        trace_id: String,
        role: ChatRole,
        /// Hash of semantic content blocks only (excludes enrichment/metadata)
        semantic_hash: u64,
    },
    /// Tool call identified by content hash (name + input)
    /// Using content hash instead of call_id because history re-sends regenerate IDs
    ToolCall {
        trace_id: String,
        /// Hash of tool name + input (call_id is ignored for identity)
        content_hash: u64,
    },
    /// Tool result identified by tool_use_id (primary) or content hash (fallback).
    ///
    /// `tool_use_id` is stable across content transformations (e.g., Vercel AI SDK's
    /// `toModelOutput`), making it the preferred identity signal. Falls back to
    /// content hash when `tool_use_id` is absent.
    ToolResult {
        trace_id: String,
        /// Hash of tool_use_id when present, or content hash as fallback
        identity_hash: u64,
    },
}

impl MessageIdentity {
    /// Create identity for a block entry.
    pub fn from_block(block: &BlockEntry) -> Self {
        // Tool use: identify by content hash (name + input)
        // We use content hash instead of call_id because history re-sends regenerate IDs
        if let ContentBlock::ToolUse { name, input, .. } = &block.content {
            return Self::ToolCall {
                trace_id: block.trace_id.clone(),
                content_hash: compute_tool_call_hash(name, input),
            };
        }

        // Tool result: identify by tool_use_id (primary) or content hash (fallback)
        // tool_use_id is stable across content transformations (Vercel toModelOutput).
        // History re-sends with regenerated IDs are caught by Phase 7 (content_hash).
        if let ContentBlock::ToolResult {
            tool_use_id,
            content,
            ..
        } = &block.content
        {
            let identity_hash = match tool_use_id {
                Some(tid) if !tid.is_empty() => compute_tool_use_id_hash(tid),
                _ => compute_tool_result_hash(content),
            };
            return Self::ToolResult {
                trace_id: block.trace_id.clone(),
                identity_hash,
            };
        }

        // Regular message: identify by role + content hash
        Self::Regular {
            trace_id: block.trace_id.clone(),
            role: block.role,
            semantic_hash: compute_semantic_hash(&block.content),
        }
    }
}

/// Compute hash for tool call identity (name + input).
fn compute_tool_call_hash(name: &str, input: &serde_json::Value) -> u64 {
    let mut hasher = DefaultHasher::new();
    "tool_call".hash(&mut hasher);
    name.hash(&mut hasher);
    normalize_json_for_hash(input).hash(&mut hasher);
    hasher.finish()
}

/// Compute hash for tool result identity (content).
/// Used as fallback when tool_use_id is absent.
fn compute_tool_result_hash(content: &serde_json::Value) -> u64 {
    let mut hasher = DefaultHasher::new();
    "tool_result".hash(&mut hasher);
    normalize_tool_result_content(content).hash(&mut hasher);
    hasher.finish()
}

/// Compute hash for tool_use_id-based identity.
/// Primary identity signal for tool results — stable across content transformations.
fn compute_tool_use_id_hash(tool_use_id: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    "tool_result_by_id".hash(&mut hasher);
    tool_use_id.hash(&mut hasher);
    hasher.finish()
}

// ============================================================================
// CONTENT NORMALIZATION FOR HASHING
// ============================================================================

/// Normalize JSON for consistent hashing (sort object keys).
pub(super) fn normalize_json_for_hash(value: &serde_json::Value) -> String {
    use serde_json::Value as JsonValue;
    match value {
        JsonValue::Object(map) => {
            let mut pairs: Vec<_> = map.iter().collect();
            pairs.sort_by_key(|(k, _)| *k);
            let sorted: Vec<String> = pairs
                .iter()
                .map(|(k, v)| format!("{}:{}", k, normalize_json_for_hash(v)))
                .collect();
            format!("{{{}}}", sorted.join(","))
        }
        JsonValue::Array(arr) => {
            let items: Vec<String> = arr.iter().map(normalize_json_for_hash).collect();
            format!("[{}]", items.join(","))
        }
        _ => value.to_string(),
    }
}

/// Normalize tool result content to canonical form for hashing.
/// Handles: string, array of blocks, object with nested json.
pub(super) fn normalize_tool_result_content(content: &serde_json::Value) -> String {
    use serde_json::Value as JsonValue;
    match content {
        JsonValue::String(s) => s.trim().to_string(),
        JsonValue::Array(arr) => {
            // Extract text from content blocks, join
            let texts: Vec<String> = arr.iter().filter_map(extract_text_from_block).collect();
            if texts.is_empty() {
                // Fallback: normalize the array as JSON
                normalize_json_for_hash(content)
            } else {
                texts.join("\n").trim().to_string()
            }
        }
        JsonValue::Object(obj) => {
            // Handle {"json": {...}} wrapper
            if let Some(inner) = obj.get("json") {
                return normalize_json_for_hash(inner);
            }
            // Handle {"type": "text", "text": "..."}
            if obj.get("type").and_then(|t| t.as_str()) == Some("text")
                && let Some(text) = obj.get("text").and_then(|t| t.as_str())
            {
                return text.trim().to_string();
            }
            // Handle {"type": "json", "data": {...}}
            if obj.get("type").and_then(|t| t.as_str()) == Some("json")
                && let Some(data) = obj.get("data")
            {
                if let Some(json) = data.get("json") {
                    return normalize_json_for_hash(json);
                }
                return normalize_json_for_hash(data);
            }
            normalize_json_for_hash(content)
        }
        _ => content.to_string(),
    }
}

/// Extract text from a content block for normalization.
fn extract_text_from_block(block: &serde_json::Value) -> Option<String> {
    let obj = block.as_object()?;
    let block_type = obj.get("type").and_then(|t| t.as_str())?;

    match block_type {
        "text" => obj
            .get("text")
            .and_then(|t| t.as_str())
            .map(|s| s.trim().to_string()),
        "json" => {
            if let Some(data) = obj.get("data") {
                if let Some(json) = data.get("json") {
                    return Some(normalize_json_for_hash(json));
                }
                return Some(normalize_json_for_hash(data));
            }
            None
        }
        _ => None,
    }
}

/// Compute hash of semantic content (excludes enrichment/metadata blocks).
/// Re-exported from mod.rs to avoid duplication.
pub(super) use super::compute_block_hash as compute_semantic_hash;

// ============================================================================
// BIRTH TIME MAP
// ============================================================================

/// Combine trace_id and hash into a single u128 lookup key.
/// Avoids String allocation on HashMap lookups.
#[inline]
fn make_key(trace_id: &str, hash: u64) -> u128 {
    let mut hasher = DefaultHasher::new();
    trace_id.hash(&mut hasher);
    let trace_hash = hasher.finish();
    ((trace_hash as u128) << 64) | (hash as u128)
}

/// Combine trace_id, role, and semantic hash into a single u128 lookup key.
#[inline]
fn make_regular_key(trace_id: &str, role: ChatRole, semantic_hash: u64) -> u128 {
    let mut hasher = DefaultHasher::new();
    trace_id.hash(&mut hasher);
    role.hash(&mut hasher);
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
struct BirthTimeMap {
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
        semantic_hash: u64,
        timestamp: DateTime<Utc>,
    ) {
        let key = make_regular_key(trace_id, role, semantic_hash);
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
    fn record_tool_call(&mut self, trace_id: &str, content_hash: u64, timestamp: DateTime<Utc>) {
        let key = make_key(trace_id, content_hash);
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
    fn record_tool_result(&mut self, trace_id: &str, identity_hash: u64, timestamp: DateTime<Utc>) {
        let key = make_key(trace_id, identity_hash);
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
        semantic_hash: u64,
    ) -> Option<DateTime<Utc>> {
        let key = make_regular_key(trace_id, role, semantic_hash);
        self.regular_times.get(&key).copied()
    }

    /// Get birth time for a tool call.
    #[inline]
    fn get_tool_call(&self, trace_id: &str, content_hash: u64) -> Option<DateTime<Utc>> {
        let key = make_key(trace_id, content_hash);
        self.tool_call_times.get(&key).copied()
    }

    /// Get birth time for a tool result.
    #[inline]
    fn get_tool_result(&self, trace_id: &str, identity_hash: u64) -> Option<DateTime<Utc>> {
        let key = make_key(trace_id, identity_hash);
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
fn build_birth_times(
    blocks: &[BlockEntry],
    span_timestamps: &HashMap<String, SpanTimestamps>,
) -> BirthTimeMap {
    let mut map = BirthTimeMap::default();

    for block in blocks {
        // Skip history blocks - they shouldn't affect birth time calculation
        // History copies have misleading timestamps (when context was assembled)
        if block.is_history {
            continue;
        }

        let effective = effective_timestamp(block, span_timestamps);
        let identity = MessageIdentity::from_block(block);

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
                map.record_tool_call(trace_id, content_hash, effective);
            }
            MessageIdentity::ToolResult {
                ref trace_id,
                identity_hash,
            } => {
                // Record tool result timestamp (earliest occurrence)
                map.record_tool_result(trace_id, identity_hash, effective);
            }
            MessageIdentity::Regular {
                ref trace_id,
                role,
                semantic_hash,
            } => {
                // Record regular message timestamp (earliest occurrence)
                map.record_regular(trace_id, role, semantic_hash, effective);
            }
        }
    }

    map
}

/// Get birth time for a block.
fn get_birth_time(
    block: &BlockEntry,
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
                .get_tool_call(trace_id, content_hash)
                .unwrap_or(effective)
        }
        MessageIdentity::ToolResult {
            ref trace_id,
            identity_hash,
        } => {
            // Tool results: look up birth time
            birth_map
                .get_tool_result(trace_id, identity_hash)
                .unwrap_or(effective)
        }
        MessageIdentity::Regular {
            ref trace_id,
            role,
            semantic_hash,
        } => {
            // Look up birth time
            birth_map
                .get_regular(trace_id, role, semantic_hash)
                .unwrap_or(effective)
        }
    }
}

// ============================================================================
// QUALITY SCORING
// ============================================================================

/// Quality score weights for deduplication.
///
/// Higher scores = preferred version when deduplicating identical content.
/// Weights are ordered by importance (non-history >> finish_reason >> enrichment >> source >> model).
mod quality {
    /// Non-history blocks are strongly preferred over history copies.
    pub const NON_HISTORY: u32 = 100;
    /// Complete responses (with finish_reason) preferred over streaming chunks.
    pub const HAS_FINISH_REASON: u32 = 10;
    /// Enrichment content (thinking blocks) adds value.
    pub const IS_ENRICHMENT: u32 = 5;
    /// Output-source blocks preferred over input-source copies.
    pub const IS_OUTPUT_SOURCE: u32 = 4;
    /// Tool result from actual tool execution span (not re-sent in generation context).
    pub const FROM_TOOL_SPAN: u32 = 3;
    /// Event source preferred over attribute source (more structured).
    pub const FROM_EVENT: u32 = 2;
    /// Having model info is a minor quality signal.
    pub const HAS_MODEL: u32 = 1;
}

/// Compute quality score for a block.
///
/// Higher score = more complete/enriched version.
/// When deduplicating, keep the highest quality version.
fn compute_quality(block: &BlockEntry) -> u32 {
    let mut score = 0u32;

    // Strong preference for non-history blocks
    // History blocks only win if there's no non-history equivalent
    if !block.is_history {
        score += quality::NON_HISTORY;
    }

    // Prefer blocks with finish_reason (complete response)
    if block.finish_reason.is_some() {
        score += quality::HAS_FINISH_REASON;
    }

    // Prefer enrichment content (thinking blocks)
    if block.content.is_enrichment() {
        score += quality::IS_ENRICHMENT;
    }

    // Prefer output-source blocks over input-source copies
    if block.is_output_source() {
        score += quality::IS_OUTPUT_SOURCE;
    }

    // Tool results from tool spans are the actual execution output, not context re-sends
    if block.is_tool_result() && block.observation_type.as_deref() == Some("tool") {
        score += quality::FROM_TOOL_SPAN;
    }

    // Prefer event source over attribute source
    if block.is_from_event() {
        score += quality::FROM_EVENT;
    }

    // Prefer blocks with model info
    if block.model.is_some() {
        score += quality::HAS_MODEL;
    }

    score
}

// ============================================================================
// DEDUPLICATION
// ============================================================================

/// Deduplicate blocks by identity, keeping highest quality version.
///
/// Note: Birth time is computed during sorting, not here. Deduplication only
/// needs identity and quality scoring.
///
/// Tool results use `tool_use_id` as identity when present, which naturally
/// handles content transformations (e.g., Vercel AI SDK's `toModelOutput`).
/// History re-sends with regenerated IDs are handled upstream by Phase 7
/// in `history.rs` (content_hash-based duplicate detection).
fn deduplicate_blocks(blocks: Vec<BlockEntry>) -> Vec<BlockEntry> {
    use std::collections::HashSet;

    let input_count = blocks.len();
    let input_text_count = blocks.iter().filter(|b| b.entry_type == "text").count();

    // First: collect identities of non-history blocks
    // History-only messages (no current-turn equivalent) will be filtered out
    let non_history_ids: HashSet<MessageIdentity> = blocks
        .iter()
        .filter(|b| !b.is_history)
        .map(MessageIdentity::from_block)
        .collect();

    // Filter: keep non-history blocks, and history blocks only if they have a non-history equivalent
    // This removes messages from previous turns that appear in history
    let blocks: Vec<_> = blocks
        .into_iter()
        .filter(|b| {
            if b.is_history {
                // Keep history only if there's a non-history version to dedupe with
                non_history_ids.contains(&MessageIdentity::from_block(b))
            } else {
                true
            }
        })
        .collect();

    let after_history_filter = blocks.len();

    // Identity-based dedup: non-history will win due to quality scoring.
    // For tool results with tool_use_id, this collapses all versions
    // (raw + transformed) into the highest quality one.
    let mut candidates: HashMap<MessageIdentity, (BlockEntry, u32)> = HashMap::new();

    for block in blocks {
        let identity = MessageIdentity::from_block(&block);
        let quality = compute_quality(&block);

        candidates
            .entry(identity)
            .and_modify(|(existing, existing_quality)| {
                if quality > *existing_quality {
                    *existing = block.clone();
                    *existing_quality = quality;
                }
            })
            .or_insert((block, quality));
    }

    let result: Vec<_> = candidates.into_values().map(|(block, _)| block).collect();

    tracing::trace!(
        input = input_count,
        input_text = input_text_count,
        non_history_ids = non_history_ids.len(),
        after_history_filter,
        output = result.len(),
        output_text = result.iter().filter(|b| b.entry_type == "text").count(),
        "deduplicate_blocks: complete"
    );

    result
}

// ============================================================================
// PUBLIC API
// ============================================================================

/// Process blocks through the deduplication and ordering pipeline.
///
/// # Pipeline
///
/// 1. Deduplicate by identity (keep highest quality)
/// 2. Compute birth times for all blocks
/// 3. Pre-compute birth times into Vec (O(n) — avoids O(n log n) recomputation in sort)
/// 4. Sort by birth time + semantic order (using pre-computed times)
/// 5. Materialize birth times into block timestamps for API clients
pub fn process_dedup(
    blocks: Vec<BlockEntry>,
    span_timestamps: HashMap<String, SpanTimestamps>,
) -> Vec<BlockEntry> {
    if blocks.is_empty() {
        return blocks;
    }

    // Deduplicate by identity (keeps highest quality version)
    let deduped = deduplicate_blocks(blocks);

    // Build birth time map (after dedup, from deduped blocks)
    let birth_map = build_birth_times(&deduped, &span_timestamps);

    // Pre-compute birth times once (O(n)) — avoids O(n log n) identity
    // recomputation (String clones + hashing) during sort comparisons.
    let birth_times: Vec<DateTime<Utc>> = deduped
        .iter()
        .map(|b| get_birth_time(b, &birth_map, &span_timestamps))
        .collect();

    // Debug: log birth times for tool blocks
    if tracing::enabled!(tracing::Level::TRACE) {
        for (i, block) in deduped.iter().enumerate() {
            if block.is_tool_use() || block.is_tool_result() {
                let effective = effective_timestamp(block, &span_timestamps);
                tracing::trace!(
                    entry_type = %block.entry_type,
                    span_id = %block.span_id,
                    uses_span_end = block.uses_span_end,
                    event_time = %block.timestamp,
                    effective_time = %effective,
                    birth_time = %birth_times[i],
                    "process_dedup: tool block"
                );
            }
        }
    }

    // Sort by birth time + semantic order, carrying pre-computed times.
    // Tuple sort keeps birth times associated with their blocks during swaps.
    let mut paired: Vec<(DateTime<Utc>, BlockEntry)> =
        birth_times.into_iter().zip(deduped).collect();

    paired.sort_by(|(a_birth, a), (b_birth, b)| {
        // Same-batch detection: same span + same event timestamp.
        // These blocks are from the same response and should preserve original order
        // regardless of their timestamp strategy (span_end vs event_time).
        let same_batch = a.span_id == b.span_id && a.timestamp == b.timestamp;

        if same_batch {
            match a.message_index.cmp(&b.message_index) {
                Ordering::Equal => return a.entry_index.cmp(&b.entry_index),
                other => return other,
            }
        }

        // Different batches: order by pre-computed birth time
        match a_birth.cmp(b_birth) {
            Ordering::Equal => {}
            other => return other,
        }

        // Same birth time but different batches: preserve message position
        match a.message_index.cmp(&b.message_index) {
            Ordering::Equal => {}
            other => return other,
        }

        a.entry_index.cmp(&b.entry_index)
    });

    // Materialize computed timestamps for API clients.
    // Raw event timestamps can be misleading (e.g., attribute-sourced messages
    // inherit span start time, not the actual production time). Birth time
    // reflects when the message was actually produced/received.
    paired
        .into_iter()
        .map(|(birth, mut block)| {
            block.timestamp = birth;
            block
        })
        .collect()
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::types::MessageCategory;
    use crate::domain::sideml::types::FinishReason;
    use chrono::TimeZone;

    fn make_test_block(
        trace_id: &str,
        span_id: &str,
        role: ChatRole,
        text: &str,
        timestamp: DateTime<Utc>,
    ) -> BlockEntry {
        BlockEntry {
            entry_type: "text".to_string(),
            content: ContentBlock::Text {
                text: text.to_string(),
            },
            role,
            trace_id: trace_id.to_string(),
            span_id: span_id.to_string(),
            session_id: None,
            message_index: 0,
            entry_index: 0,
            parent_span_id: None,
            span_path: vec![span_id.to_string()],
            timestamp,
            observation_type: None,
            model: None,
            provider: None,
            name: None,
            finish_reason: None,
            tool_use_id: None,
            tool_name: None,
            tokens: None,
            cost: None,
            status_code: None,
            is_error: false,
            source_type: "event".to_string(),
            event_name: None,
            source_attribute: None,
            category: MessageCategory::GenAIUserMessage,
            content_hash: "test".to_string(),
            is_semantic: true,
            uses_span_end: false,
            is_history: false,
        }
    }

    fn make_tool_use_block(
        trace_id: &str,
        span_id: &str,
        call_id: &str,
        name: &str,
        timestamp: DateTime<Utc>,
    ) -> BlockEntry {
        BlockEntry {
            entry_type: "tool_use".to_string(),
            content: ContentBlock::ToolUse {
                id: Some(call_id.to_string()),
                name: name.to_string(),
                input: serde_json::json!({}),
            },
            role: ChatRole::Assistant,
            trace_id: trace_id.to_string(),
            span_id: span_id.to_string(),
            session_id: None,
            message_index: 0,
            entry_index: 0,
            parent_span_id: None,
            span_path: vec![span_id.to_string()],
            timestamp,
            observation_type: None,
            model: None,
            provider: None,
            name: None,
            finish_reason: Some(FinishReason::ToolUse),
            tool_use_id: Some(call_id.to_string()),
            tool_name: Some(name.to_string()),
            tokens: None,
            cost: None,
            status_code: None,
            is_error: false,
            source_type: "event".to_string(),
            event_name: None,
            source_attribute: None,
            category: MessageCategory::GenAIChoice,
            content_hash: "test".to_string(),
            is_semantic: true,
            // ToolUse uses event_time (not span_end) - the decision to call a tool
            // happens DURING generation, not at completion. See classify::uses_span_end().
            uses_span_end: false,
            is_history: false,
        }
    }

    fn make_tool_result_block(
        trace_id: &str,
        span_id: &str,
        tool_use_id: &str,
        content: &str,
        timestamp: DateTime<Utc>,
    ) -> BlockEntry {
        BlockEntry {
            entry_type: "tool_result".to_string(),
            content: ContentBlock::ToolResult {
                tool_use_id: Some(tool_use_id.to_string()),
                content: serde_json::json!(content),
                is_error: false,
            },
            role: ChatRole::Tool,
            trace_id: trace_id.to_string(),
            span_id: span_id.to_string(),
            session_id: None,
            message_index: 0,
            entry_index: 0,
            parent_span_id: None,
            span_path: vec![span_id.to_string()],
            timestamp,
            observation_type: None,
            model: None,
            provider: None,
            name: None,
            finish_reason: None,
            tool_use_id: Some(tool_use_id.to_string()),
            tool_name: None,
            tokens: None,
            cost: None,
            status_code: None,
            is_error: false,
            source_type: "event".to_string(),
            event_name: None,
            source_attribute: None,
            category: MessageCategory::GenAIToolMessage,
            content_hash: "test".to_string(),
            is_semantic: true,
            uses_span_end: false, // Tool results are INPUT
            is_history: false,
        }
    }

    fn utc(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).unwrap()
    }

    // ========================================================================
    // BIRTH TIME TESTS
    // ========================================================================

    #[test]
    fn test_birth_time_uses_earliest_occurrence() {
        // Same content appears at T=0 and T=5, birth_time should be T=0
        let t0 = utc(0);
        let t5 = utc(5);

        let block1 = make_test_block("trace1", "span1", ChatRole::User, "Hello", t0);
        let block2 = make_test_block("trace1", "span2", ChatRole::User, "Hello", t5);

        let span_timestamps = HashMap::from([
            (
                "span1".to_string(),
                SpanTimestamps {
                    span_start: t0,
                    span_end: Some(t0),
                },
            ),
            (
                "span2".to_string(),
                SpanTimestamps {
                    span_start: t5,
                    span_end: Some(t5),
                },
            ),
        ]);

        let birth_map = build_birth_times(&[block1.clone(), block2], &span_timestamps);

        // Both should have birth_time = T=0
        let birth1 = get_birth_time(&block1, &birth_map, &span_timestamps);
        assert_eq!(birth1, t0);
    }

    #[test]
    fn test_output_uses_effective_timestamp() {
        let t0 = utc(0);
        let t5 = utc(5);

        let mut block = make_test_block("trace1", "span1", ChatRole::Assistant, "Response", t0);
        block.finish_reason = Some(FinishReason::Stop);
        block.uses_span_end = true; // Mark as OUTPUT for effective timestamp calculation

        let span_timestamps = HashMap::from([(
            "span1".to_string(),
            SpanTimestamps {
                span_start: t0,
                span_end: Some(t5),
            },
        )]);

        // OUTPUT blocks use span_end for effective timestamp
        let effective = effective_timestamp(&block, &span_timestamps);
        assert_eq!(effective, t5);
    }

    #[test]
    fn test_timestamp_materialized_for_output_blocks() {
        // After process_dedup, output blocks should have their timestamp updated
        // to span_end (not the raw event time which equals span start for attributes).
        let t_start = utc(0);
        let t_end = utc(10);

        let mut block =
            make_test_block("trace1", "span1", ChatRole::Assistant, "Response", t_start);
        block.finish_reason = Some(FinishReason::Stop);
        block.uses_span_end = true;
        block.source_type = "attribute".to_string();

        let span_timestamps = HashMap::from([(
            "span1".to_string(),
            SpanTimestamps {
                span_start: t_start,
                span_end: Some(t_end),
            },
        )]);

        // Before: timestamp is span start (raw attribute time)
        assert_eq!(block.timestamp, t_start);

        let result = process_dedup(vec![block], span_timestamps);

        // After: timestamp materialized to span_end (effective/birth time)
        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0].timestamp, t_end,
            "Output block timestamp should be materialized to span_end, not raw event time"
        );
    }

    #[test]
    fn test_tool_result_uses_own_birth_time() {
        // Tool results now use content-based identity and their own birth time
        let t0 = utc(0);
        let t5 = utc(5);

        let tool_use = make_tool_use_block("trace1", "span1", "call_123", "search", t0);
        let tool_result = make_tool_result_block("trace1", "span2", "call_123", "result", t5);

        let span_timestamps = HashMap::from([
            (
                "span1".to_string(),
                SpanTimestamps {
                    span_start: t0,
                    span_end: Some(t0),
                },
            ),
            (
                "span2".to_string(),
                SpanTimestamps {
                    span_start: t5,
                    span_end: Some(t5),
                },
            ),
        ]);

        let birth_map = build_birth_times(&[tool_use, tool_result.clone()], &span_timestamps);

        // Tool result uses its own birth time (content-based)
        let birth = get_birth_time(&tool_result, &birth_map, &span_timestamps);
        assert_eq!(birth, t5);
    }

    // ========================================================================
    // DEDUPLICATION TESTS
    // ========================================================================

    #[test]
    fn test_history_collapsed_to_first_occurrence() {
        let t0 = utc(0);
        let t5 = utc(5);

        let original = make_test_block("trace1", "span1", ChatRole::User, "Hello", t0);
        let history = make_test_block("trace1", "span2", ChatRole::User, "Hello", t5);

        let span_timestamps = HashMap::from([
            (
                "span1".to_string(),
                SpanTimestamps {
                    span_start: t0,
                    span_end: Some(t0),
                },
            ),
            (
                "span2".to_string(),
                SpanTimestamps {
                    span_start: t5,
                    span_end: Some(t5),
                },
            ),
        ]);

        let result = process_dedup(vec![original, history], span_timestamps);

        // Should dedupe to single message
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].span_id, "span1"); // Keep original
    }

    #[test]
    fn test_same_content_different_traces_both_kept() {
        let t0 = utc(0);

        let block1 = make_test_block("trace1", "span1", ChatRole::User, "Hello", t0);
        let block2 = make_test_block("trace2", "span2", ChatRole::User, "Hello", t0);

        let span_timestamps = HashMap::from([
            (
                "span1".to_string(),
                SpanTimestamps {
                    span_start: t0,
                    span_end: Some(t0),
                },
            ),
            (
                "span2".to_string(),
                SpanTimestamps {
                    span_start: t0,
                    span_end: Some(t0),
                },
            ),
        ]);

        let result = process_dedup(vec![block1, block2], span_timestamps);

        // Different traces = different identities = both kept
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_enriched_version_preferred() {
        let t0 = utc(0);

        // Plain text block
        let mut plain = make_test_block("trace1", "span1", ChatRole::Assistant, "Response", t0);
        plain.source_type = "attribute".to_string();

        // Same content but from event source (higher quality)
        let mut enriched = make_test_block("trace1", "span2", ChatRole::Assistant, "Response", t0);
        enriched.finish_reason = Some(FinishReason::Stop);
        enriched.source_type = "event".to_string();

        let span_timestamps = HashMap::from([
            (
                "span1".to_string(),
                SpanTimestamps {
                    span_start: t0,
                    span_end: Some(t0),
                },
            ),
            (
                "span2".to_string(),
                SpanTimestamps {
                    span_start: t0,
                    span_end: Some(t0),
                },
            ),
        ]);

        let result = process_dedup(vec![plain, enriched], span_timestamps);

        // Should keep enriched version (has finish_reason and event source)
        assert_eq!(result.len(), 1);
        assert!(result[0].finish_reason.is_some());
    }

    // ========================================================================
    // ORDERING TESTS
    // ========================================================================

    #[test]
    fn test_tool_use_before_tool_result() {
        let t0 = utc(0);

        let mut tool_use = make_tool_use_block("trace1", "span1", "call_123", "search", t0);
        tool_use.message_index = 0;

        let mut tool_result = make_tool_result_block("trace1", "span2", "call_123", "result", t0);
        tool_result.message_index = 1; // Comes after tool_use

        let span_timestamps = HashMap::from([
            (
                "span1".to_string(),
                SpanTimestamps {
                    span_start: t0,
                    span_end: Some(t0),
                },
            ),
            (
                "span2".to_string(),
                SpanTimestamps {
                    span_start: t0,
                    span_end: Some(t0),
                },
            ),
        ]);

        // Process in reverse order
        let result = process_dedup(vec![tool_result, tool_use], span_timestamps);

        // ToolUse should come before Tool result (by message_index)
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].entry_type, "tool_use");
        assert_eq!(result[1].entry_type, "tool_result");
    }

    #[test]
    fn test_same_batch_ordering_text_before_tool_use() {
        // When text and tool_use are from the same span with same timestamp,
        // they should preserve message_index order regardless of uses_span_end
        let t0 = utc(0);
        let t_end = utc(1);

        // Text with finish_reason (uses_span_end=true, effective_time=t_end)
        let mut text = make_test_block("trace1", "span1", ChatRole::Assistant, "I'll search", t0);
        text.message_index = 0;
        text.finish_reason = Some(FinishReason::ToolUse);
        text.uses_span_end = true; // Would use span_end for birth time

        // ToolUse (uses_span_end=false, effective_time=t0)
        let mut tool_use = make_tool_use_block("trace1", "span1", "call_1", "search", t0);
        tool_use.message_index = 1;
        tool_use.uses_span_end = false; // Uses event_time for birth time

        let span_timestamps = HashMap::from([(
            "span1".to_string(),
            SpanTimestamps {
                span_start: t0,
                span_end: Some(t_end),
            },
        )]);

        // Even though text uses span_end (t_end=1) and tool_use uses event_time (t0=0),
        // they're from the same batch (same span, same timestamp) so text should come first
        let result = process_dedup(vec![tool_use, text], span_timestamps);

        assert_eq!(result.len(), 2);
        // Text (message_index=0) should come before tool_use (message_index=1)
        assert_eq!(result[0].entry_type, "text");
        assert_eq!(result[1].entry_type, "tool_use");
    }

    #[test]
    fn test_conversation_order() {
        let t0 = utc(0);
        let t1 = utc(1);
        let t2 = utc(2);
        let t3 = utc(3);

        let user_msg = make_test_block("trace1", "span1", ChatRole::User, "Hello", t0);

        let mut assistant_msg =
            make_test_block("trace1", "span1", ChatRole::Assistant, "Hi there!", t1);
        assistant_msg.finish_reason = Some(FinishReason::Stop);

        let user_msg2 = make_test_block("trace1", "span2", ChatRole::User, "Search for X", t2);

        let tool_use = make_tool_use_block("trace1", "span2", "call_123", "search", t3);

        let span_timestamps = HashMap::from([
            (
                "span1".to_string(),
                SpanTimestamps {
                    span_start: t0,
                    span_end: Some(t1),
                },
            ),
            (
                "span2".to_string(),
                SpanTimestamps {
                    span_start: t2,
                    span_end: Some(t3),
                },
            ),
        ]);

        // Process in random order
        let result = process_dedup(
            vec![tool_use, user_msg.clone(), user_msg2.clone(), assistant_msg],
            span_timestamps,
        );

        // Should be in conversation order
        assert_eq!(result.len(), 4);
        assert_eq!(result[0].role, ChatRole::User);
        assert!(matches!(
            result[0].content,
            ContentBlock::Text { ref text } if text == "Hello"
        ));
        assert_eq!(result[1].role, ChatRole::Assistant);
        assert_eq!(result[2].role, ChatRole::User);
        assert!(matches!(
            result[2].content,
            ContentBlock::Text { ref text } if text == "Search for X"
        ));
        assert_eq!(result[3].entry_type, "tool_use");
    }

    // ========================================================================
    // IDENTITY TESTS
    // ========================================================================

    #[test]
    fn test_identity_tool_call() {
        let t0 = utc(0);
        let block = make_tool_use_block("trace1", "span1", "call_123", "search", t0);

        let identity = MessageIdentity::from_block(&block);
        assert!(matches!(
            identity,
            MessageIdentity::ToolCall {
                trace_id,
                content_hash: _,
            } if trace_id == "trace1"
        ));
    }

    #[test]
    fn test_identity_tool_result() {
        let t0 = utc(0);
        let block = make_tool_result_block("trace1", "span1", "call_123", "result", t0);

        let identity = MessageIdentity::from_block(&block);
        assert!(matches!(
            identity,
            MessageIdentity::ToolResult {
                trace_id,
                identity_hash: _,
            } if trace_id == "trace1"
        ));
    }

    #[test]
    fn test_same_tool_call_different_ids_deduped() {
        // Same tool call (same name + input) with different call_ids should be deduplicated
        let t0 = utc(0);

        // Two tool calls with SAME name+input but DIFFERENT IDs
        let tool1 = make_tool_use_block("trace1", "span1", "call_111", "search", t0);
        let tool2 = make_tool_use_block("trace1", "span2", "call_222", "search", t0);

        // They should have the same identity (content-based)
        let id1 = MessageIdentity::from_block(&tool1);
        let id2 = MessageIdentity::from_block(&tool2);
        assert_eq!(id1, id2);

        // And should be deduplicated
        let span_timestamps = HashMap::from([
            (
                "span1".to_string(),
                SpanTimestamps {
                    span_start: t0,
                    span_end: Some(t0),
                },
            ),
            (
                "span2".to_string(),
                SpanTimestamps {
                    span_start: t0,
                    span_end: Some(t0),
                },
            ),
        ]);

        let result = process_dedup(vec![tool1, tool2], span_timestamps);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_identity_regular() {
        let t0 = utc(0);
        let block = make_test_block("trace1", "span1", ChatRole::User, "Hello", t0);

        let identity = MessageIdentity::from_block(&block);
        assert!(matches!(
            identity,
            MessageIdentity::Regular {
                trace_id,
                role: ChatRole::User,
                ..
            } if trace_id == "trace1"
        ));
    }

    // ========================================================================
    // EDGE CASE TESTS
    // ========================================================================

    #[test]
    fn test_empty_blocks() {
        let result = process_dedup(vec![], HashMap::new());
        assert!(result.is_empty());
    }

    #[test]
    fn test_single_block() {
        let t0 = utc(0);
        let block = make_test_block("trace1", "span1", ChatRole::User, "Hello", t0);

        let span_timestamps = HashMap::from([(
            "span1".to_string(),
            SpanTimestamps {
                span_start: t0,
                span_end: Some(t0),
            },
        )]);

        let result = process_dedup(vec![block], span_timestamps);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_tool_result_same_id_different_content_deduped() {
        // Same tool_use_id → same identity, regardless of content format.
        // Covers Vercel AI SDK toModelOutput: raw execute() output vs transformed format.
        let t0 = utc(0);
        let t1 = utc(1);

        // Raw tool result from tool span
        let mut raw =
            make_tool_result_block("trace1", "tool_span", "call_123", "raw result data", t0);
        raw.observation_type = Some("tool".to_string());

        // Transformed tool result from generation span (different content, same tool_use_id)
        let mut transformed = BlockEntry {
            content: ContentBlock::ToolResult {
                tool_use_id: Some("call_123".to_string()),
                content: serde_json::json!({"type": "content", "value": [{"type": "text", "text": "transformed result"}]}),
                is_error: false,
            },
            span_id: "gen_span".to_string(),
            timestamp: t1,
            observation_type: Some("generation".to_string()),
            ..make_tool_result_block("trace1", "gen_span", "call_123", "", t1)
        };
        transformed.model = Some("claude-haiku".to_string());

        let span_timestamps = HashMap::from([
            (
                "tool_span".to_string(),
                SpanTimestamps {
                    span_start: t0,
                    span_end: Some(t0),
                },
            ),
            (
                "gen_span".to_string(),
                SpanTimestamps {
                    span_start: t1,
                    span_end: Some(t1),
                },
            ),
        ]);

        let result = process_dedup(vec![raw, transformed], span_timestamps);

        // Same tool_use_id → deduped to 1 (tool_use_id is identity, not content)
        assert_eq!(result.len(), 1);
        // Tool span version wins via FROM_TOOL_SPAN quality bonus
        assert_eq!(result[0].span_id, "tool_span");
    }

    #[test]
    fn test_tool_result_different_ids_not_deduped_by_tool_id() {
        // Different tool_use_ids should NOT be merged even if both are tool results
        let t0 = utc(0);

        let result1 = make_tool_result_block("trace1", "span1", "call_111", "result A", t0);
        let result2 = make_tool_result_block("trace1", "span2", "call_222", "result B", t0);

        let span_timestamps = HashMap::from([
            (
                "span1".to_string(),
                SpanTimestamps {
                    span_start: t0,
                    span_end: Some(t0),
                },
            ),
            (
                "span2".to_string(),
                SpanTimestamps {
                    span_start: t0,
                    span_end: Some(t0),
                },
            ),
        ]);

        let result = process_dedup(vec![result1, result2], span_timestamps);

        // Both should be kept (different tool_use_ids = different identities)
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_tool_result_no_tool_use_id_falls_back_to_content_hash() {
        // Without tool_use_id, identity uses content hash (existing behavior)
        let t0 = utc(0);

        let mut result1 = BlockEntry {
            content: ContentBlock::ToolResult {
                tool_use_id: None,
                content: serde_json::json!("same content"),
                is_error: false,
            },
            ..make_tool_result_block("trace1", "span1", "", "unused", t0)
        };
        result1.tool_use_id = None;

        let mut result2 = BlockEntry {
            content: ContentBlock::ToolResult {
                tool_use_id: None,
                content: serde_json::json!("same content"),
                is_error: false,
            },
            ..make_tool_result_block("trace1", "span2", "", "unused", t0)
        };
        result2.tool_use_id = None;

        // Same content, no tool_use_id → same identity via content hash
        let id1 = MessageIdentity::from_block(&result1);
        let id2 = MessageIdentity::from_block(&result2);
        assert_eq!(id1, id2, "Same content without tool_use_id should match");

        let span_timestamps = HashMap::from([
            (
                "span1".to_string(),
                SpanTimestamps {
                    span_start: t0,
                    span_end: Some(t0),
                },
            ),
            (
                "span2".to_string(),
                SpanTimestamps {
                    span_start: t0,
                    span_end: Some(t0),
                },
            ),
        ]);

        let result = process_dedup(vec![result1, result2], span_timestamps);
        assert_eq!(
            result.len(),
            1,
            "Same content without tool_use_id should dedup"
        );
    }

    #[test]
    fn test_tool_result_same_id_same_observation_type_deduped() {
        // Same tool_use_id from same observation type → still deduped.
        // tool_use_id is identity, observation type is irrelevant.
        let t0 = utc(0);
        let t1 = utc(1);

        let mut r1 = make_tool_result_block("trace1", "span1", "call_1", "First result", t0);
        r1.observation_type = Some("generation".to_string());

        let mut r2 = make_tool_result_block("trace1", "span2", "call_1", "Second result", t1);
        r2.observation_type = Some("generation".to_string());

        let span_timestamps = HashMap::from([
            (
                "span1".to_string(),
                SpanTimestamps {
                    span_start: t0,
                    span_end: Some(t0),
                },
            ),
            (
                "span2".to_string(),
                SpanTimestamps {
                    span_start: t1,
                    span_end: Some(t1),
                },
            ),
        ]);

        let result = process_dedup(vec![r1, r2], span_timestamps);

        // Same tool_use_id → same identity → deduped to 1
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_tool_result_without_matching_tool_use() {
        let t0 = utc(0);
        // Tool result with no matching tool_use in the data
        let tool_result = make_tool_result_block("trace1", "span1", "missing_call", "result", t0);

        let span_timestamps = HashMap::from([(
            "span1".to_string(),
            SpanTimestamps {
                span_start: t0,
                span_end: Some(t0),
            },
        )]);

        // Should still work, using effective timestamp as fallback
        let result = process_dedup(vec![tool_result], span_timestamps);
        assert_eq!(result.len(), 1);
    }

    // ========================================================================
    // ADVANCED DEDUPLICATION TESTS
    // ========================================================================

    #[test]
    fn test_parallel_tool_calls_different_inputs_not_deduped() {
        // Multiple tool calls with DIFFERENT inputs should NOT be deduped
        // (even though they have the same tool name)
        let t0 = utc(0);

        // Create tool calls with same name but different inputs
        let mut tool1 = make_tool_use_block("trace1", "span1", "call_1", "search", t0);
        tool1.content = ContentBlock::ToolUse {
            id: Some("call_1".to_string()),
            name: "search".to_string(),
            input: serde_json::json!({"query": "cats"}),
        };

        let mut tool2 = make_tool_use_block("trace1", "span1", "call_2", "search", t0);
        tool2.content = ContentBlock::ToolUse {
            id: Some("call_2".to_string()),
            name: "search".to_string(),
            input: serde_json::json!({"query": "dogs"}),
        };

        let span_timestamps = HashMap::from([(
            "span1".to_string(),
            SpanTimestamps {
                span_start: t0,
                span_end: Some(t0),
            },
        )]);

        let result = process_dedup(vec![tool1, tool2], span_timestamps);

        // Both should be kept (different inputs = different identities)
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_streaming_chunks_deduped() {
        // Same content appearing multiple times (streaming) should be deduped
        let t0 = utc(0);
        let t1 = utc(1);
        let t2 = utc(2);

        let chunk1 = make_test_block("trace1", "span1", ChatRole::Assistant, "Hello world", t0);
        let chunk2 = make_test_block("trace1", "span1", ChatRole::Assistant, "Hello world", t1);
        let mut chunk3 = make_test_block("trace1", "span1", ChatRole::Assistant, "Hello world", t2);
        chunk3.finish_reason = Some(FinishReason::Stop);

        let span_timestamps = HashMap::from([(
            "span1".to_string(),
            SpanTimestamps {
                span_start: t0,
                span_end: Some(t2),
            },
        )]);

        let result = process_dedup(vec![chunk1, chunk2, chunk3], span_timestamps);

        // Should be deduped to single message (with finish_reason = highest quality)
        assert_eq!(result.len(), 1);
        assert!(result[0].finish_reason.is_some());
    }

    #[test]
    fn test_different_roles_same_content_not_deduped() {
        // Same content but different roles should NOT be deduped
        let t0 = utc(0);

        let user_msg = make_test_block("trace1", "span1", ChatRole::User, "Hello", t0);
        let mut assistant_msg =
            make_test_block("trace1", "span1", ChatRole::Assistant, "Hello", t0);
        assistant_msg.finish_reason = Some(FinishReason::Stop);

        let span_timestamps = HashMap::from([(
            "span1".to_string(),
            SpanTimestamps {
                span_start: t0,
                span_end: Some(t0),
            },
        )]);

        let result = process_dedup(vec![user_msg, assistant_msg], span_timestamps);

        // Both should be kept (different roles = different identities)
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_history_at_multiple_depths_deduped() {
        // User message appears at root, child, and grandchild spans
        // Should be deduped to the earliest occurrence
        let t0 = utc(0);
        let t5 = utc(5);
        let t10 = utc(10);

        let root_msg = make_test_block("trace1", "root", ChatRole::User, "Hello", t0);
        let child_msg = make_test_block("trace1", "child", ChatRole::User, "Hello", t5);
        let grandchild_msg = make_test_block("trace1", "grandchild", ChatRole::User, "Hello", t10);

        let span_timestamps = HashMap::from([
            (
                "root".to_string(),
                SpanTimestamps {
                    span_start: t0,
                    span_end: Some(t0),
                },
            ),
            (
                "child".to_string(),
                SpanTimestamps {
                    span_start: t5,
                    span_end: Some(t5),
                },
            ),
            (
                "grandchild".to_string(),
                SpanTimestamps {
                    span_start: t10,
                    span_end: Some(t10),
                },
            ),
        ]);

        let result = process_dedup(vec![root_msg, child_msg, grandchild_msg], span_timestamps);

        // Should be deduped to single message from root span
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].span_id, "root");
    }

    #[test]
    fn test_full_tool_chain_ordering() {
        // Complete tool chain: User -> Assistant+ToolUse -> ToolResult -> Assistant
        // Each step in separate spans with proper timestamps
        let t0 = utc(0);
        let t1 = utc(1);
        let t2 = utc(2);
        let t3 = utc(3);

        let user = make_test_block("trace1", "span_user", ChatRole::User, "Search for cats", t0);
        let tool_use = make_tool_use_block("trace1", "span_tool_use", "call_1", "search", t1);
        let tool_result =
            make_tool_result_block("trace1", "span_tool_result", "call_1", "Found cats", t2);

        let mut final_response = make_test_block(
            "trace1",
            "span_final",
            ChatRole::Assistant,
            "Here are the cats",
            t3,
        );
        final_response.finish_reason = Some(FinishReason::Stop);

        // Each span has its own timestamps - OUTPUT uses span_end
        let span_timestamps = HashMap::from([
            (
                "span_user".to_string(),
                SpanTimestamps {
                    span_start: t0,
                    span_end: Some(t0),
                },
            ),
            (
                "span_tool_use".to_string(),
                SpanTimestamps {
                    span_start: t1,
                    span_end: Some(t1), // Tool use span ends at t1
                },
            ),
            (
                "span_tool_result".to_string(),
                SpanTimestamps {
                    span_start: t2,
                    span_end: Some(t2),
                },
            ),
            (
                "span_final".to_string(),
                SpanTimestamps {
                    span_start: t3,
                    span_end: Some(t3),
                },
            ),
        ]);

        // Process in random order
        let result = process_dedup(
            vec![final_response, tool_result, user.clone(), tool_use.clone()],
            span_timestamps,
        );

        // Should be in correct order: User -> ToolUse -> ToolResult -> Final response
        assert_eq!(result.len(), 4);
        assert_eq!(result[0].role, ChatRole::User);
        assert_eq!(result[1].entry_type, "tool_use");
        assert_eq!(result[2].entry_type, "tool_result");
        assert_eq!(result[3].role, ChatRole::Assistant);
        assert!(matches!(
            result[3].content,
            ContentBlock::Text { ref text } if text == "Here are the cats"
        ));
    }

    #[test]
    fn test_uses_span_end_field_on_test_helpers() {
        let t0 = utc(0);

        // User message uses event_time (uses_span_end = false)
        let user = make_test_block("trace1", "span1", ChatRole::User, "Hello", t0);
        assert!(!user.uses_span_end);

        // ToolUse uses event_time (uses_span_end = false) - the decision to call
        // a tool happens DURING generation, not at completion
        let tool_use = make_tool_use_block("trace1", "span1", "call_1", "search", t0);
        assert!(!tool_use.uses_span_end);

        // ToolResult uses event_time (uses_span_end = false) unless from tool span
        let tool_result = make_tool_result_block("trace1", "span1", "call_1", "result", t0);
        assert!(!tool_result.uses_span_end);
    }

    #[test]
    fn test_quality_scoring() {
        let t0 = utc(0);

        // Base block
        let base = make_test_block("trace1", "span1", ChatRole::Assistant, "Hello", t0);
        let base_quality = compute_quality(&base);

        // Block with finish_reason has higher quality
        let mut with_finish = base.clone();
        with_finish.finish_reason = Some(FinishReason::Stop);
        let with_finish_quality = compute_quality(&with_finish);
        assert!(with_finish_quality > base_quality);

        // Block with model info has higher quality
        let mut with_model = base.clone();
        with_model.model = Some("gpt-4".to_string());
        let with_model_quality = compute_quality(&with_model);
        assert!(with_model_quality > base_quality);

        // Event source has higher quality than attribute
        let mut from_event = base.clone();
        from_event.source_type = "event".to_string();
        let mut from_attribute = base.clone();
        from_attribute.source_type = "attribute".to_string();
        assert!(compute_quality(&from_event) > compute_quality(&from_attribute));
    }
}
