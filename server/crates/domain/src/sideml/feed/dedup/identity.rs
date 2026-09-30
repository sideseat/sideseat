use super::*;

mod agent_occurrence;
mod plain_occurrence;

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
///   Caught by the independent content-hash deduplication in `history.rs`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(in crate::sideml::feed) enum MessageIdentity {
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
        // History re-sends with regenerated IDs are caught by content-hash deduplication.
        if let ContentBlock::ToolResult {
            tool_use_id,
            name,
            content,
            is_error,
        } = &block.content
        {
            let identity_hash = match tool_use_id {
                Some(tid) if !tid.is_empty() => compute_tool_use_id_hash(tid),
                // Name and error flag, not content alone: without them two tools both reporting
                // "ok" were one message, and so were a success and a failure with matching text.
                _ => compute_tool_result_hash(name.as_deref(), *is_error, content),
            };
            return Self::ToolResult {
                trace_id: block.trace_id.clone(),
                identity_hash,
            };
        }

        // An exception is a fact *about a span*, so two spans' exceptions are two occurrences.
        //
        // This block is not read out of a payload at all - it is composed from the span's own
        // `exception_*` fields (`compose_error_text`), which is why its identity may safely carry the
        // span: there is no history re-send of a span's exception to collapse against, and a
        // re-delivered span keeps its span id, so re-delivery still collapses.
        //
        // Without it, `openai-agents/image_gen` reported **one** error where three separate
        // `generate_image` executions had each failed with the same message: the three tool results
        // survived, the three exceptions became one, and the trace showed three failures and one
        // explanation. The goldens had recorded that as correct, and `assert_no_duplicates` would have
        // reported the repair as a defect - which is exactly the false-equivalence shape the design
        // record calls the hardest to detect, found here in committed data.
        if block.category == MessageCategory::Exception {
            return Self::Regular {
                trace_id: block.trace_id.clone(),
                role: block.role,
                semantic_hash: {
                    let mut hasher = DefaultHasher::new();
                    block.span_id.hash(&mut hasher);
                    block.content_hash.hash(&mut hasher);
                    hasher.finish()
                },
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

/// How many *other* calls of the same shape precede this one in its own response.
///
/// A tool call's identity deliberately ignores the provider's call id, because a framework re-sending
/// its history regenerates ids and the same call would otherwise appear twice. The cost was that a
/// model asking for the same thing twice in one response - two `generate_image` calls with the same
/// prompt, distinct ids - collapsed into one, and its second result was left answering a call that
/// was no longer there.
///
/// Within one response, though, distinct ids are unambiguous: a provider does not issue two ids for
/// one call. So each call takes the rank of its id among the same-shaped calls of that response, and
/// that rank joins its identity. A single call ranks 0 and behaves exactly as before; a re-send of a
/// pair ranks 0 and 1 again, whatever the ids were regenerated to, so the pair still collapses onto
/// the pair rather than into one call.
///
/// A response is `(trace, span, source)` - the event or attribute the blocks arrived in. Not the
/// message index: normalisation gives every tool call a message of its own, so two calls from one
/// event already have different indices, and keying on them would put each call in a bucket of one.
/// The source is what keeps a span's input array separate from its output event, which is where the
/// same call legitimately appears twice.
///
/// A tool *result* inherits the rank of the call it answers, so two results of two identical calls
/// stay two. Everything else ranks 0: without an id there is no evidence of a genuine repeat, and
/// treating repeated text as two messages would undo the history collapsing this pipeline exists for.
/// What identifies one call among same-shaped calls of a response.
#[derive(PartialEq, Eq, Hash)]
enum CallKey<'a> {
    /// The provider's id, which is proof on its own: no provider issues two ids for one call.
    Id(&'a str),
    /// Where the call sat, used only where the carrier's structure is evidence of multiplicity.
    Position(&'a PositionPath),
    /// Nothing distinguishes it - accumulated state with no ids, where two positions describe one
    /// call. Every such call of a response collapses onto the first.
    Indistinct,
}

/// Which evidence decides whether two same-shaped calls are two calls.
///
/// An id wins unless this carrier explicitly says positions prove occurrences and it contains that
/// same id more than once. The latter is how a conversation snapshot represents a producer that
/// reuses ids across turns: each position is a distinct historical call.
fn call_key<'a>(
    block: &'a BlockEntry,
    id: Option<&'a str>,
    repeated_id_in_response: bool,
) -> CallKey<'a> {
    let semantics = crate::sideml::carrier::semantics_for_context(&block.carrier_context());
    if let Some(id) = id.filter(|s| !s.is_empty())
        && (!repeated_id_in_response || !semantics.position_proves_distinct_occurrence)
    {
        return CallKey::Id(id);
    }
    if semantics.position_proves_distinct_occurrence {
        CallKey::Position(&block.position)
    } else {
        CallKey::Indistinct
    }
}

/// Bucket key for the response-scoped position map: `(trace, span, source, event, attribute, shape)`.
type ResponseKey<'a> = (
    &'a str,
    &'a str,
    &'a str,
    Option<&'a str>,
    Option<&'a str>,
    u64,
);

type CarrierKey<'a> = (&'a str, &'a str, &'a str, Option<&'a str>, Option<&'a str>);
type CallsByCarrier<'a> = HashMap<(CarrierKey<'a>, &'a str), Vec<((i32, i32), u32)>>;
type SeenCalls<'a> = HashMap<
    ResponseKey<'a>,
    std::collections::HashSet<(Option<&'a str>, Option<&'a PositionPath>)>,
>;

pub(in crate::sideml::feed) fn call_repeat_ordinals(blocks: &[BlockEntry]) -> Vec<u32> {
    // (trace, span, source, call shape) -> the ids seen, in order of first appearance.

    // What tells two same-shaped calls of one response apart: the provider's id where there is one,
    // and the position otherwise.
    //
    // The id has to win when present. A framework's accumulated state re-lists its own messages -
    // LangChain's `output.value` carries each tool call twice - and those copies sit at different
    // positions while describing one call. Ranking by position alone turned every such echo into a
    // second call, which the goldens' duplicate invariant caught. The position still earns its place
    // where no id was sent: two identical calls with no ids are otherwise indistinguishable.
    // The rank assigned to each key, not a list to search. As a `Vec` the lookup was
    // `iter().position(..)`, so a response - or, for id-bearing calls, a whole trace - with N same-shaped
    // calls cost N^2 comparisons to rank them. A map from key to rank gives the same answer by construction:
    // the rank *is* the order of first appearance, which is the map's size when the key is inserted.
    let mut keys_by_response: HashMap<ResponseKey<'_>, HashMap<CallKey<'_>, u32>> = HashMap::new();
    // (trace, call id) -> that call's rank, for the results that answer it.
    let mut rank_by_call: HashMap<(&str, &str), u32> = HashMap::new();
    // Calls with a reused id in one carrier, ordered so each result can inherit the nearest
    // preceding call's rank rather than the first rank ever seen for that id.
    let mut calls_by_carrier: CallsByCarrier<'_> = HashMap::new();

    // How many calls of one shape a single response lists, which is what decides *which* evidence
    // separates two same-shaped calls - see `rank_scope`.
    let mut calls_seen: SeenCalls<'_> = HashMap::new();
    let mut ids_seen: HashMap<
        (ResponseKey<'_>, &str),
        std::collections::HashSet<Option<&PositionPath>>,
    > = HashMap::new();
    for block in blocks {
        if let ContentBlock::ToolUse { id, name, input } = &block.content {
            let response = response_scope(block, compute_tool_call_hash(name, input));
            let id = id.as_deref().filter(|id| !id.is_empty());
            let position = crate::sideml::carrier::semantics_for_context(&block.carrier_context())
                .position_proves_distinct_occurrence
                .then_some(&block.position);
            calls_seen
                .entry(response)
                .or_default()
                .insert((id, position));
            if let Some(id) = id {
                ids_seen.entry((response, id)).or_default().insert(position);
            }
        }
    }
    let shape_count = calls_seen
        .into_iter()
        .map(|(response, calls)| (response, calls.len()))
        .collect();
    let id_count = ids_seen
        .into_iter()
        .map(|(response, positions)| (response, positions.len()))
        .collect();

    for block in blocks {
        // Plain messages get an ordinal too, but only inside a carrier whose structure is evidence of
        // distinct occurrences - `gen_ai.choice`, `gen_ai.output.messages` and the other atomic emissions.
        // The model producing the same text twice in one emission is two turns; the same text repeating
        // inside a conversation snapshot is a re-statement, and that read is preserved by
        // `position_proves_distinct_occurrence: false` on `SNAPSHOT` and `ACCUMULATED_STATE`.
        //
        // A shape distinct from tool-call shapes, so calls and text of the same "shape" never share a
        // bucket in the response map. Text does not carry an id, so `call_key` returns `Indistinct` in
        // any snapshot/state carrier - which keeps ADK's repeated "For context:" as one message rather
        // than becoming N.
        if let ContentBlock::ToolUse { id, name, input } = &block.content {
            let shape = compute_tool_call_hash(name, input);
            let rank = record_position(
                block,
                shape,
                &mut keys_by_response,
                &mut rank_by_call,
                id.as_deref(),
                &shape_count,
                &id_count,
            );
            if let Some(id) = id.as_deref().filter(|id| !id.is_empty()) {
                calls_by_carrier
                    .entry((carrier_scope(block), id))
                    .or_default()
                    .push(((block.message_index, block.entry_index), rank));
            }
        } else if let Some(shape) = plain_message_shape(block) {
            let semantics = crate::sideml::carrier::semantics_for_context(&block.carrier_context());
            if semantics.position_proves_distinct_occurrence {
                record_position(
                    block,
                    shape,
                    &mut keys_by_response,
                    &mut rank_by_call,
                    None,
                    &shape_count,
                    &id_count,
                );
            }
        }
    }
    for calls in calls_by_carrier.values_mut() {
        calls.sort_unstable();
    }

    let mut locally_paired_results = vec![false; blocks.len()];
    let mut ordinals: Vec<u32> = blocks
        .iter()
        .enumerate()
        .map(|(index, block)| match &block.content {
            ContentBlock::ToolUse { id, name, input } => {
                let shape = compute_tool_call_hash(name, input);
                lookup_position(
                    block,
                    shape,
                    &keys_by_response,
                    id.as_deref(),
                    &shape_count,
                    &id_count,
                )
            }
            ContentBlock::ToolResult { tool_use_id, .. } => tool_use_id
                .as_deref()
                .filter(|s| !s.is_empty())
                .and_then(|id| {
                    let local = rank_for_local_result(block, id, &calls_by_carrier);
                    locally_paired_results[index] = local.is_some();
                    local.or_else(|| rank_by_call.get(&(block.trace_id.as_str(), id)).copied())
                })
                .unwrap_or(0),
            _ => {
                let Some(shape) = plain_message_shape(block) else {
                    return 0;
                };
                let semantics =
                    crate::sideml::carrier::semantics_for_context(&block.carrier_context());
                if !semantics.position_proves_distinct_occurrence {
                    return 0;
                }
                lookup_position(
                    block,
                    shape,
                    &keys_by_response,
                    None,
                    &shape_count,
                    &id_count,
                )
            }
        })
        .collect();
    apply_reused_execution_id_ordinals(
        blocks,
        &shape_count,
        &locally_paired_results,
        &mut ordinals,
    );
    agent_occurrence::align_tool_ordinals_with_agent_invocations(blocks, &mut ordinals);
    plain_occurrence::apply_plain_occurrence_ordinals(blocks, &mut ordinals);
    ordinals
}

/// Separate executions when a producer reuses one call id across distinct model responses.
///
/// Most providers make an id unique, but ADK's LiteLLM adapter derives one from the function call
/// shape and can emit the same id again on a later turn. The response carrier still proves that each
/// non-relisting occurrence is an execution. Rank those occurrences by time, then attach replayed
/// calls and results to the latest execution already visible at their timestamp.
fn apply_reused_execution_id_ordinals(
    blocks: &[BlockEntry],
    shape_count: &HashMap<ResponseKey<'_>, usize>,
    locally_paired_results: &[bool],
    ordinals: &mut [u32],
) {
    type Group<'a> = (&'a str, u64);
    type Occurrence<'a> = (
        DateTime<Utc>,
        &'a str,
        &'a str,
        Option<&'a str>,
        Option<&'a str>,
    );
    type RankedExecution<'a> = (Occurrence<'a>, usize, u32);
    type ExecutionsById<'a> = HashMap<(&'a str, &'a str), Vec<RankedExecution<'a>>>;

    let base_ordinals = ordinals.to_vec();
    let mut executions: HashMap<Group<'_>, Vec<(usize, &str, Occurrence<'_>)>> = HashMap::new();
    let mut prior_calls: HashMap<Group<'_>, Vec<(usize, &str)>> = HashMap::new();
    let mut prior_results: HashMap<(&str, &str), Vec<usize>> = HashMap::new();
    let mut seen_execution_anchors: std::collections::HashSet<(&str, &str, &str, u64, u32)> =
        std::collections::HashSet::new();
    for (index, block) in blocks.iter().enumerate() {
        if let ContentBlock::ToolResult {
            tool_use_id: Some(id),
            ..
        } = &block.content
            && !id.is_empty()
            && is_generation_input_observation(block)
        {
            prior_results
                .entry((block.trace_id.as_str(), id))
                .or_default()
                .push(index);
        }
    }
    for (index, block) in blocks.iter().enumerate() {
        let ContentBlock::ToolUse { id, name, input } = &block.content else {
            continue;
        };
        let Some(id) = id.as_deref().filter(|id| !id.is_empty()) else {
            continue;
        };
        let shape = compute_tool_call_hash(name, input);
        let response = response_scope(block, shape);
        let group = (block.trace_id.as_str(), shape);
        if !is_generation_execution_anchor(block) {
            // Only a model request proves this call already happened. Parent agents and tool spans
            // often publish their completed state on a span whose *start* precedes the generation;
            // treating that timestamp as occurrence time turns one child execution into two.
            if is_generation_input_observation(block) {
                prior_calls.entry(group).or_default().push((index, id));
            }
            continue;
        }
        if shape_count.get(&response).copied().unwrap_or(1) > 1 {
            continue;
        }
        // One generation can publish its response through more than one convention carrier.
        // Bedrock writes the same call to both `gen_ai.output.messages` and `gen_ai.choice`; that is
        // two witnesses of one model execution, not two executions. A true repeated call inside one
        // response already has a different base ordinal, so it remains distinct.
        if !seen_execution_anchors.insert((
            block.trace_id.as_str(),
            block.span_id.as_str(),
            id,
            shape,
            base_ordinals[index],
        )) {
            continue;
        }
        executions.entry(group).or_default().push((
            index,
            id,
            (
                block.timestamp,
                block.span_id.as_str(),
                block.source_type.as_str(),
                block.event_name.as_deref(),
                block.source_attribute.as_deref(),
            ),
        ));
    }

    let mut execution_ranks: HashMap<usize, u32> = HashMap::new();
    let mut ranks_by_id: ExecutionsById<'_> = HashMap::new();
    for (group, entries) in &mut executions {
        entries.sort_by_key(|(_, _, occurrence)| *occurrence);
        let mut previous_rank_by_id: HashMap<&str, u32> = HashMap::new();
        for &(index, id, occurrence) in entries.iter() {
            let mut prior_rank = previous_rank_by_id.get(id).copied();
            for &(prior, prior_id) in prior_calls.get(group).into_iter().flatten() {
                if prior_id == id && observation_precedes(blocks, prior, index) {
                    prior_rank = Some(prior_rank.unwrap_or(0).max(base_ordinals[prior]));
                }
            }
            for &prior in prior_results.get(&(group.0, id)).into_iter().flatten() {
                if observation_precedes(blocks, prior, index) {
                    prior_rank = Some(prior_rank.unwrap_or(0).max(base_ordinals[prior]));
                }
            }
            let rank = prior_rank
                .map(|prior| base_ordinals[index].max(prior.saturating_add(1)))
                .unwrap_or(base_ordinals[index]);
            execution_ranks.insert(index, rank);
            ranks_by_id
                .entry((blocks[index].trace_id.as_str(), id))
                .or_default()
                .push((occurrence, index, rank));
            previous_rank_by_id.insert(id, rank);
        }
    }
    for events in ranks_by_id.values_mut() {
        events.sort_unstable();
    }

    for (index, block) in blocks.iter().enumerate() {
        if let Some(rank) = execution_ranks.get(&index) {
            ordinals[index] = *rank;
            continue;
        }
        let (id, prior_candidate) = match &block.content {
            ContentBlock::ToolUse {
                id: Some(id),
                name,
                input,
            } if !id.is_empty() => {
                let group = (block.trace_id.as_str(), compute_tool_call_hash(name, input));
                let semantics =
                    crate::sideml::carrier::semantics_for_context(&block.carrier_context());
                (
                    Some(id.as_str()),
                    Some(
                        (block.is_history || semantics.may_restate_prior_observations)
                            && prior_calls.get(&group).is_some_and(|calls| {
                                calls.iter().any(|(member, _)| *member == index)
                            }),
                    ),
                )
            }
            ContentBlock::ToolResult {
                tool_use_id: Some(id),
                ..
            } if !id.is_empty() => (
                Some(id.as_str()),
                Some(
                    prior_results
                        .get(&(block.trace_id.as_str(), id.as_str()))
                        .is_some_and(|results| results.contains(&index)),
                ),
            ),
            _ => (None, None),
        };
        let Some(events) = id.and_then(|id| ranks_by_id.get(&(block.trace_id.as_str(), id))) else {
            continue;
        };
        let before_first = events
            .first()
            .is_some_and(|(_, execution, _)| observation_precedes(blocks, index, *execution));
        let position_proves_occurrence =
            crate::sideml::carrier::semantics_for_context(&block.carrier_context())
                .position_proves_distinct_occurrence;
        let preserve_prior_rank = prior_candidate.unwrap_or(false)
            && (before_first
                || (block.is_tool_use() && position_proves_occurrence)
                || locally_paired_results.get(index).copied().unwrap_or(false));
        if preserve_prior_rank {
            continue;
        }
        ordinals[index] = events
            .iter()
            .rev()
            .find(|(_, execution, _)| observation_precedes(blocks, *execution, index))
            .or_else(|| events.first())
            .map_or(0, |(_, _, rank)| *rank);
    }
}

/// A per-message shape that is not a tool call.
///
/// Used to key the plain-message ordinal within an atomic emission. Tool calls have their own shape and
/// their own bucket; tool results take their call's ordinal; and non-emission carriers (snapshots,
/// accumulated state) do not enter this bucket at all - see `call_repeat_ordinals`.
fn plain_message_shape(block: &BlockEntry) -> Option<u64> {
    match &block.content {
        ContentBlock::ToolUse { .. } | ContentBlock::ToolResult { .. } => None,
        _ => {
            let mut hasher = DefaultHasher::new();
            "plain_message".hash(&mut hasher);
            block.role.hash(&mut hasher);
            Some(compute_semantic_hash(&block.content) ^ hasher.finish())
        }
    }
}

/// Record a block's position among same-shape blocks of its response, and remember the id-to-rank map.
fn record_position<'a>(
    block: &'a BlockEntry,
    shape: u64,
    keys_by_response: &mut HashMap<ResponseKey<'a>, HashMap<CallKey<'a>, u32>>,
    rank_by_call: &mut HashMap<(&'a str, &'a str), u32>,
    id: Option<&'a str>,
    shape_count: &HashMap<ResponseKey<'a>, usize>,
    id_count: &HashMap<(ResponseKey<'a>, &'a str), usize>,
) -> u32 {
    let repeated_id = id.is_some_and(|id| {
        id_count
            .get(&(response_scope(block, shape), id))
            .copied()
            .unwrap_or(0)
            > 1
    });
    // An id-bearing call ranks across its **trace**, not within one response.
    //
    // Two executions of the same call in one trace arrive on different spans, and each span holds one
    // of them - so a per-response rank gave both ordinal 0 and dedup merged them. Measured on
    // `agent-framework/tool_use`: four executions with four distinct provider ids, two of them the same
    // NYC request, came back as **three** calls beside **two** NYC answers. Same shape in
    // `openai-agents/tool_use`, both `mcp_tools` suites and both `subagents` suites.
    //
    // The provider's id is what separates them, and it is trustworthy for this: a re-listing of one
    // execution repeats the *same* id (which is why the copies across three spans still collapse), while
    // a second execution gets a new one. The id is still not the *identity* - a history re-send may
    // regenerate it - which is why it decides the rank rather than the key.
    //
    // Position keeps its job where no id was sent, and only there: two identical id-less calls of one
    // response are otherwise indistinguishable, and ranking them across a trace would turn a snapshot's
    // echo into a second call.
    let seen = keys_by_response
        .entry(rank_scope(block, shape, id, shape_count, repeated_id))
        .or_default();
    let key = call_key(block, id, repeated_id);
    let next_rank = seen.len() as u32;
    let rank = *seen.entry(key).or_insert(next_rank);
    if let Some(id) = id.filter(|s| !s.is_empty()) {
        rank_by_call
            .entry((block.trace_id.as_str(), id))
            .or_insert(rank);
    }
    rank
}

/// The scope a repeat rank is counted within - see the note in `record_position`. Defined once, because
/// a recording site and a lookup site that disagree would silently rank everything zero.
fn response_scope<'a>(block: &'a BlockEntry, shape: u64) -> ResponseKey<'a> {
    (
        block.trace_id.as_str(),
        block.span_id.as_str(),
        block.source_type.as_str(),
        block.event_name.as_deref(),
        block.source_attribute.as_deref(),
        shape,
    )
}

fn carrier_scope(block: &BlockEntry) -> CarrierKey<'_> {
    (
        block.trace_id.as_str(),
        block.span_id.as_str(),
        block.source_type.as_str(),
        block.event_name.as_deref(),
        block.source_attribute.as_deref(),
    )
}

fn rank_for_local_result(
    block: &BlockEntry,
    id: &str,
    calls_by_carrier: &CallsByCarrier<'_>,
) -> Option<u32> {
    let position = (block.message_index, block.entry_index);
    calls_by_carrier
        .get(&(carrier_scope(block), id))
        .and_then(|calls| {
            calls
                .iter()
                .rev()
                .find(|(call_position, _)| *call_position < position)
                .or_else(|| calls.first())
        })
        .map(|(_, rank)| *rank)
}

fn rank_scope<'a>(
    block: &'a BlockEntry,
    shape: u64,
    id: Option<&'a str>,
    shape_count: &HashMap<ResponseKey<'a>, usize>,
    repeated_id: bool,
) -> ResponseKey<'a> {
    // Only an id from a carrier that **cannot be a re-send** ranks trace-wide. A history re-send
    // regenerates the id, so trusting a snapshot's id turns one execution's echo into a second
    // execution. The shape-multiplicity test below covers the *pair* form (a re-sent pair lists its
    // shape twice in one response); it cannot cover a **single** call re-sent once, which lists its
    // shape once in each response and so looks exactly like two executions -
    // `a_resent_single_call_with_a_regenerated_id_is_still_one_call` reproduced the duplicate.
    //
    // The fact is the carrier's, deliberately, not the block's `is_history` flag: duplicate
    // detection groups by these very ordinals, so a flag it sets cannot gate the rank that decides
    // whether it fires - the two executions this rank exists to keep (`agent-framework/tool_use` and
    // the five suites beside it) all report their calls through *emission* carriers, and a re-send by
    // definition arrives through one that `may_restate_prior_observations`. That fact and not
    // `may_contain_framework_state`: the question here is whether the id could have been *regenerated*,
    // which is a property of replaying an earlier observation, not of holding a scratchpad.
    let id_is_execution_evidence = id_is_execution_evidence(block);
    let response = response_scope(block, shape);
    let lists_shape_once = shape_count.get(&response).copied().unwrap_or(1) <= 1;
    if id_is_execution_evidence
        && id.is_some_and(|s| !s.is_empty())
        && lists_shape_once
        && !repeated_id
    {
        (block.trace_id.as_str(), "", "", None, None, shape)
    } else {
        response
    }
}

fn id_is_execution_evidence(block: &BlockEntry) -> bool {
    block.promoted_to_span_output
        || !crate::sideml::carrier::semantics_for_context(&block.carrier_context())
            .may_restate_prior_observations
}

fn is_generation_execution_anchor(block: &BlockEntry) -> bool {
    block.observation_type.as_deref() == Some("generation")
        && block.is_output_source()
        && id_is_execution_evidence(block)
}

fn is_generation_input_observation(block: &BlockEntry) -> bool {
    block.observation_type.as_deref() == Some("generation") && block.is_input_source()
}

fn observation_precedes(blocks: &[BlockEntry], observation: usize, execution: usize) -> bool {
    let observed = &blocks[observation];
    let executed = &blocks[execution];
    observed.timestamp < executed.timestamp
        || (observed.timestamp == executed.timestamp
            && observed.span_id == executed.span_id
            && (observed.message_index, observed.entry_index)
                < (executed.message_index, executed.entry_index))
}

fn lookup_position<'a>(
    block: &'a BlockEntry,
    shape: u64,
    keys_by_response: &HashMap<ResponseKey<'a>, HashMap<CallKey<'a>, u32>>,
    id: Option<&'a str>,
    shape_count: &HashMap<ResponseKey<'a>, usize>,
    id_count: &HashMap<(ResponseKey<'a>, &'a str), usize>,
) -> u32 {
    let repeated_id = id.is_some_and(|id| {
        id_count
            .get(&(response_scope(block, shape), id))
            .copied()
            .unwrap_or(0)
            > 1
    });
    let key = call_key(block, id, repeated_id);
    keys_by_response
        .get(&rank_scope(block, shape, id, shape_count, repeated_id))
        .and_then(|seen| seen.get(&key).copied())
        .unwrap_or(0)
}

/// Compute hash for tool call identity (name + input).
pub(in crate::sideml::feed) fn compute_tool_call_hash(
    name: &str,
    input: &serde_json::Value,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    "tool_call".hash(&mut hasher);
    name.hash(&mut hasher);
    hash_json_into(input, &mut hasher);
    hasher.finish()
}

/// Compute hash for tool result identity (content).
/// Used as fallback when tool_use_id is absent.
pub(super) fn compute_tool_result_hash(
    name: Option<&str>,
    is_error: bool,
    content: &serde_json::Value,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    "tool_result".hash(&mut hasher);
    name.hash(&mut hasher);
    is_error.hash(&mut hasher);
    hash_tool_result_content_into(content, &mut hasher);
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

/// Normalize JSON for consistent hashing: sort object keys.
pub(in crate::sideml::feed) fn normalize_json_for_hash(value: &serde_json::Value) -> String {
    normalize_json(value, EmptyMembers::Keep)
}

/// As [`normalize_json_for_hash`], but treating a member with no value as absent.
///
/// For a **structured answer only**. Filling a schema adds the fields the model did not produce,
/// as `null`, `[]`, `{}` or `""`, so an SDK that reports both the model's raw object and its
/// schema-shaped one - Vercel's `generateObject` puts the raw object on the inner span and the
/// normalized one on the outer - emits the same answer twice with different bytes, and both
/// reached the feed as separate assistant messages.
///
/// Deliberately not applied to tool inputs or tool results, where an explicitly empty collection
/// is a different answer from a missing one: a search result of `{"results": []}` says "no
/// matches" and `{}` says nothing, and collapsing those would drop a real message. The
/// distinction only stops mattering once a schema has supplied the empty value itself.
///
/// Only identity is affected: what the API returns is still the block's own content.
#[cfg(test)]
pub(in crate::sideml::feed) fn normalize_structured_json_for_hash(
    value: &serde_json::Value,
) -> String {
    normalize_json(value, EmptyMembers::Drop)
}

/// Whether object members carrying no value take part in identity.
#[derive(Clone, Copy, PartialEq)]
enum EmptyMembers {
    Keep,
    Drop,
}

fn normalize_json(value: &serde_json::Value, empty: EmptyMembers) -> String {
    use serde_json::Value as JsonValue;
    match value {
        JsonValue::Object(map) => {
            let mut pairs: Vec<_> = map
                .iter()
                .filter(|(_, v)| empty == EmptyMembers::Keep || !is_empty_json_member(v))
                .collect();
            pairs.sort_by_key(|(k, _)| *k);
            let sorted: Vec<String> = pairs
                .iter()
                .map(|(k, v)| format!("{}:{}", k, normalize_json(v, empty)))
                .collect();
            format!("{{{}}}", sorted.join(","))
        }
        JsonValue::Array(arr) => {
            let items: Vec<String> = arr.iter().map(|v| normalize_json(v, empty)).collect();
            format!("[{}]", items.join(","))
        }
        _ => value.to_string(),
    }
}

/// Feed a JSON value's canonical form to a hasher **without building it**.
///
/// The canonical form used to be materialised as a `String` - nested `format!` and `join` at every
/// level - purely so it could be hashed. For a tool result carrying a base64 image that copies
/// megabytes once per nesting level, and it happens twice per block (once in flatten, again in
/// history marking), which was 620 ms of a 700 ms request on `vercel-ai-js/image-gen`.
///
/// The bytes differ from the old string form, so hash *values* differ; what matters is preserved,
/// which is that equal content hashes equally and unequal content does not. Lengths are hashed
/// alongside the members so that concatenation cannot make two different shapes agree.
pub(in crate::sideml::feed) fn hash_json_into<H: Hasher>(
    value: &serde_json::Value,
    hasher: &mut H,
) {
    hash_json_streaming(value, EmptyMembers::Keep, hasher);
}

/// As [`hash_json_into`], but treating a member with no value as absent - see
/// [`normalize_structured_json_for_hash`] for why that is right for a structured answer only.
pub(in crate::sideml::feed) fn hash_structured_json_into<H: Hasher>(
    value: &serde_json::Value,
    hasher: &mut H,
) {
    hash_json_streaming(value, EmptyMembers::Drop, hasher);
}

fn hash_json_streaming<H: Hasher>(value: &serde_json::Value, empty: EmptyMembers, hasher: &mut H) {
    use serde_json::Value as JsonValue;
    // A per-kind tag, so a string cannot hash as the object whose rendering it matches.
    match value {
        JsonValue::Object(map) => {
            0u8.hash(hasher);
            let mut keys: Vec<&str> = map
                .iter()
                .filter(|(_, v)| empty == EmptyMembers::Keep || !is_empty_json_member(v))
                .map(|(k, _)| k.as_str())
                .collect();
            keys.sort_unstable();
            keys.len().hash(hasher);
            for key in keys {
                key.hash(hasher);
                if let Some(member) = map.get(key) {
                    hash_json_streaming(member, empty, hasher);
                }
            }
        }
        JsonValue::Array(arr) => {
            1u8.hash(hasher);
            arr.len().hash(hasher);
            for item in arr {
                hash_json_streaming(item, empty, hasher);
            }
        }
        // The one that matters: hashed in place, never copied or escaped.
        JsonValue::String(s) => {
            2u8.hash(hasher);
            s.hash(hasher);
        }
        JsonValue::Number(n) => {
            3u8.hash(hasher);
            n.to_string().hash(hasher);
        }
        JsonValue::Bool(b) => {
            4u8.hash(hasher);
            b.hash(hasher);
        }
        JsonValue::Null => 5u8.hash(hasher),
    }
}

/// Feed a tool result's canonical form to a hasher, unwrapping the shapes frameworks wrap them in.
///
/// A tool result arrives wrapped differently per framework - a bare string, an array of content
/// blocks, `{"json": ...}`, `{"type": "text", "text": ...}`, `{"type": "json", "data": ...}` - and two
/// wrappings of one answer must hash alike. The array-with-text branch still builds its joined text,
/// which is small by construction: the megabytes live in image blocks, which yield no text and so take
/// the streaming JSON path.
pub(in crate::sideml::feed) fn hash_tool_result_content_into<H: Hasher>(
    content: &serde_json::Value,
    hasher: &mut H,
) {
    use serde_json::Value as JsonValue;
    match content {
        JsonValue::String(s) => s.trim().hash(hasher),
        JsonValue::Array(arr) => {
            let texts: Vec<String> = arr.iter().filter_map(extract_text_from_block).collect();
            if texts.is_empty() {
                hash_json_into(content, hasher);
            } else {
                texts.join("\n").trim().hash(hasher);
            }
        }
        JsonValue::Object(obj) => {
            if let Some(inner) = obj.get("json") {
                return hash_json_into(inner, hasher);
            }
            if obj.get("type").and_then(|t| t.as_str()) == Some("text")
                && let Some(text) = obj.get("text").and_then(|t| t.as_str())
            {
                return text.trim().hash(hasher);
            }
            if obj.get("type").and_then(|t| t.as_str()) == Some("json")
                && let Some(data) = obj.get("data")
            {
                if let Some(json) = data.get("json") {
                    return hash_json_into(json, hasher);
                }
                return hash_json_into(data, hasher);
            }
            hash_json_into(content, hasher)
        }
        _ => content.to_string().hash(hasher),
    }
}

/// True for a value that carries no information: `null`, `""`, `[]`, `{}`, or a container whose
/// every member is itself empty.
fn is_empty_json_member(value: &serde_json::Value) -> bool {
    use serde_json::Value as JsonValue;
    match value {
        JsonValue::Null => true,
        JsonValue::String(s) => s.trim().is_empty(),
        JsonValue::Array(arr) => arr.iter().all(is_empty_json_member),
        JsonValue::Object(map) => map.values().all(is_empty_json_member),
        _ => false,
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
pub(in crate::sideml::feed) use super::super::compute_block_hash as compute_semantic_hash;
