use std::collections::BTreeSet;

use super::*;

/// Process spans from multiple traces with cross-trace prefix marking.
///
/// Groups rows by trace_id, sorts traces chronologically, processes each through
/// the within-trace pipeline with accumulated prefix entries from prior traces.
/// The prefix marking happens BEFORE within-trace dedup (in `process_trace_spans_core`),
/// so genuine repeated content (same content as prior trace) is preserved: the history
/// re-send copy is marked as `is_history`, while the genuine copy stays non-history
/// and wins dedup via +100 quality bonus.
///
/// # Accumulated Prefix
///
/// All non-System blocks are accumulated as `(role, content_hash)` entries.
/// Role-aware matching prevents cross-role false matches when content repeats.
/// The prefix scan handles both:
/// - **Root generation spans**: all input-source blocks, including assistant blocks, are matched directly
///   against accumulated history.
/// - **Non-root generation spans**: assistant input-source blocks are already marked as history; the prefix
///   scan consumes matching entries without marking them again.
pub(super) fn process_multi_trace_spans(
    rows: Vec<MessageSpanRow>,
    constraints: order_graph::Constraints,
) -> FeedResult {
    let trace_groups = group_and_sort_traces(rows);

    let mut accumulated = CrossTracePrefixState::default();
    let mut all_blocks: Vec<BlockEntry> = Vec::new();
    let mut all_tool_defs: Vec<serde_json::Value> = Vec::new();
    let mut all_tool_names: Vec<String> = Vec::new();
    let mut total_tokens: i64 = 0;
    let mut total_cost: f64 = 0.0;
    // One incomplete match anywhere makes the session's answer possibly-repeating, so it is reported.
    let mut replay_matching_complete = true;

    let trace_count = trace_groups.len();
    for (trace_idx, trace_rows) in trace_groups.into_iter().enumerate() {
        // The last trace's relation is never consulted, and building one is a second graph over the
        // whole trace.
        let more_traces_follow = trace_idx + 1 < trace_count;
        // Once per span, not once per row, for the same reason `compute_metadata` does it: a
        // re-ingested span is two rows in the DuckDB row set (that query reads the raw table,
        // ClickHouse reads it with FINAL), and summing rows billed the retry as a second call.
        // The session view was the last place still summing rows, so a session and the traces
        // inside it disagreed about their totals whenever a delivery had been retried.
        let mut counted: HashSet<(&str, &str)> = HashSet::new();
        let mut trace_tokens = 0i64;
        let mut trace_cost = 0.0f64;
        for row in &trace_rows {
            if counted.insert((row.trace_id.as_str(), row.span_id.as_str())) {
                trace_tokens += row.total_tokens;
                trace_cost += row.cost_total;
            }
        }

        // First trace: no prefix. Subsequent traces: pass accumulated prefix
        // for pre-dedup marking of history re-sends.
        let cross_trace_prefix = if trace_idx == 0 {
            None
        } else {
            Some(&accumulated)
        };

        let (result, transcript, relation) = reconstruct_trace(
            trace_rows,
            cross_trace_prefix,
            constraints,
            more_traces_follow,
            ReplayPolicy::Collapse,
        );

        // First trace always contributes. Subsequent traces contribute only if
        // they have new non-system content (pure replay traces are skipped).
        let has_new_content = trace_idx == 0
            || transcript
                .iter()
                .any(|b| b.role != crate::sideml::types::ChatRole::System);

        replay_matching_complete &= result.metadata.replay_matching_complete;

        if has_new_content {
            // From the transcript and its relation, never from `result.messages`: what a later trace
            // strips must not depend on how this one is presented, and the relation is exactly the part
            // a linearisation throws away.
            accumulated.push_trace(&transcript, relation);
            all_blocks.extend(result.messages);
            all_tool_defs.extend(result.tool_definitions);
            all_tool_names.extend(result.tool_names);
        }

        // Counted whether or not the trace contributed a message. Cost is what the spans in scope
        // were billed, not what survived history removal: a trace that only re-sent an earlier turn
        // still called the model, and skipping it reported a session as cheaper than it was.
        total_tokens += trace_tokens;
        total_cost += trace_cost;
    }

    let block_count = all_blocks.len();
    let span_count = all_blocks
        .iter()
        .map(|b| (&b.trace_id, &b.span_id))
        .collect::<HashSet<_>>()
        .len();
    let tool_definitions = deduplicate_tools(all_tool_defs);
    let tool_names = deduplicate_names(all_tool_names);

    FeedResult {
        messages: all_blocks,
        tool_definitions,
        tool_names,
        metadata: FeedMetadata {
            block_count,
            span_count,
            total_tokens,
            total_cost,
            // False if any trace's replay matching was cut short - the session's answer may then repeat
            // history, and saying so is the point.
            replay_matching_complete,
        },
    }
}

/// Mark input-source blocks that replay what earlier traces already showed.
///
/// Runs before `classify_blocks` so that:
/// - duplicate detection sees the marked copies as history and skips them,
///   preserving the genuine copy when content repeats.
/// - the remaining history passes layer on top correctly.
///
/// # Algorithm
///
/// 1. **Guard**: if there are no attribute-sourced input blocks, skip. Event-based frameworks (Strands
///    Python) keep original timestamps, so timestamp comparison handles their history within each trace and they
///    stay trace-independent.
/// 2. **Per-span injective match**: for each span, walk its strippable input blocks in payload order and
///    match each against a distinct prior occurrence whose position the relation permits (see
///    [`CrossTracePrefixState`]). Stop at the first block nothing matches - past that point the span is
///    sending new content, not history.
///
/// Returns whether every span's replay was matched exhaustively. `false` means a search hit its budget, so
/// some history may be shown twice - which the answer reports rather than hides.
pub(super) fn mark_cross_trace_prefix(
    blocks: &mut [BlockEntry],
    accumulated: &CrossTracePrefixState,
) -> bool {
    if accumulated.is_empty() {
        return true;
    }

    // A block is "cross-trace strippable" if it represents history re-sent to a new LLM call: input from an
    // attribute, which has no time of its own, or from an event carrier declared to replay earlier turns at
    // request time. An event carrier stamped with each turn's own time is left to timestamp comparison.
    let is_strippable = |b: &BlockEntry| {
        b.is_input_source()
            && (b.source_type == source_type::ATTRIBUTE
                || crate::sideml::carrier::declared_semantics_for_context(&b.carrier_context())
                    .is_some_and(|declared| declared.carrier_replays_across_traces))
    };

    let input_source_count = blocks.iter().filter(|b| b.is_input_source()).count();
    let strippable_input_count = blocks.iter().filter(|b| is_strippable(b)).count();
    if strippable_input_count == 0 {
        return true;
    }
    let mut replay_matching_complete = true;

    // Per span, because each generation span of a trace replays the history independently - ADK and
    // LangGraph re-send it at the start of every one, not only at the trace's start. The blocks are
    // gathered first and matched as a whole, because the choice made for one block can depend on the
    // blocks after it (see `longest_matching_prefix`).
    let mut marked = 0;
    let mut spans_scanned = 0;
    let mut span_start = 0usize;
    while span_start < blocks.len() {
        let span_id = blocks[span_start].span_id.clone();
        let mut span_end = span_start;
        while span_end < blocks.len() && blocks[span_end].span_id == span_id {
            span_end += 1;
        }
        spans_scanned += 1;

        // The span's replayable blocks, in payload order. System prompts are per-trace framing rather
        // than history, so they are transparent: skipped without ending the prefix.
        //
        // A block identical to one before it in everything that places it - payload position, carrier,
        // role, content - is that block delivered again (a retried export stores the span's row twice), not
        // a repeat inside the request: positions are unique within one payload, so only a second row can
        // repeat one. It is matched as its original rather than as a second occurrence the earlier trace
        // never showed.
        type Placement<'b> = (
            &'b PositionPath,
            &'b str,
            crate::sideml::types::ChatRole,
            Option<&'b str>,
            Option<&'b str>,
        );
        let mut originals: HashMap<Placement<'_>, usize> = HashMap::new();
        let mut copies: Vec<(usize, usize)> = Vec::new();
        let replayable: Vec<usize> = (span_start..span_end)
            .filter(|&i| {
                is_strippable(&blocks[i])
                    && blocks[i].role != crate::sideml::types::ChatRole::System
            })
            .filter(|&i| {
                if blocks[i].position.is_empty() {
                    return true;
                }
                let block = &blocks[i];
                let key = (
                    &block.position,
                    block.content_hash.as_str(),
                    block.role,
                    block.event_name.as_deref(),
                    block.source_attribute.as_deref(),
                );
                match originals.get(&key) {
                    Some(&original) => {
                        copies.push((i, original));
                        false
                    }
                    None => {
                        originals.insert(key, i);
                        true
                    }
                }
            })
            .collect();
        let identities: Vec<(crate::sideml::types::ChatRole, &str)> = replayable
            .iter()
            .map(|&i| (blocks[i].role, blocks[i].content_hash.as_str()))
            .collect();

        let (matched, exhaustive) = accumulated.longest_matching_prefix(&identities);
        replay_matching_complete &= exhaustive;
        let marked_here: HashSet<usize> = replayable.iter().take(matched.len()).copied().collect();
        let copies_marked = copies
            .iter()
            .filter(|(_, original)| marked_here.contains(original))
            .map(|&(copy, _)| copy);
        for i in marked_here
            .iter()
            .copied()
            .chain(copies_marked)
            .collect::<Vec<_>>()
        {
            blocks[i].is_history = true;
            blocks[i].is_cross_trace_history = true;
            marked += 1;
        }

        span_start = span_end;
    }

    tracing::debug!(
        accumulated_len = accumulated.len(),
        input_source_count,
        strippable_input_count,
        spans_scanned,
        marked,
        replay_matching_complete,
        "cross-trace prefix marking complete"
    );
    replay_matching_complete
}

/// Group rows by trace_id and sort trace groups chronologically.
///
/// Sort key: (min span_timestamp, min ingested_at, trace_id) - never the order the rows were read in: a
/// project page reads them newest first and a session read oldest first, and which of two simultaneous
/// traces strips the other's replay must not depend on which query asked. Traces the clocks cannot tell
/// apart are then ordered by what they carry ([`order_by_replay`]).
fn group_and_sort_traces(rows: Vec<MessageSpanRow>) -> Vec<Vec<MessageSpanRow>> {
    let mut by_trace: HashMap<String, Vec<MessageSpanRow>> = HashMap::new();
    for row in rows {
        by_trace.entry(row.trace_id.clone()).or_default().push(row);
    }

    let mut trace_groups: Vec<_> = by_trace
        .into_iter()
        .map(|(trace_id, rows)| {
            let min_ts = rows.iter().map(|r| r.span_timestamp).min().unwrap();
            let min_ingest = rows.iter().map(|r| r.ingested_at).min().unwrap();
            (trace_id, min_ts, min_ingest, rows)
        })
        .collect();

    trace_groups.sort_by(|a, b| {
        a.1.cmp(&b.1)
            .then_with(|| a.2.cmp(&b.2))
            .then_with(|| a.0.cmp(&b.0))
    });

    let mut ordered: Vec<Vec<MessageSpanRow>> = Vec::with_capacity(trace_groups.len());
    let mut tied: Vec<Vec<MessageSpanRow>> = Vec::new();
    let mut clocks = None;
    for (_, min_ts, min_ingest, rows) in trace_groups {
        if clocks != Some((min_ts, min_ingest)) {
            ordered.extend(order_by_replay(std::mem::take(&mut tied)));
            clocks = Some((min_ts, min_ingest));
        }
        tied.push(rows);
    }
    ordered.extend(order_by_replay(tied));
    ordered
}

/// Traces with the same clocks, in id order, reordered so that a trace re-sending everything another
/// carried, and more, follows it.
///
/// A later request of a conversation re-sends what the earlier ones said, so carrying another trace's
/// messages and more is evidence of coming after it; nothing weaker is. Each trace's messages are a set, so
/// a duplicated row changes nothing. Where neither carries the other, the id order stands.
fn order_by_replay(traces: Vec<Vec<MessageSpanRow>>) -> Vec<Vec<MessageSpanRow>> {
    // The comparison is pairwise, so a tie this wide - a bulk import stamped with one instant - keeps the id
    // order rather than spend quadratic work on clocks that carry no information at all.
    const WIDEST_TIE: usize = 64;
    let n = traces.len();
    if !(2..=WIDEST_TIE).contains(&n) {
        return traces;
    }
    let carried: Vec<BTreeSet<String>> = traces.iter().map(|rows| carried_messages(rows)).collect();
    let replays = |later: usize, earlier: usize| {
        !carried[earlier].is_empty()
            && carried[earlier].len() < carried[later].len()
            && carried[earlier].is_subset(&carried[later])
    };
    let mut waiting_on: Vec<usize> = (0..n)
        .map(|t| (0..n).filter(|&e| replays(t, e)).count())
        .collect();
    // Kahn's algorithm, taking the earliest id whenever several are free: the order is a function of the
    // traces alone. Proper inclusion is a strict order, so there is no cycle.
    let mut placed = vec![false; n];
    let mut order = Vec::with_capacity(n);
    while order.len() < n {
        let next = (0..n)
            .find(|&t| !placed[t] && waiting_on[t] == 0)
            .expect("proper inclusion has no cycle");
        placed[next] = true;
        order.push(next);
        for (t, waiting) in waiting_on.iter_mut().enumerate() {
            if !placed[t] && replays(t, next) {
                *waiting -= 1;
            }
        }
    }
    let mut slots: Vec<Option<Vec<MessageSpanRow>>> = traces.into_iter().map(Some).collect();
    order
        .into_iter()
        .map(|t| slots[t].take().expect("each trace is placed once"))
        .collect()
}

/// Every message a trace's rows carry, one canonical string each; a carrier holding a list contributes each
/// member.
fn carried_messages(rows: &[MessageSpanRow]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for row in rows {
        let Ok(raw) =
            serde_json::from_str::<Vec<crate::observations::RawMessage>>(&row.messages_json)
        else {
            continue;
        };
        for message in raw {
            match message.content {
                JsonValue::Array(items) => out.extend(items.iter().map(JsonValue::to_string)),
                other => {
                    out.insert(other.to_string());
                }
            }
        }
    }
    out
}

/// Process spans from multiple conversations for a feed.
///
/// Groups spans by conversation boundary (session_id or trace_id),
/// processes each conversation separately, then merges results.
pub fn process_feed(rows: Vec<MessageSpanRow>, options: &FeedOptions) -> FeedResult {
    // A trace's session is resolved from any of its rows that names one, then applied to all of
    // them.
    //
    // Reading each row's own session id split a conversation in half whenever the id is recorded on
    // the root span only, which is how several frameworks record it: the root went to the session
    // group and its children to a trace group, so history detection ran on the two halves
    // separately and had nothing to recognise a re-send against.
    // The caller's mapping first, when it supplied one: it comes from the store, so it survives the content
    // filter that may have removed every row naming the session. See `FeedOptions::session_of_trace`.
    let mut session_of_trace: HashMap<&str, &str> = options
        .session_of_trace
        .iter()
        .map(|(trace, session)| (trace.as_str(), session.as_str()))
        .collect();
    for row in &rows {
        if let Some(session) = row.session_id.as_deref().filter(|s| !s.is_empty()) {
            session_of_trace.entry(&row.trace_id).or_insert(session);
        }
    }
    // A typed key, not a formatted string. `format!("trace:{id}")` shared a namespace with real session
    // ids, so a trace whose session was literally named `trace:B` grouped with the sessionless trace B -
    // two unrelated conversations reconstructed as one, where either can strip the other's messages as
    // replayed history. Session ids come from the client, so that is a collision a caller can cause.
    #[derive(Clone, PartialEq, Eq, Hash)]
    enum Conversation {
        Session(String),
        LoneTrace(String),
    }

    let conversation_of_trace: HashMap<String, Conversation> = rows
        .iter()
        .map(|row| {
            let key = session_of_trace
                .get(row.trace_id.as_str())
                .map(|session| Conversation::Session((*session).to_string()))
                .unwrap_or_else(|| Conversation::LoneTrace(row.trace_id.clone()));
            (row.trace_id.clone(), key)
        })
        .collect();

    let mut spans_by_conversation: HashMap<Conversation, Vec<MessageSpanRow>> = HashMap::new();
    for row in rows {
        let key = conversation_of_trace
            .get(&row.trace_id)
            .cloned()
            .unwrap_or_else(|| Conversation::LoneTrace(row.trace_id.clone()));
        spans_by_conversation.entry(key).or_default().push(row);
    }

    // Process each conversation separately
    let mut all_blocks: Vec<BlockEntry> = Vec::new();
    let mut all_tool_defs: Vec<JsonValue> = Vec::new();
    let mut all_tool_names: Vec<String> = Vec::new();
    let mut total_tokens: i64 = 0;
    let mut total_cost: f64 = 0.0;
    let mut span_ids: HashSet<(String, String)> = HashSet::new();
    // One conversation's incomplete match makes the page possibly-repeating, so the page says so.
    let mut replay_matching_complete = true;

    for (_, conversation_spans) in spans_by_conversation {
        for row in &conversation_spans {
            // Once per span: a re-ingested span appears twice in the DuckDB row set, and summing
            // rows doubled the page's tokens and cost.
            if span_ids.insert((row.trace_id.clone(), row.span_id.clone())) {
                total_tokens += row.total_tokens;
                total_cost += row.cost_total;
            }
        }
        let processed = process_spans_unfiltered(conversation_spans);
        replay_matching_complete &= processed.metadata.replay_matching_complete;
        all_blocks.extend(processed.messages);
        all_tool_defs.extend(processed.tool_definitions);
        all_tool_names.extend(processed.tool_names);
    }

    let all_blocks = sort_feed_newest_first(all_blocks);

    // Deduplicate tools across conversations
    let tool_definitions = deduplicate_tools(all_tool_defs);
    let tool_names = deduplicate_names(all_tool_names);
    let block_count = all_blocks.len();

    apply_role_filter(
        FeedResult {
            messages: all_blocks,
            tool_definitions,
            tool_names,
            metadata: FeedMetadata {
                block_count,
                span_count: span_ids.len(),
                total_tokens,
                total_cost,
                replay_matching_complete,
            },
        },
        options.role.as_deref(),
    )
}

/// Order a project feed newest-first: **responses** descending, each response forward inside.
///
/// The feed is the one view that is not chronological, and that is a statement about *responses*, not
/// about blocks: a turn's intro text still precedes the call it introduces, the call still precedes
/// its result. So the newest-first part is applied by reversing the order of responses and leaving
/// each response's own order alone.
///
/// Taking each response's internal order from the reconstruction, rather than re-deriving it from
/// positions, is what lets the order resolver reach this view at all. Re-deriving it meant the feed
/// recomputed intra-response order from `(span, message_index, entry_index, after_call, hash)` - the
/// same terms the old scalar key used - so any ordering the resolver improves would have landed in the
/// three chronological views and silently vanished here.
///
/// A response is `(trace, order_time)`: `order_time` is the response's anchor, and it is `order_time`
/// rather than the displayed `timestamp` because only one of them *means* "where this sorts"
/// (`the_displayed_time_does_not_decide_the_order`). Keyed by trace as well, because two traces can
/// share an anchor and blocks of different conversations must not interleave - which is the bug the
/// previous explicit key was written to fix, where two blocks in different traces sharing a span id
/// and a time compared equal and their order followed HashMap iteration.
pub(super) fn sort_feed_newest_first(blocks: Vec<BlockEntry>) -> Vec<BlockEntry> {
    // The order the resolver produced is kept. Only the *grouping* is the feed's own.
    //
    // This function used to re-sort with a second scalar tuple - `(order_time, span, message_index,
    // entry_index, after_call, content_hash)` - which meant the resolver was not the ordering authority
    // for this view: any order it improved landed in the three chronological views and was silently
    // undone here. Measured before the change (`feed_projection_inventory`): 17 positions across six
    // feed views, of two kinds. Four were the answer text and a tool result swapped, where this sort
    // contradicted the very contract stated above - the resolver holds the call→result edge and this key
    // had only the message index. The other thirteen were two parallel calls of one response, which have
    // no semantic order at all, so the two sorts merely chose different linear extensions; the resolver's
    // tie-break at least derives from where a block was first observed, while a span id carries no
    // ordering meaning.
    //
    // Grouping is by response *identity* rather than by adjacent runs, because a valid resolved order may
    // interleave two responses and adjacency would then split one response into several runs.
    let mut groups: BTreeMap<(DateTime<Utc>, String), Vec<BlockEntry>> = BTreeMap::new();
    for block in blocks {
        groups
            .entry((block.order_time, block.trace_id.clone()))
            .or_default()
            .push(block);
    }
    groups.into_values().rev().flatten().collect()
}

/// Keep only the blocks inside a requested time window.
///
/// A window is a filter on the answer, not on the input. Applying it to the *rows* - which is what
/// passing it to the message query does for `from` - removes the earlier traces that history
/// detection and cross-trace prefix stripping read, and those stages then have nothing to
/// recognise a re-send against: a later turn's request comes back showing the whole conversation
/// again as new messages. The lower bound therefore belongs here, after the pipeline has seen the
/// context. The upper bound is still applied to the query as well, because everything after it is
/// irrelevant to what came before and there is no reason to load it.
///
/// Compares the timestamps the API returns, and is half-open: `from <= t < to`, as the queries are.
///
/// Borrows when there is no window, which is the ordinary case: a `Cow` rather than an owned return, so an
/// unwindowed read hands back the memo instead of copying every block of it to change nothing.
pub fn apply_time_window<'a>(
    result: &'a FeedResult,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
) -> Cow<'a, FeedResult> {
    if from.is_none() && to.is_none() {
        return Cow::Borrowed(result);
    }

    let messages: Vec<BlockEntry> = result
        .messages
        .iter()
        .filter(|b| from.is_none_or(|from| b.timestamp >= from))
        // Half-open at the top, matching the `timestamp_start < to` the message queries apply:
        // with `<=` here, a message exactly on the bound was returned when its span started
        // earlier and dropped when its span started on the bound too.
        .filter(|b| to.is_none_or(|to| b.timestamp < to))
        .cloned()
        .collect();
    Cow::Owned(FeedResult {
        metadata: FeedMetadata {
            block_count: messages.len(),
            span_count: distinct_span_count(&messages),
            ..result.metadata.clone()
        },
        messages,
        tool_definitions: result.tool_definitions.clone(),
        tool_names: result.tool_names.clone(),
    })
}

/// Keep only blocks whose role matches `role`, if one was requested.
///
/// Applied to the finished feed, never during flattening. The role a block reports is derived
/// from its content, not from the raw message role - a Gemini or ADK tool result arrives inside a
/// `user` message - so the filter has to see the derived role, which is why it once lived in
/// `flatten_to_blocks`. Filtering there removes blocks that later stages read:
///
/// - `role=tool` deletes the assistant `ToolUse` blocks that `correlate_tool_results` uses to give
///   an id-less result its call's id. Without the id, dedup falls back to content identity and
///   collapses two results of two different calls into one.
/// - history detection reads user and system messages to decide what is a re-send, so filtering
///   them away changes which of the *remaining* blocks are marked history.
///
/// The filter is a view over the finished feed, so it is applied to the finished feed. Block and
/// span counts are restated from the blocks that survive, so they describe the response rather
/// than the scope that was scanned. Token and cost totals are left as span-level sums: they are
/// the cost of producing the conversation, which filtering the view does not reduce.
/// [`apply_role_filter`] over a shared answer, copying only what survives.
///
/// The distinction from `apply_role_filter` is only about ownership - the predicate and the restated counts
/// are the same, and the two must stay that way, which is why this delegates the counting rather than
/// repeating it. With no role the memo is handed back untouched, which is the case that used to cost a full
/// deep clone of the answer on every read.
pub(super) fn project_role(result: Arc<FeedResult>, role: Option<&str>) -> Arc<FeedResult> {
    let Some(role) = role else {
        return result;
    };

    let messages: Vec<BlockEntry> = result
        .messages
        .iter()
        .filter(|b| b.role.as_str() == role)
        .cloned()
        .collect();

    Arc::new(FeedResult {
        metadata: FeedMetadata {
            block_count: messages.len(),
            span_count: distinct_span_count(&messages),
            ..result.metadata.clone()
        },
        messages,
        tool_definitions: result.tool_definitions.clone(),
        tool_names: result.tool_names.clone(),
    })
}

/// How many distinct `(trace, span)` pairs a set of blocks came from.
///
/// One definition, because a span count restated in two places is two answers to one question - and the
/// pair, not the span id alone: a span id is unique only within a trace, so counting ids would merge two
/// traces' same-id spans into one.
fn distinct_span_count(messages: &[BlockEntry]) -> usize {
    messages
        .iter()
        .map(|b| (&b.trace_id, &b.span_id))
        .collect::<HashSet<_>>()
        .len()
}

pub(super) fn apply_role_filter(result: FeedResult, role: Option<&str>) -> FeedResult {
    let Some(role) = role else {
        return result;
    };

    let messages: Vec<BlockEntry> = result
        .messages
        .into_iter()
        .filter(|b| b.role.as_str() == role)
        .collect();
    let span_count = messages
        .iter()
        .map(|b| (&b.trace_id, &b.span_id))
        .collect::<HashSet<_>>()
        .len();

    FeedResult {
        metadata: FeedMetadata {
            block_count: messages.len(),
            span_count,
            ..result.metadata
        },
        messages,
        ..result
    }
}
