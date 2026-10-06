//! The `sideseat.truth/2` document and its internal consistency.
//!
//! The document is written by `python -m harness truth` (examples/python/harness/harness/truth) from the
//! recorded model responses and the scenario scripts; nothing here derives truth, it only reads it. The
//! types are strict - an unknown member is an error - so a format change cannot be read as the old format.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

pub(super) const FORMAT: &str = "sideseat.truth/2";

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
    /// The truth as one fixture is checked against it: only the gaps that hold for it, and a tool call
    /// whose id that fixture's telemetry does not carry (`id_not_exported`) asserted without it - its
    /// wire id kept aside as `wire_id`.
    pub fn for_fixture(&self, fixture: &str) -> Truth {
        let mut truth = self.clone();
        truth.gaps.retain(|gap| gap.holds_for(fixture));
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

/// The closed vocabulary of gap reasons and their effects; an unknown reason is a document defect.
pub(super) fn gap_effects(reason: &str) -> Option<GapEffects> {
    let effects = |subject, withdraws, explains_extra, needs_absence_proof| GapEffects {
        subject,
        withdraws,
        explains_extra,
        needs_absence_proof,
    };
    Some(match reason {
        // The oracle cannot know the value; whatever the reconstruction shows there is unchecked.
        "reasoning_text_omitted" | "answer_quotes_framework_rendering" => {
            effects(GapSubject::Fact, true, true, false)
        }
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
        // A model call's response is missing as a unit: its parts are asserted where the conversation
        // shows them, but no span records the call and the response's own grouping is unknown.
        "call_not_exported" => effects(GapSubject::Call, false, false, true),
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

/// What is wrong with one document on its own: ids, references, sequencing, gaps, requirements.
pub(super) fn document_defects(key: &str, truth: &Truth) -> Vec<String> {
    let mut defects = Vec::new();
    let mut bad = |message: String| defects.push(format!("{key}: {message}"));

    if truth.format != FORMAT {
        bad(format!("format {:?}, expected {FORMAT}", truth.format));
    }
    if key != format!("{}/{}", truth.producer, truth.scenario) {
        bad(format!(
            "stored at {key} but names {}/{}",
            truth.producer, truth.scenario
        ));
    }
    if !["linear", "sessions", "multi_agent"].contains(&truth.topology.as_str()) {
        bad(format!("unknown topology {}", truth.topology));
    }
    if truth.generator.trim().is_empty() {
        bad("names no generator".to_string());
    }
    let bodies = match truth.source.kind.as_str() {
        "cassette" => "unrecorded",
        "fake-script" => "modelled",
        "program" => "program",
        other => {
            bad(format!("unknown source kind {other}"));
            ""
        }
    };
    if truth.request_bodies != bodies {
        bad(format!(
            "a {} source with request_bodies {}",
            truth.source.kind, truth.request_bodies
        ));
    }
    let successful = truth.calls.iter().filter(|c| c.succeeded()).count();
    if truth.model_calls != successful {
        bad(format!(
            "model_calls {} but {successful} successful calls",
            truth.model_calls
        ));
    }

    let mut ids = BTreeSet::new();
    for id in truth
        .calls
        .iter()
        .map(|c| &c.id)
        .chain(truth.facts.iter().map(|f| &f.id))
        .chain(truth.conversations.iter().map(|c| &c.id))
    {
        if !ids.insert(id.as_str()) {
            bad(format!("duplicate id {id}"));
        }
    }
    let facts: BTreeMap<&str, &Fact> = truth.facts.iter().map(|f| (f.id.as_str(), f)).collect();
    let calls: BTreeMap<&str, &Call> = truth.calls.iter().map(|c| (c.id.as_str(), c)).collect();
    let conversations: BTreeSet<&str> = truth.conversations.iter().map(|c| c.id.as_str()).collect();

    for (index, call) in truth.calls.iter().enumerate() {
        // A retry is the next attempt of the call before it, and only a failure is retried.
        let expected_attempt = match index.checked_sub(1).map(|i| &truth.calls[i]) {
            Some(previous) if !previous.succeeded() => previous.attempt + 1,
            _ => 1,
        };
        if call.attempt != expected_attempt {
            bad(format!(
                "{} is attempt {}, expected {expected_attempt}",
                call.id, call.attempt
            ));
        }
        if !conversations.contains(call.conversation.as_str()) {
            bad(format!("{} names unknown {}", call.id, call.conversation));
        }
        if !["success", "failed"].contains(&call.outcome.as_str()) {
            bad(format!("{} has outcome {}", call.id, call.outcome));
        }
        if !call.succeeded() && !call.outputs.is_empty() {
            bad(format!("failed {} lists outputs", call.id));
        }
        if call.api.is_empty() {
            bad(format!("{} names no api", call.id));
        }
        if !call.succeeded() && (call.error.is_none() || call.status.is_none_or(|s| s < 400)) {
            bad(format!("failed {} states no error and status", call.id));
        }
        for output in &call.outputs {
            match facts.get(output.as_str()) {
                None => bad(format!("{} outputs unknown {output}", call.id)),
                Some(fact) if fact.call.as_deref() != Some(call.id.as_str()) => bad(format!(
                    "{} outputs {output}, which names another call",
                    call.id
                )),
                Some(fact) if fact.conversation != call.conversation => bad(format!(
                    "{} outputs {output} of another conversation",
                    call.id
                )),
                Some(_) => {}
            }
        }
    }

    let mut sequenced: BTreeMap<&str, usize> = BTreeMap::new();
    for conversation in &truth.conversations {
        for id in &conversation.sequence {
            *sequenced.entry(id.as_str()).or_default() += 1;
            match facts.get(id.as_str()) {
                None => bad(format!("{} sequences unknown {id}", conversation.id)),
                Some(fact) if fact.conversation != conversation.id => bad(format!(
                    "{} sequences {id} of another conversation",
                    conversation.id
                )),
                Some(_) => {}
            }
        }
        for id in &conversation.final_answers {
            if !conversation.sequence.contains(id) {
                bad(format!("{} answers with unsequenced {id}", conversation.id));
            }
        }
    }
    for fact in &truth.facts {
        if sequenced.get(fact.id.as_str()) != Some(&1) {
            bad(format!("{} is not sequenced exactly once", fact.id));
        }
        if !conversations.contains(fact.conversation.as_str()) {
            bad(format!("{} names unknown {}", fact.id, fact.conversation));
        }
        let role = match fact.kind.as_str() {
            "system" => "system",
            "user_text" | "user_media" => "user",
            "text" | "reasoning" | "tool_call" => "assistant",
            "tool_result" => "tool",
            _ => "",
        };
        if fact.role != role {
            bad(format!(
                "{} is a {} with role {}",
                fact.id, fact.kind, fact.role
            ));
        }
        if !["wire", "wire-echo", "script", "program"].contains(&fact.evidence.as_str()) {
            bad(format!(
                "{} rests on unknown evidence {}",
                fact.id, fact.evidence
            ));
        }
        if let Some(call) = &fact.call {
            match calls.get(call.as_str()) {
                Some(c) if c.outputs.contains(&fact.id) => {}
                _ => bad(format!(
                    "{} names {call}, which does not output it",
                    fact.id
                )),
            }
        }
        match &fact.require {
            None => {}
            Some(require) => {
                let known_anchor = match require.anchor.as_str() {
                    "model_call" => fact.call.is_some(),
                    "conversation" => true,
                    _ => false,
                };
                if !known_anchor {
                    bad(format!(
                        "{} has unusable anchor {}",
                        fact.id, require.anchor
                    ));
                }
                if require.cardinality != "exactly_once" {
                    bad(format!(
                        "{} has cardinality {}",
                        fact.id, require.cardinality
                    ));
                }
                // Every view the anchor reaches, no fewer: a requirement that leaves a view out would
                // leave it unchecked.
                let expected: &[&str] = if require.anchor == "model_call" {
                    &["span", "trace", "session", "feed"]
                } else {
                    &["trace", "session", "feed"]
                };
                if require.views != expected {
                    bad(format!("{} has views {:?}", fact.id, require.views));
                }
                if !super::predicates::supports(&fact.kind, &require.matcher) {
                    bad(format!(
                        "{} is a {} matched by {}, which no predicate implements",
                        fact.id, fact.kind, require.matcher
                    ));
                }
            }
        }
    }
    let mut withdrawn: BTreeMap<&str, usize> = BTreeMap::new();
    for gap in &truth.gaps {
        if gap.detail.trim().is_empty() || gap.fact.trim().is_empty() {
            bad(format!(
                "gap {} names no category or explains nothing",
                gap.reason
            ));
        }
        // A closed vocabulary: the checks give each reason a meaning, and a gap about one fact or
        // call names it, so one gap cannot excuse many.
        let Some(effects) = gap_effects(&gap.reason) else {
            bad(format!("unknown gap reason {}", gap.reason));
            continue;
        };
        let names = |subject: &str| match effects.subject {
            GapSubject::Fact => facts.contains_key(subject),
            GapSubject::Call => calls.contains_key(subject),
            GapSubject::None => false,
        };
        match (&gap.subject, effects.subject) {
            (None, GapSubject::None) => {}
            (Some(subject), GapSubject::Fact | GapSubject::Call) if names(subject) => {}
            (Some(subject), GapSubject::Fact | GapSubject::Call) => {
                bad(format!("gap {} names unknown {subject}", gap.reason))
            }
            (None, _) => bad(format!("gap {} must name a subject", gap.reason)),
            (Some(_), GapSubject::None) => {
                bad(format!("gap {} must not name a subject", gap.reason))
            }
        }
        let modes: BTreeSet<&str> = truth
            .fixtures
            .iter()
            .filter_map(|f| f.split('/').nth(1))
            .collect();
        for mode in &gap.modes {
            if !modes.contains(mode.as_str()) {
                bad(format!(
                    "gap {} names mode {mode}, which no fixture has",
                    gap.reason
                ));
            }
        }
        // A withdrawn fact is unasserted everywhere, so its gap cannot hold for only some captures.
        if effects.withdraws && !gap.modes.is_empty() {
            bad(format!(
                "gap {} withdraws a fact for some modes only",
                gap.reason
            ));
        }
        let Some(fact) = gap.subject.as_deref().and_then(|s| facts.get(s)) else {
            continue;
        };
        if effects.withdraws {
            *withdrawn.entry(fact.id.as_str()).or_default() += 1;
            if fact.require.is_some() {
                bad(format!(
                    "gap {} withdraws {}, which is still asserted",
                    gap.reason, fact.id
                ));
            }
        }
        // An unexported id is a tool call's wire id; the fact keeps it, and checking moves it aside.
        if gap.reason == "id_not_exported"
            && !(fact.kind == "tool_call" && fact.value.get("id").is_some_and(Value::is_string))
        {
            bad(format!(
                "gap id_not_exported on {} needs a tool call with its wire id",
                fact.id
            ));
        }
        // An absence is proven by searching for the fact's content, so the fact keeps all of it.
        if effects.needs_absence_proof && effects.withdraws && gap.fact != fact.kind {
            bad(format!(
                "gap {} on {} names category {}, not its kind {}",
                gap.reason, fact.id, gap.fact, fact.kind
            ));
        }
    }
    for fact in truth.facts.iter().filter(|f| f.require.is_none()) {
        match withdrawn.get(fact.id.as_str()) {
            Some(1) => {}
            None => bad(format!("{} is unasserted but no gap withdraws it", fact.id)),
            Some(n) => bad(format!("{} is withdrawn by {n} gaps", fact.id)),
        }
    }

    // The edges the order checks rely on must all be there, or those checks pass vacuously.
    let count = |kind: &str, from: &str| {
        truth
            .edges
            .iter()
            .filter(|e| e.kind == kind && e.from.as_deref() == Some(from))
            .count()
    };
    for fact in &truth.facts {
        let (kind, needed) = match fact.kind.as_str() {
            "user_text" => ("prompt_of", 1),
            "tool_result" => ("result_of", 1),
            _ => continue,
        };
        if count(kind, &fact.id) != needed {
            bad(format!("{} needs exactly one {kind} edge", fact.id));
        }
    }
    if truth.topology != "multi_agent" {
        for conversation in &truth.conversations {
            let ordered: Vec<&str> = truth
                .calls
                .iter()
                .filter(|c| c.conversation == conversation.id && c.succeeded())
                .map(|c| c.id.as_str())
                .collect();
            for pair in ordered.windows(2) {
                let linked = truth.edges.iter().any(|e| {
                    e.kind == "call_order"
                        && e.before.as_deref() == Some(pair[0])
                        && e.after.as_deref() == Some(pair[1])
                });
                if !linked {
                    bad(format!(
                        "{} and {} have no call_order edge",
                        pair[0], pair[1]
                    ));
                }
            }
        }
    }
    for edge in &truth.edges {
        let resolves = match edge.kind.as_str() {
            "prompt_of" => {
                edge.from
                    .as_deref()
                    .and_then(|f| facts.get(f))
                    .is_some_and(|f| f.kind == "user_text")
                    && edge.to.as_deref().is_some_and(|c| calls.contains_key(c))
            }
            "result_of" => {
                edge.from
                    .as_deref()
                    .and_then(|f| facts.get(f))
                    .is_some_and(|f| f.kind == "tool_result")
                    && edge
                        .to
                        .as_deref()
                        .and_then(|f| facts.get(f))
                        .is_some_and(|f| f.kind == "tool_call")
            }
            "call_order" => {
                edge.before
                    .as_deref()
                    .is_some_and(|c| calls.contains_key(c))
                    && edge.after.as_deref().is_some_and(|c| calls.contains_key(c))
            }
            _ => false,
        };
        if !resolves {
            bad(format!("edge {edge:?} does not resolve"));
        }
    }
    defects
}

/// What a document claims about the repository: its source's digest and its fixtures.
pub(super) fn repository_defects(key: &str, truth: &Truth) -> Vec<String> {
    use sha2::Digest as _;
    let mut defects = Vec::new();
    let source = repo_root().join(&truth.source.path);
    match std::fs::read(&source) {
        Ok(bytes) => {
            let digest = hex_digest(&sha2::Sha256::digest(&bytes));
            if digest != truth.source.sha256 {
                defects.push(format!(
                    "{key}: {} changed since the truth was derived; run `python -m harness truth {}`",
                    truth.source.path, truth.producer
                ));
            }
        }
        Err(e) => defects.push(format!("{key}: source {}: {e}", truth.source.path)),
    }
    for fixture in &truth.fixtures {
        if !super::super::fixture_root().join(fixture).is_dir() {
            defects.push(format!("{key}: fixture {fixture} does not exist"));
        }
    }
    defects
}

pub(super) fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
