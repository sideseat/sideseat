// A call's arguments, decoded as each representation declares them (`rules::arguments`).

/// The input of a message's only call.
fn call_input(message: JsonValue) -> JsonValue {
    let output = normalize(&message);
    let calls: Vec<JsonValue> = output
        .content
        .iter()
        .map(block_to_json)
        .filter(|block| block["type"] == "tool_use")
        .collect();
    assert_eq!(calls.len(), 1, "one call: {calls:?}");
    calls[0]["input"].clone()
}

/// The adversarial case: a tool whose real argument is the string `"{\"x\":1}"` keeps it on every path. A reader
/// that guessed decoded the string again and reported an object the tool never received.
#[test]
fn a_string_argument_that_spells_an_object_stays_a_string_on_every_path() {
    let literal = json!("{\"x\":1}");
    // A chat-completions call list, whose `arguments` member is JSON text: the text of a JSON string.
    let listed = json!({"role": "assistant", "tool_calls": [{"id": "c1", "type": "function",
        "function": {"name": "f", "arguments": literal.to_string()}}]});
    assert_eq!(call_input(listed), literal, "a listed call's JSON text");
    // LangChain's `args`, which holds the arguments themselves.
    let args = json!({"role": "assistant", "tool_calls": [{"id": "c1", "name": "f", "args": literal.clone()}]});
    assert_eq!(call_input(args), literal, "a listed call's own arguments");
    // A call list the engine built, whose arguments are written as JSON text once.
    let built = json!({"role": "assistant", "tool_calls": [{"id": "c1", "type": "function",
        "function": {"name": "f",
            "arguments": crate::rules::arguments::as_json_text_once(literal.clone())}}]});
    assert_eq!(call_input(built), literal, "an engine-built call");
    // SideSeat's own call message and its canonical block.
    let call_message =
        json!({"role": "tool_call", "name": "f", "tool_call_id": "c1", "content": literal.clone()});
    assert_eq!(call_input(call_message), literal, "a tool_call message");
    let block = json!({"role": "assistant", "content": [{"type": "tool_use", "id": "c1", "name": "f",
        "input": literal.clone()}]});
    assert_eq!(call_input(block), literal, "a canonical tool_use block");
}

/// One call stated four ways - a listed call's JSON text, LangChain's `args`, SideSeat's call message and its
/// canonical block - has one value.
#[test]
fn one_call_stated_four_ways_has_one_value() {
    let object = json!({"city": "Rome", "days": 1});
    let ways = [
        json!({"role": "assistant", "tool_calls": [{"id": "c1", "type": "function",
            "function": {"name": "f", "arguments": object.to_string()}}]}),
        json!({"role": "assistant", "tool_calls": [{"id": "c1", "name": "f", "args": object.clone()}]}),
        json!({"role": "tool_call", "name": "f", "tool_call_id": "c1", "content": object.clone()}),
        json!({"role": "assistant", "content": [{"type": "tool_use", "id": "c1", "name": "f",
            "input": object.clone()}]}),
    ];
    for way in ways {
        assert_eq!(call_input(way.clone()), object, "{way}");
    }
}

/// A representation that encodes its calls' arguments twice declares it on its own envelope: OpenInference's
/// flattened `tool_call` wrapper, which Agno 0.1.34 writes as the JSON text of a string holding the object.
#[test]
fn a_call_wrapper_declares_its_second_encoding() {
    let object = json!({"city": "Paris", "days": 2});
    let twice = json!(object.to_string()).to_string();
    let wrapped = json!({"role": "assistant", "tool_calls": [{"tool_call": {"id": "c1",
        "function": {"name": "f", "arguments": twice.clone()}}}]});
    assert_eq!(
        call_input(wrapped),
        object,
        "the wrapper's declared second decode"
    );
    // The same text in a plain call list is decoded once, as its member declares.
    let listed = json!({"role": "assistant", "tool_calls": [{"id": "c1", "type": "function",
        "function": {"name": "f", "arguments": twice}}]});
    assert_eq!(call_input(listed), json!(object.to_string()));
}

/// A streamed call is decoded after its input is selected, so an encoded empty object is the call with no
/// parameters, and a provider-run call keeps the ownership the block states.
#[test]
fn a_streamed_call_decodes_after_selection_and_keeps_its_ownership() {
    let empty = json!({"role": "assistant", "content": [{"type": "tool_use", "id": "c1", "name": "f",
        "input": "{}", "index": 0}]});
    assert_eq!(call_input(empty), json!({}));
    let run = json!({"role": "assistant", "content": [{"type": "tool_use", "id": "c1", "name": "f",
        "input": "{\"q\":\"x\"}", "index": 0, "provider_executed": true}]});
    let output = normalize(&run);
    let block = block_to_json(&output.content[0]);
    assert_eq!(block["input"], json!({"q": "x"}));
    assert_eq!(block["provider_executed"], json!(true), "{block}");
}

/// Only a member the vocabulary marks as a stream position routes a call to a declared decode: any other member
/// beside a canonical call's string input leaves the string argument as written.
#[test]
fn an_unrelated_member_does_not_decode_a_string_argument() {
    let literal = json!("{\"x\":1}");
    let block = json!({"role": "assistant", "content": [{"type": "tool_use", "id": "c1", "name": "f",
        "input": literal.clone(), "metadata": {}}]});
    assert_eq!(call_input(block), literal);
}

/// A lone call a tool returned holds its arguments themselves, and a streamed call nested in a result decodes as
/// it does in a message.
#[test]
fn a_returned_call_keeps_its_string_and_a_nested_streamed_call_decodes() {
    let plan = &crate::rules::ruleset().content_blocks;
    let literal = json!("{\"x\":1}");
    let returned = crate::sideml::content::normalize_tool_result_content_in(
        plan,
        Some(json!({"type": "tool_use", "id": "c1", "name": "f", "input": literal.clone()})),
    );
    assert_eq!(returned["input"], literal, "{returned}");
    let nested = json!({"role": "assistant", "content": [{"type": "tool_result", "tool_use_id": "c0",
        "content": [{"type": "tool_use", "id": "c1", "name": "f", "input": "{\"x\":1}", "index": 0}]}]});
    let output = normalize(&nested);
    let result = block_to_json(&output.content[0]);
    assert_eq!(result["content"][0]["input"], json!({"x": 1}), "{result}");
}

/// Inside a result, a streamed call is one call whichever way its input was written, and returned data carrying an
/// `index` is not a block.
#[test]
fn nested_stream_pieces_read_alike_and_indexed_data_stays_data() {
    let nested = |input: JsonValue| {
        let output = normalize(
            &json!({"role": "assistant", "content": [{"type": "tool_result",
            "tool_use_id": "c0", "content": [{"type": "tool_use", "id": "c1", "name": "f", "input": input,
            "index": 0}]}]}),
        );
        block_to_json(&output.content[0])["content"].clone()
    };
    assert_eq!(nested(json!("{\"x\":1}")), nested(json!({"x": 1})));
    let row = json!({"type": "row", "index": 0, "x": 1});
    let output = normalize(
        &json!({"role": "assistant", "content": [{"type": "tool_result",
        "tool_use_id": "c0", "content": row.clone()}]}),
    );
    assert_eq!(block_to_json(&output.content[0])["content"], row);
}
