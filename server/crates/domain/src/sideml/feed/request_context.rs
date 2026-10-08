//! A request composed from its thread, for a producer whose request spans export what each request **added**.
//!
//! A stateless model API is sent the whole conversation on every call, and such a producer's request span carries
//! only what is new since the previous request of the same conversation. Its span view would show less than the
//! call was sent, so it is composed here from the thread's earlier requests:
//!
//! ```text
//! history(1) = delta(1)
//! history(n) = align(history(p) ++ output(p) ++ calls(p -> n), delta(n))      p = n's predecessor
//! view(n)    = frame(n) ++ history(n) ++ the rest of n's own view
//! ```
//!
//! - `delta(r)` is what `r`'s delta carriers hold (`carrier_holds_request_delta`), in `r`'s own view order;
//!   `output(r)` its output carriers' blocks; `frame(r)` its detached request frame, which is its own and never
//!   inherited.
//! - `calls(p -> n)` are the tool calls `n`'s delta returns results for, found on the tool spans offered beside
//!   the thread and ordered by when those spans started. Ownership is by result, so a call another thread answers
//!   is never this thread's.
//! - `align` drops the items of a delta that **restate** the history, which a resumed process does: it restates
//!   the whole history, its user turns in one carrier and its reminders in another. Per role, then - one cursor
//!   over the delta in carrier order would leave a reminder it restates behind a user turn it does not, and show
//!   it twice - and all or nothing: a delta restates only when every role it carries begins with all of the
//!   history's items of that role. A repeated reminder or a question asked again is otherwise a new item.
//!
//! **Evidence, not a guarantee.** Nothing in such telemetry proves a thread is unbroken, so this invents nothing:
//! every composed block is one observation of the thread, kept with the span, carrier and position it came from.
//! And it composes nothing past a break it can detect - two requests sharing a sequence position with different
//! content, or a request that failed - so a view after one shows only what its own span carried.
//!
//! The cost is one pass over the thread's requests in order. Each request's blocks are read once, and the
//! history holds each composed block once, so the work and the memory are linear in the rows read - which, outside
//! a resumed process restating the history in every delta, is the size of the answer.

use super::*;

use crate::sideml::carrier::semantics_for_context;
use crate::sideml::types::ChatRole;

/// The rows a request span's view is composed from.
#[derive(Debug, Default, Clone)]
pub struct RequestContextRows {
    /// The request span's own rows.
    pub target: Vec<MessageSpanRow>,
    /// Rows of the thread's requests up to and including the target, in any order and with repeats.
    pub thread: Vec<MessageSpanRow>,
    /// Rows of tool spans that may hold the calls the thread's deltas return results for.
    pub calls: Vec<MessageSpanRow>,
}

/// One request of the thread, read as its own span view.
struct Request {
    span_id: String,
    trace_id: String,
    started: DateTime<Utc>,
    failed: bool,
    blocks: Vec<BlockEntry>,
}

/// What a block contributes to a request's composition.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    Frame,
    Delta,
    Output,
    Other,
}

fn part_of(block: &BlockEntry) -> Part {
    let semantics = semantics_for_context(&block.carrier_context());
    if semantics.carrier_is_detached_request_frame {
        Part::Frame
    } else if semantics.carrier_holds_request_delta {
        Part::Delta
    } else if semantics.carrier_holds_span_output {
        Part::Output
    } else {
        Part::Other
    }
}

/// What two observations must share to be one item of a history: the content, and the call a result answers - a
/// resumed process re-sends a result with its call's id, and two tools that both answered `"ok"` are two results.
/// The role is the list the identity is kept in.
fn identity(block: &BlockEntry) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    compute_block_hash(&block.content).hash(&mut hasher);
    block.tool_use_id.hash(&mut hasher);
    hasher.finish()
}

/// Group rows by span, one request each.
///
/// Read **lightly**: parsed and flattened, which is where a block's content and role are settled, without the
/// ordering, history and deduplication passes a span view runs - a delta, an output and a call are each in their
/// carrier's own order, and those passes cost more than the parse on a long delta. One row per span, the first: a
/// span exported twice is one request.
fn requests_of(rows: Vec<MessageSpanRow>) -> Vec<Request> {
    let mut by_span: BTreeMap<(String, String), MessageSpanRow> = BTreeMap::new();
    for row in rows {
        by_span
            .entry((row.trace_id.clone(), row.span_id.clone()))
            .or_insert(row);
    }
    by_span
        .into_iter()
        .map(|((trace_id, span_id), row)| Request {
            span_id,
            trace_id,
            started: row.span_timestamp,
            failed: row.status_code.as_deref() == Some(status::ERROR),
            blocks: read_lightly(std::slice::from_ref(&row)),
        })
        .collect()
}

fn read_lightly(rows: &[MessageSpanRow]) -> Vec<BlockEntry> {
    flatten_to_blocks(parse_span_rows(rows), &build_span_hierarchy(rows))
}

/// The composed history: its blocks once each, in thread order, and per role the identities of its blocks of that
/// role - what a restating delta is compared with.
#[derive(Default)]
struct History {
    blocks: Vec<BlockEntry>,
    by_role: HashMap<ChatRole, Vec<u64>>,
    calls: HashSet<String>,
}

impl History {
    fn push(&mut self, block: BlockEntry) {
        self.by_role
            .entry(block.role)
            .or_default()
            .push(identity(&block));
        if block.is_tool_use()
            && let Some(id) = &block.tool_use_id
        {
            self.calls.insert(id.clone());
        }
        self.blocks.push(block);
    }

    /// The delta items that do not restate the history.
    ///
    /// A resumed process restates the **whole** history in its delta, every role it exports at once: its user
    /// turns in one carrier and its reminders in another. So a delta restates the history when, for every role it
    /// carries, its items of that role begin with all of the history's items of that role - and then those are
    /// dropped, per role. Otherwise nothing is: a delta that restates only some roles, or only part of one, is not
    /// that shape, and treating a matching item as a restatement would drop a turn the request really added - a
    /// repeated reminder, the same question asked again.
    fn new_items<'a>(&self, delta: &[&'a BlockEntry]) -> Vec<&'a BlockEntry> {
        let mut per_role: HashMap<ChatRole, Vec<u64>> = HashMap::new();
        for block in delta {
            per_role
                .entry(block.role)
                .or_default()
                .push(identity(block));
        }
        let restates = per_role.iter().all(|(role, items)| {
            let earlier = self
                .by_role
                .get(role)
                .map(Vec::as_slice)
                .unwrap_or_default();
            items.starts_with(earlier)
        });
        if !restates {
            return delta.to_vec();
        }
        let mut seen: HashMap<ChatRole, usize> = HashMap::new();
        delta
            .iter()
            .filter(|block| {
                let restated = self.by_role.get(&block.role).map_or(0, Vec::len);
                let at = seen.entry(block.role).or_insert(0);
                *at += 1;
                *at > restated
            })
            .copied()
            .collect()
    }
}

/// The span view of a request, with what its thread's earlier requests sent before it.
pub(super) fn compose(rows: RequestContextRows) -> FeedResult {
    // The thread in sequence order: span start, then span id, never arrival. The target's own rows stand for it,
    // whatever copy of it the thread rows hold, and it is the last request read: anything after it had not been
    // sent when it was.
    let target_rows = rows.target;
    let Some(target) = requests_of(target_rows.clone()).pop() else {
        return FeedResult::default();
    };
    let at = |request: &Request| {
        (
            request.started,
            request.span_id.clone(),
            request.trace_id.clone(),
        )
    };
    let mut thread: Vec<Request> = requests_of(rows.thread)
        .into_iter()
        .filter(|request| at(request) < at(&target))
        .collect();
    thread.sort_by_key(at);
    let own = process_span_unfiltered(target_rows);

    // A break composes nothing past it: two requests at one sequence position whose content differs leave their
    // order unknown, and a failed request is an attempt whose place nothing here can establish.
    let tied = thread
        .iter()
        .chain(std::iter::once(&target))
        .collect::<Vec<_>>()
        .windows(2)
        .any(|pair| {
            pair[0].started == pair[1].started
                && pair[0]
                    .blocks
                    .iter()
                    .map(identity)
                    .ne(pair[1].blocks.iter().map(identity))
        });
    if thread.is_empty() || tied || thread.iter().any(|request| request.failed) {
        return own;
    }

    // Every call the offered tool spans hold, by the id a result names: the earliest span that started holding it.
    let mut calls: HashMap<String, (DateTime<Utc>, String, BlockEntry)> = HashMap::new();
    for request in requests_of(rows.calls) {
        for block in request.blocks {
            let Some(id) = block.tool_use_id.clone().filter(|_| block.is_tool_use()) else {
                continue;
            };
            let earlier = calls
                .get(&id)
                .is_some_and(|kept| (kept.0, &kept.1) <= (request.started, &request.span_id));
            if !earlier {
                calls.insert(id, (request.started, request.span_id.clone(), block));
            }
        }
    }

    let mut history = History::default();
    let requests = thread
        .iter()
        .map(|request| request.blocks.as_slice())
        .chain(std::iter::once(own.messages.as_slice()));
    let count = thread.len() + 1;
    for (index, view) in requests.enumerate() {
        let delta: Vec<&BlockEntry> = view
            .iter()
            .filter(|block| part_of(block) == Part::Delta)
            .collect();
        if index > 0 {
            // The calls this delta answers, in the order their spans started; a call already in the history was
            // answered before and is not made again.
            let mut answered: Vec<&(DateTime<Utc>, String, BlockEntry)> = delta
                .iter()
                .filter(|block| block.is_tool_result())
                .filter_map(|block| block.tool_use_id.as_deref())
                .filter(|id| !history.calls.contains(*id))
                .filter_map(|id| calls.get(id))
                .collect();
            answered.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
            answered.dedup_by(|a, b| a.2.tool_use_id == b.2.tool_use_id);
            for (_, _, call) in answered {
                history.push(call.clone());
            }
        }
        let appended: Vec<BlockEntry> = history.new_items(&delta).into_iter().cloned().collect();
        for block in appended {
            history.push(block);
        }
        if index + 1 < count {
            for block in view.iter().filter(|block| part_of(block) == Part::Output) {
                history.push(block.clone());
            }
        }
    }

    // The request's own view with its delta replaced by the composed history, where the delta stood.
    let composed_from_requests = thread.len();
    let mut messages = Vec::with_capacity(own.messages.len() + history.blocks.len());
    let mut history = Some(history.blocks);
    for block in own.messages {
        let part = part_of(&block);
        if part != Part::Frame
            && let Some(history) = history.take()
        {
            messages.extend(history);
        }
        if part != Part::Delta {
            messages.push(block);
        }
    }
    if let Some(history) = history {
        messages.extend(history);
    }
    let metadata = FeedMetadata {
        block_count: messages.len(),
        composed_from_requests,
        ..own.metadata
    };
    FeedResult {
        messages,
        metadata,
        tool_definitions: own.tool_definitions,
        tool_names: own.tool_names,
    }
}

#[cfg(test)]
#[path = "request_context_tests.rs"]
mod tests;
