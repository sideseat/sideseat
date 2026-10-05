//! Which generation span recorded which truth call, and whether its metadata says what the wire said.
//!
//! The assignment is injective and never "closest": a call whose output two spans show equally is
//! ambiguous, and that is a violation, because a nearest-match rule would let a duplicated or misplaced
//! response pass as the original.

use std::collections::{BTreeMap, BTreeSet};

use sideseat_domain::sideml::FinishReason;

use super::predicates::{Shows, shows};
use super::recon::{Block, Generation, Recon};
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

fn shown_in(fact: &Fact, blocks: &[&Block]) -> bool {
    blocks.iter().any(|b| shows(fact, b, None) != Shows::No)
}

pub(super) fn match_calls(truth: &Truth, recon: &Recon, out: &mut Vec<Violation>) -> Matching {
    let mut matching = Matching::default();
    let gen_outputs: Vec<Vec<&Block>> = recon
        .generations
        .iter()
        .map(|g| outputs(recon, g))
        .collect();

    let mut candidates: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
    for call in truth.calls.iter().filter(|c| c.succeeded()) {
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
pub(super) fn check_metadata(
    truth: &Truth,
    recon: &Recon,
    matching: &Matching,
    out: &mut Vec<Violation>,
) {
    for call in truth.calls.iter().filter(|c| c.succeeded()) {
        let Some(&index) = matching.span_of.get(&call.id) else {
            continue;
        };
        let generation = &recon.generations[index];
        let carried = Carried::of(generation.raw.as_deref());
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
            if !stated.iter().any(names) && (states_any || carried.string(model)) {
                differ("call.model", model, describe(&stated));
            }
        }
        let pairs = [
            (
                "call.response_model",
                &call.response_model,
                &generation.response_model,
            ),
            (
                "call.response_id",
                &call.response_id,
                &generation.response_id,
            ),
        ];
        for (assertion, expected, actual) in pairs {
            if let Some(e) = expected
                && actual.as_ref() != Some(e)
                && (actual.is_some() || carried.string(e))
            {
                differ(assertion, e, describe(&[actual]));
            }
        }
        if let Some(finish) = call.finish.as_deref() {
            let expected = FinishReason::from_str_normalized(finish);
            let agrees = generation
                .finish
                .iter()
                .any(|f| FinishReason::from_str_normalized(f) == expected);
            let words = [Some(finish), call.stop_reason.as_deref()];
            if !agrees
                && (!generation.finish.is_empty()
                    || words.iter().flatten().any(|w| carried.string(w)))
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
            if generation.input != input && generation.input != other && reported {
                differ(
                    "call.usage.input",
                    &input.to_string(),
                    generation.input.to_string(),
                );
            }
        }
        let fields = [
            ("call.usage.output", usage.output, generation.output),
            (
                "call.usage.cache_read",
                usage.cache_read,
                generation.cache_read,
            ),
            (
                "call.usage.cache_write",
                usage.cache_write,
                generation.cache_write,
            ),
            (
                "call.usage.reasoning",
                usage.reasoning,
                generation.reasoning,
            ),
        ];
        for (assertion, expected, actual) in fields {
            if let Some(e) = expected
                && actual != e
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
}

impl Carried {
    fn of(raw: Option<&str>) -> Self {
        let mut carried = Carried::default();
        if let Some(value) = raw.and_then(|r| serde_json::from_str::<serde_json::Value>(r).ok()) {
            carried.walk(&value, 0);
        }
        carried
    }

    fn walk(&mut self, value: &serde_json::Value, depth: usize) {
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
                    self.walk(&inner, depth + 1);
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
            Value::Array(items) => items.iter().for_each(|v| self.walk(v, depth)),
            Value::Object(map) => map.values().for_each(|v| self.walk(v, depth)),
            _ => {}
        }
    }

    fn string(&self, value: &str) -> bool {
        self.strings.contains(value)
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
