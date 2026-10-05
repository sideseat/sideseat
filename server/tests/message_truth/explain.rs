//! Exactness: a trace or session view holds the truth's conversation and nothing it cannot explain,
//! and every message sits on the span that produced it.
//!
//! The truth is complete except where it says it is not: each fact it cannot know is a gap with a
//! reason, and a gap is the only thing that may account for a reconstructed block no fact claims. A
//! system prompt the cassette never recorded, a signed reasoning block with no visible text, a fake
//! model's answer, a non-deterministic tool's result, what one agent passed another - each explains
//! as many blocks as it covers, and every other unclaimed block is a violation.

use std::collections::{BTreeMap, BTreeSet};

use super::checks::{Assigned, Context, Scope, preview};
use super::recon::{Block, ViewKind};
use super::{Violation, ViolationView};

pub(super) fn check(
    context: &Context<'_>,
    scope: &Scope<'_>,
    assigned: &Assigned,
    out: &mut Vec<Violation>,
) {
    if matches!(scope.kind, ViewKind::Trace | ViewKind::Session) {
        check_unexplained(context, scope, assigned, out);
    }
    if scope.kind != ViewKind::Span {
        check_attribution(context, scope, assigned, out);
    }
}

/// What the truth's gaps allow beyond its facts, each bound to where it applies.
///
/// A gap explains blocks only in the traces of the calls it concerns: an unknowable answer or
/// reasoning block in its call's trace, an unrecorded system prompt once per trace of its conversation
/// (each distinct prompt once, when agents route between them), a non-deterministic tool's result by
/// its call id. A slot is consumed by the block it explains, so a gap never covers two.
struct Allowance {
    system_prompt: bool,
    routing: bool,
    /// Traces any truth call ran in; routing and system prompts are bounded to them.
    traces: BTreeSet<String>,
    /// (class, trace) slots for unknowable facts; `None` when the fact's call has no span.
    slots: Vec<(&'static str, Option<String>)>,
    user_text: usize,
    /// Call ids whose result the truth cannot know.
    results_of: BTreeSet<String>,
}

impl Allowance {
    fn of(context: &Context<'_>, assigned: &Assigned) -> Self {
        let truth = context.truth;
        let gap = |reason: &str| truth.gaps.iter().any(|g| g.reason == reason);
        let results_of = truth
            .gaps
            .iter()
            .filter(|g| g.reason == "tool_not_deterministic")
            .filter_map(|g| g.subject.as_deref())
            .filter_map(|subject| {
                let call = context.fact(subject)?;
                let id = assigned
                    .rewritten
                    .get(&call.id)
                    .cloned()
                    .or_else(|| call.value.get("id")?.as_str().map(str::to_owned))?;
                Some(id)
            })
            .collect();
        let slots = truth
            .facts
            .iter()
            .filter(|f| f.require.is_none())
            .filter_map(|f| {
                let class = match f.kind.as_str() {
                    "reasoning" => "reasoning",
                    "text" => "text",
                    _ => return None,
                };
                Some((class, context.home_trace.get(f.id.as_str()).cloned()))
            })
            .collect();
        Allowance {
            system_prompt: gap("request_body_unrecorded") || gap("request_modelled"),
            routing: gap("multi_agent_routing"),
            traces: context
                .matching
                .span_of
                .values()
                .map(|&i| context.recon.generations[i].trace.clone())
                .collect(),
            slots,
            user_text: truth
                .gaps
                .iter()
                .filter(|g| g.reason == "prompt_without_model_call")
                .count(),
            results_of,
        }
    }

    fn take(&mut self, class: &str, trace: &str) -> bool {
        let found = self
            .slots
            .iter()
            .position(|(c, t)| *c == class && t.as_deref().is_none_or(|t| t == trace));
        found.map(|at| self.slots.remove(at)).is_some()
    }

    fn in_conversation(&self, trace: &str) -> bool {
        self.traces.contains(trace)
    }
}

fn check_unexplained(
    context: &Context<'_>,
    scope: &Scope<'_>,
    assigned: &Assigned,
    out: &mut Vec<Violation>,
) {
    let claimed: BTreeSet<usize> = assigned
        .block_of
        .values()
        .copied()
        .chain(assigned.consumed.iter().copied())
        .collect();
    // A copy of a claimed block is a duplicate or a leak, which the assignment already reported.
    let claimed_digests: BTreeSet<&str> = claimed
        .iter()
        .map(|&at| scope.blocks[at].2.identity.as_str())
        .collect();
    let mut allowance = Allowance::of(context, assigned);
    let mut system_seen: BTreeSet<(&str, &str)> = BTreeSet::new();
    let mut unexplained: BTreeMap<&str, (usize, &Block)> = BTreeMap::new();
    for (at, (_, _, block)) in scope.blocks.iter().enumerate() {
        if claimed.contains(&at) || claimed_digests.contains(block.identity.as_str()) {
            continue;
        }
        let trace = block.trace.as_str();
        let routed = allowance.routing && allowance.in_conversation(trace);
        let explained = match (block.role.as_str(), block.kind.as_str()) {
            ("system", _) => {
                allowance.system_prompt
                    && allowance.in_conversation(trace)
                    && system_seen.insert((
                        trace,
                        if allowance.routing {
                            block.digest.as_str()
                        } else {
                            ""
                        },
                    ))
            }
            ("assistant", "thinking" | "redacted_thinking") => allowance.take("reasoning", trace),
            ("assistant", "text" | "json") => allowance.take("text", trace),
            ("tool", "tool_result") => {
                routed
                    || block
                        .result_call_id()
                        .is_some_and(|id| allowance.results_of.remove(id))
            }
            ("user", "text") => routed || take(&mut allowance.user_text),
            _ => false,
        };
        if !explained {
            unexplained
                .entry(block.digest.as_str())
                .or_insert((0, block))
                .0 += 1;
        }
    }
    for (digest, (count, block)) in unexplained {
        let content = super::super::canonical_json(&block.content);
        out.push(Violation::new(
            ViolationView::from(scope.kind),
            "extra.unexplained",
            digest,
            format!(
                "{count} block(s) no fact or gap accounts for: {}",
                preview(&content)
            ),
        ));
    }
}

fn take(budget: &mut usize) -> bool {
    if *budget == 0 {
        return false;
    }
    *budget -= 1;
    true
}

/// A response's parts are shown on the span that recorded the call; a prompt, attachment or system
/// prompt on that span or one enclosing it. A model call's span starts no earlier than the call
/// before it.
fn check_attribution(
    context: &Context<'_>,
    scope: &Scope<'_>,
    assigned: &Assigned,
    out: &mut Vec<Violation>,
) {
    let recon = context.recon;
    let span_of_call = |call: &str| {
        context
            .matching
            .span_of
            .get(call)
            .map(|&i| &recon.generations[i])
    };
    for (fact_id, &at) in &assigned.block_of {
        let Some(fact) = context.fact(fact_id) else {
            continue;
        };
        // Which span reports a result - the tool's, or the next request's - is the framework's, and
        // so is the span of a call no model response states.
        if fact.call.is_none()
            && !["user_text", "user_media", "system"].contains(&fact.kind.as_str())
        {
            continue;
        }
        let Some(generation) = context.home_call(fact).and_then(span_of_call) else {
            continue;
        };
        let block = scope.blocks[at].2;
        let allowed = block.span == generation.span
            || (fact.call.is_none() && generation.ancestors.contains(&block.span));
        if !allowed {
            let shown_on = recon
                .generations
                .iter()
                .find(|g| g.span == block.span)
                .map_or("an unknown span", |g| g.label.as_str());
            out.push(Violation::new(
                ViolationView::from(scope.kind),
                "attribution.span",
                fact_id,
                format!("shown on {shown_on}, not {}", generation.label),
            ));
        }
    }
    if scope.kind != ViewKind::Feed {
        return;
    }
    // Once per fixture: the feed is a single view.
    for edge in context
        .truth
        .edges
        .iter()
        .filter(|e| e.kind == "call_order")
    {
        let (Some(a), Some(b)) = (
            edge.before.as_deref().and_then(span_of_call),
            edge.after.as_deref().and_then(span_of_call),
        ) else {
            continue;
        };
        if b.start < a.start {
            out.push(Violation::new(
                ViolationView::Call,
                "attribution.call_order",
                &format!(
                    "{}<{}",
                    edge.before.as_deref().unwrap_or(""),
                    edge.after.as_deref().unwrap_or("")
                ),
                format!("{} starts before {}", b.label, a.label),
            ));
        }
    }
}
