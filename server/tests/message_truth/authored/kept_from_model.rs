//! A framework text kept from the model (`kept_from_model`): refused wherever a later request carries it.

use super::super::truth::Truth;
use super::refusal;

/// A reply the framework keeps from the model is proven by no later request carrying it: one that does - under
/// any role, as written - refuses it, and so does declaring it sent where the later requests lack it.
#[test]
fn a_reply_kept_from_the_model_is_refused_where_a_later_request_carries_it() {
    let fixture = "agentscope/sdk/multi_agent";
    let paths = crate::discover_fixtures()
        .into_iter()
        .find(|(label, _)| label == fixture)
        .map(|(_, paths)| paths)
        .expect("the capture");
    let truth = super::super::truth::load_all()["agentscope/multi_agent"].for_fixture(fixture);
    let recon = super::super::recon::build(fixture, &paths);
    let matching = super::super::matching::match_calls(&truth, &recon, &mut Vec::new());
    let reply = truth
        .facts
        .iter()
        .find(|f| f.evidence == "framework" && f.kept_from_model)
        .expect("a reply kept from the model")
        .clone();
    assert_eq!(refusal(&truth, fixture, &reply, &recon, &matching), None);
    // Refused as carried, not for anything else a mutation might also break.
    let carried_by = |truth: &Truth| {
        refusal(truth, fixture, &reply, &recon, &matching)
            .is_some_and(|why| why.contains("which a reply kept from the model is not"))
    };

    let mut sent = reply.clone();
    sent.kept_from_model = false;
    assert!(
        refusal(&truth, fixture, &sent, &recon, &matching).is_some(),
        "declared sent, where the later requests lack it"
    );

    let mut carried = truth.clone();
    let request = carried
        .requests
        .get_mut(fixture)
        .expect("recorded requests");
    let last = request.calls.values_mut().last().expect("a request");
    let message = last
        .messages
        .iter_mut()
        .find(|m| m.role == "assistant")
        .expect("an assistant turn");
    let mut part = message.parts[0].clone();
    part.part = serde_json::json!({"type": "text", "text": reply.text()});
    part.new_fact = None;
    part.replay_of = None;
    message.parts.push(part);
    assert!(
        carried_by(&carried),
        "a later request carries it as written"
    );

    // Re-wrapped, rendered, or sent to a later attempt that failed: still carried.
    let later_part = |truth: &mut Truth, edit: &dyn Fn(&mut super::super::truth::Occurrence)| {
        let request = truth.requests.get_mut(fixture).expect("recorded requests");
        let last = request.calls.values_mut().last().expect("a request");
        let message = last
            .messages
            .iter_mut()
            .find(|m| m.role == "assistant")
            .expect("an assistant turn");
        let mut part = message.parts[0].clone();
        part.new_fact = None;
        part.replay_of = None;
        edit(&mut part);
        message.parts.push(part);
    };
    let mut rewrapped = truth.clone();
    later_part(&mut rewrapped, &|part| {
        part.part = serde_json::json!({"type": "text", "text": format!("Status: {}", reply.text().replace(' ', "\n"))});
    });
    assert!(
        carried_by(&rewrapped),
        "a later request carries it re-wrapped"
    );
    let mut rendered = truth.clone();
    later_part(&mut rendered, &|part| {
        part.part = serde_json::json!({"type": "text", "text": "the leader is waiting"});
        part.renders = vec![reply.id.clone()];
    });
    assert!(carried_by(&rendered), "a later request renders it");
    let mut failed = truth.clone();
    let mut attempt = failed.calls.last().expect("a call").clone();
    attempt.id = "call-failed".into();
    attempt.outcome = "error".into();
    attempt.outputs = Vec::new();
    failed.calls.push(attempt);
    let request = failed.requests.get_mut(fixture).expect("recorded requests");
    let mut sent = request.calls.values().last().expect("a request").clone();
    // As the assistant's turn, which the user-or-system check above does not read.
    let turn = sent
        .messages
        .iter_mut()
        .find(|m| m.role == "assistant")
        .expect("an assistant turn");
    let mut part = turn.parts[0].clone();
    part.part = serde_json::json!({"type": "text", "text": reply.text()});
    part.new_fact = None;
    part.replay_of = None;
    turn.parts.push(part);
    request.calls.insert("call-failed".into(), sent);
    assert!(
        carried_by(&failed),
        "a later attempt that failed was sent it"
    );

    // Split across two parts of one turn, mid-word or between words.
    for split in ["permis", "permission "] {
        let (head, tail) = reply
            .text()
            .split_at(reply.text().find(split).expect("the word") + split.len());
        let mut halves = truth.clone();
        later_part(&mut halves, &|part| {
            part.part = serde_json::json!({"type": "text", "text": head})
        });
        later_part(&mut halves, &|part| {
            part.part = serde_json::json!({"type": "text", "text": tail})
        });
        assert!(
            carried_by(&halves),
            "a later request carries it split at {split:?}"
        );
    }
    // Rendered into a later request's system content.
    let mut system = truth.clone();
    let request = system.requests.get_mut(fixture).expect("recorded requests");
    let last = request.calls.values_mut().last().expect("a request");
    let mut part = last.messages[0].parts[0].clone();
    part.part = serde_json::json!({"type": "text", "text": "the leader is waiting"});
    part.new_fact = None;
    part.replay_of = None;
    part.renders = vec![reply.id.clone()];
    last.system.push(part);
    assert!(
        carried_by(&system),
        "a later request's system content renders it"
    );
    // Split mid-word across two system parts.
    let mut split_system = truth.clone();
    let request = split_system
        .requests
        .get_mut(fixture)
        .expect("recorded requests");
    let last = request.calls.values_mut().last().expect("a request");
    let (head, tail) = reply
        .text()
        .split_at(reply.text().find("permis").expect("the word") + 6);
    for half in [head, tail] {
        let mut part = last.messages[0].parts[0].clone();
        part.part = serde_json::json!({"type": "text", "text": half});
        part.new_fact = None;
        part.replay_of = None;
        part.renders = Vec::new();
        last.system.push(part);
    }
    assert!(
        carried_by(&split_system),
        "a later request's system content holds it split"
    );
    // Sent to a later call of another conversation.
    let mut elsewhere = truth.clone();
    for call in elsewhere.calls.iter_mut().skip(1) {
        call.conversation = "conv-elsewhere".into();
    }
    later_part(&mut elsewhere, &|part| {
        part.part = serde_json::json!({"type": "text", "text": reply.text()})
    });
    assert!(
        carried_by(&elsewhere),
        "a later request of another conversation carries it"
    );
    // Split at two boundaries, one mid-word and one between words.
    let mut mixed = truth.clone();
    let text = reply.text();
    let at_word = text.find("permis").expect("the word") + "permis".len();
    let at_space = text.find(" or ").expect("a word break") + 1;
    // The middle piece without its trailing space, so neither uniform join reproduces the reply.
    for piece in [
        &text[..at_word],
        text[at_word..at_space].trim_end(),
        &text[at_space..],
    ] {
        later_part(&mut mixed, &|part| {
            part.part = serde_json::json!({"type": "text", "text": piece})
        });
    }
    assert!(
        carried_by(&mixed),
        "a later request carries it split at mixed boundaries"
    );
    // In the request of a call the cassette lists earlier: any recorded request counts, whatever order.
    let mut earlier = truth.clone();
    let request = earlier
        .requests
        .get_mut(fixture)
        .expect("recorded requests");
    let first = request.calls.values_mut().next().expect("a request");
    let mut part = first.messages[0].parts[0].clone();
    part.part = serde_json::json!({"type": "text", "text": reply.text()});
    part.new_fact = None;
    part.replay_of = None;
    first.messages.push(super::super::truth::RequestMessage {
        role: "assistant".into(),
        parts: vec![part],
    });
    assert!(carried_by(&earlier), "an earlier-listed request carries it");
}
