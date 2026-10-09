/// The declared provider formats answer exactly as the four retired readers did, shape by shape.
///
/// Every form each reader recognised, its boundary variants, and the shapes where it **declined** - declining
/// is what leaves a block to the rest of the chain - compared at the provider position alone (so a decline is
/// visible as a decline, not masked by a later case) and through the whole chain. As rendered text, because a
/// nested result keeps member order. The corpus-wide comparison is
/// `the_declared_content_chain_matches_the_readers_it_replaced_over_the_corpus` in the server's goldens; this
/// one holds the shapes no fixture happens to carry.
#[test]
fn the_declared_provider_formats_match_the_readers_they_replace() {
    use crate::rules::schema::ChainPosition;

    let plan = &crate::rules::ruleset().content_blocks;
    let ctor = "TextBlock(type='text', text='Sunny', id='generated')";
    let cases: Vec<(&str, JsonValue)> = vec![
        // Prose under the three type names, and its second spelling.
        ("text", json!({"type": "text", "text": "hi"})),
        ("input text", json!({"type": "input_text", "text": "hi"})),
        (
            "output text",
            json!({"type": "output_text", "text": "hi", "annotations": []}),
        ),
        (
            "text under content",
            json!({"type": "text", "content": "hi"}),
        ),
        (
            "output text under content",
            json!({"type": "output_text", "content": "hi"}),
        ),
        (
            "text that is not a string",
            json!({"type": "text", "text": {"value": "hi"}}),
        ),
        (
            "text not a string beside content",
            json!({"type": "text", "text": 3, "content": "hi"}),
        ),
        (
            "content that is not a string",
            json!({"type": "text", "content": ["hi"]}),
        ),
        ("a text type with neither", json!({"type": "input_text"})),
        // Binary data by URI.
        (
            "data by url",
            json!({"type": "data", "uri": "https://x/a.png", "media_type": "image/png"}),
        ),
        (
            "data by reference",
            json!({"type": "data", "uri": "#!B64!#image/png::abc", "media_type": "image/png", "filename": "a.png"}),
        ),
        (
            "data with no media type",
            json!({"type": "data", "uri": "https://x/a"}),
        ),
        (
            "data with a non-string media type",
            json!({"type": "data", "uri": "https://x/a", "media_type": 7}),
        ),
        (
            "data with no uri",
            json!({"type": "data", "media_type": "image/png"}),
        ),
        (
            "data with a non-string filename",
            json!({"type": "data", "uri": "u://x", "filename": 1}),
        ),
        // Tool call and response parts.
        (
            "call with object arguments",
            json!({"type": "tool_call", "id": "c1", "name": "f", "arguments": {"a": 1}}),
        ),
        (
            "call with JSON text arguments",
            json!({"type": "tool_call", "id": "c1", "name": "f", "arguments": "{\"a\": 1}"}),
        ),
        (
            "call with scalar JSON arguments",
            json!({"type": "tool_call", "id": "c1", "name": "f", "arguments": "42"}),
        ),
        (
            "call with text arguments",
            json!({"type": "tool_call", "name": "f", "arguments": "not json"}),
        ),
        (
            "call with empty arguments",
            json!({"type": "tool_call", "name": "f", "arguments": {}}),
        ),
        (
            "call with no arguments",
            json!({"type": "tool_call", "name": "f"}),
        ),
        (
            "call with a non-string id",
            json!({"type": "tool_call", "id": 3, "name": "f"}),
        ),
        (
            "call with no name",
            json!({"type": "tool_call", "id": "c1", "arguments": {}}),
        ),
        (
            "response as JSON object text",
            json!({"type": "tool_call_response", "id": "c1", "response": "{\"ok\": true}"}),
        ),
        (
            "response as JSON array text",
            json!({"type": "tool_call_response", "id": "c1", "response": "[1, 2]"}),
        ),
        (
            "response as JSON string text",
            json!({"type": "tool_call_response", "id": "c1", "response": "\"done\""}),
        ),
        (
            "response as a JSON number's text",
            json!({"type": "tool_call_response", "id": "c1", "response": "4.20"}),
        ),
        (
            "response as plain text",
            json!({"type": "tool_call_response", "id": "c1", "response": "sunny"}),
        ),
        (
            "response as a constructor",
            json!({"type": "tool_call_response", "id": "c1", "response": ctor}),
        ),
        (
            "response as encoded constructors",
            json!({"type": "tool_call_response", "id": "c1", "response": serde_json::to_string(&[ctor]).unwrap()}),
        ),
        (
            "response as an object",
            json!({"type": "tool_call_response", "id": "c1", "response": {"ok": true}}),
        ),
        (
            "response as a number",
            json!({"type": "tool_call_response", "id": "c1", "response": 7}),
        ),
        (
            "response as null",
            json!({"type": "tool_call_response", "id": "c1", "response": null}),
        ),
        (
            "response absent",
            json!({"type": "tool_call_response", "id": "c1"}),
        ),
        // Images.
        (
            "image url object",
            json!({"type": "image_url", "image_url": {"url": "https://x/a.png", "detail": "low"}}),
        ),
        (
            "image data url",
            json!({"type": "image_url", "image_url": {"url": "data:image/png;base64,iVBORw0K"}}),
        ),
        (
            "image reference",
            json!({"type": "image_url", "image_url": {"url": "#!B64!#image/png::abc"}}),
        ),
        (
            "image as a bare url",
            json!({"type": "input_image", "image_url": "https://x/a.png"}),
        ),
        (
            "image url not a string",
            json!({"type": "image_url", "image_url": {"url": 1}}),
        ),
        ("image with no image_url", json!({"type": "image_url"})),
        (
            "image with a non-string detail",
            json!({"type": "image_url", "image_url": {"url": "https://x", "detail": 1}}),
        ),
        // Reasoning parts.
        (
            "reasoning text",
            json!({"type": "reasoning", "text": "hmm"}),
        ),
        (
            "reasoning content",
            json!({"type": "reasoning", "content": "hmm"}),
        ),
        (
            "reasoning content empty",
            json!({"type": "reasoning", "content": ""}),
        ),
        (
            "reasoning text not a string",
            json!({"type": "reasoning", "text": 1, "content": "hmm"}),
        ),
        ("reasoning with neither", json!({"type": "reasoning"})),
        // Blobs.
        (
            "blob data",
            json!({"type": "blob", "data": "iVBORw0K", "mime_type": "image/png"}),
        ),
        (
            "blob uri",
            json!({"type": "blob", "uri": "https://x/a.pdf", "media_type": "application/pdf"}),
        ),
        (
            "blob data url",
            json!({"type": "blob", "data": "data:image/png;base64,iVBOR"}),
        ),
        (
            "blob reference",
            json!({"type": "blob", "data": "#!B64!#image/png::abc", "media_type": "application/pdf"}),
        ),
        ("blob with no type", json!({"type": "blob", "data": "AAAA"})),
        (
            "blob with a non-string media type",
            json!({"type": "blob", "data": "AAAA", "media_type": 1, "mime_type": "image/png"}),
        ),
        (
            "blob with no payload",
            json!({"type": "blob", "mime_type": "image/png"}),
        ),
        // Audio.
        (
            "input audio",
            json!({"type": "input_audio", "input_audio": {"data": "UklGR", "format": "wav"}}),
        ),
        (
            "output audio",
            json!({"type": "audio", "audio": {"data": "UklGR", "format": "mp3"}}),
        ),
        (
            "audio reference",
            json!({"type": "audio", "audio": {"data": "#!B64!#audio/wav::abc"}}),
        ),
        (
            "audio with a non-string format",
            json!({"type": "audio", "audio": {"data": "UklGR", "format": 1}}),
        ),
        (
            "audio with no data",
            json!({"type": "input_audio", "input_audio": {"format": "wav"}}),
        ),
        (
            "audio in the other member",
            json!({"type": "audio", "input_audio": {"data": "UklGR"}}),
        ),
        // Files.
        (
            "file data",
            json!({"type": "file", "file": {"file_data": "data:application/pdf;base64,JVBER", "filename": "a.pdf"}}),
        ),
        (
            "input file data of no type",
            json!({"type": "input_file", "file_data": "JVBER"}),
        ),
        (
            "input file url",
            json!({"type": "input_file", "file_url": "https://x/a.pdf"}),
        ),
        (
            "input file id",
            json!({"type": "input_file", "file_id": "file-1"}),
        ),
        (
            "input file data not a string",
            json!({"type": "input_file", "file_data": 1, "file_id": "file-1"}),
        ),
        (
            "file by url",
            json!({"type": "file", "file": {"file_url": "https://x"}}),
        ),
        (
            "file by id",
            json!({"type": "file", "file": {"file_id": "file-1"}}),
        ),
        (
            "file without its object",
            json!({"type": "file", "file": "x", "mediaType": "image/png", "data": "iVBOR"}),
        ),
        ("file with nothing", json!({"type": "input_file"})),
        // Refusals and structured output.
        (
            "refusal under refusal",
            json!({"type": "refusal", "refusal": "no"}),
        ),
        (
            "refusal not a string",
            json!({"type": "refusal", "refusal": 1}),
        ),
        (
            "output json",
            json!({"type": "output_json", "json": {"a": 1}}),
        ),
        ("json object with nothing", json!({"type": "json_object"})),
        // Reasoning blocks.
        (
            "thinking chunks",
            json!({"type": "thinking", "thinking": [{"type": "text", "text": "a"}, {"type": "image"}, {"type": "text", "text": "b"}], "signature": "s"}),
        ),
        (
            "thinking chunk empty",
            json!({"type": "thinking", "thinking": [{"type": "text", "text": ""}]}),
        ),
        (
            "thinking chunks with no text",
            json!({"type": "thinking", "thinking": [{"type": "image"}], "text": "t"}),
        ),
        (
            "thinking string",
            json!({"type": "thinking", "thinking": "hmm", "signature": "s"}),
        ),
        (
            "thinking null beside content",
            json!({"type": "thinking", "thinking": null, "content": "x"}),
        ),
        (
            "thinking text beside a thinking number",
            json!({"type": "thinking", "thinking": 3, "text": "t"}),
        ),
        (
            "thinking text not a string beside content",
            json!({"type": "thinking", "text": 1, "content": "c"}),
        ),
        (
            "thinking with a signature only",
            json!({"type": "thinking", "signature": "s"}),
        ),
        (
            "thinking with a non-string signature",
            json!({"type": "thinking", "thinking": [], "signature": 1}),
        ),
        (
            "redacted thinking",
            json!({"type": "redacted_thinking", "data": "opaque"}),
        ),
        (
            "redacted thinking with nothing",
            json!({"type": "redacted_thinking"}),
        ),
        // Media behind a source object.
        (
            "image base64",
            json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "iVBOR"}}),
        ),
        (
            "image base64 reference",
            json!({"type": "image", "source": {"type": "base64", "data": "#!B64!#image/png::abc"}}),
        ),
        (
            "image url source",
            json!({"type": "image", "source": {"type": "url", "url": "https://x/a.png"}}),
        ),
        (
            "document base64",
            json!({"type": "document", "source": {"type": "base64", "media_type": "application/pdf", "data": "JVBER"}}),
        ),
        (
            "document url source",
            json!({"type": "document", "source": {"type": "url", "url": "https://x/a.pdf"}}),
        ),
        (
            "image of an unknown source type",
            json!({"type": "image", "source": {"type": "file", "file_id": "f"}}),
        ),
        (
            "image source without data",
            json!({"type": "image", "source": {"type": "base64"}}),
        ),
        ("image with no source", json!({"type": "image"})),
        // Calls and results that are already canonical, read as a tool's returned value.
        (
            "canonical call",
            json!({"type": "tool_use", "id": "c1", "name": "f", "input": {"a": 1}}),
        ),
        (
            "canonical call with text input",
            json!({"type": "tool_use", "id": "c1", "name": "f", "input": "{}"}),
        ),
        (
            "canonical result",
            json!({"type": "tool_result", "tool_use_id": "c1", "content": "ok", "is_error": true}),
        ),
        (
            "canonical result with nothing",
            json!({"type": "tool_result"}),
        ),
        (
            "canonical result with a non-bool flag",
            json!({"type": "tool_result", "content": [], "is_error": "yes"}),
        ),
        // The flat camelCase form of a Converse call and result.
        (
            "flat call",
            json!({"type": "toolUse", "toolUseId": "t1", "name": "f", "input": {"a": 1}}),
        ),
        (
            "flat call with no input",
            json!({"type": "toolUse", "toolUseId": "t1", "name": "f"}),
        ),
        (
            "flat result",
            json!({"type": "toolResult", "toolUseId": "t1", "content": [{"text": "ok"}], "status": "success"}),
        ),
        (
            "flat failed result",
            json!({"type": "toolResult", "toolUseId": "t1", "content": [{"text": "boom"}], "status": "error"}),
        ),
        (
            "flat result with a non-string status",
            json!({"type": "toolResult", "toolUseId": "t1", "status": true}),
        ),
        // Converse blocks.
        ("exactly text", json!({"text": "hi"})),
        (
            "text beside another member",
            json!({"text": "hi", "confidence": 0.9}),
        ),
        ("text that is not a string", json!({"text": 1})),
        ("exactly json", json!({"json": {"a": 1}})),
        ("exactly json holding null", json!({"json": null})),
        (
            "json beside another member",
            json!({"json": {"a": 1}, "source": "cache"}),
        ),
        (
            "reasoning text",
            json!({"reasoningContent": {"reasoningText": {"text": "hmm", "signature": "s"}}}),
        ),
        (
            "reasoning text with nothing",
            json!({"reasoningContent": {"reasoningText": {}}}),
        ),
        (
            "redacted reasoning",
            json!({"reasoningContent": {"redactedContent": {"data": "opaque"}}}),
        ),
        (
            "redacted reasoning with nothing",
            json!({"reasoningContent": {"redactedContent": {}}}),
        ),
        (
            "an unknown reasoning variant",
            json!({"reasoningContent": {"summary": "x"}}),
        ),
        (
            "reasoning that is not an object",
            json!({"reasoningContent": "x"}),
        ),
        (
            "an image",
            json!({"image": {"format": "png", "source": {"bytes": "iVBOR"}}}),
        ),
        (
            "an image reference",
            json!({"image": {"format": "png", "source": {"bytes": "#!B64!#image/png::abc"}}}),
        ),
        (
            "an image with no format",
            json!({"image": {"source": {"bytes": "iVBOR"}}}),
        ),
        (
            "an image with no bytes",
            json!({"image": {"format": "png", "source": {}}, "video": {"format": "mp4", "source": {"bytes": "AAAA"}}}),
        ),
        (
            "a document",
            json!({"document": {"format": "pdf", "name": "a.pdf", "source": {"bytes": "JVBER"}}}),
        ),
        (
            "a document with a non-string name",
            json!({"document": {"format": "pdf", "name": 1, "source": {"bytes": "JVBER"}}}),
        ),
        (
            "a video",
            json!({"video": {"format": "mp4", "source": {"bytes": "AAAA"}}}),
        ),
        (
            "an image beside a call",
            json!({"image": {"format": "png", "source": {"bytes": "iVBOR"}}, "toolUse": {"toolUseId": "t", "name": "f"}}),
        ),
        (
            "a call",
            json!({"toolUse": {"toolUseId": "t1", "name": "f", "input": {"a": 1}}}),
        ),
        (
            "a call with no input",
            json!({"toolUse": {"toolUseId": "t1", "name": "f"}}),
        ),
        (
            "a result",
            json!({"toolResult": {"toolUseId": "t1", "content": [{"text": "ok"}], "status": "success"}}),
        ),
        (
            "a result of structured data",
            json!({"toolResult": {"toolUseId": "t1", "content": [{"json": {"a": 1}}]}}),
        ),
        (
            "a result of two blocks",
            json!({"toolResult": {"toolUseId": "t1", "content": [{"text": "a"}, {"text": "b"}]}}),
        ),
        (
            "a failed result",
            json!({"toolResult": {"toolUseId": "t1", "content": [{"text": "boom"}], "status": "error"}}),
        ),
        (
            "a result with no content",
            json!({"toolResult": {"toolUseId": "t1"}}),
        ),
        (
            "a result beside a call",
            json!({"toolResult": {"toolUseId": "t1"}, "toolUse": {"toolUseId": "t1", "name": "f"}}),
        ),
        // Gemini parts.
        ("exactly thinking", json!({"thinking": "hmm"})),
        (
            "thinking beside another member",
            json!({"thinking": "hmm", "x": 1}),
        ),
        ("a thought", json!({"text": "hmm", "thought": true})),
        ("not a thought", json!({"text": "hmm", "thought": false})),
        (
            "a thought flag that is not a bool",
            json!({"text": "hmm", "thought": "true"}),
        ),
        ("a thought with no text", json!({"thought": true})),
        (
            "inline data",
            json!({"inline_data": {"mime_type": "image/png", "data": "iVBOR"}}),
        ),
        (
            "inline data reference",
            json!({"inline_data": {"mime_type": "image/png", "data": "#!B64!#image/png::abc"}}),
        ),
        (
            "inline data with no type",
            json!({"inline_data": {"data": "iVBOR"}}),
        ),
        (
            "inline data with no type beside file data",
            json!({"inline_data": {"data": "iVBOR"}, "file_data": {"mime_type": "image/png", "file_uri": "gs://x"}}),
        ),
        (
            "file data",
            json!({"file_data": {"mime_type": "application/pdf", "file_uri": "gs://x/a.pdf"}}),
        ),
        (
            "file data with no type",
            json!({"file_data": {"file_uri": "gs://x"}}),
        ),
        // Shapes nothing here reads.
        ("an unknown type", json!({"type": "custom", "value": 1})),
        ("plain data", json!({"temperature": 21})),
        (
            "a type that is not a string",
            json!({"type": 5, "text": "hi"}),
        ),
        ("an empty object", json!({})),
        ("a list", json!([{"text": "hi"}])),
        ("a number", json!(4)),
    ];

    // A file part's members are compared as a value: its member order is the one stated difference, below.
    let is_file_part = |block: &JsonValue| {
        matches!(
            block.get("type").and_then(JsonValue::as_str),
            Some("input_file" | "file")
        )
    };
    fn sorted(value: &JsonValue) -> JsonValue {
        match value {
            JsonValue::Object(members) => {
                let mut keys: Vec<&String> = members.keys().collect();
                keys.sort();
                JsonValue::Object(
                    keys.into_iter()
                        .map(|key| (key.clone(), sorted(&members[key])))
                        .collect(),
                )
            }
            JsonValue::Array(items) => JsonValue::Array(items.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    let render = |block: &JsonValue, value: Option<JsonValue>| {
        if is_file_part(block) {
            value.map(|value| sorted(&value).to_string())
        } else {
            value.map(|value| value.to_string())
        }
    };
    for (what, block) in &cases {
        assert_eq!(
            render(block, plan.normalize(block, ChainPosition::ProviderFormats)),
            render(block, legacy_provider_formats(block)),
            "the declared provider formats disagree with the readers they replace: {what}"
        );
        assert_eq!(
            render(block, normalize_content_block(block)),
            render(block, legacy_normalize_block(block, true)),
            "the whole chain disagrees with the retired one: {what}"
        );
    }

    // The stated differences, each on a shape no producer in the corpus writes. The retired readers built a
    // block a typed reader cannot hold - a call with no name, an id that is not a string - which survived only
    // as an unknown block wrapping the malformed build; the declared cases decline instead, so the producer's
    // own block survives in its place. A blank id is no id, as it already was for every declared case.
    let stated: Vec<(&str, JsonValue, Option<JsonValue>, Option<JsonValue>)> = vec![
        (
            "a flat call with no name",
            json!({"type": "toolUse", "toolUseId": "t1"}),
            Some(json!({"type": "tool_use", "id": "t1", "name": null, "input": {}})),
            None,
        ),
        (
            "a canonical call with no name",
            json!({"type": "tool_use", "id": "c1"}),
            Some(json!({"type": "tool_use", "id": "c1", "name": null, "input": {}})),
            None,
        ),
        (
            "a call whose id is a number",
            json!({"toolUse": {"toolUseId": 5, "name": "f"}}),
            Some(json!({"type": "tool_use", "id": 5, "name": "f", "input": {}})),
            Some(json!({"type": "tool_use", "id": null, "name": "f", "input": {}})),
        ),
        (
            "a result whose id is a number",
            json!({"toolResult": {"toolUseId": 5}}),
            Some(
                json!({"type": "tool_result", "tool_use_id": 5, "content": null, "is_error": false}),
            ),
            Some(
                json!({"type": "tool_result", "tool_use_id": null, "content": null, "is_error": false}),
            ),
        ),
        (
            "a call with a blank id",
            json!({"type": "tool_call", "id": "", "name": "f"}),
            Some(json!({"type": "tool_use", "id": "", "name": "f", "input": {}})),
            Some(json!({"type": "tool_use", "id": null, "name": "f", "input": {}})),
        ),
        // RFC 9535 filters an object's members as it does an array's elements.
        (
            "reasoning chunks keyed in an object rather than listed",
            json!({"type": "thinking", "thinking": {"a": {"type": "text", "text": "x"}}}),
            Some(json!({"type": "thinking", "text": "", "signature": null})),
            Some(json!({"type": "thinking", "text": "x", "signature": null})),
        ),
        // Representation-independence, which the encoded form already had: a response holding constructor
        // reprs is read as the blocks they describe whether or not the producer serialised the list first.
        (
            "a response holding constructors unencoded",
            json!({"type": "tool_call_response", "id": "c1", "response": [ctor]}),
            Some(
                json!({"type": "tool_result", "tool_use_id": "c1", "content": [{"type": "json", "data": [ctor]}], "is_error": false}),
            ),
            Some(
                json!({"type": "tool_result", "tool_use_id": "c1", "content": [{"type": "text", "text": "Sunny"}], "is_error": false}),
            ),
        ),
    ];
    for (what, block, retired, declared) in stated {
        assert_eq!(
            legacy_provider_formats(&block),
            retired,
            "the retired reader's answer, as stated: {what}"
        );
        assert_eq!(
            plan.normalize(&block, ChainPosition::ProviderFormats),
            declared,
            "the declared answer, as stated: {what}"
        );
    }

    // And member order, which only a stored nested result shows: a file part's media type sits where every
    // other media block puts it, not last.
    let file = json!({"type": "input_file", "file_data": "data:application/pdf;base64,JVBER"});
    let (declared, retired) = (
        plan.normalize(&file, ChainPosition::ProviderFormats),
        legacy_provider_formats(&file),
    );
    assert_eq!(declared, retired, "the same members");
    assert_eq!(
        declared.map(|block| block.to_string()),
        Some(
            r#"{"type":"document","media_type":"application/pdf","source":"base64","data":"JVBER"}"#
                .to_string()
        )
    );
}

/// A Responses API reasoning item: its reasoning text where the provider returns it, else its summary, and
/// signed where the encrypted reasoning a later request replays is there. One whose summary is empty and
/// whose reasoning is only encrypted is reasoning with no text, signed - not dropped, and not redacted.
#[test]
fn a_responses_reasoning_item_is_signed_reasoning_with_its_visible_text() {
    // As the view serves it: the normalised block, read as a SideML block and written back.
    let read = |item: JsonValue| {
        let block = normalize_content_block(&item).expect("a reasoning item is read");
        let block: crate::sideml::ContentBlock =
            serde_json::from_value(block).expect("a SideML block");
        serde_json::to_value(block).expect("serialisable")
    };
    let withheld = read(json!({
        "type": "reasoning", "id": "rs_1", "summary": [], "encrypted_content": "gAAAA-opaque"
    }));
    assert_eq!(withheld["type"], "thinking");
    assert_eq!(withheld["text"], "");
    assert_eq!(withheld["signed"], true);
    let summarised = read(json!({
        "type": "reasoning",
        "summary": [
            {"type": "summary_text", "text": "First."},
            {"type": "summary_text", "text": "Then."}
        ]
    }));
    assert_eq!(summarised["text"], "First.\n\nThen.");
    assert!(summarised.get("signed").is_none());
    let reasoned = read(json!({
        "type": "reasoning",
        "summary": [{"type": "summary_text", "text": "A summary."}],
        "content": [{"type": "reasoning_text", "text": "The reasoning itself."}],
        "encrypted_content": "gAAAA-opaque"
    }));
    assert_eq!(reasoned["text"], "The reasoning itself.");
    assert_eq!(reasoned["signed"], true);
}

#[test]
fn a_reasoning_part_streamed_in_chunks_reads_every_chunk_in_order() {
    // A reasoning part may carry its text as several strings; reading only the first would
    // drop the rest of the model's reasoning without any sign that something is missing.
    let block = normalize_content_block(&json!({
        "type": "ai.koog.prompt.message.MessagePart.Reasoning",
        "content": ["First, ", "then ", "finally."],
        "encrypted": "sig-1"
    }))
    .expect("a reasoning part is read");
    assert_eq!(block["type"], "thinking");
    assert_eq!(block["text"], "First, then finally.");
    assert_eq!(block["signature"], "sig-1");
}
