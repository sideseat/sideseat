use super::*;

/// The captured shape that motivated the predicate: agent-framework via Bedrock's
/// OpenAI-compatible endpoint emits a reasoning part with no text and no signature, which
/// reached the feed as a blank reasoning bubble.
#[test]
fn empty_reasoning_part_is_dropped() {
    let content = json!([{"type": "reasoning", "content": ""}]);
    assert_eq!(normalize_content(Some(&content)), json!([]));
}

#[test]
fn whitespace_only_reasoning_is_dropped_in_every_source_shape() {
    for shape in [
        json!([{"type": "reasoning", "content": "   "}]),
        json!([{"type": "reasoning", "text": "\n\t"}]),
        json!([{"type": "thinking", "text": ""}]),
        json!([{"thinking": ""}]),
    ] {
        let out = normalize_content(Some(&shape));
        let blocks = out.as_array().expect("array");
        assert!(
            !blocks
                .iter()
                .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("thinking")),
            "a blank thinking block survived for {shape}"
        );
    }
}

/// Signature-only reasoning is replay state a multi-turn request needs, so it survives with no
/// visible text - as hidden reasoning, not a blank thinking bubble.
#[test]
fn signed_thinking_with_withheld_text_is_hidden_reasoning() {
    let content = json!([
        {"type": "thinking", "text": "", "signature": "sig-1"},
        {"type": "text", "text": "Paris."}
    ]);
    let out = normalize_content(Some(&content));
    assert_eq!(
        out[0],
        json!({"type": "redacted_thinking", "data": "sig-1"})
    );
    assert_eq!(out[1]["text"], "Paris.");
}

#[test]
fn visible_reasoning_is_kept() {
    let content = json!([{"type": "reasoning", "content": "Let me check the units."}]);
    let out = normalize_content(Some(&content));
    assert_eq!(out[0]["type"], "thinking");
    assert_eq!(out[0]["text"], "Let me check the units.");
}

#[test]
fn redacted_thinking_is_kept_when_it_carries_data() {
    assert!(is_renderable_block(
        &json!({"type": "redacted_thinking", "data": "opaque"})
    ));
    assert!(!is_renderable_block(
        &json!({"type": "redacted_thinking", "data": ""})
    ));
}

/// An empty tool result still records that the tool ran. A blanket "has text" rule would
/// have removed it.
#[test]
fn empty_tool_result_is_kept() {
    assert!(is_renderable_block(
        &json!({"type": "tool_result", "tool_use_id": "1", "content": []})
    ));
}

/// An empty assistant text block is a legitimate response (e.g. a tool-only turn).
#[test]
fn empty_text_block_is_kept() {
    assert!(is_renderable_block(&json!({"type": "text", "text": ""})));
}

/// One decoder for what a media value **is**, where there were two with opposite mistakes.
///
/// `data_source_kind` answered `base64` for anything that was not a file reference, so an ordinary
/// `https://` URL was labelled as inline bytes. `parse_data_url` answered `url` for anything that was not a
/// file reference or a data URL, so raw base64 was labelled a URL. Each was wrong about exactly what the
/// other got right.
#[test]
fn a_media_value_is_decoded_the_same_way_by_every_caller() {
    // A stored reference, which also carries the media type the bytes were stored under.
    assert_eq!(
        decode_media_source("#!B64!#application/pdf::abc123"),
        ("file", Some("application/pdf"))
    );
    // A data URL: the payload is base64 and the prefix states its type.
    assert_eq!(
        decode_media_source("data:image/png;base64,AAAA"),
        ("base64", Some("image/png"))
    );
    // A URL is a fetch, not bytes - this is what was called `base64`.
    assert_eq!(
        decode_media_source("https://example.com/a.png"),
        ("url", None)
    );
    assert_eq!(decode_media_source("s3://bucket/key"), ("url", None));
    // And a bare value is the bytes themselves - this is what was called `url`.
    assert_eq!(decode_media_source("iVBORw0KGgoAAAANSU"), ("base64", None));
    // A scheme-looking prefix that is not one: `://` is the test, so a base64 payload containing a colon is
    // still bytes.
    assert_eq!(decode_media_source("abc:def"), ("base64", None));

    // `parse_data_url` agrees, and returns the data URL's *payload* rather than the whole URL.
    assert_eq!(
        parse_data_url("data:image/png;base64,AAAA"),
        ("base64", "AAAA".to_string(), Some("image/png".to_string()))
    );
    assert_eq!(
        parse_data_url("https://example.com/a.png"),
        ("url", "https://example.com/a.png".to_string(), None)
    );
    assert_eq!(
        parse_data_url("iVBORw0KGgoAAAANSU"),
        ("base64", "iVBORw0KGgoAAAANSU".to_string(), None)
    );
}

/// Two positions collapse only when one source **envelopes** the other.
///
/// The test used to be "the sources differ", and that does not hold: two *genuine* occurrences can be written
/// differently. `[{"text":"retry"}, {"type":"text","text":"retry"}]` contains two positions a
/// producer wrote, where the second was discarded because its JSON did not match the first's. Different
/// encodings are not evidence of one datum.
///
/// What is evidence is containment: `{type:"json", value: X}` holds `X`, which is the relation between a
/// wrapping and the thing wrapped. Two independently written encodings of one value do not contain each
/// other, so the cases separate with no declaration - and a producer inventing a new wrapper is covered by
/// the same relation.
#[test]
fn two_positions_collapse_only_when_one_envelopes_the_other() {
    let parts = |content: serde_json::Value| {
        normalize_tool_result_content(Some(content))
            .as_array()
            .map_or(0, Vec::len)
    };

    // Two encodings in two positions are both retained.
    assert_eq!(
        parts(json!([{"text": "retry"}, {"type": "text", "text": "retry"}])),
        2,
        "two differently-written positions are two occurrences - a producer wrote both"
    );
    // A genuine repeat, identically written: also two, which was already right.
    assert_eq!(
        parts(json!([{"text": "retry"}, {"text": "retry"}])),
        2,
        "and an identical repeat is still two"
    );

    // The envelope relation, in both orders - the wrapping may come first or second.
    let datum = json!({"status": "success", "rows": [1, 2]});
    assert_eq!(
        parts(json!([datum, {"type": "json", "value": datum}])),
        1,
        "a value beside its own wrapping is one datum written twice, which is what this exists for"
    );
    assert_eq!(
        parts(json!([{"type": "json", "value": datum}, datum])),
        1,
        "and the order does not matter"
    );

    // Two *different* data, each wrapped, stay two: containment is about one pair, not about the shape.
    assert_eq!(
        parts(json!([
            {"type": "json", "value": {"a": 1}},
            {"type": "json", "value": {"b": 2}}
        ])),
        2
    );
}
