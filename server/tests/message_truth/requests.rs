//! Rubric v3, slice 1: what each call was sent, against the input its span shows.
//!
//! The call is found by its output (`matching`), never by the input under test. Its recorded request -
//! system parts, then each message's parts in order - is then assigned to the span's input blocks by an
//! exact, ordered, injective matching: every occurrence to at most one block, every block to at most one
//! occurrence, order kept. What the assignment leaves over on either side is the defect, named by what it
//! is: missing, extra, duplicated, out of order, under another role.
//!
//! Each occurrence is compared through the same predicates as the facts, so a request part and a fact
//! of the same content are judged identically. Expected values come from the transcript the fixture's
//! own run recorded, never from the reconstruction.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::absence::haystack::Haystack;
use super::absence::{Proof, prove};
use super::matching::Matching;
use super::predicates::{Shows, shows};
use super::recon::{Block, Recon, ViewKind};
use super::truth::{CallRequest, Fact, Requirement, Truth};
use super::{Violation, ViolationView};

/// One expected input block: where it sits in the request, its role, and the fact it must show.
struct Expected {
    label: String,
    role: &'static str,
    fact: Fact,
    part: Value,
    /// Which message of the request sent it; `None` for a system part.
    message: Option<usize>,
}

/// The role a reconstruction shows a part under: a tool's result is the tool's turn, whatever message
/// carried it, and a provider's own names for the assistant and the tool are normalised.
fn shown_role(message_role: &str, part: &Value) -> &'static str {
    if part.get("type").and_then(Value::as_str) == Some("tool_result") {
        return "tool";
    }
    match message_role {
        "system" => "system",
        "user" => "user",
        "assistant" | "model" => "assistant",
        "tool" | "function" => "tool",
        _ => "unknown",
    }
}

fn requirement(matcher: &str) -> Option<Requirement> {
    Some(Requirement {
        anchor: "model_call".into(),
        views: vec!["span".into()],
        cardinality: "exactly_once".into(),
        matcher: matcher.into(),
    })
}

/// The fact an occurrence must show as, judged by the fact predicates.
fn as_fact(
    label: &str,
    role: &'static str,
    part: &Value,
    rewrites: &BTreeMap<String, String>,
) -> Option<Fact> {
    let kind = part.get("type").and_then(Value::as_str)?;
    let (kind, value, matcher) = match (kind, role) {
        ("text", "system") => ("system", json!({"text": part["text"]}), "exact"),
        ("text", "user") => ("user_text", json!({"text": part["text"]}), "exact"),
        ("text", _) => ("text", json!({"text": part["text"]}), "exact"),
        ("media", _) => (
            "user_media",
            json!({
                "modality": part["modality"],
                "media_type": part["media_type"],
                "sha256": part["sha256"],
            }),
            "digest",
        ),
        ("tool_call", _) => (
            "tool_call",
            json!({
                "id": rewritten(&part["id"], rewrites),
                "name": part["name"],
                "arguments": part["arguments"],
            }),
            "semantic",
        ),
        ("tool_result", _) => (
            "tool_result",
            json!({
                "call_id": rewritten(&part["id"], rewrites),
                "name": Value::Null,
                "value": result_value(&part["content"]),
                "is_error": part["is_error"],
            }),
            "semantic",
        ),
        ("reasoning", _) if part["text"].is_string() => {
            ("reasoning", json!({"text": part["text"]}), "exact")
        }
        ("reasoning", _) => ("reasoning", json!({}), "presence"),
        _ => return None,
    };
    Some(Fact {
        id: label.to_string(),
        kind: kind.to_string(),
        role: role.to_string(),
        conversation: String::new(),
        evidence: "wire".into(),
        value,
        require: requirement(matcher),
        call: None,
    })
}

/// What a tool result carried: one JSON value or one text as itself, several parts as their list.
fn result_value(content: &Value) -> Value {
    let parts = content.as_array().cloned().unwrap_or_default();
    let single = |part: &Value| match part.get("type").and_then(Value::as_str) {
        Some("json") => part["value"].clone(),
        Some("text") => {
            let text = part["text"].as_str().unwrap_or("");
            serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string()))
        }
        _ => part.clone(),
    };
    match parts.as_slice() {
        [one] => single(one),
        many => Value::Array(many.iter().map(single).collect()),
    }
}

/// The id a view shows a wire id under, where the framework rewrote it.
fn rewritten(id: &Value, rewrites: &BTreeMap<String, String>) -> Value {
    match id.as_str().and_then(|id| rewrites.get(id)) {
        Some(shown) => Value::String(shown.clone()),
        None => id.clone(),
    }
}

/// Which id each of the request's call ids appears under on this span's input.
///
/// A framework that reissues the provider's ids does so consistently, and the fact checks report that
/// as `tool_call.id_rewritten` once. Resolving the rewrite here keeps the request assignment from
/// reporting the same rewrite again as a missing call and a missing result.
fn rewrites(request: &CallRequest, recon: &Recon, blocks: &[&Block]) -> BTreeMap<String, String> {
    // Every block of the conversation, not only this span's input: a framework that reissues the ids
    // shows the call on the span that produced it, which is not the span that was then sent it.
    let shown_anywhere: Vec<&Block> = recon
        .views
        .iter()
        .filter(|view| view.kind == ViewKind::Trace)
        .flat_map(|view| view.blocks.iter())
        .chain(blocks.iter().copied())
        .collect();
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
            .filter(|shown| *shown != wire && !shown.is_empty())
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

fn expected(
    call: &str,
    request: &CallRequest,
    rewrites: &BTreeMap<String, String>,
) -> Vec<Expected> {
    let mut out = Vec::new();
    for (p, occurrence) in request.system.iter().enumerate() {
        let label = format!("{call}:system.{p}");
        if let Some(fact) = as_fact(&label, "system", &occurrence.part, rewrites) {
            out.push(Expected {
                label,
                role: "system",
                fact,
                part: occurrence.part.clone(),
                message: None,
            });
        }
    }
    for (m, message) in request.messages.iter().enumerate() {
        for (p, occurrence) in message.parts.iter().enumerate() {
            let role = shown_role(&message.role, &occurrence.part);
            let label = format!("{call}:m{m}.{p}");
            if let Some(fact) = as_fact(&label, role, &occurrence.part, rewrites) {
                out.push(Expected {
                    label,
                    role,
                    fact,
                    part: occurrence.part.clone(),
                    message: Some(m),
                });
            }
        }
    }
    out
}

/// The part, judged as if it had been sent under the role the block shows: the same content, another role.
fn shown_under(expected: &Expected, block: &Block) -> bool {
    let role = shown_role(&block.role, &expected.part);
    as_fact(&expected.label, role, &expected.part, &BTreeMap::new())
        .is_some_and(|fact| matches!(shows(&fact, block, None), Shows::Yes | Shows::Assigned(_)))
}

fn matches(expected: &Expected, block: &Block) -> bool {
    block.role == expected.role && !matches!(shows(&expected.fact, block, None), Shows::No)
}

/// The longest ordered assignment of expected occurrences to input blocks, as `(expected, block)` pairs.
fn assignment(expected: &[Expected], blocks: &[&Block]) -> Vec<(usize, usize)> {
    let (n, m) = (expected.len(), blocks.len());
    let mut best = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            best[i][j] = if matches(&expected[i], blocks[j]) {
                best[i + 1][j + 1] + 1
            } else {
                best[i + 1][j].max(best[i][j + 1])
            };
        }
    }
    let (mut i, mut j, mut pairs) = (0, 0, Vec::new());
    while i < n && j < m {
        if matches(&expected[i], blocks[j]) && best[i][j] == best[i + 1][j + 1] + 1 {
            pairs.push((i, j));
            i += 1;
            j += 1;
        } else if best[i + 1][j] >= best[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    pairs
}

/// One fixture's decoded payloads, built once per fixture however many calls consult them.
///
/// Keyed by the fixture, which is sound under the mutation catalogue: a mutation edits a reconstruction
/// or a truth, never the captured payloads.
fn haystack(recon: &Recon) -> &'static Haystack {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<BTreeMap<String, &'static Haystack>>> =
        std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let mut cache = cache.lock().expect("the haystack cache is not poisoned");
    cache
        .entry(recon.fixture.clone())
        .or_insert_with(|| Box::leak(Box::new(Haystack::of_fixture(&recon.paths))))
}

/// Accept an occurrence shown inside its own message's span of the input, in another position.
///
/// The parts of one message are a batch, not a sequence: a provider's parallel calls - and the results
/// answering them - arrive together, and a reconstruction may order a batch by completion. Across
/// messages the request's order stands, which is what `request.order` reports.
fn recover_within_messages(
    expected: &[Expected],
    blocks: &[&Block],
    pairs: &[(usize, usize)],
    assigned_expected: &mut [bool],
    assigned_block: &mut [bool],
) {
    let span_of = |message: Option<usize>| {
        let placed: Vec<usize> = pairs
            .iter()
            .filter(|&&(e, _)| expected[e].message == message)
            .map(|&(_, b)| b)
            .collect();
        match (placed.iter().min(), placed.iter().max()) {
            (Some(&first), Some(&last)) => Some((first, last)),
            _ => None,
        }
    };
    for i in 0..expected.len() {
        if assigned_expected[i] {
            continue;
        }
        let Some((first, last)) = span_of(expected[i].message) else {
            continue;
        };
        let found =
            (first..=last).find(|&j| !assigned_block[j] && matches(&expected[i], blocks[j]));
        if let Some(j) = found {
            assigned_expected[i] = true;
            assigned_block[j] = true;
        }
    }
}

/// Requests against span inputs, for every matched call the fixture's transcript recorded.
pub(super) fn check_requests(
    truth: &Truth,
    recon: &Recon,
    matching: &Matching,
    out: &mut Vec<Violation>,
) {
    let Some(recorded) = truth.requests.get(&recon.fixture) else {
        return;
    };
    for (call, request) in &recorded.calls {
        // Only a call whose span its *output* established. A failed attempt is tied to an ERROR span by
        // its error message, and a call with no asserted output is tied by cardinality: in both the span
        // is a guess, so its input would be compared against the wrong request.
        let established = truth
            .calls
            .iter()
            .any(|c| &c.id == call && c.succeeded() && !matching.unmatchable.contains(&c.id));
        let Some(&generation) = matching.span_of.get(call).filter(|_| established) else {
            continue;
        };
        let span = &recon.generations[generation].span;
        let Some(view) = recon.span_view(span) else {
            continue;
        };
        let blocks: Vec<&Block> = view.blocks.iter().filter(|b| !b.output).collect();
        let wanted = expected(call, request, &rewrites(request, recon, &blocks));
        let pairs = assignment(&wanted, &blocks);
        let mut assigned_expected: Vec<bool> = (0..wanted.len())
            .map(|i| pairs.iter().any(|&(e, _)| e == i))
            .collect();
        let mut assigned_block: Vec<bool> = (0..blocks.len())
            .map(|j| pairs.iter().any(|&(_, b)| b == j))
            .collect();
        recover_within_messages(
            &wanted,
            &blocks,
            &pairs,
            &mut assigned_expected,
            &mut assigned_block,
        );
        let mut explained = vec![false; blocks.len()];
        for (_, item) in wanted
            .iter()
            .enumerate()
            .filter(|(i, _)| !assigned_expected[*i])
        {
            let elsewhere = (0..blocks.len())
                .find(|&j| !assigned_block[j] && !explained[j] && matches(item, blocks[j]));
            let (assertion, detail) = if let Some(j) = elsewhere {
                explained[j] = true;
                (
                    "request.order",
                    "shown, but not where the request put it".to_string(),
                )
            } else if let Some(j) = (0..blocks.len()).find(|&j| {
                !assigned_block[j]
                    && !explained[j]
                    && blocks[j].role != item.role
                    && shown_under(item, blocks[j])
            }) {
                explained[j] = true;
                (
                    "request.role",
                    format!("shown as {}, sent as {}", blocks[j].role, item.role),
                )
            } else {
                // Whether the content is in the payloads at all decides what this is. Present: the
                // telemetry carried what the call was sent and the reconstruction lost it. Absent: the
                // producer never exported it, which is a limitation of that framework's telemetry, not
                // a parsing defect. Unprovable fails closed, as every absence claim does.
                // Proven from the part as the request carried it, never from the id a view reissued:
                // whether the telemetry holds what the call was sent is a question about the payloads.
                let sent = as_fact(&item.label, item.role, &item.part, &BTreeMap::new());
                match prove(sent.as_ref().unwrap_or(&item.fact), haystack(recon)) {
                    Proof::Absent => (
                        "request.not_exported",
                        format!(
                            "no payload carries this {}: the producer does not export it",
                            item.fact.kind
                        ),
                    ),
                    Proof::Present(at) | Proof::Partial(at) => (
                        "request.missing",
                        format!(
                            "{at} carries this {}, and no input shows it",
                            item.fact.kind
                        ),
                    ),
                    Proof::Unprovable(why) => (
                        "request.missing",
                        format!(
                            "the span's input does not show this {}, and its absence from the \
                             payloads cannot be proven: {why}",
                            item.fact.kind
                        ),
                    ),
                }
            };
            out.push(Violation::new(
                ViolationView::Request,
                assertion,
                &item.label,
                detail,
            ));
        }
        for (j, block) in blocks.iter().enumerate() {
            if assigned_block[j] || explained[j] {
                continue;
            }
            let twin = pairs
                .iter()
                .find(|&&(e, _)| matches(&wanted[e], block))
                .map(|&(e, _)| wanted[e].label.clone());
            // The input's position, never its content: a legitimate re-encoding of a block must not
            // read as a different violation of the same reconstruction.
            let subject = format!("{call}:in{j}:{}/{}", block.role, block.kind);
            match twin {
                Some(of) => out.push(Violation::new(
                    ViolationView::Request,
                    "request.duplicated",
                    &subject,
                    format!("a second copy of {of}"),
                )),
                None => out.push(Violation::new(
                    ViolationView::Request,
                    "request.extra",
                    &subject,
                    "the span's input shows what the request did not send".to_string(),
                )),
            }
        }
    }
}
