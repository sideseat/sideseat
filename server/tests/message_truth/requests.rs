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

mod python;
mod rewrites;
#[cfg(test)]
mod tests;

use rewrites::{rewrites, wire_ids};

/// What a fixture's recorded requests account for in its views.
#[derive(Default)]
pub(super) struct Accounted {
    /// Blocks a request explains that no conversation fact holds.
    pub blocks: BTreeSet<String>,
}

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
    /// The conversation facts this part renders, where it renders rather than states them.
    renders: Vec<String>,
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
        ("media", _) if part.get("reference").is_some() => (
            "user_media",
            json!({
                "modality": part["modality"],
                "media_type": part["media_type"],
                "source": part["source"],
                "reference": part["reference"],
            }),
            "reference",
        ),
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
        // Withheld: the model signed reasoning whose text the client never had, and sends it back as such.
        ("reasoning", _) if part["text"] == "" && part["signed"] == true => {
            ("reasoning", json!({"text": "", "signed": true}), "signed")
        }
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
        fixtures: None,
        seal: None,
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

/// The withheld reasoning facts a fixture's telemetry carries no signature for (`signature_not_exported`):
/// a request part re-sending one is shown unsigned, as the fact is.
fn unsigned(truth: &Truth) -> BTreeSet<&str> {
    truth
        .facts
        .iter()
        .filter(|f| f.kind == "reasoning" && f.value.get("signed") == Some(&Value::Bool(false)))
        .map(|f| f.id.as_str())
        .collect()
}

fn expected(
    call: &str,
    request: &CallRequest,
    rewrites: &BTreeMap<String, String>,
    unsigned: &BTreeSet<&str>,
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
                renders: occurrence.renders.clone(),
            });
        }
    }
    for (m, message) in request.messages.iter().enumerate() {
        for (p, occurrence) in message.parts.iter().enumerate() {
            let role = shown_role(&message.role, &occurrence.part);
            let label = format!("{call}:m{m}.{p}");
            if let Some(mut fact) = as_fact(&label, role, &occurrence.part, rewrites) {
                if occurrence
                    .replay_of
                    .as_deref()
                    .is_some_and(|f| unsigned.contains(f))
                {
                    fact.value["signed"] = Value::Bool(false);
                }
                out.push(Expected {
                    label,
                    role,
                    fact,
                    part: occurrence.part.clone(),
                    message: Some(m),
                    is_fact: names_fact(occurrence),
                    renders: occurrence.renders.clone(),
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
    block.role == expected.role
        && (!matches!(shows(&expected.fact, block, None), Shows::No)
            || renders_as_sent(expected, block)
            || reduced_as_sent(expected, block))
}

/// A tool result the request sent in an encoding of its own - a text part holding the JSON of its content
/// list - shown in the canonical form the view reduces a result to. The sent value is read through the same
/// encodings the view's side is, and one of them must be the shown value exactly.
fn reduced_as_sent(expected: &Expected, block: &Block) -> bool {
    if expected.fact.kind != "tool_result" || !block.is_tool_result() {
        return false;
    }
    super::predicates::interpretations(&expected.fact.value["value"], 0)
        .into_iter()
        .skip(1)
        .any(|value| {
            let mut fact = expected.fact.clone();
            fact.value["value"] = value;
            !matches!(shows(&fact, block, None), Shows::No)
        })
}

/// A tool result the client sent as Python's `str()` of what the tool returned, shown as that value.
///
/// Accepted only as a bijection: the shown value's `repr` must be the sent text byte for byte - its quotes,
/// `True`/`None`, key order, spacing - so the two are one rendering of each other and nothing was lost or
/// added between them. A near miss (a key reordered, a string quoted another way, a tuple shown as a list)
/// stays a mismatch.
fn renders_as_sent(expected: &Expected, block: &Block) -> bool {
    if expected.fact.kind != "tool_result" || !block.is_tool_result() {
        return false;
    }
    let (Some(sent), Some(shown)) = (
        expected.fact.value.get("value").and_then(Value::as_str),
        block.content.get("content"),
    ) else {
        return false;
    };
    // Through the same encodings the semantic comparison reads a shown result in - JSON in a string, text
    // parts, an envelope - so a result the view holds in any of them is the value it holds.
    super::predicates::interpretations(shown, 0)
        .into_iter()
        .filter(|value| !value.is_string())
        .any(|value| {
            if python::repr(&value).as_deref() != Some(sent) {
                return false;
            }
            let mut fact = expected.fact.clone();
            fact.value["value"] = value;
            !matches!(shows(&fact, block, None), Shows::No)
        })
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

/// Accept a tool call or result shown inside its own batch's span of the input, in another position.
///
/// A message's parallel calls - and the results answering them, in one message or in a run of result
/// messages - are a batch: they were requested at once and a reconstruction may order them by completion.
/// Nothing else is: text and instructions are a sequence wherever they sit, so moving one is
/// `request.order`.
fn recover_within_messages(
    expected: &[Expected],
    blocks: &[&Block],
    pairs: &[(usize, usize)],
    assigned_expected: &mut [bool],
    assigned_block: &mut [bool],
) {
    // A turn's parallel results may each be a message of their own - a run of consecutive messages holding
    // nothing but results - and are one batch all the same, as the results inside one message are.
    let only_results = |message: usize| {
        expected
            .iter()
            .filter(|e| e.message == Some(message))
            .all(|e| e.part.get("type").and_then(Value::as_str) == Some("tool_result"))
    };
    let batch_of = |item: &Expected| -> Option<usize> {
        let message = item.message?;
        if !only_results(message) {
            return Some(message);
        }
        let mut first = message;
        while first > 0
            && expected.iter().any(|e| e.message == Some(first - 1))
            && only_results(first - 1)
        {
            first -= 1;
        }
        Some(first)
    };
    let span_of = |batch: Option<usize>| {
        let placed: Vec<usize> = pairs
            .iter()
            .filter(|&&(e, _)| batch_of(&expected[e]) == batch)
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
        let Some((first, last)) = span_of(batch_of(&expected[i])) else {
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
    let issued = wire_ids(truth, recorded);
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
        let wanted = expected(
            call,
            request,
            &rewrites(request, recon, &blocks, &issued),
            &unsigned(truth),
        );
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

/// The truth without the request-only facts this fixture's payloads do not carry.
///
/// Such a fact states what the model was told; the telemetry does not, so no reconstruction can show it, and
/// demanding it would report a producer's limitation as a parsing defect. The proof is over the payloads
/// alone, so the withdrawal never depends on what the parser produced; `absence::request_limitations`
/// documents the same parts.
pub(super) fn without_unexported_facts(truth: &Truth, recon: &Recon) -> Truth {
    let withdrawn: BTreeSet<String> = truth
        .facts
        .iter()
        .filter(|fact| fact.fixtures.is_some() && fact.require.is_some())
        .filter(|fact| match prove(fact, haystack(recon)) {
            Proof::Absent => true,
            // Exported only cut short, as a preview: no reconstruction can show it whole either.
            Proof::Partial(_) => {
                super::absence::truncated_at(fact.text(), haystack(recon)).is_some()
            }
            _ => false,
        })
        .map(|fact| fact.id.clone())
        .collect();
    let mut out = truth.clone();
    out.withdraw(&withdrawn);
    out
}

/// Which calls each fact a request carries was sent to: its home is wherever the model was told it.
///
/// A client's preamble and the environment block it appends go with *every* request of a session, so a fact
/// for them belongs to every trace that was sent it, and a block showing it belongs on any of those spans.
/// Only facts the requests mint, which is why an ordinary turn's home stays the call that prompted it.
pub(super) fn sent_to(truth: &Truth, recon: &Recon) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let Some(recorded) = truth.requests.get(&recon.fixture) else {
        return out;
    };
    let scoped: BTreeSet<&str> = truth
        .facts
        .iter()
        .filter(|fact| fact.fixtures.is_some())
        .map(|fact| fact.id.as_str())
        .collect();
    for (call, request) in &recorded.calls {
        // The instruction is re-sent with every request and is owed wherever it was sent. A message is
        // owed where it was first told: one a later request hands back is history, which the views show
        // once, exactly as they show a conversation's own turns.
        let system = request
            .system
            .iter()
            .flat_map(|o| [&o.new_fact, &o.replay_of].into_iter().flatten());
        let messages = request
            .messages
            .iter()
            .flat_map(|m| m.parts.iter())
            .filter_map(|o| o.new_fact.as_ref());
        for named in system
            .chain(messages)
            .filter(|named| scoped.contains(named.as_str()))
        {
            out.entry(named.clone()).or_default().insert(call.clone());
        }
    }
    out
}

/// The span a call was sent on, as the matcher established it from the call's output - or, for a call it
/// could not find, the span of the nearest call of the same conversation it did find, earlier first.
///
/// A request's preamble goes with every call, including one whose output no span shows; the trace that
/// call ran in is then the trace of its neighbours, which share its turn.
pub(super) fn span_sent(truth: &Truth, matching: &Matching, call: &str) -> Option<usize> {
    if let Some(&span) = matching.span_of.get(call) {
        return Some(span);
    }
    let at = truth.calls.iter().position(|c| c.id == call)?;
    let conversation = &truth.calls[at].conversation;
    let found = |c: &&super::truth::Call| {
        &c.conversation == conversation && c.succeeded() && matching.span_of.contains_key(&c.id)
    };
    truth.calls[..at]
        .iter()
        .rev()
        .find(found)
        .or_else(|| truth.calls[at + 1..].iter().find(found))
        .and_then(|c| matching.span_of.get(&c.id).copied())
}

/// Where request part `i` may be shown: after every block an earlier settled part was shown by, and before
/// every block a later one was. `anchors` holds (part, first block, last block).
fn merge_window(
    i: usize,
    anchors: &[(usize, usize, usize)],
    blocks: usize,
) -> std::ops::Range<usize> {
    let after = anchors
        .iter()
        .filter(|&&(e, _, _)| e < i)
        .map(|&(_, _, last)| last + 1)
        .max();
    let before = anchors
        .iter()
        .filter(|&&(e, _, _)| e > i)
        .map(|&(_, first, _)| first)
        .min();
    after.unwrap_or(0)..before.unwrap_or(blocks)
}

/// The consecutive input blocks a merged request part is shown as, if it is: the client joins consecutive
/// messages of one role with a newline, and an empty one contributes only its newline.
///
/// Exact equality on that join, never a looser comparison, and only where the join is evidenced: several
/// blocks, or a newline at an end where the span's own payload holds an empty message of that role next to
/// the part - so an ordinary part still needs a block of its own, and a lost byte is no merge. The blocks
/// must sit in `window`, between those the request's neighbouring parts were shown by. The empty message
/// that evidences a newline must neighbour the payload message the block itself shows: the n-th block of a
/// role and text is bound to the payload's n-th message of that role and text, never to another message that
/// happens to read the same.
fn merged_run(
    item: &Expected,
    blocks: &[&Block],
    assigned_block: &[bool],
    explained: &[bool],
    window: std::ops::Range<usize>,
    neighbours: &EmptyNeighbours,
) -> Option<std::ops::RangeInclusive<usize>> {
    const JOIN: &str = "\n";
    const LONGEST: usize = 4;
    let text = item.part.get("text").and_then(Value::as_str)?;
    let free = |j: usize| !assigned_block[j] && !explained[j] && blocks[j].role == item.role;
    // Which of the span's blocks of the same role and text this one is, counted from the first.
    let occurrence = |j: usize| {
        (0..j)
            .filter(|&k| {
                blocks[k].role == blocks[j].role
                    && blocks[k].text().is_some()
                    && blocks[k].text() == blocks[j].text()
            })
            .count()
    };
    let end_of_window = window.end.min(blocks.len());
    (window.start..end_of_window).find_map(|start| {
        let mut joined = String::new();
        let mut first: Option<&str> = None;
        for (end, block) in blocks
            .iter()
            .enumerate()
            .take(end_of_window)
            .skip(start)
            .take(LONGEST)
        {
            if !free(end) {
                return None;
            }
            let piece = block.text()?;
            let head = *first.get_or_insert(piece);
            if end > start {
                joined.push_str(JOIN);
            }
            joined.push_str(piece);
            let several = end > start;
            let leading = neighbours.empty_before(head, occurrence(start), item.role);
            let trailing = neighbours.empty_after(piece, occurrence(end), item.role);
            let candidates = [
                (joined.clone(), several),
                (format!("{JOIN}{joined}"), leading),
                (format!("{joined}{JOIN}"), trailing),
                (format!("{JOIN}{joined}{JOIN}"), leading && trailing),
            ];
            if candidates
                .iter()
                .any(|(form, evidenced)| *evidenced && form == text)
            {
                return Some(start..=end);
            }
        }
        None
    })
}

/// A span's indexed input messages that carry a role and no content: the empty turns a client merges into
/// their neighbours, leaving only the newline it joins them with.
///
/// Read from the span's own exported attributes, `<family>.<n>.message.role` beside its content keys, so the
/// evidence is the payload and never the reconstruction under test.
#[derive(Default)]
pub(super) struct EmptyNeighbours {
    /// (family, index) -> role, for every indexed message.
    roles: BTreeMap<(String, u64), String>,
    /// (family, index) of every message that has content.
    filled: BTreeSet<(String, u64)>,
    /// text -> (family, index) where a message holds exactly that text.
    texts: BTreeMap<String, Vec<(String, u64)>>,
}

impl EmptyNeighbours {
    fn of_span(recon: &Recon, span: &str) -> Self {
        use opentelemetry_proto::tonic::common::v1::any_value::Value as Any;
        let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let mut out = EmptyNeighbours::default();
        for path in &recon.paths {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
            if !name.is_some_and(|n| n.starts_with("req-")) {
                continue;
            }
            let request = crate::decode_request(path);
            let spans = request
                .resource_spans
                .iter()
                .flat_map(|r| r.scope_spans.iter())
                .flat_map(|s| s.spans.iter())
                .filter(|s| hex(&s.span_id) == span);
            for found in spans {
                for attribute in &found.attributes {
                    if let Some(Any::StringValue(text)) =
                        attribute.value.as_ref().and_then(|v| v.value.as_ref())
                    {
                        out.record(&attribute.key, text.clone());
                    }
                }
            }
        }
        out
    }

    fn record(&mut self, key: &str, value: String) {
        let Some((head, member)) = key.split_once(".message.") else {
            return;
        };
        let Some((family, index)) = head
            .rsplit_once('.')
            .and_then(|(family, index)| Some((family.to_string(), index.parse::<u64>().ok()?)))
        else {
            return;
        };
        if member == "role" {
            self.roles.insert((family, index), value);
            return;
        }
        if member.starts_with("content") {
            if member == "content" || member.ends_with(".text") {
                self.texts
                    .entry(value)
                    .or_default()
                    .push((family.clone(), index));
            }
            self.filled.insert((family, index));
        }
    }

    /// Whether the `occurrence`-th payload message holding exactly `text` that could be shown under `role`, in
    /// payload order, has an empty message of that role at `offset` from it.
    ///
    /// A holder of another known role is not the message the block shows, so its neighbours evidence nothing
    /// about it. A holder under a role spelling the rubric does not know - a producer's own pseudo-role, which
    /// the reconstruction shows under a canonical one - may be it, and is kept.
    fn neighbour(&self, text: &str, occurrence: usize, role: &str, offset: i64) -> bool {
        let of_role = |key: &(String, u64)| {
            self.roles
                .get(key)
                .is_some_and(|r| shown_role(r, &Value::Null) == role)
        };
        let could_be = |key: &(String, u64)| {
            self.roles
                .get(key)
                .is_some_and(|r| matches!(shown_role(r, &Value::Null), "unknown") || of_role(key))
        };
        let mut holders: Vec<&(String, u64)> = self
            .texts
            .get(text)
            .into_iter()
            .flatten()
            .filter(|key| could_be(key))
            .collect();
        holders.sort_unstable();
        holders.dedup();
        holders.get(occurrence).is_some_and(|(family, index)| {
            index.checked_add_signed(offset).is_some_and(|other| {
                let key = (family.clone(), other);
                !self.filled.contains(&key) && of_role(&key)
            })
        })
    }

    fn empty_before(&self, text: &str, occurrence: usize, role: &str) -> bool {
        self.neighbour(text, occurrence, role, -1)
    }

    fn empty_after(&self, text: &str, occurrence: usize, role: &str) -> bool {
        self.neighbour(text, occurrence, role, 1)
    }
}

/// Requests against span inputs, for every matched call the fixture's transcript recorded.
pub(super) fn check_requests(
    truth: &Truth,
    recon: &Recon,
    matching: &Matching,
    out: &mut Vec<Violation>,
) -> Accounted {
    // What a request accounts for that no conversation fact holds: the client's own preamble, the
    // environment block it appends, a turn it composed. The conversation views may show it - the model was
    // sent it - and `extra.unexplained` would otherwise call it content from nowhere.
    let mut accounted = Accounted::default();
    let Some(recorded) = truth.requests.get(&recon.fixture) else {
        return accounted;
    };
    let issued = wire_ids(truth, recorded);
    // Every view's blocks, so content a request carried is accounted for wherever the reconstruction
    // shows it. Which span should show it is the assignment's business, below.
    let everywhere: Vec<&Block> = recon.views.iter().flat_map(|v| v.blocks.iter()).collect();
    for (call, request) in &recorded.calls {
        for item in expected(call, request, &BTreeMap::new(), &unsigned(truth))
            .iter()
            // Content of its own only: a rendering of facts the conversation has is explained by those
            // facts and the framework's declared restatements, which must still be used.
            .filter(|item| !item.is_fact && item.renders.is_empty())
        {
            for block in everywhere.iter().filter(|block| matches(item, block)) {
                accounted.blocks.insert(block.identity.clone());
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
        let wanted = expected(
            call,
            request,
            &rewrites(request, recon, &blocks, &issued),
            &unsigned(truth),
        );
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
        let neighbours = EmptyNeighbours::of_span(recon, span);
        // Where each settled part was shown, as (part, first block, last block): the exact assignment, and
        // every merged part as it is recovered, so two merges are held to the request's order between them
        // as much as a merge and an exact part are.
        let mut anchors: Vec<(usize, usize, usize)> =
            pairs.iter().map(|&(e, b)| (e, b, b)).collect();
        for (i, item) in wanted
            .iter()
            .enumerate()
            .filter(|(i, _)| !assigned_expected[*i])
        {
            // A message the client merged from consecutive ones before sending - their texts joined by a
            // newline, an empty one leaving only its newline - while the telemetry exports them apart. The
            // boundary is not in the payloads, but every part is, so the span must still show each, in
            // order, under the role the request sent them with. Only the boundary is excused.
            // Between the blocks the parts around it were shown by, so a merge keeps the request's order.
            let window = merge_window(i, &anchors, blocks.len());
            if let Some(run) = merged_run(
                item,
                &blocks,
                &assigned_block,
                &explained,
                window,
                &neighbours,
            ) {
                anchors.push((i, *run.start(), *run.end()));
                for j in run {
                    explained[j] = true;
                }
                continue;
            }
            let elsewhere = (0..blocks.len())
                .find(|&j| !assigned_block[j] && !explained[j] && matches(item, blocks[j]));
            let (assertion, detail) = if let Some(j) = elsewhere {
                explained[j] = true;
                (
                    "request.order",
                    "shown, but not where the request put it".to_string(),
                )
            } else if let Some(run) = merged_run(
                item,
                &blocks,
                &assigned_block,
                &explained,
                0..blocks.len(),
                &neighbours,
            ) {
                // Merged, but outside the place the request's other parts leave for it.
                for j in run {
                    explained[j] = true;
                }
                (
                    "request.order",
                    "shown merged, but not where the request put it".to_string(),
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
                let sent_fact = sent.as_ref().unwrap_or(&item.fact);
                match prove(sent_fact, haystack(recon)) {
                    // Proven absent: the producer does not export this part of the request. That is a
                    // limitation of its telemetry, which `absence::request_limitations` documents per
                    // framework - never a violation, exactly as a declared and proven gap is not one.
                    Proof::Absent if !shown_elsewhere => continue,
                    // Exported only as a truncated preview: a limitation of the telemetry too, documented
                    // beside the absent parts.
                    Proof::Partial(_)
                        if !shown_elsewhere
                            && super::absence::truncated_at(sent_fact.text(), haystack(recon))
                                .is_some() =>
                    {
                        continue;
                    }
                    Proof::Absent => (
                        "request.missing",
                        format!(
                            "the conversation shows this {} and the span sent it does not",
                            item.fact.kind
                        ),
                    ),
                    Proof::Present(at) | Proof::Partial(at) => (
                        "request.missing",
                        match item.renders.as_slice() {
                            // Naming what a part renders is what makes the finding actionable: a
                            // framework's own wrapper around turns the conversation already has reads
                            // differently from content of its own.
                            [] => format!(
                                "{at} carries this {}, and no input shows it",
                                item.fact.kind
                            ),
                            rendered => format!(
                                "{at} carries this {}, rendering {}, and no input shows it",
                                item.fact.kind,
                                rendered.join(", ")
                            ),
                        },
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
