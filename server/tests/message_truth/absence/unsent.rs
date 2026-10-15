//! A turn the producer's telemetry states a call was sent and the call's request never carried
//! (`producer_states_unsent`): a framework composes the message, records the request with it, and its own client
//! drops it before sending.

use super::Proof;
use super::haystack::Haystack;
use crate::message_truth::matching::Matching;
use crate::message_truth::recon::Recon;
use crate::message_truth::truth::Truth;
use serde_json::Value;

/// The gap holds where the capture's transcript records the call's request and no string of it holds the text,
/// and the span the call is tied to by its output states it as a turn of the request: the reading shows it on that
/// span's input side as a user text, exactly, and a message of the span's own raw payload stating the `user` role
/// holds it as one whole string. Text found anywhere else in the span - its output, a diagnostic attribute - is no
/// statement that it was sent. A request that carries it - the release that does send the turn - refuses it; so
/// does a call with no recorded request or no span of its own, since then neither half can be seen.
pub(super) fn prove_unsent(
    truth: &Truth,
    call: &str,
    text: &str,
    recon: &Recon,
    matching: &Matching,
    haystack: &Haystack,
) -> Proof {
    if text.trim().is_empty() {
        return Proof::Unprovable("the gap states no text".to_string());
    }
    let Some(request) = truth
        .requests
        .get(&recon.fixture)
        .and_then(|recorded| recorded.calls.get(call))
    else {
        return Proof::Unprovable(format!("the transcript records no request of {call}"));
    };
    let parts = request
        .system
        .iter()
        .chain(request.messages.iter().flat_map(|m| m.parts.iter()));
    for occurrence in parts {
        if holds_text(&occurrence.part, text) {
            return Proof::Present(format!("{call}'s recorded request carries it"));
        }
    }
    let Some(&generation) = matching.span_of.get(call) else {
        return Proof::Unprovable(format!("{call} is tied to no span of its own"));
    };
    let generation = &recon.generations[generation];
    let shown_as_sent = recon.generation_view(generation).is_some_and(|view| {
        view.blocks
            .iter()
            .any(|b| !b.output && b.is("user", "text") && b.text() == Some(text))
    });
    if !shown_as_sent {
        return Proof::Unprovable(format!(
            "{} does not show it as a user turn of its request",
            generation.label
        ));
    }
    let stated = haystack
        .carriers
        .iter()
        .filter(|c| {
            c.trace.as_deref() == Some(generation.trace.as_str())
                && c.span.as_deref() == Some(generation.span.as_str())
        })
        .any(|c| super::find_node(c, |node| user_message_holding(node, text)).is_some());
    if !stated {
        return Proof::Unprovable(format!(
            "no user message of {}'s raw payload holds it",
            generation.label
        ));
    }
    Proof::Absent
}

/// A message stating the `user` role that holds the text as one whole string, at any depth of its members.
fn user_message_holding(node: &Value, text: &str) -> bool {
    fn whole(value: &Value, text: &str) -> bool {
        match value {
            Value::String(s) => s == text,
            Value::Array(items) => items.iter().any(|v| whole(v, text)),
            Value::Object(members) => members.values().any(|v| whole(v, text)),
            _ => false,
        }
    }
    node.get("role").and_then(Value::as_str) == Some("user") && whole(node, text)
}

/// Whether any string of a recorded part holds the text, whole or inside a longer one.
fn holds_text(value: &Value, text: &str) -> bool {
    match value {
        Value::String(s) => s.contains(text),
        Value::Array(items) => items.iter().any(|v| holds_text(v, text)),
        Value::Object(members) => members.values().any(|v| holds_text(v, text)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message_truth::truth;

    const TEXT: &str = "Handle the requests as specified in the System Instruction.";

    /// The proof on one capture of the ADK tool-use truth, for the call the declared gap names.
    fn proof(fixture: &str, text: &str) -> Proof {
        let fixtures: std::collections::BTreeMap<String, Vec<std::path::PathBuf>> =
            crate::discover_fixtures().into_iter().collect();
        let checked = truth::load_all()["adk/tool_use"].for_fixture(fixture);
        let recon = crate::message_truth::recon::build(fixture, &fixtures[fixture]);
        let matching =
            crate::message_truth::matching::match_calls(&checked, &recon, &mut Vec::new());
        let haystack = Haystack::of_fixture(&fixtures[fixture]);
        prove_unsent(&checked, "call-002", text, &recon, &matching, &haystack)
    }

    /// The turn must be shown on the call's own span, in its trace: a view of another trace reusing the span id
    /// that shows it, while the call's own view lost it, proves nothing.
    #[test]
    fn a_turn_shown_only_by_another_traces_span_refuses_it() {
        let fixture = "adk/native@1.22.1/tool_use";
        let fixtures: std::collections::BTreeMap<String, Vec<std::path::PathBuf>> =
            crate::discover_fixtures().into_iter().collect();
        let checked = truth::load_all()["adk/tool_use"].for_fixture(fixture);
        let mut recon = crate::message_truth::recon::build(fixture, &fixtures[fixture]);
        let matching =
            crate::message_truth::matching::match_calls(&checked, &recon, &mut Vec::new());
        let generation = recon.generations[matching.span_of["call-002"]].clone();
        let at = recon
            .views
            .iter()
            .position(|v| v.key == generation.span && v.trace == generation.trace)
            .expect("the call's span view");
        let mut elsewhere = recon.views[at].clone();
        elsewhere.trace = "000000000000000000000000000000aa".to_string();
        recon.views[at]
            .blocks
            .retain(|b| !(b.is("user", "text") && b.text() == Some(TEXT)));
        recon.views.insert(0, elsewhere);
        let haystack = Haystack::of_fixture(&fixtures[fixture]);
        let refused = prove_unsent(&checked, "call-002", TEXT, &recon, &matching, &haystack);
        assert!(matches!(refused, Proof::Unprovable(_)), "{refused:?}");
    }

    /// Only a message stating the user role, holding the text whole, is a statement that it was sent.
    #[test]
    fn only_a_user_message_holding_the_text_states_it() {
        let holds = |v: serde_json::Value| user_message_holding(&v, TEXT);
        assert!(holds(
            serde_json::json!({"role": "user", "parts": [{"text": TEXT}]})
        ));
        assert!(!holds(
            serde_json::json!({"role": "model", "parts": [{"text": TEXT}]})
        ));
        assert!(!holds(serde_json::json!({"parts": [{"text": TEXT}]})));
        assert!(!holds(
            serde_json::json!({"role": "user", "parts": [{"text": format!("{TEXT} Now.")}]})
        ));
    }

    /// The release whose client drops the turn proves it; the next release, whose request carries it, refuses
    /// the same gap; and a text the span does not state - another framework's wording - is refused too.
    #[test]
    fn a_turn_is_unsent_only_where_the_span_states_it_and_the_request_lacks_it() {
        assert_eq!(proof("adk/native@1.22.1/tool_use", TEXT), Proof::Absent);
        assert!(matches!(
            proof("adk/native@1.23.0/tool_use", TEXT),
            Proof::Present(_)
        ));
        assert!(matches!(
            proof(
                "adk/native@1.22.1/tool_use",
                "Handle the requests as the system says."
            ),
            Proof::Unprovable(_)
        ));
        assert!(matches!(
            proof("adk/native@1.22.1/tool_use", " "),
            Proof::Unprovable(_)
        ));
    }
}
