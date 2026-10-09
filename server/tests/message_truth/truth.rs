//! The `sideseat.truth/3` document and its internal consistency.
//!
//! The document is written by `python -m harness truth` (examples/python/harness/harness/truth) from the
//! recorded model responses and the scenario scripts; nothing here derives truth, it only reads it. The
//! types are strict - an unknown member is an error - so a format change cannot be read as the old format.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

pub(super) const FORMAT: &str = "sideseat.truth/3";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Truth {
    pub format: String,
    pub producer: String,
    pub scenario: String,
    pub fixtures: Vec<String>,
    pub source: Source,
    pub generator: String,
    pub topology: String,
    pub request_bodies: String,
    pub model_calls: usize,
    pub calls: Vec<Call>,
    pub conversations: Vec<Conversation>,
    pub facts: Vec<Fact>,
    pub edges: Vec<Edge>,
    pub gaps: Vec<Gap>,
    /// What each fixture's calls were sent, from that fixture's request transcript (rubric v3). A fixture
    /// without one has no entry.
    #[serde(default)]
    pub requests: BTreeMap<String, FixtureRequests>,
}

/// One fixture's recorded requests, by the call each answered.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FixtureRequests {
    pub transcript: String,
    pub calls: BTreeMap<String, CallRequest>,
    /// Model requests no truth call answered: a retry the cassette does not hold, or a course change.
    pub unpaired_requests: Vec<String>,
}

impl FixtureRequests {
    /// What makes the recorded requests unusable as a truth: a missing transcript, a call the document
    /// does not have or under another API, a part without exactly one lineage or naming no fact.
    pub fn defects(&self, fixture: &str, truth: &Truth) -> Vec<String> {
        let mut out = Vec::new();
        if !repo_root().join(&self.transcript).exists() {
            out.push(format!(
                "{fixture}: transcript {} is missing",
                self.transcript
            ));
        }
        if !self.unpaired_requests.is_empty() {
            out.push(format!(
                "{fixture}: {} recorded request(s) no call answered",
                self.unpaired_requests.len()
            ));
        }
        let facts: BTreeSet<&str> = truth.facts.iter().map(|f| f.id.as_str()).collect();
        for (id, request) in &self.calls {
            match truth.calls.iter().find(|c| &c.id == id) {
                None => out.push(format!("{fixture}: request for unknown call {id}")),
                Some(call) if call.api.split('.').next() != request.api.split('.').next() => out
                    .push(format!(
                        "{fixture}: {id} was answered over {} but sent over {}",
                        call.api, request.api
                    )),
                Some(_) => {}
            }
            if request.tools.iter().any(String::is_empty) {
                out.push(format!("{fixture}: {id} offers a tool without a name"));
            }
            let parts = request
                .system
                .iter()
                .chain(request.messages.iter().flat_map(|m| m.parts.iter()));
            for occurrence in parts {
                let lineages = [
                    &occurrence.new_fact,
                    &occurrence.replay_of,
                    &occurrence.new,
                    &occurrence.lineage_unknown,
                ];
                if lineages.iter().filter(|l| l.is_some()).count() != 1 {
                    out.push(format!(
                        "{fixture}: {id} has a part without exactly one lineage"
                    ));
                }
                for named in [&occurrence.new_fact, &occurrence.replay_of]
                    .into_iter()
                    .flatten()
                {
                    if named.starts_with("fact-") && !facts.contains(named.as_str()) {
                        out.push(format!("{fixture}: {id} names unknown {named}"));
                    }
                }
            }
        }
        out
    }
}

/// What one call was sent: system parts and messages, each part an occurrence with its lineage.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CallRequest {
    pub api: String,
    pub system: Vec<Occurrence>,
    pub messages: Vec<RequestMessage>,
    pub tools: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RequestMessage {
    pub role: String,
    pub parts: Vec<Occurrence>,
}

/// One part of a request and where it came from. Exactly one lineage member is set.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Occurrence {
    pub part: Value,
    #[serde(default)]
    pub new_fact: Option<String>,
    #[serde(default)]
    pub replay_of: Option<String>,
    #[serde(default)]
    pub new: Option<String>,
    #[serde(default)]
    pub lineage_unknown: Option<String>,
    /// The conversation facts this part renders: a framework's state message quoting the task, a digest of
    /// the turn so far. It is no fact of its own - two facts must never demand one block - and the views are
    /// not obliged to show it, though the span that was sent it must.
    #[serde(default)]
    pub renders: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Source {
    pub kind: String,
    pub path: String,
    pub sha256: String,
    // Provenance the Rust side has no use for, named so the strict reader still accepts it.
    #[serde(default, rename = "non_model_requests")]
    _non_model_requests: Option<u64>,
    #[serde(default, rename = "surface")]
    _surface: Option<String>,
    #[serde(default, rename = "values_from")]
    _values_from: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Call {
    pub id: String,
    pub attempt: u32,
    pub outcome: String,
    pub conversation: String,
    pub api: String,
    #[serde(rename = "streamed")]
    _streamed: bool,
    pub status: Option<u16>,
    pub model: Option<String>,
    pub response_model: Option<String>,
    pub response_id: Option<String>,
    /// The provider's own word; `finish` is its normalised form, which is what is compared.
    pub stop_reason: Option<String>,
    pub finish: Option<String>,
    pub usage: Option<Usage>,
    pub outputs: Vec<String>,
    #[serde(default)]
    pub error: Option<String>,
}

impl Call {
    pub fn succeeded(&self) -> bool {
        self.outcome == "success"
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Usage {
    pub input: Option<i64>,
    pub output: Option<i64>,
    pub cache_read: Option<i64>,
    pub cache_write: Option<i64>,
    pub reasoning: Option<i64>,
    pub input_includes_cache: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Conversation {
    pub id: String,
    pub sequence: Vec<String>,
    pub final_answers: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Fact {
    pub id: String,
    pub kind: String,
    pub role: String,
    pub conversation: String,
    pub evidence: String,
    pub value: Value,
    pub require: Option<Requirement>,
    #[serde(default)]
    pub call: Option<String>,
    /// The fixtures whose requests carry this fact, where only some do; absent means every fixture.
    ///
    /// Content only a request carries - a client's preamble, the environment block it appends - is part of
    /// the conversation the model saw, and which fixtures sent it is a property of each run, not of the
    /// scenario. `for_fixture` projects a document onto one fixture before anything is checked.
    #[serde(default)]
    pub fixtures: Option<Vec<String>>,
    /// The SHA-256 of signed reasoning's signature, where the provider returned one: the identity an
    /// absence proof searches the telemetry for, so one withheld turn is told from another although
    /// neither has text. Outside `value`, because no view shows it.
    #[serde(default)]
    pub seal: Option<String>,
}

impl Fact {
    pub fn text(&self) -> &str {
        self.value.get("text").and_then(Value::as_str).unwrap_or("")
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Requirement {
    pub anchor: String,
    pub views: Vec<String>,
    pub cardinality: String,
    #[serde(rename = "match")]
    pub matcher: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Edge {
    pub kind: String,
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub before: Option<String>,
    #[serde(default)]
    pub after: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Gap {
    pub fact: String,
    pub reason: String,
    pub detail: String,
    #[serde(default)]
    pub subject: Option<String>,
    /// The capture modes (`native@1.0b1`) the gap holds for; empty for every fixture of the truth. A
    /// release that exports what another omits is told apart here, never by weakening the fact.
    #[serde(default)]
    pub modes: Vec<String>,
}

impl Gap {
    /// Whether the gap holds for this fixture label.
    pub fn holds_for(&self, fixture: &str) -> bool {
        self.modes.is_empty()
            || fixture
                .split('/')
                .nth(1)
                .is_some_and(|mode| self.modes.iter().any(|m| m == mode))
    }
}

impl Truth {
    /// Remove these facts and everything that names them, leaving a document still internally consistent.
    pub fn withdraw(&mut self, facts: &BTreeSet<String>) {
        if facts.is_empty() {
            return;
        }
        self.facts.retain(|f| !facts.contains(&f.id));
        for conversation in &mut self.conversations {
            conversation.sequence.retain(|id| !facts.contains(id));
            conversation.final_answers.retain(|id| !facts.contains(id));
        }
        self.edges.retain(|edge| {
            ![&edge.from, &edge.to, &edge.before, &edge.after]
                .into_iter()
                .flatten()
                .any(|id| facts.contains(id))
        });
        self.gaps
            .retain(|gap| gap.subject.as_ref().is_none_or(|s| !facts.contains(s)));
        for call in &mut self.calls {
            call.outputs.retain(|id| !facts.contains(id));
        }
    }

    /// The truth as one fixture is checked against it: only the gaps that hold for it, a tool call
    /// whose id that fixture's telemetry does not carry (`id_not_exported`) asserted without it - its
    /// wire id kept aside as `wire_id` - withheld reasoning whose signature it does not carry
    /// (`signature_not_exported`) asserted unsigned, and a response part its producing span does not carry
    /// (`output_not_exported`) owed by the conversation views only.
    pub fn for_fixture(&self, fixture: &str) -> Truth {
        let mut truth = self.clone();
        // Facts only some fixtures' requests carry: the projection drops the rest, and everything that
        // names them, so no check sees a fact this run never sent.
        let dropped: BTreeSet<String> = truth
            .facts
            .iter()
            .filter(|f| {
                f.fixtures
                    .as_ref()
                    .is_some_and(|only| !only.iter().any(|listed| listed == fixture))
            })
            .map(|f| f.id.clone())
            .collect();
        truth.withdraw(&dropped);
        truth.gaps.retain(|gap| gap.holds_for(fixture));
        // A withdrawal some captures only make - a release that left the fact out - is the fixture's: the fact
        // stays asserted in the document, owed by every other capture, and is unasserted here alone.
        let withdrawn_here: BTreeSet<String> = truth
            .gaps
            .iter()
            .filter(|g| !g.modes.is_empty() && gap_effects(&g.reason).is_some_and(|e| e.withdraws))
            .filter_map(|g| g.subject.clone())
            .collect();
        for fact in truth
            .facts
            .iter_mut()
            .filter(|f| withdrawn_here.contains(&f.id))
        {
            fact.require = None;
        }
        let unexported: BTreeSet<String> = truth
            .gaps
            .iter()
            .filter(|g| g.reason == "id_not_exported")
            .filter_map(|g| g.subject.clone())
            .collect();
        for fact in truth
            .facts
            .iter_mut()
            .filter(|f| unexported.contains(&f.id))
        {
            if let Some(id) = fact.value.get("id").cloned() {
                fact.value["wire_id"] = id;
                fact.value["id"] = Value::Null;
            }
        }
        let unsigned: BTreeSet<String> = truth
            .gaps
            .iter()
            .filter(|g| g.reason == "signature_not_exported")
            .filter_map(|g| g.subject.clone())
            .collect();
        for fact in truth.facts.iter_mut().filter(|f| unsigned.contains(&f.id)) {
            fact.value["signed"] = Value::Bool(false);
        }
        let unsigned_on_span: BTreeSet<String> = truth
            .gaps
            .iter()
            .filter(|g| g.reason == "span_signature_not_exported")
            .filter_map(|g| g.subject.clone())
            .collect();
        for fact in truth
            .facts
            .iter_mut()
            .filter(|f| unsigned_on_span.contains(&f.id))
        {
            fact.value[UNSIGNED_ON_SPAN] = Value::Bool(true);
        }
        let off_span: BTreeSet<String> = truth
            .gaps
            .iter()
            .filter(|g| g.reason == "output_not_exported")
            .filter_map(|g| g.subject.clone())
            .collect();
        for fact in truth.facts.iter_mut().filter(|f| off_span.contains(&f.id)) {
            if let Some(require) = &mut fact.require {
                require.views.retain(|v| v != "span");
            }
        }
        let plain: BTreeSet<String> = truth
            .gaps
            .iter()
            .filter(|g| g.reason == "kind_not_exported")
            .filter_map(|g| g.subject.clone())
            .collect();
        for fact in truth.facts.iter_mut().filter(|f| plain.contains(&f.id)) {
            fact.kind = "text".to_string();
        }
        truth
    }
}

impl Gap {
    /// The gap that makes one fact unknowable, for building test truths.
    pub fn for_subject(fact: &str) -> Self {
        Gap {
            fact: "assistant_text".to_string(),
            reason: "answer_quotes_framework_rendering".to_string(),
            detail: "made unknowable by a test".to_string(),
            subject: Some(fact.to_string()),
            modes: Vec::new(),
        }
    }
}

/// What a gap names: one fact, one call, or nothing (a gap about the whole scenario).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GapSubject {
    Fact,
    Call,
    None,
}

/// What a gap reason does. Each effect is separate because the reasons combine them differently: an
/// unknowable answer withdraws its fact and may explain the block a reconstruction shows in its place,
/// while a fact the telemetry never carried is withdrawn but explains nothing - and is accepted only with
/// an absence proof (`absence`).
#[derive(Debug, Clone, Copy)]
pub(super) struct GapEffects {
    pub subject: GapSubject,
    /// The named fact is unasserted (`require: null`) because of this gap.
    pub withdraws: bool,
    /// The gap may account for a reconstructed block no fact claims (`explain`).
    pub explains_extra: bool,
    /// The fact's content must be shown absent from every captured payload of every fixture.
    pub needs_absence_proof: bool,
}

/// The member checking marks withheld reasoning with whose signature its producing span does not carry
/// (`span_signature_not_exported`): that span's view owes it unsigned.
pub(super) const UNSIGNED_ON_SPAN: &str = "unsigned_on_span";

/// The call metadata a `metadata_not_exported` gap may name, as `call.<field>` assertions spell it.
pub(super) const METADATA_FIELDS: &[&str] = &["model", "response_model", "response_id", "finish"];

/// The closed vocabulary of gap reasons and their effects; an unknown reason is a document defect.
pub(super) fn gap_effects(reason: &str) -> Option<GapEffects> {
    let effects = |subject, withdraws, explains_extra, needs_absence_proof| GapEffects {
        subject,
        withdraws,
        explains_extra,
        needs_absence_proof,
    };
    Some(match reason {
        // An encrypted reasoning item with no text at all, not yet owed: how it is shown waits on SideML
        // slice S1. Withheld reasoning with an empty text is owed, by its signed mark.
        "reasoning_text_omitted" => effects(GapSubject::Fact, true, true, false),
        // The oracle cannot know the value; whatever the reconstruction shows there is unchecked.
        "answer_quotes_framework_rendering" => effects(GapSubject::Fact, true, true, false),
        // Names the call whose result the oracle cannot compute; no result fact exists.
        "tool_not_deterministic" => effects(GapSubject::Fact, false, true, false),
        // A failed attempt owes no output.
        "no_output_obligation" => effects(GapSubject::Call, false, false, false),
        // The framework's own per-step state message restates the prompt in the call's request: one
        // user text containing the prompt, in that call's trace.
        "framework_restates_prompt" => effects(GapSubject::Call, false, true, false),
        // The telemetry does not carry the fact: proven, never assumed.
        "not_exported" => effects(GapSubject::Fact, true, false, true),
        // Only a tool call's wire id is missing: the call is still asserted, under whatever id it is shown.
        "id_not_exported" => effects(GapSubject::Fact, false, false, true),
        // Only withheld reasoning's signature is missing: the step is still asserted, shown unsigned.
        "signature_not_exported" => effects(GapSubject::Fact, false, false, true),
        // Only withheld reasoning's signature is missing from the span that produced it - another carrier
        // holds it: that span's view owes the step unsigned, the conversation views signed.
        "span_signature_not_exported" => effects(GapSubject::Fact, false, false, true),
        // A response's part is missing from the span that produced it - a later request re-sends it: only
        // that span's view stops owing it; the conversation views still do.
        "output_not_exported" => effects(GapSubject::Fact, false, false, true),
        // A model call's response is missing as a unit: its parts are asserted where the conversation
        // shows them, but no span records the call and the response's own grouping is unknown.
        "call_not_exported" => effects(GapSubject::Call, false, false, true),
        // A call's model or finish the producer states wrongly, where the right one is in no payload:
        // the span cannot state what the telemetry never carried.
        "metadata_not_exported" => effects(GapSubject::Call, false, false, true),
        // Visible reasoning a producer records as ordinary text, with nothing marking it as reasoning:
        // the text is asserted, as the text it was exported as.
        "kind_not_exported" => effects(GapSubject::Fact, false, false, true),
        "request_body_unrecorded"
        | "request_modelled"
        | "fake_model_echoes_request"
        | "multi_agent_routing"
        | "prompt_without_model_call" => effects(GapSubject::None, false, true, false),
        _ => return None,
    })
}

pub(super) fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the server crate sits in the repository root")
        .to_path_buf()
}

pub(super) fn truth_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/truth")
}

/// Every committed truth document, keyed `<producer>/<scenario>`.
pub(super) fn load_all() -> BTreeMap<String, Truth> {
    let mut out = BTreeMap::new();
    let root = truth_root();
    let Ok(producers) = std::fs::read_dir(&root) else {
        return out;
    };
    for producer in producers.flatten().filter(|e| e.path().is_dir()) {
        for entry in std::fs::read_dir(producer.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
            let truth: Truth = serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("{} is not {FORMAT}: {e}", path.display()));
            let key = format!(
                "{}/{}",
                producer.file_name().to_string_lossy(),
                path.file_stem().unwrap_or_default().to_string_lossy()
            );
            out.insert(key, truth);
        }
    }
    out
}

/// Fixture labels no truth may describe, with the reason - the same rule the deriving CLI applies
/// (`harness.truth.cli._fixtures_without_truth`), so the two sides cannot disagree about coverage.
pub(super) fn reason_without_truth(label: &str) -> Option<&'static str> {
    let mut parts = label.split('/');
    let (producer, mode, scenario) = (parts.next()?, parts.next(), parts.next());
    if producer == "_synthetic" {
        return Some("hand-written shapes, no producer");
    }
    if mode == Some("legacy") {
        return Some("a pre-catalog capture with no cassette or script");
    }
    // One producer's logging channel, not the mode in itself: `claude-code/logs` is covered, because that
    // CLI's records carry a span per model call.
    if (producer, mode) == ("autogen", Some("logs")) {
        return Some(
            "the channel reports no span per model call, so the per-call truth has nothing to key on",
        );
    }
    if scenario == Some("canonical") {
        return Some("a pre-catalog capture whose emitting program is not in the repository");
    }
    // The fake model answers these frameworks' delegation tools from schemas the framework declares,
    // and those requests are not recorded, so the derivation refuses this scenario.
    if matches!(producer, "autogen" | "adk-go" | "genkit-go") && scenario == Some("multi_agent") {
        return Some("handoff answers depend on unrecorded framework schemas");
    }
    None
}

pub(super) fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
