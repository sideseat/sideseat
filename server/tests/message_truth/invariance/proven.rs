//! A release difference its truth proves is not a release difference.
//!
//! A release capture is held to the current release's conversation block for block. Where the truth proves, for
//! that release and not the current one, that the telemetry leaves something out, the comparison sets aside exactly
//! that and nothing else - each rule tied to the proven gap kind it rests on, every one of which
//! `every_absence_gap_is_proven` holds to an absence proof:
//!
//! - **`not_exported`**: the fact is in none of the release's payloads, so the current release's blocks showing it
//!   are left out of its side; the release must show everything else.
//! - **`id_not_exported`**: the call's wire id is in none of the release's payloads, so that call's tool use is
//!   compared without its `id`, and the results answering it without their `tool_use_id`, on both sides.
//! - **`output_not_exported`, `call_not_exported`, `call_span_not_exported`**: the part is in none of the payloads
//!   of the span that would place it, so the release reads it from another span and the feed - which orders by
//!   each block's span - lists it by that span's time. In the feed those blocks are compared as a set within the
//!   same response: between the same two outputs of other calls. Trace and session views stay exact.
//!
//! Without such a gap nothing is set aside, so an unproven difference still fails.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::super::predicates::{Shows, shows};
use super::super::recon::{Block, Recon, ViewKind};
use super::super::truth::{Fact, Truth};

/// The facts the release's own proven gaps let it show differently, by what each lets differ.
#[derive(Default)]
struct Proven {
    /// `not_exported`: absent from the release.
    absent: BTreeSet<String>,
    /// `id_not_exported`: tool calls shown without their wire id.
    no_id: BTreeSet<String>,
    /// `output_not_exported`, `call_not_exported`, `call_span_not_exported`: placed by another span.
    elsewhere: BTreeSet<String>,
    /// `call_order_not_exported`: a response's parallel tool calls, whose order among themselves no payload holds,
    /// each with its run - the response and which consecutive run of its calls - so no set mixes with another.
    unordered: BTreeMap<String, String>,
    /// The results both truths leave in one open batch (`open_order`): their calls' order unknown, the order they
    /// arrive in is the run's, as `open_order` already holds for a release that differs there alone.
    batches: Vec<BTreeSet<String>>,
}

impl Proven {
    fn is_empty(&self) -> bool {
        self.absent.is_empty()
            && self.no_id.is_empty()
            && self.elsewhere.is_empty()
            && self.unordered.is_empty()
    }
}

/// The gaps that hold for the release and not for the current release, as the facts they let differ.
fn proven(release: &Truth, current: &Truth) -> Proven {
    let batches = super::open_order::open_batches(release)
        .intersection(&super::open_order::open_batches(current))
        .cloned()
        .collect();
    let key = |reason: &str, subject: &Option<String>| (reason.to_string(), subject.clone());
    let current_gaps: BTreeSet<_> = current
        .gaps
        .iter()
        .map(|g| key(&g.reason, &g.subject))
        .collect();
    let mut out = Proven {
        batches,
        ..Proven::default()
    };
    for gap in release
        .gaps
        .iter()
        .filter(|g| !current_gaps.contains(&key(&g.reason, &g.subject)))
    {
        let Some(subject) = gap.subject.clone() else {
            continue;
        };
        match gap.reason.as_str() {
            "not_exported" => {
                out.absent.insert(subject);
            }
            "id_not_exported" => {
                out.no_id.insert(subject);
            }
            "output_not_exported" => {
                out.elsewhere.insert(subject);
            }
            // A call's gap covers the parts of its response.
            "call_not_exported" | "call_span_not_exported" => {
                if let Some(call) = current.calls.iter().find(|c| c.id == subject) {
                    out.elsewhere.extend(call.outputs.iter().cloned());
                }
            }
            // Only a run of consecutive calls is a set: a text between two runs of one response keeps them
            // apart, as the response's part order does.
            "call_order_not_exported" => {
                if let Some(call) = current.calls.iter().find(|c| c.id == subject) {
                    let is_call = |id: &String| {
                        current
                            .facts
                            .iter()
                            .any(|f| &f.id == id && f.kind == "tool_call")
                    };
                    let mut run = 0usize;
                    let mut in_run = false;
                    for id in &call.outputs {
                        if is_call(id) {
                            if !in_run {
                                run += 1;
                                in_run = true;
                            }
                            out.unordered.insert(id.clone(), format!("{subject}#{run}"));
                        } else {
                            in_run = false;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// One block as the comparison sees it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Entry {
    /// Compared in place.
    Kept(String),
    /// Compared as one of its response's set (`elsewhere`), in the feed only.
    Placed(String),
}

/// The facts a block shows, judged by the truth of the fixture it is in. A result whose call's id is proven absent
/// (`no_id`) is matched as if it carried the call's id, since the release cannot show it. A fact the block shows
/// exactly wins over one it shows only under another id: two calls of one shape are told apart by their ids
/// wherever the ids are there, so one call's missing id never masks another's.
fn shown<'t>(truth: &'t Truth, block: &Block, proven: &Proven) -> Vec<&'t Fact> {
    let exact: Vec<&Fact> = truth
        .facts
        .iter()
        .filter(|fact| shows(fact, block, None) == Shows::Yes)
        .collect();
    if !exact.is_empty() {
        return exact;
    }
    let call_of_result = |fact: &Fact| {
        fact.value
            .get("call_id")
            .and_then(Value::as_str)
            .and_then(|id| {
                truth
                    .facts
                    .iter()
                    .find(|f| f.kind == "tool_call" && wire_id(f) == Some(id))
            })
            .map(|call| call.id.clone())
    };
    truth
        .facts
        .iter()
        .filter(|fact| {
            shows(fact, block, None) != Shows::No
                || (fact.kind == "tool_result"
                    && block.is_tool_result()
                    && block.result_call_id().is_none()
                    && call_of_result(fact).is_some_and(|call| proven.no_id.contains(&call))
                    && shows(fact, &named_as(block, fact), None) != Shows::No)
        })
        .collect()
}

/// A tool call's id on the wire: its own, or the one `id_not_exported` moved aside.
fn wire_id(fact: &Fact) -> Option<&str> {
    fact.value
        .get("wire_id")
        .or_else(|| fact.value.get("id"))
        .and_then(Value::as_str)
}

/// `block` carrying the call id `fact` answers, beside it and in its content.
fn named_as(block: &Block, fact: &Fact) -> Block {
    let mut named = block.clone();
    let id = fact
        .value
        .get("call_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if let Some(content) = named.content.as_object_mut() {
        content.insert("tool_use_id".to_string(), Value::String(id.clone()));
    }
    named.tool_use_id = Some(id);
    named
}

/// A block's identity, without the call id where the release proves it cannot carry one.
fn identity(block: &Block, facts: &[&Fact], truth: &Truth, proven: &Proven) -> String {
    let unnamed_call = facts
        .iter()
        .any(|f| f.kind == "tool_call" && proven.no_id.contains(&f.id));
    let unnamed_result = facts.iter().any(|f| {
        f.kind == "tool_result"
            && truth.facts.iter().any(|call| {
                call.kind == "tool_call"
                    && proven.no_id.contains(&call.id)
                    && f.value.get("call_id").and_then(Value::as_str) == wire_id(call)
            })
    });
    if !(unnamed_call || unnamed_result) {
        return block.digest.clone();
    }
    let mut content = block.content.clone();
    if let Some(map) = content.as_object_mut() {
        map.remove(if unnamed_call { "id" } else { "tool_use_id" });
    }
    format!(
        "{}/{}/{}",
        block.role,
        block.kind,
        crate::content_digest(&content)
    )
}

/// The run a block may move within: one response's parallel calls, or one open batch of results - never a mix.
fn group(facts: &[&Fact], proven: &Proven) -> Option<String> {
    let first = facts.first()?;
    if let Some(call) = proven.unordered.get(&first.id)
        && facts
            .iter()
            .all(|f| proven.unordered.get(&f.id) == Some(call))
    {
        return Some(format!("calls:{call}"));
    }
    proven
        .batches
        .iter()
        .position(|batch| facts.iter().all(|f| batch.contains(&f.id)))
        .map(|i| format!("batch:{i}"))
}

/// Whether every view but the feed - span, trace and session - orders each run's members alike: a set compared as
/// a set must still be one order on its side, so two views of one fixture cannot each show a different one. The
/// orders the views state together must be acyclic, as `open_order` holds them; the feed orders by arrival and is
/// its own presentation.
fn one_order(recon: &Recon, truth: &Truth, proven: &Proven) -> bool {
    let mut after: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for view in recon.views.iter().filter(|v| v.kind != ViewKind::Feed) {
        let members: Vec<(String, String)> = view
            .blocks
            .iter()
            .filter_map(|block| {
                let facts = shown(truth, block, proven);
                let group = group(&facts, proven)?;
                Some((
                    group.clone(),
                    format!("{group}={}", identity(block, &facts, truth, proven)),
                ))
            })
            .collect();
        for (i, (group, a)) in members.iter().enumerate() {
            for (other, b) in &members[i + 1..] {
                if other == group && a != b {
                    after.entry(a.clone()).or_default().insert(b.clone());
                }
            }
        }
    }
    acyclic(&after)
}

/// Whether a "comes before" relation holds no cycle.
fn acyclic(after: &BTreeMap<String, BTreeSet<String>>) -> bool {
    // 1 while a node is on the path, 2 once it is finished.
    fn visit<'a>(
        node: &'a str,
        after: &'a BTreeMap<String, BTreeSet<String>>,
        state: &mut BTreeMap<&'a str, u8>,
    ) -> bool {
        match state.get(node) {
            Some(1) => return false,
            Some(_) => return true,
            None => {}
        }
        state.insert(node, 1);
        for next in after.get(node).into_iter().flatten() {
            if !visit(next, after, state) {
                return false;
            }
        }
        state.insert(node, 2);
        true
    }
    let mut state = BTreeMap::new();
    after.keys().all(|node| visit(node, after, &mut state))
}

/// A fixture's conversation - every trace view by its stable label, every session view, the feed - with what the
/// proven gaps set aside set aside. `current` says which side this is: only the current release's side leaves out
/// what the release proves absent.
fn projection(
    recon: &Recon,
    labels: &BTreeMap<String, String>,
    truth: &Truth,
    proven: &Proven,
    current: bool,
) -> Vec<(&'static str, String, Vec<Entry>)> {
    let mut out = Vec::new();
    for view in &recon.views {
        let (tag, key) = match view.kind {
            ViewKind::Trace => (
                "trace",
                labels
                    .get(&view.key)
                    .cloned()
                    .unwrap_or_else(|| view.key.clone()),
            ),
            ViewKind::Session => ("session", view.key.clone()),
            ViewKind::Feed => ("feed", String::new()),
            ViewKind::Span => continue,
        };
        let feed = view.kind == ViewKind::Feed;
        let mut entries = Vec::with_capacity(view.blocks.len());
        // Model outputs seen so far in the feed, asserted or not: a block placed by another span keeps its response,
        // the run between two of them.
        let mut outputs = 0usize;
        let mut placed = Vec::new();
        // Which kept entries are parallel calls of an unordered response, each run of them compared as a set.
        let mut free = Vec::with_capacity(view.blocks.len());
        for block in &view.blocks {
            let facts = shown(truth, block, proven);
            let only = |set: &BTreeSet<String>| {
                !facts.is_empty() && facts.iter().all(|f| set.contains(&f.id))
            };
            if current && only(&proven.absent) {
                continue;
            }
            let id = identity(block, &facts, truth, proven);
            if feed && only(&proven.elsewhere) {
                let calls: BTreeSet<&str> =
                    facts.iter().filter_map(|f| f.call.as_deref()).collect();
                let calls = calls.into_iter().collect::<Vec<_>>().join(",");
                placed.push((format!("{outputs}:{calls}"), Entry::Placed(id)));
                continue;
            }
            if block.role == "assistant" {
                outputs += 1;
            }
            let run = group(&facts, proven);
            // A member of a set carries its set and the facts it shows, so a block of the same content from
            // outside the set can never stand in for it.
            entries.push(Entry::Kept(match &run {
                Some(run) => {
                    let ids: Vec<&str> = facts.iter().map(|f| f.id.as_str()).collect();
                    format!("{run}|{}|{id}", ids.join(","))
                }
                None => id,
            }));
            free.push(run);
        }
        let mut start = 0;
        while start < entries.len() {
            let mut end = start + 1;
            if free[start].is_some() {
                while end < entries.len() && free[end] == free[start] {
                    end += 1;
                }
                entries[start..end].sort();
            }
            start = end;
        }
        // Within one response, one call's placed parts are a set; the placed parts of different calls, or of
        // different responses, keep their order.
        let mut at = 0;
        while at < placed.len() {
            let mut end = at + 1;
            while end < placed.len() && placed[end].0 == placed[at].0 {
                end += 1;
            }
            placed[at..end].sort();
            at = end;
        }
        entries.extend(placed.into_iter().map(|(slot, entry)| match entry {
            Entry::Placed(id) => Entry::Placed(format!("{slot}:{id}")),
            kept => kept,
        }));
        out.push((tag, key, entries));
    }
    out.sort();
    out
}

/// Whether the release's conversation differs from the current release's only where the release's own proven gaps
/// say it must. A release with no such gap explains nothing.
pub(super) fn explains(
    release: (&Recon, &BTreeMap<String, String>, &Truth),
    current: (&Recon, &BTreeMap<String, String>, &Truth),
) -> bool {
    let mut proven = proven(release.2, current.2);
    // A batch some block may show a member of without being that member alone cannot be ordered, so it is not
    // set aside - as `open_order` holds.
    for unclear in [
        super::open_order::ambiguous(
            release.0,
            release.2,
            &proven.batches.iter().cloned().collect(),
        ),
        super::open_order::ambiguous(
            current.0,
            current.2,
            &proven.batches.iter().cloned().collect(),
        ),
    ] {
        proven.batches.retain(|batch| !unclear.contains(batch));
    }
    !proven.is_empty()
        && one_order(release.0, release.2, &proven)
        && one_order(current.0, current.2, &proven)
        && [(release.0, release.2), (current.0, current.2)]
            .into_iter()
            .all(|(recon, truth)| {
                super::open_order::one_order_per_batch(
                    recon,
                    truth,
                    &proven.batches.iter().cloned().collect(),
                )
            })
        && projection(release.0, release.1, release.2, &proven, false)
            == projection(current.0, current.1, current.2, &proven, true)
}

#[cfg(test)]
mod tests;
