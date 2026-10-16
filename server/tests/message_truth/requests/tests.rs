//! What shows a request part: renderings, reissued calls, references, provider runs and encoded results.

use super::*;

pub(super) fn block(role: &str, text: &str) -> Block {
    Block {
        role: role.to_string(),
        kind: "text".to_string(),
        content: json!({ "type": "text", "text": text }),
        tool_use_id: None,
        trace: "t".to_string(),
        span: "s".to_string(),
        output: false,
        finish: None,
        media_sha256: None,
        digest: text.to_string(),
        identity: text.to_string(),
        carrier: String::new(),
        position: String::new(),
    }
}

pub(super) fn part(role: &'static str, text: &str) -> Expected {
    let part = json!({ "type": "text", "text": text });
    Expected {
        label: "call-001:m0.0".to_string(),
        role,
        fact: as_fact("call-001:m0.0", role, &part, &BTreeMap::new())
            .expect("a text part is a fact"),
        part,
        message: Some(0),
        is_fact: false,
        renders: Vec::new(),
    }
}

fn sent_result(text: &str) -> Expected {
    let part = json!({
        "type": "tool_result",
        "id": "call-1",
        "content": [{"type": "text", "text": text}],
        "is_error": false,
    });
    Expected {
        label: "call-002:m2.0".to_string(),
        role: "tool",
        fact: as_fact("call-002:m2.0", "tool", &part, &BTreeMap::new())
            .expect("a tool result part is a fact"),
        part,
        message: Some(2),
        is_fact: false,
        renders: Vec::new(),
    }
}

fn shown_result(value: Value) -> Block {
    let content = json!({"type": "tool_result", "tool_use_id": "call-1", "content": value});
    Block {
        role: "tool".to_string(),
        kind: "tool_result".to_string(),
        digest: content.to_string(),
        identity: content.to_string(),
        carrier: String::new(),
        position: String::new(),
        content,
        tool_use_id: Some("call-1".to_string()),
        trace: "t".to_string(),
        span: "s".to_string(),
        output: false,
        finish: None,
        media_sha256: None,
    }
}

/// A result the client sent as Python's `str()` of the tool's value is shown by that value only where the
/// value's `repr` is the sent text exactly.
#[test]
fn a_python_rendering_is_shown_only_by_the_value_it_renders_byte_for_byte() {
    let sent = sent_result(
        "{'city': 'Paris', 'days': [1, 2], 'rain': True, 'note': None, 'high_c': 21.5, 'say': \"it's\"}",
    );
    let faithful = shown_result(json!({
        "city": "Paris", "days": [1, 2], "rain": true, "note": null, "high_c": 21.5, "say": "it's"
    }));
    assert!(matches(&sent, &faithful));
    // A key in another order is another rendering.
    let reordered = shown_result(json!({
        "days": [1, 2], "city": "Paris", "rain": true, "note": null, "high_c": 21.5, "say": "it's"
    }));
    assert!(!matches(&sent, &reordered));
    // A string quoted where Python would not quote it so.
    let quoted = sent_result("{'city': \"Paris\"}");
    assert!(!matches(&quoted, &shown_result(json!({"city": "Paris"}))));
    // A tuple is not the list it would be shown as.
    let tuple = sent_result("{'days': (1, 2)}");
    assert!(!matches(&tuple, &shown_result(json!({"days": [1, 2]}))));
    // Spacing is part of the rendering.
    let spaced = sent_result("{'city':'Paris'}");
    assert!(!matches(&spaced, &shown_result(json!({"city": "Paris"}))));
    // And a value of another call is not this one's, however it renders.
    let mut other = shown_result(json!({"city": "Paris"}));
    other.content["tool_use_id"] = json!("call-2");
    other.tool_use_id = Some("call-2".to_string());
    assert!(!matches(&sent_result("{'city': 'Paris'}"), &other));
}

fn shown_call(id: &str) -> Block {
    let content =
        json!({"type": "tool_use", "id": id, "name": "book_flight", "input": {"to": "Oslo"}});
    Block {
        role: "assistant".to_string(),
        kind: "tool_use".to_string(),
        digest: content.to_string(),
        identity: content.to_string(),
        carrier: String::new(),
        position: String::new(),
        content,
        tool_use_id: Some(id.to_string()),
        trace: "t".to_string(),
        span: "s".to_string(),
        output: false,
        finish: None,
        media_sha256: None,
    }
}

/// Two calls with one name and the same arguments - a retry - each shown under its own id are not each other
/// rewritten: that reading swapped their results and reported both out of order. A call shown under an id
/// the provider never issued is still a rewrite.
#[test]
fn a_call_shown_under_another_call_s_id_is_not_a_reissue() {
    let call = |id: &str| json!({"part": {"type": "tool_call", "id": id, "name": "book_flight", "arguments": {"to": "Oslo"}}});
    let request: CallRequest = serde_json::from_value(json!({
        "api": "bedrock.converse", "system": [], "tools": [],
        "messages": [{"role": "assistant", "parts": [call("id-a")]}, {"role": "assistant", "parts": [call("id-b")]}],
    }))
    .expect("a request");
    let both = [shown_call("id-a"), shown_call("id-b")];
    let shown: Vec<&Block> = both.iter().collect();
    let issued: BTreeSet<&str> = ["id-a", "id-b"].into();
    assert_eq!(
        super::rewrites::rewrite_map(&request, &shown, &issued),
        BTreeMap::new()
    );
    let unaware = super::rewrites::rewrite_map(&request, &shown, &BTreeSet::new());
    assert_eq!(
        unaware.get("id-a").map(String::as_str),
        Some("id-b"),
        "without the issued ids they swap"
    );

    let reissued = [shown_call("framework-1")];
    let shown: Vec<&Block> = reissued.iter().collect();
    let map = super::rewrites::rewrite_map(&request, &shown, &issued);
    assert_eq!(map.get("id-a").map(String::as_str), Some("framework-1"));
}

fn shown_media(kind: &str, source: &str, data: &str) -> Block {
    let content = json!({ "type": kind, "source": source, "data": data });
    Block {
        role: "user".to_string(),
        kind: kind.to_string(),
        content: content.clone(),
        tool_use_id: None,
        trace: "t".to_string(),
        span: "s".to_string(),
        output: false,
        finish: None,
        media_sha256: None,
        digest: content.to_string(),
        identity: content.to_string(),
        carrier: String::new(),
        position: String::new(),
    }
}

#[test]
fn an_attachment_sent_by_reference_is_shown_only_as_that_reference() {
    let sent = |modality: &str, source: &str, reference: &str| {
        let part = json!({"type": "media", "modality": modality, "media_type": null,
            "source": source, "reference": reference});
        as_fact("call-001:m0.1", "user", &part, &BTreeMap::new()).expect("a media part is a fact")
    };
    let shown = |fact: &Fact, block: &Block| matches!(shows(fact, block, None), Shows::Yes);
    let url = "https://example.com/photo.jpg";
    let image = sent("image", "url", url);
    assert_eq!(
        image.require.as_ref().map(|r| r.matcher.as_str()),
        Some("reference")
    );
    assert!(shown(&image, &shown_media("image", "url", url)));
    assert!(!shown(
        &image,
        &shown_media("image", "url", "https://example.com/other.jpg")
    ));
    assert!(!shown(&image, &shown_media("document", "url", url)));
    assert!(!shown(&image, &shown_media("image", "base64", url)));
    // A request that names a plain file does not say what kind it is: any media kind shows it, by its id.
    let file = sent("file", "file_id", "file-1234567890");
    assert!(shown(
        &file,
        &shown_media("document", "file_id", "file-1234567890")
    ));
    assert!(shown(
        &file,
        &shown_media("file", "file_id", "file-1234567890")
    ));
    assert!(!shown(
        &file,
        &shown_media("text", "file_id", "file-1234567890")
    ));
    assert!(!shown(
        &file,
        &shown_media("file", "file_id", "file-0987654321")
    ));
}

#[test]
fn a_provider_run_call_is_shown_only_by_the_whole_values_it_searched_for() {
    let fact = Fact {
        id: "fact-001".to_string(),
        kind: "tool_call".to_string(),
        role: "assistant".to_string(),
        conversation: String::new(),
        evidence: "wire".into(),
        value: json!({"id": "ws_1", "name": "web_search", "arguments": {"query": "Louvre opening hours"}}),
        require: requirement("contains"),
        call: None,
        fixtures: None,
        seal: None,
        kept_from_model: false,
    };
    let call = |input: Value| {
        let content = json!({"type": "tool_use", "id": "ws_1", "name": "web_search", "input": input,
            "provider_executed": true});
        Block {
            role: "assistant".to_string(),
            kind: "tool_use".to_string(),
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
    };
    let shown = |input: Value| shows(&fact, &call(input), None) == Shows::Yes;
    // Re-shaped in an instrumentation's own words, or written as a JSON document, it is the same search.
    assert!(shown(
        json!({"type": "web_search", "action": {"query": "Louvre opening hours"}})
    ));
    assert!(shown(
        json!({"action": "{\"query\": \"Louvre opening hours\"}"})
    ));
    // A longer query is another search.
    assert!(!shown(json!({"query": "Louvre opening hours tomorrow"})));
    assert!(!shown(json!({})));
    // The same search shown as a call the application ran is not the provider's run.
    let mut ran_by_client = call(json!({"query": "Louvre opening hours"}));
    ran_by_client.content["provider_executed"] = json!(false);
    assert_eq!(shows(&fact, &ran_by_client, None), Shows::No);
}

#[test]
fn a_provider_run_that_found_nothing_is_answered_by_the_result_paired_to_it() {
    let fact = |call_id: &str| Fact {
        id: "fact-002".to_string(),
        kind: "tool_result".to_string(),
        role: "tool".to_string(),
        conversation: String::new(),
        evidence: "wire".into(),
        value: json!({"call_id": call_id, "name": "web_search", "value": {"sources": []}, "is_error": false}),
        require: requirement("contains"),
        call: None,
        fixtures: None,
        seal: None,
        kept_from_model: false,
    };
    let result_of = |call_id: &str, shown: Value| {
        let content = json!({"type": "tool_result", "tool_use_id": call_id, "content": shown,
            "provider_executed": true});
        Block {
            role: "tool".to_string(),
            kind: "tool_result".to_string(),
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
    };
    let nothing = json!([{"type": "web_search", "status": "completed", "sources": []}]);
    let result = |call_id: &str| result_of(call_id, nothing.clone());
    assert_eq!(shows(&fact("ws_1"), &result("ws_1"), None), Shows::Yes);
    assert_eq!(
        shows(&fact("ws_1"), &result_of("ws_1", json!([])), None),
        Shows::Yes
    );
    assert_eq!(shows(&fact("ws_1"), &result("ws_2"), None), Shows::No);
    // A result paired to the call that shows something after all does not show that nothing was found.
    for something in [
        json!([{"sources": [{"url": "https://a"}]}]),
        json!("sunny"),
        Value::Null,
        json!([{"error_code": "unavailable"}]),
        json!([{"type": "web_search", "status": "completed"}]),
    ] {
        assert_eq!(
            shows(&fact("ws_1"), &result_of("ws_1", something.clone()), None),
            Shows::No,
            "{something}"
        );
    }
    // With no id to pair by, an empty expectation is no evidence at all.
    assert_eq!(shows(&fact(""), &result(""), None), Shows::No);
    // A result shown as one the application's tool returned is not the provider's.
    let mut returned = result("ws_1");
    returned.content["provider_executed"] = json!(false);
    assert_eq!(shows(&fact("ws_1"), &returned, None), Shows::No);
}

/// A result sent in an encoding of the client's own - a text part holding the JSON of its content list - is shown
/// by the canonical value the view reduces it to, exactly, and by no other value.
#[test]
fn a_result_sent_encoded_is_shown_by_the_value_it_reduces_to() {
    let sent = sent_result(r#"[{"type":"text","text":"395.0"}]"#);
    assert!(matches(&sent, &shown_result(json!("395.0"))));
    assert!(matches(
        &sent,
        &shown_result(json!([{"type": "text", "text": "395.0"}]))
    ));
    assert!(!matches(&sent, &shown_result(json!("396.0"))));
    assert!(!matches(&sent, &shown_result(json!("the answer is 395.0"))));
}
