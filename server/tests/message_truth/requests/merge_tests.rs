//! The merge excuse: a request part the client joined from consecutive messages, shown as those messages, and
//! the empty turns of the span's own payload that evidence a merge's newline.

use super::tests::{block, part};
use super::*;

/// A block read from `carrier`.
fn at(role: &str, text: &str, carrier: &str) -> Block {
    Block {
        carrier: carrier.to_string(),
        ..block(role, text)
    }
}

/// The carrier an indexed input message `index` is read from.
fn indexed(index: u64) -> String {
    format!("attr:llm.input_messages.{index}.message")
}

/// Indexed input attributes: each message's role and, where it has one, its text.
fn indexed_payload(messages: &[(&str, &str)]) -> EmptyNeighbours {
    let mut neighbours = EmptyNeighbours::default();
    for (index, (role, text)) in messages.iter().enumerate() {
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
    neighbours
}

/// A text attribute holding one JSON message list, as the span records it.
fn decoded_payload(attribute: &str, messages: Vec<Value>) -> EmptyNeighbours {
    let mut neighbours = EmptyNeighbours::default();
    neighbours.record(attribute, json!({ "messages": messages }).to_string());
    neighbours
}

fn run(
    item: &str,
    role: &'static str,
    shown: &[Block],
    neighbours: &EmptyNeighbours,
    window: std::ops::Range<usize>,
) -> Option<std::ops::RangeInclusive<usize>> {
    let blocks: Vec<&Block> = shown.iter().collect();
    let free = vec![false; blocks.len()];
    merged_run(&part(role, item), &blocks, &free, &free, window, neighbours)
}

#[test]
fn a_merged_part_is_shown_only_by_its_exact_join() {
    // The payload: an empty user turn, then the two the span shows.
    let neighbours = indexed_payload(&[
        ("user", ""),
        ("user", "Observation: rain"),
        ("user", "New task: pack"),
    ]);
    let shown = [
        at("user", "Observation: rain", &indexed(1)),
        at("user", "New task: pack", &indexed(2)),
    ];
    let merged = |text: &str| run(text, "user", &shown, &neighbours, 0..2);
    // Two consecutive messages joined by a newline, and an empty one the payload holds before the first.
    assert_eq!(merged("Observation: rain\nNew task: pack"), Some(0..=1));
    assert_eq!(merged("\nObservation: rain"), Some(0..=0));
    // A newline the payload gives no empty message for is a lost byte, not a merge.
    assert_eq!(merged("Observation: rain\n"), None);
    // Outside the window the request's neighbouring parts allow, a match keeps no order and is refused.
    assert_eq!(
        run(
            "Observation: rain\nNew task: pack",
            "user",
            &shown,
            &neighbours,
            1..2
        ),
        None
    );
    // A single block is no merge, and a near miss - another separator, other text - is none either.
    assert_eq!(merged("Observation: rain"), None);
    assert_eq!(merged("Observation: rain New task: pack"), None);
    assert_eq!(merged("Observation: rain\nNew task: packs"), None);
    // Under another role it is not the part the request sent.
    let assistant = [
        at("assistant", "Observation: rain", &indexed(1)),
        at("assistant", "New task: pack", &indexed(2)),
    ];
    assert_eq!(
        run(
            "Observation: rain\nNew task: pack",
            "user",
            &assistant,
            &neighbours,
            0..2
        ),
        None
    );
}

/// The empty message that evidences a newline is the one beside the payload message a block shows, which is known
/// only where the span shows the payload's input verbatim. A payload of `user(""), user("Hello"),
/// assistant("Reply"), user("\nHello")` shown as `Hello, Reply, \nHello` evidences the first block's newline; shown
/// as `Hello, Reply, Hello` - the last block lost its own - it evidences nothing, since no block's place is known.
#[test]
fn an_empty_neighbour_evidences_only_the_message_beside_it() {
    let neighbours = indexed_payload(&[
        ("user", ""),
        ("user", "Hello"),
        ("assistant", "Reply"),
        ("user", "\nHello"),
    ]);
    let verbatim = [
        at("user", "Hello", &indexed(1)),
        at("assistant", "Reply", &indexed(2)),
        at("user", "\nHello", &indexed(3)),
    ];
    assert_eq!(
        run("\nHello", "user", &verbatim, &neighbours, 0..1),
        Some(0..=0)
    );
    assert_eq!(run("\nHello", "user", &verbatim, &neighbours, 2..3), None);
    let damaged = [
        at("user", "Hello", &indexed(1)),
        at("assistant", "Reply", &indexed(2)),
        at("user", "Hello", &indexed(3)),
    ];
    assert_eq!(run("\nHello", "user", &damaged, &neighbours, 0..1), None);
    assert_eq!(run("\nHello", "user", &damaged, &neighbours, 2..3), None);
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
    let neighbours = indexed_payload(&[
        ("user", "\nA"),
        ("user", "B"),
        ("user", ""),
        ("assistant", "A"),
    ]);
    let shown = [
        at("user", "A", &indexed(0)),
        at("user", "B", &indexed(1)),
        at("assistant", "A", &indexed(3)),
    ];
    assert_eq!(run("\nA\nB\n", "user", &shown, &neighbours, 0..2), None);
}

/// A decoded list evidences a merge's newline exactly as the indexed attributes do.
#[test]
fn a_decoded_empty_turn_evidences_a_merge_where_the_span_shows_its_list_verbatim() {
    let task = json!({ "role": "user", "content": [{ "type": "text", "text": "New task" }] });
    let call =
        json!({ "role": "tool-call", "content": [{ "type": "text", "text": "Calling tools: x" }] });
    let shown = [
        at("user", "New task", "attr:input.value"),
        at("assistant", "Calling tools: x", "attr:input.value"),
    ];
    let merged = |messages: Vec<Value>| {
        run(
            "\nCalling tools: x",
            "assistant",
            &shown,
            &decoded_payload("input.value", messages),
            0..2,
        )
    };
    // The members as Python renderings, the empty turn's role an enumeration member.
    let rendered = vec![
        json!("Message(role='user', content=[{'type': 'text', 'text': 'New task'}])"),
        json!(
            "Message(role=<Role.ASSISTANT: 'assistant'>, content=[{'type': 'text', 'text': ''}], tool_calls=None)"
        ),
        json!("Message(role='tool-call', content=[{'type': 'text', 'text': 'Calling tools: x'}])"),
    ];
    assert_eq!(merged(rendered), Some(1..=1));
    assert_eq!(
        merged(vec![
            task.clone(),
            json!({ "role": "assistant" }),
            call.clone()
        ]),
        Some(1..=1),
        "a turn with no content at all is empty too"
    );
    // After the call: the control for an empty turn that follows the block.
    let after = [
        at("user", "New task", "attr:input.value"),
        at("assistant", "Calling tools: x", "attr:input.value"),
    ];
    assert_eq!(
        run(
            "Calling tools: x\n",
            "assistant",
            &after,
            &decoded_payload(
                "input.value",
                vec![
                    task.clone(),
                    json!({ "role": "assistant", "content": "Calling tools: x" }),
                    json!({ "role": "assistant", "content": "" }),
                ]
            ),
            0..2
        ),
        Some(1..=1)
    );
}

/// Where the span does not show an export verbatim, read from it, no block's place is known, so no merge is
/// excused however the payload repeats itself. Each payload's earlier block lost its newline, and a later message
/// holding the same text beside an empty turn would lend it one by text alone: alone, in like surroundings, after
/// media, after input exported outside the list, with the request listed twice, and from another attribute's list.
#[test]
fn a_decoded_empty_turn_is_never_lent_to_a_block_whose_place_is_unknown() {
    let user = |text: &str| json!({ "role": "user", "content": text });
    let reply = || json!({ "role": "assistant", "content": "Reply" });
    let from = |role: &str, text: &str| at(role, text, "attr:input.value");
    let image = |name: &str| Block {
        kind: "image".to_string(),
        content: json!({ "type": "image", "url": name }),
        ..from("user", name)
    };
    let shown_alone = [
        from("user", "Hello"),
        from("assistant", "Reply"),
        from("user", "\nHello"),
    ];
    let alone = decoded_payload(
        "input.value",
        vec![user("\nHello"), reply(), user(""), user("Hello")],
    );
    assert_eq!(
        run("\nHello", "user", &shown_alone, &alone, 0..1),
        None,
        "alone"
    );

    let shown_like = [
        from("assistant", "Reply"),
        from("user", "Hello"),
        from("assistant", "Reply"),
        from("user", "\nHello"),
    ];
    let like = decoded_payload(
        "input.value",
        vec![reply(), user("\nHello"), reply(), user(""), user("Hello")],
    );
    assert_eq!(
        run("\nHello", "user", &shown_like, &like, 1..2),
        None,
        "in like surroundings"
    );

    let shown_media = [
        image("a.png"),
        image("b.png"),
        from("user", "Hello"),
        from("assistant", "Reply"),
        from("user", "\nHello"),
    ];
    let media = decoded_payload(
        "input.value",
        vec![
            json!({ "role": "user", "content": [
                { "type": "image", "url": "a.png" },
                { "type": "image", "url": "b.png" },
            ] }),
            user("\nHello"),
            reply(),
            user(""),
            user("Hello"),
        ],
    );
    assert_eq!(
        run("\nHello", "user", &shown_media, &media, 2..3),
        None,
        "after media"
    );

    let shown_system = [
        at("system", "S1", "attr:system"),
        at("system", "S2", "attr:system"),
        from("user", "Hello"),
        from("assistant", "Reply"),
        from("user", "\nHello"),
    ];
    assert_eq!(
        run("\nHello", "user", &shown_system, &alone, 2..3),
        None,
        "after input exported outside the list"
    );

    let shown_twice = [
        from("user", "Hello"),
        from("assistant", "Reply"),
        from("user", "Hello"),
    ];
    let mut twice = decoded_payload(
        "input.value",
        vec![user(""), user("Hello"), reply(), user("\nHello")],
    );
    twice.record(
        "output.value",
        json!({ "messages": [user(""), user("Hello"), reply(), user("\nHello")] }).to_string(),
    );
    assert_eq!(
        run("\nHello", "user", &shown_twice, &twice, 2..3),
        None,
        "listed twice"
    );

    // The span's input is `input.value`, which lost the byte; another attribute's list reads `""`, `Hello` and
    // must not lend it its empty turn.
    let shown_other = [from("user", "Hello")];
    let mut other = decoded_payload("input.value", vec![user("\nHello")]);
    other.record(
        "diagnostics",
        json!({ "messages": [user(""), user("Hello")] }).to_string(),
    );
    assert_eq!(
        run("\nHello", "user", &shown_other, &other, 0..1),
        None,
        "another attribute's list"
    );
}

/// Each of the rule's conditions refuses on its own, in a payload every other condition accepts.
#[test]
fn each_condition_of_an_aligned_export_refuses_on_its_own() {
    let task = json!({ "role": "user", "content": "New task" });
    let empty = json!({ "role": "assistant", "content": "" });
    let call = json!({ "role": "tool-call", "content": "Calling tools: x" });
    let from = |role: &str, text: &str| at(role, text, "attr:input.value");
    let shown = [
        from("user", "New task"),
        from("assistant", "Calling tools: x"),
    ];
    let merged = |shown: &[Block], neighbours: &EmptyNeighbours, window| {
        run("\nCalling tools: x", "assistant", shown, neighbours, window)
    };
    let control = decoded_payload(
        "input.value",
        vec![task.clone(), empty.clone(), call.clone()],
    );
    assert_eq!(
        merged(&shown, &control, 1..2),
        Some(1..=1),
        "the control is accepted"
    );

    let longer = [
        from("user", "New task"),
        from("assistant", "Calling tools: x"),
        from("user", "a block the export does not hold"),
    ];
    assert_eq!(
        merged(&longer, &control, 1..2),
        None,
        "a block beyond the export's texts"
    );

    let elsewhere = [
        from("user", "New task"),
        at("assistant", "Calling tools: x", "attr:output.value"),
    ];
    assert_eq!(
        merged(&elsewhere, &control, 1..2),
        None,
        "a block read from another carrier"
    );

    let with_media = decoded_payload(
        "input.value",
        vec![
            json!({ "role": "user", "content": [
                { "type": "text", "text": "New task" },
                { "type": "image", "url": "a.png" },
            ] }),
            empty.clone(),
            call.clone(),
        ],
    );
    assert_eq!(
        merged(&shown, &with_media, 1..2),
        None,
        "a part the span does not show"
    );

    for (case, unread) in [
        ("a number", json!(42)),
        ("a null", Value::Null),
        ("a string that is no message", json!("not a message")),
    ] {
        let payload = decoded_payload(
            "input.value",
            vec![task.clone(), empty.clone(), call.clone(), unread],
        );
        assert_eq!(
            merged(&shown, &payload, 1..2),
            None,
            "a member that is {case}"
        );
    }

    let mut two_lists = EmptyNeighbours::default();
    two_lists.record(
        "input.value",
        json!({
            "messages": [task.clone(), empty.clone(), call.clone()],
            "history": [task.clone()],
        })
        .to_string(),
    );
    assert_eq!(
        merged(&shown, &two_lists, 1..2),
        None,
        "a second message list in the attribute"
    );

    let gapped = {
        let mut neighbours = EmptyNeighbours::default();
        for (index, role, text) in [
            (1, "user", "New task"),
            (2, "assistant", ""),
            (3, "tool-call", "Calling tools: x"),
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
        neighbours
    };
    let shown_gapped = [
        at("user", "New task", &indexed(1)),
        at("assistant", "Calling tools: x", &indexed(3)),
    ];
    assert_eq!(
        merged(&shown_gapped, &gapped, 1..2),
        None,
        "a member the payload never states"
    );

    let holder_role = decoded_payload(
        "input.value",
        vec![
            task.clone(),
            empty.clone(),
            json!({ "role": "user", "content": "Calling tools: x" }),
        ],
    );
    assert_eq!(
        merged(&shown, &holder_role, 1..2),
        None,
        "a block under another role than its message's"
    );

    let wrong_role = decoded_payload(
        "input.value",
        vec![
            task.clone(),
            json!({ "role": "user", "content": "" }),
            call.clone(),
        ],
    );
    assert_eq!(
        merged(&shown, &wrong_role, 1..2),
        None,
        "an empty turn of another role"
    );

    let shown_two = [
        from("user", "New task"),
        from("assistant", "Thought"),
        from("assistant", "Calling tools: x"),
    ];
    let leading = decoded_payload(
        "input.value",
        vec![
            task.clone(),
            empty.clone(),
            json!({ "role": "assistant", "content": [
                { "type": "text", "text": "Thought" },
                { "type": "text", "text": "Calling tools: x" },
            ] }),
        ],
    );
    assert_eq!(
        merged(&shown_two, &leading, 2..3),
        None,
        "a block that is not its message's first text"
    );

    let shown_trailing = [
        from("user", "New task"),
        from("assistant", "Calling tools: x"),
        from("assistant", "After"),
    ];
    let trailing = decoded_payload(
        "input.value",
        vec![
            task.clone(),
            json!({ "role": "assistant", "content": [
                { "type": "text", "text": "Calling tools: x" },
                { "type": "text", "text": "After" },
            ] }),
            empty.clone(),
        ],
    );
    assert_eq!(
        run(
            "Calling tools: x\n",
            "assistant",
            &shown_trailing,
            &trailing,
            1..2
        ),
        None,
        "a block that is not its message's last text"
    );

    let neighbour_text = decoded_payload(
        "input.value",
        vec![
            task.clone(),
            json!({ "role": "assistant", "content": "thinking" }),
            call.clone(),
        ],
    );
    assert_eq!(
        merged(&shown, &neighbour_text, 1..2),
        None,
        "a neighbour with text"
    );

    let neighbour_member = decoded_payload(
        "input.value",
        vec![
            task.clone(),
            json!({ "role": "assistant", "content": "", "tool_calls": [{ "id": "c1" }] }),
            call.clone(),
        ],
    );
    assert_eq!(
        merged(&shown, &neighbour_member, 1..2),
        None,
        "a neighbour with a member with a value"
    );
}

/// Three more ways a payload could pass for the empty turn it does not hold: a member of the empty turn carrying
/// constructor arguments, a second list encoded in a string beside the first, and two receipts of the span that
/// only together read as the list - none of them evidences a newline.
#[test]
fn a_payload_that_only_seems_to_hold_the_empty_turn_evidences_nothing() {
    let user = |text: &str| json!({ "role": "user", "content": text });
    let shown = [at("user", "Hello", "attr:input.value")];
    let merged = |neighbours: &EmptyNeighbours| run("\nHello", "user", &shown, neighbours, 0..1);
    assert_eq!(
        merged(&decoded_payload(
            "input.value",
            vec![user(""), user("Hello")]
        )),
        Some(0..=0),
        "the control is accepted"
    );

    let with_arguments = decoded_payload(
        "input.value",
        vec![
            json!({ "role": "user", "content": "", "__python_args": ["unaccounted content"] }),
            user("Hello"),
        ],
    );
    assert_eq!(
        merged(&with_arguments),
        None,
        "an empty turn's constructor arguments"
    );

    let mut encoded = EmptyNeighbours::default();
    encoded.record(
        "input.value",
        json!({
            "diagnostics": [user(""), user("Hello")],
            "messages": json!([user("\nHello")]).to_string(),
        })
        .to_string(),
    );
    assert_eq!(merged(&encoded), None, "a second list encoded in a string");

    let mut receipts = EmptyNeighbours::default();
    receipts.begin_receipt();
    receipts.record(
        "input.value",
        json!({ "messages": [user(""), user("\nHello")] }).to_string(),
    );
    receipts.begin_receipt();
    receipts.record(
        "input.value",
        json!({ "messages": [Value::Null, user("Hello")] }).to_string(),
    );
    assert_eq!(
        merged(&receipts),
        None,
        "two receipts that only together read as the list"
    );

    // A string nested deeper than the search reads may hide a second list, so the attribute evidences nothing.
    let mut nested = json!([user("\nHello")]).to_string();
    for _ in 0..10 {
        nested = Value::String(nested).to_string();
    }
    let mut deep = EmptyNeighbours::default();
    deep.record(
        "input.value",
        json!({ "messages": [user(""), user("Hello")], "elsewhere": nested }).to_string(),
    );
    assert_eq!(merged(&deep), None, "a rendering nested past the search");

    // JSON nested past what the JSON reader reads is no text: it may hold a second list.
    let too_deep = format!(
        "{}{}{}",
        "[".repeat(200),
        json!([user("\nHello")]),
        "]".repeat(200)
    );
    let mut unread = EmptyNeighbours::default();
    unread.record(
        "input.value",
        json!({ "messages": [user(""), user("Hello")], "elsewhere": too_deep }).to_string(),
    );
    assert_eq!(merged(&unread), None, "a rendering no reader decodes");

    // A block of another kind holding a `text` member is not the message's text.
    let thinking = [Block {
        kind: "thinking".to_string(),
        content: json!({ "type": "thinking", "text": "Hello" }),
        ..at("user", "Hello", "attr:input.value")
    }];
    assert_eq!(
        run(
            "\nHello",
            "user",
            &thinking,
            &decoded_payload("input.value", vec![user(""), user("Hello")]),
            0..1
        ),
        None,
        "a block of another kind"
    );

    // Tuples are containers too.
    let tuples = format!("{}{}{}", "(".repeat(200), "1", ",)".repeat(200));
    let mut unread_tuples = EmptyNeighbours::default();
    unread_tuples.record(
        "input.value",
        json!({ "messages": [user(""), user("Hello")], "elsewhere": tuples }).to_string(),
    );
    assert_eq!(merged(&unread_tuples), None, "tuples no reader decodes");

    // A part of another kind holding text, and an empty text part with another member, are not empty text.
    for (case, part) in [
        (
            "a part of another kind with text",
            json!({ "type": "image", "text": "" }),
        ),
        (
            "an empty text part with another member",
            json!({ "type": "text", "text": "", "url": "a.png" }),
        ),
    ] {
        let payload = decoded_payload(
            "input.value",
            vec![json!({ "role": "user", "content": [part] }), user("Hello")],
        );
        assert_eq!(merged(&payload), None, "{case}");
    }

    // An indexed message stating its text twice, as `content` and as `contents`, which disagree.
    let mut twice_stated = EmptyNeighbours::default();
    twice_stated.record("llm.input_messages.0.message.role", "user".to_string());
    twice_stated.record("llm.input_messages.1.message.role", "user".to_string());
    twice_stated.record(
        "llm.input_messages.1.message.contents.0.message_content.text",
        "\nHello".to_string(),
    );
    twice_stated.record("llm.input_messages.1.message.content", "Hello".to_string());
    let shown_indexed = [at("user", "Hello", &indexed(1))];
    assert_eq!(
        run("\nHello", "user", &shown_indexed, &twice_stated, 0..1),
        None,
        "a message stating its text twice"
    );
}
