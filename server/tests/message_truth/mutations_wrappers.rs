//! Mutations for a framework's model-call step around the model call itself (ADK's `call_llm` over
//! `generate_content`). The step's copy of what the call produced is that call's execution, shown once on the
//! call's span; the rubric excuses the step only for that copy, so a second execution and a block of the
//! step's own are both still caught.

use super::mutations::{added, baseline};
use super::predicates::{Shows, shows};
use super::recon::{Recon, ViewKind};

const FIXTURE: &str = "adk/sdk/mcp_tools";

/// The typed generation span with a typed generation below it: the step around the model call.
fn wrapper(recon: &Recon) -> String {
    recon
        .generations
        .iter()
        .find(|outer| {
            outer.typed
                && recon
                    .generations
                    .iter()
                    .any(|inner| inner.typed && inner.ancestors.contains(&outer.span))
        })
        .map(|g| g.span.clone())
        .expect("a model-call step around a model call")
}

#[test]
fn a_step_s_copy_of_its_call_s_tool_call_shown_as_a_second_execution_is_caught() {
    let (truth, recon, baseline) = baseline(FIXTURE);
    let call = truth
        .facts
        .iter()
        .find(|f| f.kind == "tool_call")
        .expect("a tool call");
    let step = wrapper(&recon);
    let mut doubled = recon.clone();
    let mut copies = 0;
    for view in doubled
        .views
        .iter_mut()
        .filter(|v| v.kind == ViewKind::Trace)
    {
        if let Some(at) = view
            .blocks
            .iter()
            .position(|b| shows(call, b, None) != Shows::No)
        {
            let mut copy = view.blocks[at].clone();
            copy.span = step.clone();
            view.blocks.insert(at + 1, copy);
            copies += 1;
        }
    }
    assert!(copies > 0, "the trace view shows the call");
    let new = added(&truth, &doubled, &baseline);
    assert!(
        new.iter()
            .any(|v| v.starts_with("tool_call.duplicated:") && v.ends_with(&call.id)),
        "the step's copy shown as a second execution was not caught: {new:?}"
    );
}

#[test]
fn a_block_the_step_adds_of_its_own_keeps_it_a_generation_no_call_made() {
    let (truth, recon, baseline) = baseline(FIXTURE);
    let step = wrapper(&recon);
    let mut extra = recon.clone();
    let view = extra
        .views
        .iter_mut()
        .find(|v| v.kind == ViewKind::Span && v.key == step)
        .expect("the step's span view");
    let mut own = view
        .blocks
        .iter()
        .find(|b| b.output && b.role == "assistant")
        .expect("an output the step restates")
        .clone();
    own.kind = "text".to_string();
    own.content = serde_json::json!({"type": "text", "text": "A sentence no model call wrote."});
    own.digest = "step-own".to_string();
    own.identity = "step-own".to_string();
    view.blocks.push(own);
    let new = added(&truth, &extra, &baseline);
    assert!(
        new.iter().any(|v| v.starts_with("generation.unexpected:")),
        "the step's own block passed as a restatement: {new:?}"
    );
}

/// The step's span view showing one of its call's outputs twice is a second copy no call explains.
#[test]
fn a_step_repeating_its_call_s_output_is_a_generation_no_call_made() {
    let (truth, recon, baseline) = baseline(FIXTURE);
    let step = wrapper(&recon);
    let mut repeated = recon.clone();
    let view = repeated
        .views
        .iter_mut()
        .find(|v| v.kind == ViewKind::Span && v.key == step)
        .expect("the step's span view");
    let copy = view
        .blocks
        .iter()
        .find(|b| b.output && b.role == "assistant")
        .expect("an output the step restates")
        .clone();
    view.blocks.push(copy);
    let new = added(&truth, &repeated, &baseline);
    assert!(
        new.iter().any(|v| v.starts_with("generation.unexpected:")),
        "the step's repeated copy passed as a restatement: {new:?}"
    );
}

/// A call the step lists under an id of its own is another call, not its call's restated.
#[test]
fn a_step_s_call_under_another_id_is_a_generation_no_call_made() {
    let (truth, recon, baseline) = baseline(FIXTURE);
    let mut renamed = recon.clone();
    let steps: Vec<String> = recon
        .generations
        .iter()
        .filter(|outer| {
            outer.typed
                && recon
                    .generations
                    .iter()
                    .any(|inner| inner.typed && inner.ancestors.contains(&outer.span))
        })
        .map(|g| g.span.clone())
        .collect();
    let call = renamed
        .views
        .iter_mut()
        .filter(|v| v.kind == ViewKind::Span && steps.contains(&v.key))
        .flat_map(|v| v.blocks.iter_mut())
        .find(|b| b.output && b.is("assistant", "tool_use"))
        .expect("a step's copy of a tool call");
    call.content["id"] = serde_json::json!("call_of_the_step");
    call.tool_use_id = Some("call_of_the_step".to_string());
    let new = added(&truth, &renamed, &baseline);
    assert!(
        new.iter().any(|v| v.starts_with("generation.unexpected:")),
        "the step's renamed call passed as a restatement: {new:?}"
    );
}
