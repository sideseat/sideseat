//! Ids a framework reissued: which id each call of a request appears under in the conversation.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::super::absence::wire_id;
use super::super::predicates::{Shows, shows};
use super::super::recon::{Block, Recon, ViewKind};
use super::super::truth::{CallRequest, FixtureRequests, Truth};
use super::as_fact;

/// Which id each of the request's call ids appears under on this span's input.
///
/// A framework that reissues the provider's ids does so consistently, and the fact checks report that
/// as `tool_call.id_rewritten` once. Resolving the rewrite here keeps the request assignment from
/// reporting the same rewrite again as a missing call and a missing result.
pub(super) fn rewrites(
    request: &CallRequest,
    recon: &Recon,
    blocks: &[&Block],
    wire_ids: &BTreeSet<&str>,
) -> BTreeMap<String, String> {
    // Every block of the conversation, not only this span's input: a framework that reissues the ids
    // shows the call on the span that produced it, which is not the span that was then sent it.
    let shown_anywhere: Vec<&Block> = recon
        .views
        .iter()
        .filter(|view| view.kind == ViewKind::Trace)
        .flat_map(|view| view.blocks.iter())
        .chain(blocks.iter().copied())
        .collect();
    rewrite_map(request, &shown_anywhere, wire_ids)
}

/// Every call id the provider issued in one fixture: what its requests sent and its calls returned.
pub(super) fn wire_ids<'t>(truth: &'t Truth, recorded: &'t FixtureRequests) -> BTreeSet<&'t str> {
    let sent = recorded
        .calls
        .values()
        .flat_map(|r| r.messages.iter().flat_map(|m| &m.parts));
    let sent = sent
        .filter(|o| o.part["type"] == "tool_call")
        .filter_map(|o| o.part["id"].as_str());
    let returned = truth
        .facts
        .iter()
        .filter(|f| f.kind == "tool_call")
        .filter_map(wire_id);
    sent.chain(returned).filter(|id| !id.is_empty()).collect()
}

/// The rewrites `shown_anywhere` evidences. An id the provider issued to another call is never a reissue:
/// two calls with one name and the same arguments show each other's call under their own ids, which read
/// as each rewritten to the other and swapped their results.
pub(super) fn rewrite_map(
    request: &CallRequest,
    shown_anywhere: &[&Block],
    wire_ids: &BTreeSet<&str>,
) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    let mut ambiguous: BTreeSet<String> = BTreeSet::new();
    for occurrence in request.messages.iter().flat_map(|m| m.parts.iter()) {
        let part = &occurrence.part;
        if part.get("type").and_then(Value::as_str) != Some("tool_call") {
            continue;
        }
        let Some(wire) = part["id"].as_str().filter(|id| !id.is_empty()) else {
            continue;
        };
        let Some(fact) = as_fact("rewrite", "assistant", part, &BTreeMap::new()) else {
            continue;
        };
        let shown: BTreeSet<&str> = shown_anywhere
            .iter()
            .filter(|block| {
                block.role == "assistant" && !matches!(shows(&fact, block, None), Shows::No)
            })
            .filter_map(|block| block.call_id())
            .filter(|shown| *shown != wire && !shown.is_empty() && !wire_ids.contains(shown))
            .collect();
        match shown.into_iter().collect::<Vec<_>>().as_slice() {
            [one] => {
                out.insert(wire.to_string(), (*one).to_string());
            }
            [] => {}
            // Two different ids for one call: which one it was reissued as is unknowable, so neither
            // is assumed and the strict comparison stands.
            _ => {
                ambiguous.insert(wire.to_string());
            }
        }
    }
    out.retain(|wire, _| !ambiguous.contains(wire));
    out
}
