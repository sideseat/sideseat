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
use super::truth::{CallRequest, Fact, Occurrence, Requirement, Truth};
use super::{Violation, ViolationView};

/// One expected input block: where it sits in the request, its role, and the fact it must show.
struct Expected {
    label: String,
    role: &'static str,
    fact: Fact,
    part: Value,
    /// Which message of the request sent it; `None` for a system part.
    message: Option<usize>,
    /// The occurrence names a conversation fact, rather than content only the request carries.
    is_fact: bool,
}

/// The role a reconstruction shows a part under: a tool's result is the tool's turn, whatever message
/// carried it, and a provider's own names for the assistant and the tool are normalised.
pub(super) fn shown_role(message_role: &str, part: &Value) -> &'static str {
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
pub(super) fn as_fact(
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

/// Whether an occurrence's lineage names a conversation fact, which the fact checks then account for.
fn names_fact(occurrence: &Occurrence) -> bool {
    [&occurrence.new_fact, &occurrence.replay_of]
        .into_iter()
        .flatten()
        .any(|named| named.starts_with("fact-"))
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
                is_fact: names_fact(occurrence),
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
                    is_fact: names_fact(occurrence),
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

/// Accept a tool call or result shown inside its own message's span of the input, in another position.
///
/// A message's parallel calls - and the results answering them - are a batch: they were requested at once
/// and a reconstruction may order them by completion. Nothing else is: text and instructions are a
/// sequence wherever they sit, so moving one is `request.order`.
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
    let batched = |part: &Value| {
        matches!(
            part.get("type").and_then(Value::as_str),
            Some("tool_call" | "tool_result")
        )
    };
    for i in 0..expected.len() {
        if assigned_expected[i] || !batched(&expected[i].part) {
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

/// Which of a span view's blocks a call's request is shown by, for the mutations that move them.
///
/// Only the blocks whose occurrence is a sequence - not a message's parallel tool batch - so a mutation
/// that reorders them is a defect the check must catch rather than a permutation it accepts.
pub(super) fn sequenced_inputs(
    truth: &Truth,
    recon: &Recon,
    matching: &Matching,
) -> Option<(usize, Vec<usize>)> {
    let recorded = truth.requests.get(&recon.fixture)?;
    for (call, request) in &recorded.calls {
        let Some(&generation) = matching.span_of.get(call) else {
            continue;
        };
        let span = &recon.generations[generation].span;
        let Some(view) = recon
            .views
            .iter()
            .position(|v| v.kind == ViewKind::Span && &v.key == span)
        else {
            continue;
        };
        let inputs: Vec<usize> = recon.views[view]
            .blocks
            .iter()
            .enumerate()
            .filter(|(_, b)| !b.output)
            .map(|(i, _)| i)
            .collect();
        let blocks: Vec<&Block> = inputs
            .iter()
            .map(|&i| &recon.views[view].blocks[i])
            .collect();
        let wanted = expected(call, request, &rewrites(request, recon, &blocks));
        let pairs = assignment(&wanted, &blocks);
        let shown: Vec<usize> = pairs
            .iter()
            .filter(|&&(e, _)| {
                !matches!(
                    wanted[e].part.get("type").and_then(Value::as_str),
                    Some("tool_call" | "tool_result")
                )
            })
            .map(|&(_, b)| inputs[b])
            .collect();
        if shown.len() >= 2 {
            return Some((view, shown));
        }
    }
    None
}

/// Requests against span inputs, for every matched call the fixture's transcript recorded.
pub(super) fn check_requests(
    truth: &Truth,
    recon: &Recon,
    matching: &Matching,
    out: &mut Vec<Violation>,
) -> BTreeSet<String> {
    // What a request accounts for that no conversation fact holds: the client's own preamble, the
    // environment block it appends, a turn it composed. The conversation views may show it - the model was
    // sent it - and `extra.unexplained` would otherwise call it content from nowhere.
    let mut accounted = BTreeSet::new();
    let Some(recorded) = truth.requests.get(&recon.fixture) else {
        return accounted;
    };
    // Every view's blocks, so content a request carried is accounted for wherever the reconstruction
    // shows it. Which span should show it is the assignment's business, below.
    let everywhere: Vec<&Block> = recon.views.iter().flat_map(|v| v.blocks.iter()).collect();
    for (call, request) in &recorded.calls {
        for item in expected(call, request, &BTreeMap::new())
            .iter()
            .filter(|item| !item.is_fact)
        {
            for block in everywhere.iter().filter(|block| matches(item, block)) {
                accounted.insert(block.identity.clone());
            }
        }
    }
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
                // A reconstruction that shows the part somewhere disproves any claim that the payloads
                // lack it: the parser read it from them. So absence counts only when no view shows it.
                let shown_elsewhere = everywhere.iter().any(|block| matches(item, block));
                match prove(sent.as_ref().unwrap_or(&item.fact), haystack(recon)) {
                    // Proven absent: the producer does not export this part of the request. That is a
                    // limitation of its telemetry, which `absence::request_limitations` documents per
                    // framework - never a violation, exactly as a declared and proven gap is not one.
                    Proof::Absent if !shown_elsewhere => continue,
                    Proof::Absent => (
                        "request.missing",
                        format!(
                            "the conversation shows this {} and the span sent it does not",
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
    accounted
}
