//! The rubric's own test: every defect in the catalogue, applied to real reconstructions, is caught,
//! and every legitimate variation is not.
//!
//! A mutation edits the reconstruction model (or, for the cardinality cases, the truth) the way a
//! parser defect would, then the full rubric runs again. A mutation must add a violation the
//! unmutated fixture does not have; a positive control must add none. A mutation that finds no
//! fixture to apply to fails too - a catalogue entry nothing exercises is a check nobody runs. A
//! surviving mutation is a hole in the rubric, to be closed there, never exempted here.

use std::collections::BTreeSet;

use serde_json::{Value, json};

use super::mutate::*;
use super::mutate_framework::*;
use super::recon::Recon;
use super::truth::Truth;

/// Reconstructions the catalogue edits: clean or nearly clean fixtures that between them hold
/// parallel tool calls, sessions, visible reasoning, attachments with bytes, an echoed system
/// prompt, a failed-then-retried attempt and a terminal answer tool.
const POOL: &[&str] = &[
    "strands/sdk/tool_use",
    "strands/sdk/session",
    "strands/sdk/reasoning",
    "strands/sdk/files",
    "strands/sdk/multi_turn",
    "openai-agents/sdk/multi_turn",
    "openai-agents/sdk/tool_use",
    "logfire/sdk/chat",
    "smolagents/sdk/tool_use",
    "claude-agent-sdk/sdk/streaming",
    "logfire/sdk/files",
];

#[derive(Clone, Copy)]
enum Expect {
    /// A defect: at least one new violation.
    Caught,
    /// A legitimate variation: no new violation.
    Clean,
    /// A defect the rubric classes precisely: new violations, all of these assertions.
    Only(&'static [&'static str]),
}

type Apply = fn(&mut Truth, &mut Recon) -> bool;

const CATALOGUE: &[(&str, Expect, Apply)] = &[
    ("delete a response part", Expect::Caught, |t, r| {
        remove_fact(t, r, |f| f.kind == "text" && f.call.is_some())
    }),
    ("delete a call", Expect::Caught, delete_call),
    ("delete a prompt", Expect::Caught, |t, r| {
        remove_fact(t, r, |f| f.kind == "user_text")
    }),
    ("delete an attachment", Expect::Caught, |t, r| {
        remove_fact(t, r, |f| f.kind == "user_media")
    }),
    ("delete a tool result", Expect::Caught, |t, r| {
        remove_fact(t, r, |f| f.kind == "tool_result")
    }),
    ("delete a final answer", Expect::Caught, delete_final_answer),
    ("alter one text byte", Expect::Caught, |t, r| {
        edit_text(t, r, 0, |s| flip(s, 0))
    }),
    ("alter a byte past the preview", Expect::Caught, |t, r| {
        edit_text(t, r, 260, |s| flip(s, 250))
    }),
    ("alter whitespace only", Expect::Caught, |t, r| {
        edit_text(t, r, 0, |s| s.replacen(' ', "  ", 1))
    }),
    ("alter an attachment's bytes", Expect::Caught, alter_media),
    ("alter a tool name", Expect::Caught, |t, r| {
        edit_fact(
            t,
            r,
            |f| f.kind == "tool_call",
            |b| set(b, "name", json!("renamed_tool")),
        )
    }),
    ("alter one argument", Expect::Caught, alter_argument),
    ("alter one result value", Expect::Caught, |t, r| {
        edit_fact(
            t,
            r,
            |f| f.kind == "tool_result",
            |b| set(b, "content", json!({"unexpected": true})),
        )
    }),
    ("change a role", Expect::Caught, |t, r| {
        edit_fact(
            t,
            r,
            |f| f.kind == "text" && f.call.is_some(),
            |b| b.role = "user".into(),
        )
    }),
    ("change a block type", Expect::Caught, |t, r| {
        edit_fact(
            t,
            r,
            |f| f.kind == "text" && f.call.is_some(),
            |b| b.kind = "refusal".into(),
        )
    }),
    ("show reasoning as text", Expect::Caught, |t, r| {
        edit_fact(t, r, visible_reasoning, |b| b.kind = "text".into())
    }),
    (
        "show visible reasoning as redacted",
        Expect::Caught,
        |t, r| {
            edit_fact(t, r, visible_reasoning, |b| {
                b.kind = "redacted_thinking".into();
                b.content = json!({"type": "redacted_thinking", "data": "opaque"});
            })
        },
    ),
    ("swap two parts of a response", Expect::Caught, swap_parts),
    ("swap two calls", Expect::Caught, swap_calls),
    (
        "swap a call and its result",
        Expect::Caught,
        swap_call_and_result,
    ),
    ("swap two session turns", Expect::Caught, swap_session_turns),
    ("duplicate a part", Expect::Caught, |t, r| {
        duplicate(t, r, |f| f.kind == "text" && f.call.is_some())
    }),
    ("duplicate a response", Expect::Caught, |t, r| {
        duplicate(t, r, |f| f.call.is_some())
    }),
    ("re-send history", Expect::Caught, resend_history),
    ("remove a call id", Expect::Caught, |t, r| {
        edit_fact(
            t,
            r,
            |f| f.kind == "tool_call",
            |b| {
                strip(b, "id");
                b.tool_use_id = None;
            },
        )
    }),
    ("change a call id", Expect::Caught, |t, r| {
        edit_fact(
            t,
            r,
            |f| f.kind == "tool_call",
            |b| {
                set(b, "id", json!("call_changed"));
                b.tool_use_id = Some("call_changed".into());
            },
        )
    }),
    ("reuse a call id", Expect::Caught, reuse_call_id),
    (
        "attach a result to the wrong call",
        Expect::Caught,
        result_to_wrong_call,
    ),
    (
        "one block claimed by two identical facts",
        Expect::Caught,
        twin_fact,
    ),
    ("change the model", Expect::Caught, |t, r| {
        edit_generation(
            t,
            r,
            |c| c.model.is_some(),
            |g| {
                g.request_model = Some("another-model".into());
                g.response_model = Some("another-model".into());
            },
        )
    }),
    ("change the response id", Expect::Caught, |t, r| {
        edit_generation(
            t,
            r,
            |c| c.response_id.is_some(),
            |g| g.response_id = Some("resp_other".into()),
        )
    }),
    ("change the finish", Expect::Caught, |t, r| {
        edit_generation(
            t,
            r,
            |c| c.finish.is_some(),
            |g| g.finish = vec!["content_filter".into()],
        )
    }),
    ("change input usage by one", Expect::Caught, |t, r| {
        edit_generation(t, r, |c| usage(c, |u| u.input), |g| g.input += 1)
    }),
    ("change output usage by one", Expect::Caught, |t, r| {
        edit_generation(t, r, |c| usage(c, |u| u.output), |g| g.output += 1)
    }),
    ("change cache-read usage by one", Expect::Caught, |t, r| {
        edit_generation(t, r, |c| usage(c, |u| u.cache_read), |g| g.cache_read += 1)
    }),
    ("change cache-write usage by one", Expect::Caught, |t, r| {
        edit_generation(
            t,
            r,
            |c| usage(c, |u| u.cache_write),
            |g| g.cache_write += 1,
        )
    }),
    ("change reasoning usage by one", Expect::Caught, |t, r| {
        edit_generation(t, r, |c| usage(c, |u| u.reasoning), |g| g.reasoning += 1)
    }),
    (
        "remove a fact from one view",
        Expect::Caught,
        remove_from_trace_view,
    ),
    ("put a fact in the wrong trace", Expect::Caught, wrong_trace),
    (
        "reverse a response's parts in the feed",
        Expect::Caught,
        reverse_in_feed,
    ),
    (
        "turn a failed attempt into output",
        Expect::Caught,
        |t, r| failed_span(t, r, true),
    ),
    ("suppress the retry", Expect::Caught, suppress_retry),
    (
        "make call matching ambiguous",
        Expect::Caught,
        ambiguous_span,
    ),
    (
        "positive: a terminal answer tool needs no result",
        Expect::Clean,
        terminal_answer_without_result,
    ),
    (
        "add content no fact or gap explains",
        Expect::Only(&["extra.unexplained"]),
        extra_content,
    ),
    (
        "positive: a system prompt the truth cannot know",
        Expect::Clean,
        unknowable_system_prompt,
    ),
    (
        "positive: a result as a structured part",
        Expect::Clean,
        |t, r| reencode_result(t, r, |v| json!([{"type": "json", "data": v}])),
    ),
    ("change the response model", Expect::Caught, |t, r| {
        edit_generation(
            t,
            r,
            |c| c.response_model.is_some(),
            |g| g.response_model = Some("another-model".into()),
        )
    }),
    (
        "show a response part on another span",
        Expect::Caught,
        misattribute,
    ),
    (
        "start a later call's span first",
        Expect::Caught,
        reorder_call_spans,
    ),
    (
        "an unexpected generation span",
        Expect::Caught,
        unexpected_generation,
    ),
    (
        "share one span between two calls",
        Expect::Caught,
        share_span,
    ),
    (
        "drop the span of a call nothing asserts",
        Expect::Caught,
        drop_unknowable_call_span,
    ),
    (
        "leak a prompt into another trace",
        Expect::Caught,
        leak_prompt,
    ),
    (
        "show a prompt differently in one view",
        Expect::Caught,
        restyle_in_session,
    ),
    (
        "put a result after the next response",
        Expect::Caught,
        result_after_next_response,
    ),
    (
        "put a prompt after its response",
        Expect::Caught,
        prompt_after_response,
    ),
    (
        "positive: a result in an envelope",
        Expect::Clean,
        |t, r| reencode_result(t, r, |v| json!({"result": v})),
    ),
    ("positive: a result as text parts", Expect::Clean, |t, r| {
        reencode_result(t, r, |v| json!([{"type": "text", "text": v.to_string()}]))
    }),
    (
        "positive: a result as a JSON string",
        Expect::Clean,
        |t, r| reencode_result(t, r, |v| Value::String(v.to_string())),
    ),
    (
        "positive: parallel results in completion order",
        Expect::Clean,
        parallel_results_reordered,
    ),
    (
        "positive: a failed attempt, then the retry",
        Expect::Clean,
        |t, r| failed_span(t, r, false),
    ),
    (
        "positive: a framework-namespaced tool name",
        Expect::Clean,
        |t, r| {
            edit_fact(
                t,
                r,
                |f| f.kind == "tool_result",
                |b| {
                    let name = b
                        .content
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("get_weather")
                        .to_string();
                    set(b, "name", json!(format!("travel-{name}")));
                },
            )
        },
    ),
    (
        "positive: rewritten but consistent ids",
        Expect::Only(&["tool_call.id_rewritten"]),
        rewrite_ids_consistently,
    ),
    (
        "positive: a call the framework names, under the framework's id",
        Expect::Clean,
        framework_named_call,
    ),
    (
        "positive: the framework restates the prompt in a later step's request",
        Expect::Clean,
        |t, r| restate_prompt(t, r, 1),
    ),
    (
        "restate the prompt more often than the framework does",
        Expect::Only(&["extra.unexplained"]),
        |t, r| restate_prompt(t, r, 2),
    ),
    (
        "declare a restated prompt the reconstruction does not show",
        Expect::Only(&["gap.unused"]),
        |t, r| restate_prompt(t, r, 0),
    ),
];

#[test]
fn truth_rubric_rejects_each_mutation() {
    let fixtures: std::collections::BTreeMap<String, Vec<std::path::PathBuf>> =
        crate::discover_fixtures().into_iter().collect();
    let truths = super::Truths::load();
    let pool: Vec<(Truth, Recon, BTreeSet<String>)> = POOL
        .iter()
        .map(|fixture| {
            let paths = fixtures
                .get(*fixture)
                .unwrap_or_else(|| panic!("mutation pool fixture {fixture} is missing"));
            let truth = truths.documents[&truths.of_fixture[*fixture]].for_fixture(fixture);
            let recon = super::recon::build(fixture, paths);
            let baseline = observed(&truth, &recon);
            (truth, recon, baseline)
        })
        .collect();

    let mut failures = Vec::new();
    let mut fired: BTreeSet<String> = BTreeSet::new();
    for (name, expect, apply) in CATALOGUE {
        let mut exercised = false;
        for (truth, recon, baseline) in &pool {
            let (mut truth, mut recon) = (truth.clone(), recon.clone());
            if !apply(&mut truth, &mut recon) {
                continue;
            }
            exercised = true;
            let new: Vec<String> = observed(&truth, &recon)
                .difference(baseline)
                .cloned()
                .collect();
            if !matches!(expect, Expect::Clean) {
                fired.extend(new.iter().map(|v| super::family(assertion_of(v))));
            }
            let verdict = match expect {
                Expect::Caught => (!new.is_empty())
                    .then_some(())
                    .ok_or("survived".to_string()),
                Expect::Clean => new
                    .is_empty()
                    .then_some(())
                    .ok_or(format!("flagged a legitimate variation: {new:?}")),
                Expect::Only(allowed) => {
                    let all_allowed = new
                        .iter()
                        .all(|v| allowed.iter().any(|a| v.contains(&format!(":{a}:"))));
                    (!new.is_empty() && all_allowed)
                        .then_some(())
                        .ok_or(format!("expected only {allowed:?}, got {new:?}"))
                }
            };
            if let Err(why) = verdict {
                failures.push(format!("{name} on {}: {why}", recon.fixture));
            }
        }
        if !exercised {
            failures.push(format!("{name}: no fixture in the pool to apply it to"));
        }
    }
    // Every check the rubric has must be made to fire by some defect; one nothing triggers is
    // untested, however plausible it reads.
    for family in super::ASSERTION_FAMILIES {
        if !fired.contains(*family) {
            failures.push(format!(
                "{family}: no mutation in the catalogue makes this check fire"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "the rubric has holes - close them in the checks, never by exempting the mutation:\n  {}",
        failures.join("\n  ")
    );
}

/// The assertion of an observed `fixture:view:assertion:subject#fingerprint`.
fn assertion_of(observed: &str) -> &str {
    observed.split(':').nth(2).unwrap_or("")
}

/// Violation ids with their fingerprints, so a violation that changes is a new one.
fn observed(truth: &Truth, recon: &Recon) -> BTreeSet<String> {
    super::check(truth, recon)
        .into_iter()
        .map(|v| format!("{}#{}", v.id(), v.fingerprint()))
        .collect()
}
