//! The request a call's span is held to where the producer states a turn its client never sent
//! (`producer_states_unsent`, proven in `absence::unsent`): the recorded request, with the turn appended to its
//! last user message, which is where the framework wrote it.

use std::borrow::Cow;

use serde_json::json;

use crate::message_truth::matching::Matching;
use crate::message_truth::recon::{Block, Recon};
use crate::message_truth::truth::{CallRequest, Occurrence, RequestMessage, Truth};

/// The texts proven gaps of this fixture's mode state were sent to a call (`producer_states_unsent`).
fn stated_texts<'t>(truth: &'t Truth, fixture: &str, call: &str) -> Vec<&'t str> {
    truth
        .gaps
        .iter()
        .filter(|g| {
            g.reason == "producer_states_unsent"
                && g.subject.as_deref() == Some(call)
                && g.holds_for(fixture)
        })
        .filter_map(|g| g.text.as_deref())
        .collect()
}

/// The blocks a stated turn accounts for in the conversation views: a user text exactly the stated text, on the
/// span the call is tied to by its output, in that span's trace - never the same words anywhere else.
pub(super) fn account_stated(
    truth: &Truth,
    recon: &Recon,
    matching: &Matching,
    everywhere: &[&Block],
    accounted: &mut super::Accounted,
) {
    for (call, &at) in &matching.span_of {
        let generation = &recon.generations[at];
        for text in stated_texts(truth, &recon.fixture, call) {
            for block in everywhere.iter().filter(|b| {
                b.trace == generation.trace
                    && b.span == generation.span
                    && b.is("user", "text")
                    && b.text() == Some(text)
            }) {
                accounted.stated.insert((
                    block.trace.clone(),
                    block.span.clone(),
                    block.identity.clone(),
                ));
            }
        }
    }
}

/// The call's recorded request as the producer states it was sent: unchanged unless a proven gap of this
/// fixture's mode names the call, and then with each gap's text as one more text part of the last user message
/// (a new one where there is none). The part names no fact, so it accounts only for blocks showing that text.
pub(super) fn as_stated<'r>(
    truth: &Truth,
    fixture: &str,
    call: &str,
    request: &'r CallRequest,
) -> Cow<'r, CallRequest> {
    let texts = stated_texts(truth, fixture, call);
    if texts.is_empty() {
        return Cow::Borrowed(request);
    }
    let mut stated = request.clone();
    let at = match stated.messages.iter().rposition(|m| m.role == "user") {
        Some(at) => at,
        None => {
            stated.messages.push(RequestMessage {
                role: "user".to_string(),
                parts: Vec::new(),
            });
            stated.messages.len() - 1
        }
    };
    for text in texts {
        stated.messages[at].parts.push(Occurrence {
            part: json!({"type": "text", "text": text}),
            new_fact: None,
            replay_of: None,
            new: Some("producer_states_unsent".to_string()),
            lineage_unknown: None,
            renders: Vec::new(),
        });
    }
    Cow::Owned(stated)
}

#[cfg(test)]
mod tests {
    use crate::message_truth::truth;

    /// The request views of ADK 1.22.1's tool-use capture, with the declared gap's text replaced by `text`.
    fn request_extras(text: &str) -> Vec<String> {
        let fixture = "adk/native@1.22.1/tool_use";
        let fixtures: std::collections::BTreeMap<String, Vec<std::path::PathBuf>> =
            crate::discover_fixtures().into_iter().collect();
        let mut checked = truth::load_all()["adk/tool_use"].for_fixture(fixture);
        for gap in checked
            .gaps
            .iter_mut()
            .filter(|g| g.reason == "producer_states_unsent")
        {
            gap.text = Some(text.to_string());
        }
        let recon = crate::message_truth::recon::build(fixture, &fixtures[fixture]);
        let matching =
            crate::message_truth::matching::match_calls(&checked, &recon, &mut Vec::new());
        let mut out = Vec::new();
        super::super::check_requests(&checked, &recon, &matching, &mut out);
        out.into_iter()
            .filter(|v| v.assertion == "request.extra")
            .map(|v| v.subject)
            .collect()
    }

    /// The stated text accounts for its block on the call's span alone: the same words on another span of the
    /// trace, or in another trace - a parser leaking the turn - stay unexplained.
    #[test]
    fn a_stated_turn_explains_no_copy_elsewhere() {
        let fixture = "adk/native@1.22.1/tool_use";
        let fixtures: std::collections::BTreeMap<String, Vec<std::path::PathBuf>> =
            crate::discover_fixtures().into_iter().collect();
        let checked = truth::load_all()["adk/tool_use"].for_fixture(fixture);
        let recon = crate::message_truth::recon::build(fixture, &fixtures[fixture]);
        let matching =
            crate::message_truth::matching::match_calls(&checked, &recon, &mut Vec::new());
        let stated = recon
            .views
            .iter()
            .flat_map(|v| v.blocks.iter())
            .find(|b| {
                b.is("user", "text")
                    && b.text()
                        == Some("Handle the requests as specified in the System Instruction.")
            })
            .expect("the stated turn is shown")
            .clone();
        let mut other_span = stated.clone();
        other_span.span = "00000000000000aa".to_string();
        let mut other_trace = stated.clone();
        other_trace.trace = "000000000000000000000000000000aa".to_string();
        let everywhere = [&stated, &other_span, &other_trace];
        let mut accounted = super::super::Accounted::default();
        super::account_stated(&checked, &recon, &matching, &everywhere, &mut accounted);
        assert!(accounted.explains(&stated));
        assert!(!accounted.explains(&other_span));
        assert!(!accounted.explains(&other_trace));
    }

    /// The stated turn accounts for the one block showing its own text: the declared text leaves the span's input
    /// with nothing extra, and any other text - a mutated declaration - leaves the block the request did not send.
    #[test]
    fn a_stated_turn_accounts_only_for_its_own_text() {
        assert_eq!(
            request_extras("Handle the requests as specified in the System Instruction."),
            Vec::<String>::new()
        );
        assert_eq!(
            request_extras("Handle the requests as the system instruction says."),
            vec!["call-002:in10:user/text".to_string()]
        );
    }
}
