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
