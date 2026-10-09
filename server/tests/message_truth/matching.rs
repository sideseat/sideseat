//! Which generation span recorded which truth call, and whether its metadata says what the wire said.
//!
//! The assignment is injective and never "closest": a call whose output two spans show equally is
//! ambiguous, and that is a violation, because a nearest-match rule would let a duplicated or misplaced
//! response pass as the original.

use std::collections::{BTreeMap, BTreeSet};

use sideseat_domain::sideml::FinishReason;

use super::predicates::{Shows, shows};
use super::recon::{Block, Generation, Recon, ViewKind};
use super::truth::{Call, Fact, Truth};
use super::{Violation, ViolationView};

/// The outcome of matching: truth call id -> index into `recon.generations`.
#[derive(Debug, Default)]
pub(super) struct Matching {
    pub span_of: BTreeMap<String, usize>,
    /// Successful calls nothing asserted about, so no span can be tied to them.
    pub unmatchable: BTreeSet<String>,
}

/// The facts that identify a call: its asserted outputs.
pub(super) fn signature<'a>(truth: &'a Truth, call: &Call) -> Vec<&'a Fact> {
    call.outputs
        .iter()
        .filter_map(|id| truth.facts.iter().find(|f| &f.id == id))
        .filter(|f| f.require.as_ref().is_some_and(|r| r.anchor == "model_call"))
        .collect()
}

/// A generation span's output blocks - the side that says what it produced.
pub(super) fn outputs<'a>(recon: &'a Recon, generation: &Generation) -> Vec<&'a Block> {
    recon
        .span_view(&generation.span)
        .map(|view| view.blocks.iter().filter(|b| b.output).collect())
        .unwrap_or_default()
}

pub(super) fn shown_in(fact: &Fact, blocks: &[&Block]) -> bool {
    blocks.iter().any(|b| shows_on_span(fact, b) != Shows::No) || shown_in_segments(fact, blocks)
}

fn shown_exactly(fact: &Fact, blocks: &[&Block]) -> bool {
    blocks
        .iter()
        .any(|b| matches!(shows_on_span(fact, b), Shows::Yes | Shows::Assigned(_)))
        || shown_in_segments(fact, blocks)
}

/// An answer the provider returned as several text blocks - split around a citation, say - shown as those
/// blocks: its segments as consecutive output blocks, one each, as the view checks accept it.
fn shown_in_segments(fact: &Fact, blocks: &[&Block]) -> bool {
    let Some(segments) = fact.value.get("segments").and_then(|s| s.as_array()) else {
        return false;
    };
    segments.len() >= 2
        && blocks.windows(segments.len()).any(|run| {
            run.iter()
                .zip(segments)
                .all(|(b, s)| b.is("assistant", "text") && b.text() == s.as_str())
        })
}

/// Whether a block of a generation's own output shows the fact as that span can: withheld reasoning whose
/// signature the producing span does not carry is shown there unsigned.
fn shows_on_span(fact: &Fact, block: &Block) -> Shows {
    if fact.value.get(super::truth::UNSIGNED_ON_SPAN).is_some() {
        let mut unsigned = fact.clone();
        unsigned.value["signed"] = serde_json::Value::Bool(false);
        return shows(&unsigned, block, None);
    }
    shows(fact, block, None)
}

pub(super) fn match_calls(truth: &Truth, recon: &Recon, out: &mut Vec<Violation>) -> Matching {
    let mut matching = Matching::default();
    let gen_outputs: Vec<Vec<&Block>> = recon
        .generations
        .iter()
        .map(|g| outputs(recon, g))
        .collect();

    let mut candidates: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
    let mut by_meta: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
    let proven = |reason: &str| -> BTreeSet<&str> {
        truth
            .gaps
            .iter()
            .filter(|g| g.reason == reason)
            .filter_map(|g| g.subject.as_deref())
            .collect()
    };
    // A response proven absent from the telemetry has no span to find.
    let unexported = proven("call_not_exported");
    // A call proven to have no span of its own (`call_span_not_exported`) is tied to none.
    let spanless = proven("call_span_not_exported");
    let off_span = proven("output_not_exported");
    for call in truth.calls.iter().filter(|c| {
        c.succeeded() && !unexported.contains(c.id.as_str()) && !spanless.contains(c.id.as_str())
    }) {
        let signature = signature(truth, call);
        if signature.is_empty() {
            matching.unmatchable.insert(call.id.clone());
            continue;
        }
        let pinned: BTreeSet<usize> = call
            .response_id
            .as_ref()
            .map(|id| {
                recon
                    .generations
                    .iter()
                    .enumerate()
                    .filter(|(_, g)| g.response_id.as_ref() == Some(id))
                    .map(|(i, _)| i)
                    .collect()
            })
            .unwrap_or_default();
        if signature.iter().all(|f| off_span.contains(f.id.as_str())) {
            let found = if pinned.is_empty() {
                by_metadata(recon, call, &gen_outputs)
            } else {
                pinned
            };
            by_meta.insert(call.id.clone(), found);
            continue;
        }
        let any: BTreeSet<usize> = if pinned.is_empty() {
            (0..recon.generations.len())
                .filter(|&i| signature.iter().any(|f| shown_in(f, &gen_outputs[i])))
                .collect()
        } else {
            pinned
        };
        let all: BTreeSet<usize> = any
            .iter()
            .copied()
            .filter(|&i| signature.iter().all(|f| shown_in(f, &gen_outputs[i])))
            .collect();
        let shown = if any.len() > 1 && !all.is_empty() {
            all
        } else {
            any
        };
        // A span showing the call's tool calls under their own ids is evidence a span showing them
        // under other ids is not: two identical calls differ only by id.
        let exact: BTreeSet<usize> = shown
            .iter()
            .copied()
            .filter(|&i| signature.iter().all(|f| shown_exactly(f, &gen_outputs[i])))
            .collect();
        let shown = if shown.len() > 1 && !exact.is_empty() {
            exact
        } else {
            shown
        };
        let shown: BTreeSet<usize> = shown
            .into_iter()
            .filter(|&i| !relists_from_below(recon, &signature, i))
            .collect();
        let chosen = innermost(recon, prefer_typed(recon, shown));
        candidates.insert(call.id.clone(), chosen);
    }

    // Unique assignment, propagated: a span that is a call's only candidate is taken from every other.
    let original = candidates.clone();
    let mut taken: BTreeMap<usize, String> = BTreeMap::new();
    while let Some((call, span)) = candidates
        .iter()
        .find(|(c, s)| s.len() == 1 && !matching.span_of.contains_key(*c))
        .map(|(c, s)| (c.clone(), *s.iter().next().expect("one candidate")))
    {
        matching.span_of.insert(call.clone(), span);
        taken.insert(span, call.clone());
        for (other, spans) in candidates.iter_mut() {
            if *other != call {
                spans.remove(&span);
            }
        }
    }
    for (call, spans) in &candidates {
        if matching.span_of.contains_key(call) {
            continue;
        }
        let (assertion, detail) = match (original[call].len(), spans.len()) {
            (0, _) => (
                "call.unmatched",
                "no generation span's output shows any of this call's asserted output".to_string(),
            ),
            (_, 0) => (
                "call.span_shared",
                format!(
                    "the only span showing this call's output is claimed by {}",
                    original[call]
                        .iter()
                        .filter_map(|s| taken.get(s))
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ),
            (_, n) => (
                "call.ambiguous",
                format!(
                    "{n} generation spans show this call's output equally: {}",
                    spans
                        .iter()
                        .map(|&s| recon.generations[s].label.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ),
        };
        out.push(Violation::new(ViolationView::Call, assertion, call, detail));
    }

    assign_by_metadata(recon, &by_meta, &mut matching, out);
    match_failed_attempts(truth, recon, &gen_outputs, &mut matching, out);

    // What is left: typed generation spans that spoke but recorded no asserted call.
    let matched: BTreeSet<usize> = matching.span_of.values().copied().collect();
    let mut unclaimed: Vec<usize> = (0..recon.generations.len())
        .filter(|i| {
            let g = &recon.generations[*i];
            g.typed
                && !g.failed
                && !matched.contains(i)
                && gen_outputs[*i].iter().any(|b| b.role == "assistant")
        })
        .collect();
    unclaimed.sort_by_key(|&i| {
        (
            recon.generations[i].start,
            recon.generations[i].label.clone(),
        )
    });

    // A call whose output is unknowable still happened. When exactly as many unclaimed spans remain,
    // they are its spans in call order - cardinality, not similarity, so a missing or extra span
    // fails rather than being absorbed.
    let unmatchable: Vec<&str> = truth
        .calls
        .iter()
        .filter(|c| matching.unmatchable.contains(&c.id))
        .map(|c| c.id.as_str())
        .collect();
    if unmatchable.len() == unclaimed.len() {
        for (call, span) in unmatchable.iter().zip(&unclaimed) {
            matching.span_of.insert((*call).to_string(), *span);
        }
    } else if !unmatchable.is_empty() {
        out.push(Violation::new(
            ViolationView::Call,
            "call.count",
            &unmatchable.join("+"),
            format!(
                "{} call(s) with unasserted output but {} unclaimed generation span(s)",
                unmatchable.len(),
                unclaimed.len()
            ),
        ));
    } else {
        for &index in &unclaimed {
            out.push(Violation::new(
                ViolationView::Call,
                "generation.unexpected",
                &recon.generations[index].label,
                "a generation span with assistant output that no truth call made".to_string(),
            ));
        }
    }
    matching
}

/// Whether a span states the call's finish: its normalised category, or the provider's own word. A word no
/// table knows agrees only with the same word, never with another unknown one.
fn finish_agrees(call: &Call, generation: &Generation) -> bool {
    let words = [call.finish.as_deref(), call.stop_reason.as_deref()];
    generation.finish.iter().any(|f| {
        words.iter().flatten().any(|w| {
            match (
                FinishReason::from_str_normalized(w),
                FinishReason::from_str_normalized(f),
            ) {
                (Some(a), Some(b)) => a == b,
                _ => w.eq_ignore_ascii_case(f),
            }
        })
    })
}

/// Ties each call whose span is proven to carry none of its response (`output_not_exported` on every output)
/// to the one span its metadata names. The weakest evidence the rubric accepts, so it is never a guess: a call
/// with no candidate, or more than one, or whose only candidate is another such call's too, is left unmatched
/// - nothing is eliminated to make it unique - and a span another call's output already claimed is no candidate.
fn assign_by_metadata(
    recon: &Recon,
    by_meta: &BTreeMap<String, BTreeSet<usize>>,
    matching: &mut Matching,
    out: &mut Vec<Violation>,
) {
    let taken: BTreeSet<usize> = matching.span_of.values().copied().collect();
    let free = |spans: &BTreeSet<usize>| -> BTreeSet<usize> { spans - &taken };
    let label = |spans: &BTreeSet<usize>| {
        spans
            .iter()
            .map(|&s| recon.generations[s].label.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    for (call, spans) in by_meta {
        let spans = free(spans);
        let rivals: Vec<&str> = by_meta
            .iter()
            .filter(|(other, theirs)| *other != call && !free(theirs).is_disjoint(&spans))
            .map(|(other, _)| other.as_str())
            .collect();
        let detail = match (spans.len(), rivals.is_empty()) {
            (1, true) => {
                let span = *spans.iter().next().expect("one candidate");
                matching.span_of.insert(call.clone(), span);
                continue;
            }
            (0, _) => {
                out.push(Violation::new(
                    ViolationView::Call,
                    "call.unmatched",
                    call,
                    "its span carries none of its response, and no generation span that recorded \
                     nothing states its finish and output count"
                        .to_string(),
                ));
                continue;
            }
            (_, true) => format!(
                "its span carries none of its response, and {} spans that recorded nothing state its \
                 finish and output count: {}",
                spans.len(),
                label(&spans)
            ),
            (_, false) => format!(
                "its span carries none of its response, and the spans its finish and output count name \
                 ({}) are also named by {}",
                label(&spans),
                rivals.join(", ")
            ),
        };
        out.push(Violation::new(
            ViolationView::Call,
            "call.ambiguous",
            call,
            detail,
        ));
    }
}

/// The spans that could have recorded a call whose span carries none of its response: typed generation spans
/// that recorded nothing of their own, ended as the call did, and counted the output tokens it produced. The
/// metadata stands in for the output only because none is shown; a span that shows anything is another
/// call's, and a call with no output count is found by nothing.
fn by_metadata(recon: &Recon, call: &Call, gen_outputs: &[Vec<&Block>]) -> BTreeSet<usize> {
    let Some(produced) = call
        .usage
        .as_ref()
        .and_then(|u| u.output)
        .filter(|&n| n > 0)
    else {
        return BTreeSet::new();
    };
    recon
        .generations
        .iter()
        .enumerate()
        .filter(|(i, g)| {
            g.typed
                && !g.failed
                && gen_outputs[*i].is_empty()
                && g.output == produced
                && finish_agrees(call, g)
        })
        .map(|(i, _)| i)
        .collect()
}

/// Typed generation spans, when any candidate is one: a framework span re-listing a response as its
/// own output shows it as faithfully as the span that recorded the call.
fn prefer_typed(recon: &Recon, candidates: BTreeSet<usize>) -> BTreeSet<usize> {
    let typed: BTreeSet<usize> = candidates
        .iter()
        .copied()
        .filter(|&i| recon.generations[i].typed)
        .collect();
    if typed.is_empty() { candidates } else { typed }
}

/// The span re-lists outputs the conversation shows on spans below it, and only there.
///
/// A run's agent span naming what its tool spans ran, where no model span exists: the reconstruction keeps
/// each call on the span that ran it, so the agent's copy is a re-listing and no call's span. A span whose
/// copy is the one the conversation keeps - no span below shows it - stays a candidate.
fn relists_from_below(recon: &Recon, signature: &[&Fact], candidate: usize) -> bool {
    let span = &recon.generations[candidate].span;
    !signature.is_empty()
        && signature.iter().all(|fact| {
            let shown_at: BTreeSet<&str> = recon
                .views
                .iter()
                .filter(|view| view.kind == ViewKind::Trace)
                .flat_map(|view| &view.blocks)
                .filter(|block| shows(fact, block, None) != Shows::No)
                .map(|block| block.span.as_str())
                .collect();
            !shown_at.is_empty()
                && !shown_at.contains(span.as_str())
                && shown_at.iter().all(|at| {
                    recon
                        .generations
                        .iter()
                        .any(|g| g.span == *at && g.ancestors.contains(span))
                })
        })
}

/// Candidates with no candidate below them: the model call is the innermost span showing its output,
/// and every enclosing agent or chain span that reports the same output is a re-listing.
fn innermost(recon: &Recon, candidates: BTreeSet<usize>) -> BTreeSet<usize> {
    candidates
        .iter()
        .copied()
        .filter(|&i| {
            let span = &recon.generations[i].span;
            !candidates
                .iter()
                .any(|&j| j != i && recon.generations[j].ancestors.contains(span))
        })
        .collect()
}

/// A failed attempt is tied to an ERROR generation span naming its error, when one exists; clients
/// retry inside one span often enough that absence is not a defect. A tied span must say nothing.
fn match_failed_attempts(
    truth: &Truth,
    recon: &Recon,
    gen_outputs: &[Vec<&Block>],
    matching: &mut Matching,
    out: &mut Vec<Violation>,
) {
    for call in truth.calls.iter().filter(|c| !c.succeeded()) {
        let Some(error) = call.error.as_deref() else {
            continue;
        };
        let used: BTreeSet<usize> = matching.span_of.values().copied().collect();
        let candidates: Vec<usize> = recon
            .generations
            .iter()
            .enumerate()
            .filter(|(i, g)| {
                !used.contains(i)
                    && g.failed
                    && g.error.as_deref().is_some_and(|e| e.contains(error))
            })
            .map(|(i, _)| i)
            .collect();
        if let [only] = candidates[..] {
            matching.span_of.insert(call.id.clone(), only);
            if !gen_outputs[only].is_empty() {
                out.push(Violation::new(
                    ViolationView::Call,
                    "call.failed_attempt_has_output",
                    &call.id,
                    "the span of a failed attempt shows output".to_string(),
                ));
            }
        } else if candidates.len() > 1 {
            out.push(Violation::new(
                ViolationView::Call,
                "call.ambiguous",
                &call.id,
                format!("{} failed spans name this error", candidates.len()),
            ));
        }
    }
}

/// Model, response id, finish and usage of every matched call, wherever the truth states them.
///
/// A span stating another value is a violation. A span stating none is one only when the telemetry
/// carried the value - somewhere in the raw span, as an attribute or inside a JSON payload - and the
/// pipeline did not read it; a value the producer never exported is not a parsing defect. The span
/// schema stores an absent count as 0, so for usage "states none" means 0.
/// One usage count: its assertion, the truth's value, the span's, and how to read it from a call.
type UsageField = (
    &'static str,
    Option<i64>,
    i64,
    fn(&super::truth::Usage) -> Option<i64>,
);

pub(super) fn check_metadata(
    truth: &Truth,
    recon: &Recon,
    matching: &Matching,
    out: &mut Vec<Violation>,
) {
    let unexported: BTreeSet<&str> = truth
        .gaps
        .iter()
        .filter(|g| g.reason == "call_not_exported")
        .filter_map(|g| g.subject.as_deref())
        .collect();
    // Metadata a gap proves absent from every payload: the span cannot state it.
    let not_exported: BTreeSet<(&str, String)> = truth
        .gaps
        .iter()
        .filter(|g| g.reason == "metadata_not_exported")
        .filter_map(|g| Some((g.subject.as_deref()?, format!("call.{}", g.fact))))
        .collect();
    for call in truth.calls.iter().filter(|c| c.succeeded()) {
        let Some(&index) = matching.span_of.get(&call.id) else {
            continue;
        };
        let generation = &recon.generations[index];
        let carried = Carried::of(generation.raw.as_deref());
        // What the span says about the *answer*: a member stating what was requested is not a statement
        // of which model answered, even when the two values coincide.
        let answered = Carried::of_answer(generation.raw.as_deref());
        // A gap waives a field only for what the producer itself said: every value the span states is
        // in the span's own payload. A value the reconstruction made up is still a violation.
        let waived = |assertion: &str, stated: &[&str], finish: bool| {
            not_exported.contains(&(call.id.as_str(), assertion.to_string()))
                && stated.iter().all(|value| {
                    if finish {
                        carried.finish_word(value)
                    } else {
                        carried.string(value)
                    }
                })
        };
        let mut differ = |assertion: &str, expected: &str, actual: String| {
            out.push(Violation::new(
                ViolationView::Call,
                assertion,
                &call.id,
                format!("truth {expected}, span {actual}"),
            ));
        };
        // The truth's `model` is the model addressed where the wire shows it, otherwise the one that
        // answered, so a span may state it as either - and under a router's `provider/` prefix.
        if let Some(model) = &call.model {
            let stated = [&generation.request_model, &generation.response_model];
            let names = |m: &&Option<String>| {
                m.as_deref()
                    .is_some_and(|m| m == model || m.ends_with(&format!("/{model}")))
            };
            let states_any = stated.iter().any(|m| m.is_some());
            let values: Vec<&str> = stated.iter().filter_map(|m| m.as_deref()).collect();
            if !stated.iter().any(names)
                && (states_any || carried.string(model))
                && !waived("call.model", &values, false)
            {
                differ("call.model", model, describe(&stated));
            }
        }
        let pairs = [
            (
                "call.response_model",
                &call.response_model,
                &generation.response_model,
                &answered,
            ),
            (
                "call.response_id",
                &call.response_id,
                &generation.response_id,
                &carried,
            ),
        ];
        for (assertion, expected, actual, raw) in pairs {
            if let Some(e) = expected
                && actual.as_ref() != Some(e)
                && (actual.is_some() || raw.string(e))
                && !waived(
                    assertion,
                    &actual.iter().map(String::as_str).collect::<Vec<_>>(),
                    false,
                )
            {
                differ(assertion, e, describe(&[actual]));
            }
        }
        if let Some(finish) = call.finish.as_deref() {
            // The normalised category, or the provider's own word: Gemini answers a function call
            // with `STOP`, and a span stating `stop` reports exactly what the wire said.
            // A word no table knows agrees only with the same word, never with another unknown one.
            let words = [Some(finish), call.stop_reason.as_deref()];
            let agrees = finish_agrees(call, generation);
            let stated_finish: Vec<&str> = generation.finish.iter().map(String::as_str).collect();
            if !agrees
                && (!generation.finish.is_empty()
                    || words.iter().flatten().any(|w| carried.finish_word(w)))
                && !waived("call.finish", &stated_finish, true)
            {
                let stated = if generation.finish.is_empty() {
                    "states none".to_string()
                } else {
                    generation.finish.join(",")
                };
                differ("call.finish", finish, stated);
            }
        }
        let Some(usage) = &call.usage else {
            continue;
        };
        // A span that records a run of model calls whose earlier responses the telemetry does not carry
        // (`call_not_exported`) may report the run's usage: the sum over this call and the unexported
        // calls immediately before it in its conversation.
        let run_total = |count: fn(&super::truth::Usage) -> Option<i64>| -> Option<i64> {
            let position = truth.calls.iter().position(|c| c.id == call.id)?;
            let earlier: Vec<&Call> = truth.calls[..position]
                .iter()
                .rev()
                .filter(|c| c.conversation == call.conversation && c.succeeded())
                .take_while(|c| unexported.contains(c.id.as_str()))
                .collect();
            if earlier.is_empty() {
                return None;
            }
            earlier
                .iter()
                .chain(std::iter::once(&call))
                .map(|c| c.usage.as_ref().and_then(count))
                .sum()
        };
        let cache = usage.cache_read.unwrap_or(0) + usage.cache_write.unwrap_or(0);
        if let Some(input) = usage.input {
            // Providers disagree on whether input counts cached tokens; either convention is a
            // faithful report of the same call.
            let other = if usage.input_includes_cache {
                input - cache
            } else {
                input + cache
            };
            let reported = generation.input != 0 || carried.number(input) || carried.number(other);
            if generation.input != input
                && generation.input != other
                && !(run_total(|u| u.input) == Some(generation.input)
                    && carried.number(generation.input))
                && reported
            {
                differ(
                    "call.usage.input",
                    &input.to_string(),
                    generation.input.to_string(),
                );
            }
        }
        let fields: [UsageField; 4] = [
            ("call.usage.output", usage.output, generation.output, |u| {
                u.output
            }),
            (
                "call.usage.cache_read",
                usage.cache_read,
                generation.cache_read,
                |u| u.cache_read,
            ),
            (
                "call.usage.cache_write",
                usage.cache_write,
                generation.cache_write,
                |u| u.cache_write,
            ),
            (
                "call.usage.reasoning",
                usage.reasoning,
                generation.reasoning,
                |u| u.reasoning,
            ),
        ];
        for (assertion, expected, actual, count) in fields {
            if let Some(e) = expected
                && actual != e
                && !(run_total(count) == Some(actual) && carried.number(actual))
                && (actual != 0 || carried.number(e))
            {
                differ(assertion, &e.to_string(), actual.to_string());
            }
        }
    }
}

/// Every scalar a raw span holds, JSON payloads inside its strings included.
#[derive(Default)]
struct Carried {
    strings: BTreeSet<String>,
    numbers: BTreeSet<i64>,
    /// The strings under a member that names a finish (`finish_reason`, `stopReason`): a finish word
    /// elsewhere - an output item's own `status`, a sentence - is not the span stating the call's.
    finish_words: BTreeSet<String>,
    skip_requests: bool,
}

/// A member whose name puts it in a request namespace (`gen_ai.request.model`, `request_model`).
fn states_a_request(key: &str) -> bool {
    key.split(['.', '_'])
        .any(|segment| segment.eq_ignore_ascii_case("request"))
}

impl Carried {
    fn of(raw: Option<&str>) -> Self {
        Self::walked(raw, false)
    }

    /// The scalars outside every member that states a request: `gen_ai.request.model` names the model
    /// asked for, so its value is not evidence that the span states the model that answered.
    fn of_answer(raw: Option<&str>) -> Self {
        Self::walked(raw, true)
    }

    fn walked(raw: Option<&str>, skip_requests: bool) -> Self {
        let mut carried = Carried {
            skip_requests,
            ..Carried::default()
        };
        if let Some(value) = raw.and_then(|r| serde_json::from_str::<serde_json::Value>(r).ok()) {
            carried.walk(&value, 0, false);
        }
        carried
    }

    fn walk(&mut self, value: &serde_json::Value, depth: usize, finish: bool) {
        use serde_json::Value;
        match value {
            Value::String(text) => {
                if let Ok(n) = text.trim().parse::<i64>() {
                    self.numbers.insert(n);
                }
                if depth < 8
                    && matches!(text.trim_start().chars().next(), Some('{' | '['))
                    && let Ok(inner) = serde_json::from_str::<Value>(text)
                {
                    self.walk(&inner, depth + 1, finish);
                }
                if finish {
                    self.finish_words.insert(text.to_ascii_lowercase());
                }
                self.strings.insert(text.clone());
            }
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    self.numbers.insert(i);
                } else if let Some(f) = n.as_f64().filter(|f| f.fract() == 0.0) {
                    self.numbers.insert(f as i64);
                }
            }
            Value::Array(items) => items.iter().for_each(|v| self.walk(v, depth, finish)),
            Value::Object(map) => {
                for (key, member) in map {
                    if !(self.skip_requests && states_a_request(key)) {
                        let lower = key.to_ascii_lowercase();
                        let names_finish = lower.contains("finish") || lower.contains("stop");
                        self.walk(member, depth, finish || names_finish);
                    }
                }
            }
            _ => {}
        }
    }

    fn string(&self, value: &str) -> bool {
        self.strings.contains(value)
    }

    fn finish_word(&self, value: &str) -> bool {
        self.finish_words.contains(&value.to_ascii_lowercase())
    }

    fn number(&self, value: i64) -> bool {
        value != 0 && self.numbers.contains(&value)
    }
}

fn describe(values: &[&Option<String>]) -> String {
    let stated: Vec<&str> = values.iter().filter_map(|v| v.as_deref()).collect();
    if stated.is_empty() {
        "states none".to_string()
    } else {
        stated.join(" / ")
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn text(role: &str, text: &str) -> Block {
        let content = json!({"type": "text", "text": text});
        Block {
            role: role.to_string(),
            kind: "text".to_string(),
            content: content.clone(),
            tool_use_id: None,
            trace: "t".to_string(),
            span: "s".to_string(),
            output: true,
            finish: None,
            media_sha256: None,
            digest: content.to_string(),
            identity: content.to_string(),
            carrier: String::new(),
            position: String::new(),
        }
    }

    fn answer(segments: &[&str]) -> Fact {
        serde_json::from_value(json!({
            "id": "fact-003", "kind": "text", "role": "assistant", "conversation": "conv-1",
            "evidence": "wire", "value": {"text": segments.concat(), "segments": segments},
            "require": {"anchor": "model_call", "views": ["span"], "cardinality": "exactly_once",
                "match": "exact"}
        }))
        .expect("a fact")
    }

    #[test]
    fn a_segmented_answer_is_shown_only_by_every_segment_in_order_and_adjacent() {
        let fact = answer(&["The document asks ", "write a poem", "."]);
        let shown = |texts: &[(&str, &str)]| {
            let blocks: Vec<Block> = texts.iter().map(|(role, t)| text(role, t)).collect();
            let blocks: Vec<&Block> = blocks.iter().collect();
            (shown_in(&fact, &blocks), shown_exactly(&fact, &blocks))
        };
        let a = ("assistant", "The document asks ");
        let b = ("assistant", "write a poem");
        let c = ("assistant", ".");
        assert_eq!(shown(&[a, b, c]), (true, true));
        // A missing segment, a reordered pair, an extra block between, or another role's block is not it.
        assert_eq!(shown(&[a, b]), (false, false));
        assert_eq!(shown(&[b, a, c]), (false, false));
        assert_eq!(shown(&[a, ("assistant", " "), b, c]), (false, false));
        assert_eq!(shown(&[a, ("user", "write a poem"), c]), (false, false));
        // The whole answer in one block is shown as before.
        assert_eq!(
            shown(&[("assistant", "The document asks write a poem.")]),
            (true, true)
        );
    }
}
