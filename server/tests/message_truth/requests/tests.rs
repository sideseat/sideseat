//! The merge excuse: a request part the client joined from consecutive messages, shown as those messages.

use super::*;

fn block(role: &str, text: &str) -> Block {
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

fn part(role: &'static str, text: &str) -> Expected {
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

#[test]
fn a_merged_part_is_shown_only_by_its_exact_join() {
    let shown = [
        block("user", "Observation: rain"),
        block("user", "New task: pack"),
    ];
    let blocks: Vec<&Block> = shown.iter().collect();
    let free = [false, false];
    let mut neighbours = EmptyNeighbours::default();
    neighbours.record("llm.input_messages.3.message.role", "user".to_string());
    neighbours.record("llm.input_messages.4.message.role", "user".to_string());
    neighbours.record(
        "llm.input_messages.4.message.contents.0.message_content.text",
        "Observation: rain".to_string(),
    );
    let run = |text: &str| {
        merged_run(
            &part("user", text),
            &blocks,
            &free,
            &free,
            0..2,
            &neighbours,
        )
    };
    // Two consecutive messages joined by a newline, and an empty one the payload holds before the first.
    assert_eq!(run("Observation: rain\nNew task: pack"), Some(0..=1));
    assert_eq!(run("\nObservation: rain"), Some(0..=0));
    // A newline the payload gives no empty message for is a lost byte, not a merge.
    assert_eq!(run("Observation: rain\n"), None);
    // Outside the window the request's neighbouring parts allow, a match keeps no order and is refused.
    let late = merged_run(
        &part("user", "Observation: rain\nNew task: pack"),
        &blocks,
        &free,
        &free,
        1..2,
        &neighbours,
    );
    assert_eq!(late, None);
    // A single block is no merge, and a near miss - another separator, other text - is none either.
    assert_eq!(run("Observation: rain"), None);
    assert_eq!(run("Observation: rain New task: pack"), None);
    assert_eq!(run("Observation: rain\nNew task: packs"), None);
    // Under another role it is not the part the request sent.
    let assistant = [
        block("assistant", "Observation: rain"),
        block("assistant", "New task: pack"),
    ];
    let other: Vec<&Block> = assistant.iter().collect();
    assert_eq!(
        merged_run(
            &part("user", "Observation: rain\nNew task: pack"),
            &other,
            &free,
            &free,
            0..2,
            &neighbours
        ),
        None
    );
}

/// The empty message that evidences a newline is the one beside the payload message a block shows. A
/// payload of `user(""), user("Hello"), assistant("Reply"), user("\nHello")` evidences the first block's
/// newline; the last block, which lost its own, cannot borrow it.
#[test]
fn an_empty_neighbour_evidences_only_the_message_beside_it() {
    let shown = [
        block("user", "Hello"),
        block("assistant", "Reply"),
        block("user", "Hello"),
    ];
    let blocks: Vec<&Block> = shown.iter().collect();
    let free = [false, false, false];
    let mut neighbours = EmptyNeighbours::default();
    for (index, role, text) in [
        (0, "user", ""),
        (1, "user", "Hello"),
        (2, "assistant", "Reply"),
        (3, "user", "\nHello"),
    ] {
        neighbours.record(
            &format!("llm.input_messages.{index}.message.role"),
            role.to_string(),
        );
        if !text.is_empty() {
            neighbours.record(
                &format!("llm.input_messages.{index}.message.content"),
                text.to_string(),
            );
        }
    }
    let run = |window| {
        merged_run(
            &part("user", "\nHello"),
            &blocks,
            &free,
            &free,
            window,
            &neighbours,
        )
    };
    assert_eq!(run(0..1), Some(0..=0));
    assert_eq!(run(2..3), None);
}

/// A merged part settles where it was shown, so the next part - merged or not - must follow it. Expected
/// `user("A\nB"), assistant("C\nD")` against blocks `assistant C, assistant D, user A, user B` is a reversal,
/// and the second merge is found only outside its place.
#[test]
fn a_recovered_merge_holds_the_next_one_to_the_request_order() {
    let shown = [
        block("assistant", "C"),
        block("assistant", "D"),
        block("user", "A"),
        block("user", "B"),
    ];
    let blocks: Vec<&Block> = shown.iter().collect();
    let assigned = [false; 4];
    let mut explained = [false; 4];
    let neighbours = EmptyNeighbours::default();
    let wanted = [part("user", "A\nB"), part("assistant", "C\nD")];
    let mut anchors: Vec<(usize, usize, usize)> = Vec::new();

    let first = merged_run(
        &wanted[0],
        &blocks,
        &assigned,
        &explained,
        merge_window(0, &anchors, blocks.len()),
        &neighbours,
    )
    .expect("the first merge is in place");
    assert_eq!(first, 2..=3);
    anchors.push((0, *first.start(), *first.end()));
    for j in first {
        explained[j] = true;
    }

    let window = merge_window(1, &anchors, blocks.len());
    assert_eq!(window, 4..4);
    assert_eq!(
        merged_run(
            &wanted[1],
            &blocks,
            &assigned,
            &explained,
            window,
            &neighbours
        ),
        None
    );
    // Found only by looking everywhere: the reconstruction shows it, out of order.
    assert_eq!(
        merged_run(
            &wanted[1],
            &blocks,
            &assigned,
            &explained,
            0..4,
            &neighbours
        ),
        Some(0..=1)
    );
}

/// The payload message a block is bound to has the block's role. The payload holds `user("\nA"), user("B"),
/// user(""), assistant("A")` and the request sent `user("\nA\nB\n")`: the user block `A` lost its newline,
/// and the assistant message reading `A`, with the empty user message before it, is no evidence about it.
#[test]
fn an_empty_neighbour_of_another_role_s_message_evidences_nothing() {
    let shown = [
        block("user", "A"),
        block("user", "B"),
        block("assistant", "A"),
    ];
    let blocks: Vec<&Block> = shown.iter().collect();
    let free = [false, false, false];
    let mut neighbours = EmptyNeighbours::default();
    for (index, role, text) in [
        (0, "user", "\nA"),
        (1, "user", "B"),
        (2, "user", ""),
        (3, "assistant", "A"),
    ] {
        neighbours.record(
            &format!("llm.input_messages.{index}.message.role"),
            role.to_string(),
        );
        if !text.is_empty() {
            neighbours.record(
                &format!("llm.input_messages.{index}.message.content"),
                text.to_string(),
            );
        }
    }
    assert_eq!(
        merged_run(
            &part("user", "\nA\nB\n"),
            &blocks,
            &free,
            &free,
            0..2,
            &neighbours
        ),
        None
    );
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
