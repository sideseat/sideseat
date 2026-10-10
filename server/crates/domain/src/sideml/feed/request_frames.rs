//! A framed request's view, opened with the detached frames recorded apart from it (`frames_requests`).
//!
//! A producer that records a request's frame once - a run's system instruction, on the span that started the run -
//! states a key on the frame's record and on every request it framed. The store reads the frame records of the
//! request's trace stating the request's key (`get_request_frames`), and the view opens with them:
//!
//! ```text
//! view = merge(own frame, joined frames) ++ the rest of the request's own view
//! ```
//!
//! **Merged, not prepended.** A request may carry part of its frame itself - a preview of the same instruction,
//! cut short, its first sections whole - so the joined frame and the request's own repeat each other. The merge is
//! the order-preserving union of the two sequences: their longest common subsequence, by role and content, is
//! kept once, as the request's own copy, and what only one of them holds is kept in its place, the request's
//! first where both have something between two shared blocks. Order and multiplicity survive on both sides.
//!
//! Bounded three times: the statement reads at most `REQUEST_FRAMES_MAX_RECORDS` records, at most
//! `REQUEST_FRAMES_MAX_BYTES` of them are parsed, and the merge tabulates at most `REQUEST_FRAMES_MERGE_MAX_CELLS`
//! cells. Each says so in the view's metadata when it bounds the answer.

use super::*;

use crate::observations::RawMessage;
use crate::sideml::ChatRole;
use crate::sideml::carrier::semantics_for_context;
use sideseat_core::constants::{
    REQUEST_FRAMES_MAX_BYTES, REQUEST_FRAMES_MAX_RECORDS, REQUEST_FRAMES_MERGE_MAX_CELLS,
};
use sideseat_ports::types::RequestFrameRecord;

/// The request's view with the joined frames merged in at its head.
pub(super) fn frame(view: FeedResult, frames: Vec<RequestFrameRecord>) -> FeedResult {
    let (records, truncated) = within_bounds(frames);
    let joined: Vec<BlockEntry> = blocks_of(&records).into_iter().filter(is_frame).collect();
    // Nothing joined leaves the view as it was, in its own order: only a joined frame moves the request's own.
    if joined.is_empty() {
        return FeedResult {
            metadata: FeedMetadata {
                frames_truncated: truncated,
                ..view.metadata
            },
            ..view
        };
    }
    let FeedResult {
        messages,
        metadata,
        tool_definitions,
        tool_names,
    } = view;
    let (own, rest): (Vec<BlockEntry>, Vec<BlockEntry>) = messages.into_iter().partition(is_frame);
    let (mut messages, exact) = merged(own, joined);
    messages.extend(rest);
    numbered_below_zero(&mut messages);
    let spans: HashSet<(&str, &str)> = messages
        .iter()
        .map(|block| (block.trace_id.as_str(), block.span_id.as_str()))
        .collect();
    let span_count = spans.len();
    FeedResult {
        metadata: FeedMetadata {
            block_count: messages.len(),
            span_count,
            framed_by_records: records.len(),
            frames_truncated: truncated || !exact,
            ..metadata
        },
        messages,
        tool_definitions,
        tool_names,
    }
}

/// A joined frame's messages numbered below zero, per span and in order: `-n` for the first of a span's `n`.
///
/// A block's `message_index` is public - a client groups a message's blocks by span and index - and a frame's
/// indices come from a read of its records alone, so they would repeat the indices of anything else the span
/// gives this view, such as its request in a composed thread. Below zero they repeat nothing, and they still
/// ascend in the order the frame is shown, ahead of everything else its span contributes.
fn numbered_below_zero(messages: &mut [BlockEntry]) {
    use crate::sideml::provenance::PathSegment;
    let message_of = |block: &BlockEntry| match block.position.segments() {
        [PathSegment::Key(record), PathSegment::Index(index), ..]
            if record.starts_with("frame:") =>
        {
            Some((
                block.trace_id.clone(),
                block.span_id.clone(),
                record.clone(),
                *index,
                block.message_index,
            ))
        }
        _ => None,
    };
    // A message is its record's observation and, as one observation can read as several messages, the index the
    // read gave it.
    let mut ordinal: HashMap<(String, String, String, usize, i32), i32> = HashMap::new();
    let mut count: HashMap<(String, String), i32> = HashMap::new();
    for message in messages.iter().filter_map(message_of) {
        let span = (message.0.clone(), message.1.clone());
        if let std::collections::hash_map::Entry::Vacant(slot) = ordinal.entry(message) {
            let seen = count.entry(span).or_default();
            slot.insert(*seen);
            *seen += 1;
        }
    }
    for block in messages.iter_mut() {
        if let Some(message) = message_of(block) {
            block.message_index = ordinal[&message] - count[&(message.0, message.1)];
        }
    }
}

/// The records within both bounds, in the order read, and whether either cut one. The statement reads one past
/// the record bound, so a trace with more frames than may be joined is told from one that has exactly that many,
/// and returns a record past the byte bound without its bytes; the bound is checked here again, over what came.
fn within_bounds(mut records: Vec<RequestFrameRecord>) -> (Vec<RequestFrameRecord>, bool) {
    let mut truncated = records.len() > REQUEST_FRAMES_MAX_RECORDS;
    records.truncate(REQUEST_FRAMES_MAX_RECORDS);
    let mut bytes = 0usize;
    let within = records
        .iter()
        .take_while(|record| {
            let Some(messages) = &record.messages_json else {
                return false;
            };
            bytes = bytes.saturating_add(messages.len());
            bytes <= REQUEST_FRAMES_MAX_BYTES
        })
        .count();
    truncated |= within < records.len();
    records.truncate(within);
    (records, truncated)
}

/// The blocks the frame records hold, in record order and then each record's own order. Read lightly - parsed
/// and flattened - as a thread's earlier requests are: a frame is placed by the merge, not by the view's ordering
/// passes, and each block keeps the span it was recorded on as its provenance.
///
/// One row per span, its records' messages concatenated in record order as the store joins a span's log records,
/// so two records of one span are distinct occurrences of it - their positions and indices follow on from each
/// other rather than both starting at the first. The blocks are then put back in record order: a block's position
/// names the observation it was read from in that list, and so the record the list took it from.
///
/// Each block's position is then rooted at its record - `frame:<digest>:<ordinal>`, then the message's index in
/// that record - rather than at its place in the list read here. The read joins the frame records alone, so that
/// place is not the observation's place in its span's stored list, where the span's other records come first: a
/// span that recorded a prompt before its frame holds the frame's message second. Named by its record, the block
/// names an observation that exists.
fn blocks_of(records: &[RequestFrameRecord]) -> Vec<BlockEntry> {
    let mut rows: Vec<MessageSpanRow> = Vec::new();
    let mut messages: Vec<Vec<RawMessage>> = Vec::new();
    // Per span, where each of its records' observations start in its list, and that record's place in the order.
    let mut starts: Vec<Vec<(usize, usize)>> = Vec::new();
    for (order, record) in records.iter().enumerate() {
        let parsed: Vec<RawMessage> = record
            .messages_json
            .as_deref()
            .and_then(|text| serde_json::from_str(text).ok())
            .unwrap_or_default();
        let at = match rows
            .iter()
            .position(|row| row.trace_id == record.trace_id && row.span_id == record.span_id)
        {
            Some(at) => at,
            None => {
                rows.push(frame_row(record));
                messages.push(Vec::new());
                starts.push(Vec::new());
                rows.len() - 1
            }
        };
        starts[at].push((messages[at].len(), order));
        messages[at].extend(parsed);
    }
    for (row, messages) in rows.iter_mut().zip(messages) {
        row.log_messages_json =
            serde_json::to_string(&messages).unwrap_or_else(|_| "[]".to_string());
    }
    let mut blocks = flatten_to_blocks(parse_span_rows(&rows), &build_span_hierarchy(&rows));
    use crate::sideml::provenance::PathSegment;
    // The record a block was read from, and the block's message's index in it.
    let record_of = |block: &BlockEntry| -> Option<(usize, usize)> {
        let span = rows
            .iter()
            .position(|row| row.trace_id == block.trace_id && row.span_id == block.span_id)?;
        let Some(PathSegment::Index(observation)) = block.position.segments().first() else {
            return None;
        };
        starts[span]
            .iter()
            .rev()
            .find(|(start, _)| start <= observation)
            .map(|(start, order)| (*order, observation - start))
    };
    let mut placed: Vec<(usize, BlockEntry)> = blocks
        .drain(..)
        .map(|mut block| match record_of(&block) {
            Some((order, index)) => {
                let record = &records[order];
                block.position = block.position.with_root([
                    PathSegment::Key(format!("frame:{}:{}", record.log_digest, record.ordinal)),
                    PathSegment::Index(index),
                ]);
                (order, block)
            }
            None => (usize::MAX, block),
        })
        .collect();
    // Stable, so each record's blocks keep their order.
    placed.sort_by_key(|(order, _)| *order);
    placed.into_iter().map(|(_, block)| block).collect()
}

/// A record's span as the row its log records' messages join to: its span and the first record's instant.
fn frame_row(record: &RequestFrameRecord) -> MessageSpanRow {
    MessageSpanRow {
        trace_id: record.trace_id.clone(),
        span_id: record.span_id.clone(),
        parent_span_id: None,
        span_timestamp: record.timestamp,
        span_end_timestamp: None,
        messages_json: "[]".to_string(),
        tool_definitions_json: "[]".to_string(),
        tool_names_json: "[]".to_string(),
        log_messages_json: "[]".to_string(),
        body_cache_key: None,
        model: None,
        provider: None,
        status_code: None,
        exception_type: None,
        exception_message: None,
        exception_stacktrace: None,
        input_tokens: 0,
        output_tokens: 0,
        total_tokens: 0,
        cost_total: 0.0,
        observation_type: None,
        session_id: None,
        ingested_at: record.timestamp,
        scope_name: None,
        scope_version: None,
        span_name: None,
        framework: None,
        response_model: None,
        response_id: None,
        temperature: None,
        top_p: None,
        max_tokens: None,
        finish_reasons: None,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        cost_input: 0.0,
        cost_output: 0.0,
        request_thread: String::new(),
        span_marks: 0,
        request_frame: String::new(),
    }
}

fn is_frame(block: &BlockEntry) -> bool {
    semantics_for_context(&block.carrier_context()).carrier_is_detached_request_frame
}

/// A block as the merge compares it: its role and its whole content, as one key computed once per block. The hash
/// only speeds a comparison; two blocks are the same only when the role and every field of the content are. The
/// key is the content's serialisation with every object's members in key order, and a thinking block's signature
/// beside it: the serialisation writes the signature as a mark that one is present, which is the only field it does
/// not write whole.
#[derive(PartialEq, Eq, Hash)]
struct Same {
    role: ChatRole,
    hash: u64,
    content: String,
}

impl Same {
    fn of(block: &BlockEntry) -> Self {
        use std::hash::{Hash, Hasher};
        let mut content = String::new();
        canonical(
            &serde_json::to_value(&block.content).unwrap_or_default(),
            &mut content,
        );
        if let ContentBlock::Thinking {
            signature: Some(signature),
            ..
        } = &block.content
        {
            content.push('\0');
            content.push_str(signature);
        }
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        content.hash(&mut hasher);
        Self {
            role: block.role,
            hash: hasher.finish(),
            content,
        }
    }
}

/// `value` written with every object's members in key order, so equal content is equal text whatever order its
/// members were written in.
fn canonical(value: &serde_json::Value, out: &mut String) {
    match value {
        serde_json::Value::Object(members) => {
            let mut keys: Vec<&String> = members.keys().collect();
            keys.sort();
            out.push('{');
            for (at, key) in keys.into_iter().enumerate() {
                if at > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::Value::String(key.clone()).to_string());
                out.push(':');
                canonical(&members[key], out);
            }
            out.push('}');
        }
        serde_json::Value::Array(items) => {
            out.push('[');
            for (at, item) in items.iter().enumerate() {
                if at > 0 {
                    out.push(',');
                }
                canonical(item, out);
            }
            out.push(']');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

/// The order-preserving union of the request's own frame and the joined one, and whether it is exact: their
/// longest common subsequence by role and content once, as the request's own blocks, and every other block in its
/// place - the request's before the joined frame's where both hold something between two shared blocks.
///
/// The common head and tail are matched first, which is exact for a longest common subsequence and is all a frame
/// whose preview is its own beginning needs. The rest is tabulated only within `REQUEST_FRAMES_MERGE_MAX_CELLS`;
/// past it, the shared blocks are matched greedily in order, which keeps both sides' order and every block but may
/// keep a shared block twice, and the merge says it is not exact.
fn merged(own: Vec<BlockEntry>, joined: Vec<BlockEntry>) -> (Vec<BlockEntry>, bool) {
    merged_within(own, joined, REQUEST_FRAMES_MERGE_MAX_CELLS)
}

/// [`merged`], tabulating at most `max_cells` cells.
fn merged_within(
    own: Vec<BlockEntry>,
    joined: Vec<BlockEntry>,
    max_cells: usize,
) -> (Vec<BlockEntry>, bool) {
    if joined.is_empty() {
        return (own, true);
    }
    let mine: Vec<Same> = own.iter().map(Same::of).collect();
    let theirs: Vec<Same> = joined.iter().map(Same::of).collect();
    let head = mine.iter().zip(&theirs).take_while(|(a, b)| a == b).count();
    let tail = mine[head..]
        .iter()
        .rev()
        .zip(theirs[head..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let (n, m) = (mine.len() - head - tail, theirs.len() - head - tail);
    let mut own = own;
    let own_tail = own.split_off(own.len() - tail);
    let own_middle = own.split_off(head);
    let joined_middle: Vec<BlockEntry> = joined.into_iter().skip(head).take(m).collect();
    let (mine, theirs) = (&mine[head..head + n], &theirs[head..head + m]);
    let cells = (n + 1).saturating_mul(m + 1);
    let (middle, exact) = if n == 0 || m == 0 {
        (own_middle.into_iter().chain(joined_middle).collect(), true)
    } else if cells <= max_cells {
        (interleaved(own_middle, joined_middle, mine, theirs), true)
    } else {
        (greedy(own_middle, joined_middle, mine, theirs), false)
    };
    own.extend(middle);
    own.extend(own_tail);
    (own, exact)
}

/// The exact merge of two sequences by their longest common subsequence, from one table of `mine.len() + 1` by
/// `theirs.len() + 1` cells.
fn interleaved(
    own: Vec<BlockEntry>,
    joined: Vec<BlockEntry>,
    mine: &[Same],
    theirs: &[Same],
) -> Vec<BlockEntry> {
    // `common[i * width + j]`: the longest common subsequence of `mine[i..]` and `theirs[j..]`.
    let (n, m) = (mine.len(), theirs.len());
    let width = m + 1;
    let mut common = vec![0u32; (n + 1) * width];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            common[i * width + j] = if mine[i] == theirs[j] {
                common[(i + 1) * width + j + 1] + 1
            } else {
                common[(i + 1) * width + j].max(common[i * width + j + 1])
            };
        }
    }
    let mut pairs = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if mine[i] == theirs[j] && common[i * width + j] == common[(i + 1) * width + j + 1] + 1 {
            pairs.push((i, j));
            i += 1;
            j += 1;
        } else if common[(i + 1) * width + j] >= common[i * width + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    stitched(own, joined, &pairs)
}

/// The merge past the table's bound: each joined block matched to the request's next equal block after the last
/// match, so the matches are in order on both sides.
fn greedy(
    own: Vec<BlockEntry>,
    joined: Vec<BlockEntry>,
    mine: &[Same],
    theirs: &[Same],
) -> Vec<BlockEntry> {
    let mut at: HashMap<&Same, std::collections::VecDeque<usize>> = HashMap::new();
    for (index, key) in mine.iter().enumerate() {
        at.entry(key).or_default().push_back(index);
    }
    let mut pairs = Vec::new();
    let mut from = 0;
    for (j, key) in theirs.iter().enumerate() {
        let Some(positions) = at.get_mut(key) else {
            continue;
        };
        while positions.front().is_some_and(|index| *index < from) {
            positions.pop_front();
        }
        if let Some(i) = positions.pop_front() {
            pairs.push((i, j));
            from = i + 1;
        }
    }
    stitched(own, joined, &pairs)
}

/// The two sequences as one, given the pairs matched in order on both sides: between two matches the request's
/// blocks and then the joined frame's, and each match once, as the request's block.
fn stitched(
    own: Vec<BlockEntry>,
    joined: Vec<BlockEntry>,
    pairs: &[(usize, usize)],
) -> Vec<BlockEntry> {
    let mut out = Vec::with_capacity(own.len() + joined.len());
    let mut own = own.into_iter().enumerate().peekable();
    let mut joined = joined.into_iter().enumerate().peekable();
    for &(i, j) in pairs {
        while let Some((_, block)) = own.next_if(|(index, _)| *index < i) {
            out.push(block);
        }
        while let Some((_, block)) = joined.next_if(|(index, _)| *index < j) {
            out.push(block);
        }
        out.extend(own.next().map(|(_, block)| block));
        joined.next();
    }
    out.extend(own.map(|(_, block)| block));
    out.extend(joined.map(|(_, block)| block));
    out
}

#[cfg(test)]
#[path = "request_frames_tests.rs"]
mod tests;
