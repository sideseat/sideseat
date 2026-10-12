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
    // A span view owes its own call's response, and a block of its output nothing explains is invented there
    // as much as in a trace.
    if matches!(
        scope.kind,
        ViewKind::Trace | ViewKind::Session | ViewKind::Span
    ) {
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
struct Allowance<'a> {
    system_prompt: bool,
    routing: bool,
    /// Traces any truth call ran in; routing and system prompts are bounded to them.
    traces: BTreeSet<String>,
    /// (class, trace, fact id) slots for unknowable facts; the trace is `None` when the fact's call has no
    /// span. The fact id bounds a span scope to its own calls' slots.
    slots: Vec<(&'static str, Option<String>, &'a str)>,
    user_text: usize,
    /// Call ids whose result the truth cannot know.
    results_of: BTreeSet<String>,
    /// One slot per restated request: the call, its trace (`None` when it has no span) and the prompt
    /// of its turn, which the restating user text must contain.
    restated: Vec<(&'a str, Option<String>, &'a str)>,
}

impl<'a> Allowance<'a> {
    fn of(context: &Context<'a>, assigned: &Assigned) -> Self {
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
        // Only a fact the oracle cannot know leaves a slot; one the telemetry never carried cannot be
        // what an unclaimed block shows.
        let unknowable: BTreeSet<&str> = truth
            .gaps
            .iter()
            .filter(|g| {
                super::truth::gap_effects(&g.reason)
                    .is_some_and(|e| e.withdraws && e.explains_extra)
            })
            .filter_map(|g| g.subject.as_deref())
            .collect();
        let slots = truth
            .facts
            .iter()
            .filter(|f| f.require.is_none() && unknowable.contains(f.id.as_str()))
            .filter_map(|f| {
                let class = match f.kind.as_str() {
                    "reasoning" => "reasoning",
                    "text" => "text",
                    _ => return None,
                };
                Some((
                    class,
                    context.home_trace.get(f.id.as_str()).cloned(),
                    f.id.as_str(),
                ))
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
            restated: {
                // The prompt each call answers: the latest one sent to a call of its conversation.
                let mut turn: BTreeMap<&str, &str> = BTreeMap::new();
                let mut governing: BTreeMap<&str, &str> = BTreeMap::new();
                for call in &truth.calls {
                    if let Some(prompt) = truth
                        .edges
                        .iter()
                        .find(|e| e.kind == "prompt_of" && e.to.as_deref() == Some(&call.id))
                        .and_then(|e| e.from.as_deref())
                        .and_then(|id| context.fact(id))
                    {
                        turn.insert(call.conversation.as_str(), prompt.text());
                    }
                    if let Some(&prompt) = turn.get(call.conversation.as_str()) {
                        governing.insert(call.id.as_str(), prompt);
                    }
                }
                truth
                    .gaps
                    .iter()
                    .filter(|g| g.reason == "framework_restates_prompt")
                    .filter_map(|g| g.subject.as_deref())
                    .filter_map(|call| Some((call, *governing.get(call)?)))
                    .map(|(call, prompt)| {
                        let trace = context
                            .matching
                            .span_of
                            .get(call)
                            .map(|&i| context.recon.generations[i].trace.clone());
                        (call, trace, prompt)
                    })
                    .collect()
            },
        }
    }

    /// A user text restating a prompt, in the trace of a call whose request restates it.
    fn take_restated(&mut self, text: &str, trace: &str) -> bool {
        let found = self.restated.iter().position(|(_, t, prompt)| {
            t.as_deref().is_none_or(|t| t == trace) && !prompt.is_empty() && text.contains(prompt)
        });
        found.map(|at| self.restated.remove(at)).is_some()
    }

    /// A slot of this class in this trace and - in a span scope, which sees one call's output - of one of the
    /// scope's own facts: another call's unknowable answer is not what a block of this span shows. A slot whose
    /// call has no span of its own (an unexported call) is not bounded that way: its answer surfaces wherever
    /// the exporting call recorded it.
    fn take(&mut self, class: &str, trace: &str, own: Option<&BTreeSet<&str>>) -> bool {
        let found = self.slots.iter().position(|(c, t, fact)| {
            *c == class
                && t.as_deref().is_none_or(|t| t == trace)
                && (t.is_none() || own.is_none_or(|own| own.contains(fact)))
        });
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
    let mut system_seen: BTreeSet<(&str, &str, &str)> = BTreeSet::new();
    // Built on first need: which (trace, span, digest) triples each span's *own* view shows. A block another
    // span's composed view shows is that view's statement about its thread, never the home span's.
    let mut span_backed: Option<BTreeSet<(&str, &str, &str)>> = None;
    // Which request-only content a trace has already been credited with: one copy each.
    let mut request_seen: BTreeSet<(&str, &str)> = BTreeSet::new();
    let mut unexplained: BTreeMap<&str, (usize, &Block)> = BTreeMap::new();
    // A span view also shows the context its call was sent, which may re-state another call's turn - a tool
    // result the request carries back, an earlier answer. A block a truth fact of the block's trace shows is
    // that fact's copy, credited once per fact AND once per content: two facts that state one content - two
    // calls' identical reasoning - still excuse only one block, since the request sent each context once. Never
    // a fact of this span's own call, asserted or not: its block the assignment claims, so a second copy of
    // the span's own output stays a duplicate.
    let own: BTreeSet<&str> = scope
        .own
        .as_ref()
        .map(|own| own.outputs.iter().copied().collect())
        .unwrap_or_default();
    let mut fact_used: BTreeSet<&str> = BTreeSet::new();
    let mut content_used: BTreeSet<&str> = BTreeSet::new();
    // One budget per result call id across every path in a span scope: two encodings of one recorded result
    // carry one id, and the request sent that result once.
    let mut result_ids_used: BTreeSet<&str> = BTreeSet::new();
    // The span scope's own output ids, bounding its gap slots as well as the credit above.
    let own_slots: Option<BTreeSet<&str>> = scope
        .own
        .as_ref()
        .map(|own| own.outputs.iter().copied().collect());
    // The order calls ran in: context a span was sent can only be an earlier turn, so a later call's output
    // copied into this view is never credited. A tool result follows its call's output - a server tool's
    // arrives inside the call's own exchange - so it is context from its call's position on, where any other
    // output is context only from the next call's.
    let call_index: BTreeMap<&str, usize> = context
        .truth
        .calls
        .iter()
        .enumerate()
        .map(|(index, call)| (call.id.as_str(), index))
        .collect();
    // The tool-call ids of the scope's own calls: a non-deterministic result of one of them may sit in this
    // span's output (the gap says its value cannot be checked), where another call's must be what the scope's
    // request carried.
    let own_result_ids: BTreeSet<String> = scope
        .own
        .as_ref()
        .map(|own| {
            context
                .truth
                .facts
                .iter()
                .filter(|f| f.kind == "tool_call")
                .filter(|f| {
                    context
                        .home_call(f)
                        .is_some_and(|call| own.calls.contains(&call))
                })
                .filter_map(|f| {
                    assigned.rewritten.get(&f.id).cloned().or_else(|| {
                        f.value
                            .get("id")
                            .and_then(|v| v.as_str())
                            .map(str::to_owned)
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let scope_call = scope.own.as_ref().and_then(|own| {
        own.calls
            .iter()
            .filter_map(|call| call_index.get(call).copied())
            .max()
    });
    // Where the scope's calls have recorded requests, the context they were sent is known exactly: the credit
    // then also requires some recorded request of this scope to carry the block. `None` where no request is
    // recorded, and the call-order bound stands alone.
    let scope_requests: Option<Vec<super::requests::Expected>> =
        scope.own.as_ref().and_then(|own| {
            let recorded = context.truth.requests.get(&context.recon.fixture)?;
            let unsigned = super::requests::unsigned(context.truth);
            Some(
                own.calls
                    .iter()
                    .filter_map(|call| Some((call, recorded.calls.get(*call)?)))
                    .flat_map(|(call, request)| {
                        super::requests::expected(call, request, &BTreeMap::new(), &unsigned)
                    })
                    .collect::<Vec<super::requests::Expected>>(),
            )
        });
    // The block an unknowable answer's slot explained last, as (view, index, span): the next block of
    // the same span is that answer's next segment when it is assistant text too - a streamed answer
    // arrives as consecutive text parts, and the truth cannot know where it was split.
    let mut answer_run: Option<(usize, usize, &str, &str)> = None;
    for (at, (view, index, block)) in scope.blocks.iter().enumerate() {
        if claimed.contains(&at) || claimed_digests.contains(block.identity.as_str()) {
            continue;
        }
        let trace = block.trace.as_str();
        // What the model was sent, from the request its call recorded, shown once in its trace. A bounded
        // allowance, not a blanket one: the second copy is a duplicate the views must not show, and
        // "some request carried these bytes" would otherwise excuse any number of them.
        // In a span scope the request allowance and the fact credit below share one content budget - a system
        // prompt both a request carries as its own and the truth states as a fact must not pass twice - and the
        // request must be one of this scope's: another call's preamble is not what this span was sent.
        // Of a result block only: a tool call carries the same id on its side, and crediting the call must
        // not spend its result's budget.
        let result_id_spent = |used: &BTreeSet<&str>, block: &Block| {
            scope.kind == ViewKind::Span
                && block.kind == "tool_result"
                && block.result_call_id().is_some_and(|id| used.contains(id))
        };
        if context.accounted.blocks.contains(&block.identity)
            && !result_id_spent(&result_ids_used, block)
            && (scope.kind != ViewKind::Span
                || (!content_used.contains(block.identity.as_str())
                    && scope_requests.as_ref().is_none_or(|items| {
                        items
                            .iter()
                            .any(|item| super::requests::matches(item, block))
                    })))
            && request_seen.insert((trace, block.identity.as_str()))
        {
            if scope.kind == ViewKind::Span {
                content_used.insert(block.identity.as_str());
                // A result explained here spends the call id's budget and gap allowance too: the gap must not
                // pass a second result of the same call whose bytes differ, nor the request a second encoding.
                if block.kind == "tool_result"
                    && let Some(id) = block.result_call_id()
                {
                    allowance.results_of.remove(id);
                    result_ids_used.insert(id);
                }
            }
            continue;
        }
        if scope.kind == ViewKind::Span
            && !content_used.contains(block.identity.as_str())
            && !result_id_spent(&result_ids_used, block)
        {
            let credited = context.truth.facts.iter().any(|fact| {
                !own.contains(fact.id.as_str())
                    && !fact_used.contains(fact.id.as_str())
                    && context.home_trace.get(fact.id.as_str()).map(String::as_str) == Some(trace)
                    && context
                        .home_call(fact)
                        .and_then(|call| call_index.get(call))
                        .zip(scope_call.as_ref())
                        .is_some_and(|(fact_call, own_call)| {
                            if fact.kind == "tool_result" {
                                fact_call <= own_call
                            } else {
                                fact_call < own_call
                            }
                        })
                    && (own_result(context, scope, fact)
                        || scope_requests.as_ref().is_none_or(|items| {
                            items
                                .iter()
                                .any(|item| super::requests::matches(item, block))
                        }))
                    && matches!(
                        super::predicates::shows(
                            fact,
                            block,
                            super::checks::paired_call_id(context, fact, assigned).as_deref(),
                        ),
                        super::predicates::Shows::Yes | super::predicates::Shows::Assigned(_)
                    )
                    && fact_used.insert(fact.id.as_str())
            });
            if credited {
                content_used.insert(block.identity.as_str());
                if block.kind == "tool_result"
                    && let Some(id) = block.result_call_id()
                {
                    allowance.results_of.remove(id);
                    result_ids_used.insert(id);
                }
                continue;
            }
        }
        let routed = allowance.routing && allowance.in_conversation(trace);
        // The budget covers every explanation path in a span scope: a second block of one content is a second
        // copy, whichever gap would have explained the first.
        let spent = scope.kind == ViewKind::Span && content_used.contains(block.identity.as_str());
        let explained = !spent
            && match (block.role.as_str(), block.kind.as_str()) {
                ("system", _) => {
                    // With routing unknowable, which agent was sent which instruction is the gap's own
                    // statement - so identical system texts on different spans may each be an agent's, and
                    // each span gets one slot. Only a span whose own view shows the same text vouches for an
                    // independent occurrence: an invented copy attributed to some other span of the trace is
                    // not explained by naming a span, and the same text twice from one span stays unexplained.
                    let routed_slot = allowance.routing
                        && span_backed
                            .get_or_insert_with(|| {
                                context
                                    .recon
                                    .views
                                    .iter()
                                    .filter(|view| view.kind == ViewKind::Span)
                                    .flat_map(|view| {
                                        view.blocks
                                            .iter()
                                            // The view's target pair, not the block's own claim: span ids are
                                            // not unique across traces, and a composed view may show another
                                            // trace's block attributed to a span spelled like its own.
                                            .filter(|shown| {
                                                shown.span == view.key && shown.trace == view.trace
                                            })
                                            .map(|shown| {
                                                (
                                                    shown.trace.as_str(),
                                                    shown.span.as_str(),
                                                    shown.digest.as_str(),
                                                )
                                            })
                                    })
                                    .collect()
                            })
                            .contains(&(
                                block.trace.as_str(),
                                block.span.as_str(),
                                block.digest.as_str(),
                            ));
                    allowance.system_prompt
                        && allowance.in_conversation(trace)
                        && (!allowance.routing || routed_slot)
                        && system_seen.insert((
                            trace,
                            if allowance.routing {
                                block.digest.as_str()
                            } else {
                                ""
                            },
                            if allowance.routing {
                                block.span.as_str()
                            } else {
                                ""
                            },
                        ))
                }
                ("assistant", "thinking" | "redacted_thinking") => {
                    allowance.take("reasoning", trace, own_slots.as_ref())
                }
                // A segment, not a copy: a block repeating the one before it is a duplicate.
                ("assistant", "text")
                    if answer_run.is_some_and(|(v, i, span, digest)| {
                        v == *view
                            && i + 1 == *index
                            && span == block.span.as_str()
                            && digest != block.digest.as_str()
                    }) =>
                {
                    true
                }
                ("assistant", "text" | "json") => allowance.take("text", trace, own_slots.as_ref()),
                ("tool", "tool_result") => {
                    (routed && !result_id_spent(&result_ids_used, block))
                        || block.result_call_id().is_some_and(|id| {
                            allowance.results_of.contains(id)
                                && (scope.kind != ViewKind::Span
                                    || own_result_ids.contains(id)
                                    || scope_requests.as_ref().is_none_or(|items| {
                                        items
                                            .iter()
                                            .any(|item| super::requests::matches(item, block))
                                    }))
                                && allowance.results_of.remove(id)
                                && result_ids_used.insert(id)
                        })
                }
                ("user", "text") => {
                    routed
                        || block
                            .text()
                            .is_some_and(|text| allowance.take_restated(text, trace))
                        || take(&mut allowance.user_text)
                }
                _ => false,
            };
        if explained && scope.kind == ViewKind::Span {
            content_used.insert(block.identity.as_str());
            // Whatever explained a result, its call id's budget is spent.
            if block.kind == "tool_result"
                && let Some(id) = block.result_call_id()
            {
                result_ids_used.insert(id);
            }
        }
        answer_run = (explained && block.role == "assistant" && block.kind == "text").then_some((
            *view,
            *index,
            block.span.as_str(),
            block.digest.as_str(),
        ));
        if !explained {
            unexplained
                .entry(block.digest.as_str())
                .or_insert((0, block))
                .0 += 1;
        }
    }
    // An allowance nothing used is a declaration the capture contradicts: the framework was said to
    // restate the prompt in this call's request, and the reconstruction shows no such message.
    let traces: BTreeSet<&str> = scope
        .blocks
        .iter()
        .map(|(_, _, b)| b.trace.as_str())
        .collect();
    for (call, trace, _) in &allowance.restated {
        // A span scope holds only its call's outputs, so the restating user text - request context - can never
        // sit in it: the declaration is held to account where the request is visible.
        if scope.kind != ViewKind::Span && trace.as_deref().is_some_and(|t| traces.contains(t)) {
            out.push(Violation::new(
                ViolationView::from(scope.kind),
                "gap.unused",
                call,
                "the framework is declared to restate the prompt in this call's request, and no \
                 message does"
                    .to_string(),
            ));
        }
    }
    for (digest, (count, block)) in unexplained {
        let content = super::super::canonical_json(&block.content);
        // Each span view is a scope of its own: its obligation is named for the span, as a trace's is not.
        let subject = match &scope.own {
            Some(own) => format!("{digest}@{}", own.label),
            None => digest.to_string(),
        };
        out.push(Violation::new(
            ViolationView::from(scope.kind),
            "extra.unexplained",
            &subject,
            format!(
                "{count} block(s) no fact or gap accounts for: {}",
                preview(&content)
            ),
        ));
    }
}

/// Whether the fact is a tool result of one of the scope's own calls: a server tool's result arrives inside
/// the call's own exchange, so it can never sit in that call's recorded request - the call-order bound and the
/// content match are the evidence there is.
fn own_result(context: &Context<'_>, scope: &Scope<'_>, fact: &super::truth::Fact) -> bool {
    fact.kind == "tool_result"
        && scope.own.as_ref().is_some_and(|own| {
            context
                .home_call(fact)
                .is_some_and(|call| own.calls.contains(&call))
        })
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
    // A call recorded on the run's span (`call_span_not_exported`) is attributed to that span.
    let span_of_call = |call: &str| {
        context
            .matching
            .span_showing(call)
            .map(|i| &recon.generations[i])
    };
    for (fact_id, &at) in &assigned.block_of {
        let Some(fact) = context.fact(fact_id) else {
            continue;
        };
        let block = scope.blocks[at].2;
        let shown_on = || {
            recon
                .generations
                .iter()
                .find(|g| g.span == block.span)
                .map_or("an unknown span", |g| g.label.as_str())
        };
        // A split-off trace shows what its call was sent - a result too - on that call's span, and nowhere else.
        if let Some(split) = scope.split.filter(|s| s.history.contains(fact_id)) {
            if block.span != split.span {
                out.push(Violation::new(
                    ViolationView::from(scope.kind),
                    "attribution.span",
                    fact_id,
                    format!("shown on {}, not the span of {}", shown_on(), split.call),
                ));
            }
            continue;
        }
        // A text the framework writes itself is shown on a span whose payload holds it as written.
        if let Some(spans) = context.authored_spans.get(fact_id) {
            if !spans.contains(&block.span) {
                out.push(Violation::new(
                    ViolationView::from(scope.kind),
                    "attribution.span",
                    fact_id,
                    format!("shown on {}, which did not write it", shown_on()),
                ));
            }
            continue;
        }
        // A part of a call whose request span is silent is shown on a span whose own payload records it.
        if let Some(spans) = context.carried_spans.get(fact_id) {
            if !spans.contains(&(block.trace.clone(), block.span.clone())) {
                out.push(Violation::new(
                    ViolationView::from(scope.kind),
                    "attribution.span",
                    fact_id,
                    format!("shown on {}, whose payload does not record it", shown_on()),
                ));
            }
            continue;
        }
        // Which span reports a result - the tool's, or the next request's - is the framework's, and
        // so is the span of a call no model response states.
        if fact.call.is_none()
            && !["user_text", "user_media", "system"].contains(&fact.kind.as_str())
        {
            continue;
        }
        // A part its producing span does not carry (`output_not_exported`, proven) is owed off that span:
        // on the span of a later call that was handed it back, or for a tool call on the span that ran it,
        // or one enclosing either, and nowhere else. Where neither is known no span can be named for it.
        let off_span = context.owed_off_span(fact);
        let sent = if off_span {
            context.off_span_homes.get(fact_id)
        } else {
            context.sent_spans.get(fact_id)
        };
        if off_span && sent.is_none() {
            continue;
        }
        // An unexported response has no span of its own to be attributed to.
        if fact.call.as_deref().is_some_and(|c| context.unexported(c)) {
            continue;
        }
        let Some(generation) = context.home_call(fact).and_then(span_of_call) else {
            continue;
        };
        // A fact the requests re-sent belongs on a span that was sent it, or an enclosing one, and nowhere
        // else: a client's preamble goes with every request, so any of those spans is right, and the
        // conversation's first call - the ordinary home of a system prompt - is right only if it was sent it.
        let allowed = match sent {
            Some(spans) => {
                spans.contains(&block.span)
                    || recon
                        .generations
                        .iter()
                        .any(|g| spans.contains(&g.span) && g.ancestors.contains(&block.span))
            }
            None => {
                block.span == generation.span
                    || (fact.call.is_none() && generation.ancestors.contains(&block.span))
            }
        };
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
    if scope.kind != ViewKind::Feed || scope.split.is_some() {
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
