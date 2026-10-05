// ========== MIME-bearing file URI tests ==========

#[test]
fn test_parse_data_url_file_uri_with_mime() {
    let (source, data, media_type) = parse_data_url("#!B64!#image/png::hash123");
    assert_eq!(source, "file");
    assert_eq!(data, "#!B64!#image/png::hash123");
    assert_eq!(media_type, Some("image/png".to_string()));
}

#[test]
fn test_parse_data_url_file_uri_without_mime() {
    let (source, data, media_type) = parse_data_url("#!B64!#::hash123");
    assert_eq!(source, "file");
    assert_eq!(data, "#!B64!#::hash123");
    assert_eq!(media_type, None);
}

#[test]
fn test_media_fallback_bare_file_ref_with_mime() {
    let block = json!({"data": "#!B64!#image/jpeg::ea5c033d"});
    let result = try_media_fallback(&block).unwrap();
    assert_eq!(result["type"], "image");
    assert_eq!(result["source"], "file");
    assert_eq!(result["media_type"], "image/jpeg");
}

#[test]
fn test_media_fallback_bare_file_ref_with_pdf_mime() {
    let block = json!({"data": "#!B64!#application/pdf::hash123"});
    let result = try_media_fallback(&block).unwrap();
    assert_eq!(result["type"], "document");
    assert_eq!(result["media_type"], "application/pdf");
}

#[test]
fn test_media_fallback_bare_file_ref_with_audio_mime() {
    let block = json!({"data": "#!B64!#audio/mpeg::hash123"});
    let result = try_media_fallback(&block).unwrap();
    assert_eq!(result["type"], "audio");
    assert_eq!(result["media_type"], "audio/mpeg");
}

#[test]
fn test_media_fallback_bare_file_ref_without_mime() {
    // Without MIME, should default to "file" type (no media_type field)
    let block = json!({"data": "#!B64!#::hash123"});
    let result = try_media_fallback(&block).unwrap();
    assert_eq!(result["type"], "file");
    assert_eq!(result["source"], "file");
    // No media_type field present when MIME is unknown
    assert!(
        result.get("media_type").is_none() || result["media_type"].is_null(),
        "media_type should be absent or null when no MIME"
    );
}

#[test]
fn test_normalize_content_mixed_array_with_mime() {
    // AutoGen MultiModalMessage with MIME-bearing URI
    let content = json!([
        "Describe the image contents in detail.",
        {"data": "#!B64!#image/jpeg::ea5c033d"}
    ]);
    let result = normalize_content(Some(&content));
    let blocks = result.as_array().unwrap();
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0]["type"], "text");
    assert_eq!(blocks[1]["type"], "image");
    assert_eq!(blocks[1]["media_type"], "image/jpeg");
    assert_eq!(blocks[1]["source"], "file");
}

#[test]
fn test_openai_image_url_with_mime_file_ref() {
    let block = json!({
        "type": "image_url",
        "image_url": {
            "url": "#!B64!#image/png::hash123"
        }
    });
    let result = try_openai_format(&block).unwrap();
    assert_eq!(result["type"], "image");
    assert_eq!(result["source"], "file");
    assert_eq!(result["media_type"], "image/png");
}

/// A PDF sent through Chat Completions read as an unknown block: its members sit under `file`.
#[test]
fn a_chat_completions_file_part_is_a_document() {
    let block = json!({
        "type": "file",
        "file": {"filename": "task.pdf", "file_data": "data:application/pdf;base64,JVBERi0x"}
    });
    let result = normalize_content_block(&block).unwrap();
    assert_eq!(result["type"], "document");
    assert_eq!(result["media_type"], "application/pdf");
    assert_eq!(result["source"], "base64");
    assert_eq!(result["data"], "JVBERi0x");
    assert_eq!(result["name"], "task.pdf");

    let stored = json!({"type": "file", "file": {"file_data": "#!B64!#application/pdf::hash123"}});
    let result = normalize_content_block(&stored).unwrap();
    assert_eq!(result["type"], "document");
    assert_eq!(result["source"], "file");

    let uploaded = json!({"type": "file", "file": {"file_id": "file-abc"}});
    assert_eq!(
        normalize_content_block(&uploaded).unwrap()["data"],
        "file-abc"
    );

    // A `file` block in another dialect's shape is not claimed here.
    let other = json!({"type": "file", "mediaType": "image/png", "data": "#!B64!#image/png::h"});
    assert!(try_openai_format(&other).is_none());
}

#[test]
fn test_bedrock_media_with_mime_file_ref() {
    let block = json!({
        "image": {
            "format": "jpeg",
            "source": {
                "bytes": "#!B64!#image/jpeg::hash123"
            }
        }
    });
    let result = try_bedrock_format(&block).unwrap();
    assert_eq!(result["type"], "image");
    assert_eq!(result["source"], "file");
}

#[test]
fn test_gemini_inline_data_with_mime_file_ref() {
    let block = json!({
        "inline_data": {
            "mime_type": "image/webp",
            "data": "#!B64!#image/webp::hash123"
        }
    });
    let result = try_gemini_format(&block).unwrap();
    assert_eq!(result["type"], "image");
    assert_eq!(result["source"], "file");
}

#[test]
fn test_vercel_file_with_mime_file_ref() {
    let block = json!({
        "type": "file",
        "mediaType": "image/png",
        "data": "#!B64!#image/png::hash123"
    });
    let result = try_vercel_format(&block).unwrap();
    assert_eq!(result["type"], "image");
    assert_eq!(result["source"], "file");
}

#[test]
fn test_anthropic_media_with_mime_file_ref() {
    let block = json!({
        "type": "image",
        "source": {
            "type": "base64",
            "media_type": "image/jpeg",
            "data": "#!B64!#image/jpeg::hash123"
        }
    });
    let result = try_anthropic_format(&block).unwrap();
    assert_eq!(result["type"], "image");
    assert_eq!(result["source"], "file");
}

#[test]
fn python_constructor_repr_preserves_data_and_exposes_volatile_metadata_to_rules() {
    let parsed = try_parse_python_constructor_repr(
        "ToolResponse(content=[TextBlock(type='text', text='Sunny, 22°C', \
         id='generated', created_at='2026-10-01T15:23:10', finished_at=None)], \
         state=<ToolResultState.SUCCESS: 'success'>, metadata={}, id='response-id')",
    )
    .expect("the supported constructor repr parses");

    assert_eq!(parsed["__python_constructor"], "ToolResponse");
    assert_eq!(parsed["content"][0]["__python_constructor"], "TextBlock");
    assert_eq!(parsed["content"][0]["text"], "Sunny, 22°C");
    assert_eq!(parsed["content"][0]["id"], "generated");
    assert_eq!(parsed["state"], "success");
}

#[test]
fn constructor_repr_inside_a_tool_response_becomes_semantic_content() {
    let repr = "TextBlock(type='text', text='Sunny, 22°C', id='generated', \
                created_at='2026-10-01T15:23:10', finished_at=None)";
    let block = json!({
        "type": "tool_call_response",
        "id": "call-1",
        "response": serde_json::to_string(&vec![repr]).expect("the response serialises"),
    });

    assert_eq!(
        normalize_content_block(&block),
        Some(json!({
            "type": "tool_result",
            "tool_use_id": "call-1",
            "content": [{"type": "text", "text": "Sunny, 22°C"}],
            "is_error": false,
        })),
        "generated ids and timestamps are not product content"
    );
}

#[test]
fn constructor_repr_keeps_agentscope_media_content_and_its_name() {
    let repr = "DataBlock(type='data', id='generated', \
                source=URLSource(type='url', url=AnyUrl('https://example.com/chart.png'), \
                media_type='image/png'), name='chart.png', \
                created_at='2026-10-01T15:23:10', finished_at=None)";
    let block = json!({
        "type": "tool_call_response",
        "id": "call-1",
        "response": serde_json::to_string(&vec![repr]).expect("the response serialises"),
    });

    assert_eq!(
        normalize_content_block(&block),
        Some(json!({
            "type": "tool_result",
            "tool_use_id": "call-1",
            "content": [{
                "type": "image",
                "media_type": "image/png",
                "source": "url",
                "data": "https://example.com/chart.png",
                "name": "chart.png",
            }],
            "is_error": false,
        }))
    );
}

#[test]
fn an_unknown_constructor_remains_text() {
    let block = json!({
        "type": "tool_call_response",
        "id": "call-1",
        "response": "BusinessResult(value='keep this wording')",
    });
    assert_eq!(
        normalize_content_block(&block).expect("the tool result normalises")["content"],
        json!([{"type": "text", "text": "BusinessResult(value='keep this wording')"}])
    );
}

#[test]
fn canonical_tool_results_preserve_scalar_and_structured_json() {
    for content in [
        json!(0.0),
        json!(7),
        json!(true),
        json!({"value": {"amount": 7}}),
    ] {
        let block = json!({
            "type": "tool_result",
            "tool_use_id": "call-1",
            "content": content,
        });
        let normalized = normalize_content_block(&block).expect("the canonical block survives");
        assert_eq!(
            normalized["content"], content,
            "canonical result data is not a provider payload to reinterpret"
        );
    }
}

/// The conventions' binary part carries its payload in `content` beside `mime_type`. It used to fall through
/// to an unknown block, so an image and a PDF a user sent rendered as raw JSON.
#[test]
fn a_semconv_blob_part_is_a_media_block_of_its_mime_type() {
    for (mime, kind) in [("image/jpeg", "image"), ("application/pdf", "document")] {
        let block =
            json!({"type": "blob", "mime_type": mime, "modality": "image", "content": "AAAA"});
        let normalized = normalize_content_block(&block).expect("the blob normalises");
        assert_eq!(normalized["type"], kind, "{mime}");
        assert_eq!(normalized["media_type"], mime);
        assert_eq!(normalized["data"], "AAAA");
    }
}

/// A tool result under `result` rather than `response` keeps its value and its tool name. Read only as
/// `response`, it became an empty result with no name - invisible wherever no tool span restated it.
#[test]
fn a_tool_call_response_under_result_keeps_its_value() {
    let block = json!({
        "type": "tool_call_response",
        "id": "call-1",
        "name": "final_result",
        "result": "Final result processed.",
    });
    let normalized = normalize_content_block(&block).expect("the tool result normalises");
    assert_eq!(normalized["type"], "tool_result");
    assert_eq!(normalized["tool_use_id"], "call-1");
    assert_eq!(normalized["name"], "final_result");
    assert_eq!(normalized["content"], "Final result processed.");
}

/// AgentScope writes the conventions' binary part with `media_type` where the conventions say `mime_type`.
/// Unread, its image and PDF rendered as unknown blocks.
#[test]
fn a_semconv_blob_part_under_media_type_is_a_media_block() {
    let block = json!({"type": "blob", "media_type": "application/pdf", "modality": "unknown", "content": "AAAA"});
    let normalized = normalize_content_block(&block).expect("the blob normalises");
    assert_eq!(normalized["type"], "document");
    assert_eq!(normalized["media_type"], "application/pdf");
}

/// Converse writes a tool's structured result as exactly `{"json": value}`. Read as plain data, the value came
/// back wrapped in the member that only labels it.
#[test]
fn a_converse_json_result_block_is_its_value() {
    let block = json!({"json": {"city": "Rome", "high_c": 21}});
    assert_eq!(
        normalize_content_block(&block),
        Some(json!({"type": "json", "data": {"city": "Rome", "high_c": 21}}))
    );
    let data = json!({"json": {"city": "Rome"}, "source": "cache"});
    assert_ne!(
        normalize_content_block(&data).map(|b| b["data"].clone()),
        Some(json!({"city": "Rome"})),
        "a value with members beside `json` is the producer's own data"
    );
}

#[test]
fn the_declared_gemini_parts_match_the_reader_they_replace() {
    use crate::rules::schema::ChainPosition;

    let plan = &crate::rules::ruleset().content_blocks;
    let cases: Vec<(&str, JsonValue)> = vec![
        (
            "a call",
            json!({"functionCall": {"name": "get_weather", "args": {"city": "Paris"}}}),
        ),
        (
            "a call under the snake-case member",
            json!({"function_call": {"name": "get_weather", "args": {"city": "Paris"}}}),
        ),
        (
            "a call with no arguments",
            json!({"functionCall": {"name": "now"}}),
        ),
        (
            "a call with empty arguments",
            json!({"function_call": {"name": "now", "args": {}}}),
        ),
        (
            "the same function with other arguments, which must not share the id",
            json!({"functionCall": {"name": "get_weather", "args": {"city": "Tokyo"}}}),
        ),
        (
            "a part holding both envelopes, read from the camelCase one",
            json!({"functionCall": {"name": "a", "args": {"x": 1}}, "function_call": {"name": "a", "args": {"x": 1}}}),
        ),
        (
            "a response",
            json!({"functionResponse": {"name": "get_weather", "response": {"city": "Paris"}}}),
        ),
        (
            "a response under the snake-case member",
            json!({"function_response": {"name": "get_weather", "response": {"result": "10%"}}}),
        ),
        (
            "a response with no content",
            json!({"functionResponse": {"name": "get_weather"}}),
        ),
        ("a text part, which neither reads", json!({"text": "hello"})),
    ];
    // The declared case states its result id as `null` where the retired reader left the member out; the
    // typed block reads both as no id.
    let declared_form = |mut value: JsonValue| {
        if let Some(object) = value.as_object_mut()
            && object.get("tool_use_id") == Some(&JsonValue::Null)
        {
            object.remove("tool_use_id");
        }
        value
    };
    for (what, block) in cases {
        let declared = plan
            .normalize(&block, ChainPosition::AfterProviderFormats)
            .map(declared_form);
        let retired = try_gemini_function_format(&block);
        assert_eq!(
            declared, retired,
            "the declared cases disagree with the reader they replace: {what}"
        );
    }

    // The one stated difference: a call with no name. The retired reader named it `unknown`, which no
    // function is; the declared case leaves the block to the rest of the chain.
    let nameless = json!({"functionCall": {"args": {"x": 1}}});
    assert_eq!(
        try_gemini_function_format(&nameless).map(|b| b["name"].clone()),
        Some(json!("unknown"))
    );
    assert_eq!(
        plan.normalize(&nameless, ChainPosition::AfterProviderFormats),
        None
    );
}

#[test]
fn an_id_template_is_refused_unless_it_can_build_distinct_ids() {
    let rule = |template: &str, id_after: bool| {
        let mut id = vec![json!({"template": template})];
        if id_after {
            id.push(json!("$.id"));
        }
        serde_json::from_value::<crate::rules::schema::RuleFile>(json!({
            "id": "t",
            "content_blocks": [{
                "id": "t.call", "at": "after_provider_formats", "legacy_rank": 1,
                "require": {"all": [{"path": "$.call", "exists": true}]},
                "tool_use": {"id": id, "name": ["$.call.name"]}
            }]
        }))
        .expect("the test asset parses")
    };
    let compile = |file| crate::rules::content_blocks::ContentBlockPlan::compile(&[file]);
    assert!(compile(rule("c_{name}_{stable_hash(input)}", false)).is_ok());
    for (template, id_after) in [
        ("constant", false),
        ("c_{nmae}", false),
        ("c_{name", false),
        ("c_}{name}", false),
        ("c_{name}", true),
    ] {
        assert!(
            compile(rule(template, id_after)).is_err(),
            "`{template}` (followed by a path: {id_after}) must be refused"
        );
    }
}

#[test]
fn a_python_literal_parses_only_as_a_whole_container_of_literals() {
    assert_eq!(
        try_parse_python_literal(
            "{'city': 'Paris', 'ok': True, 'n': None, 'days': [1, 2.5], 'pair': ('a', \"b's\")}"
        ),
        Some(
            json!({"city": "Paris", "ok": true, "n": null, "days": [1, 2.5], "pair": ["a", "b's"]})
        )
    );
    assert_eq!(
        try_parse_python_literal("  [1, 'x']  "),
        Some(json!([1, "x"]))
    );
    for refused in [
        "1",
        "'text'",
        "Paris",
        "{'a': Sunny}",
        "[TextContent(type='text', text='x')]",
        "{'state': <State.OK: 'ok'>}",
        "{'a': 1} trailing",
        "{'a': 1",
        "{'a': 1, 'a': 2}",
    ] {
        assert_eq!(try_parse_python_literal(refused), None, "{refused}");
    }
}

/// A tool result part whose `response` is a list of text parts is content; a list of values is a value.
#[test]
fn a_tool_call_response_list_is_content_only_when_every_member_is_a_text_part() {
    let result = |response: JsonValue| {
        normalize_content_block(
            &json!({"type": "tool_call_response", "id": "c1", "response": response}),
        )
        .expect("a tool result normalises")["content"]
            .clone()
    };
    assert_eq!(
        result(json!([{"text": "sunny"}, {"type": "text", "text": "warm"}])),
        json!([{"type": "text", "text": "sunny"}, {"type": "text", "text": "warm"}])
    );
    assert_eq!(
        result(json!([1, 2])),
        json!([{"type": "json", "data": [1, 2]}])
    );
    assert_eq!(
        result(json!([{"text": "sunny"}, {"temp": 72}])),
        json!([{"type": "json", "data": [{"text": "sunny"}, {"temp": 72}]}])
    );
}

/// A part holding a whole provider content list is spliced into the message, each member read as the block
/// it is and in order; a text part holding prose is untouched.
#[test]
fn a_declared_splice_puts_each_member_of_a_content_list_in_the_message() {
    let content = json!([
        {"type": "text", "content": [
            {"text": "Checking."},
            {"toolUse": {"toolUseId": "t1", "name": "get_weather", "input": {"city": "Rome"}}}
        ]},
        {"type": "text", "content": "Done."}
    ]);
    let blocks = normalize_content(Some(&content));
    let kinds: Vec<&str> = blocks
        .as_array()
        .expect("a block list")
        .iter()
        .filter_map(|block| block["type"].as_str())
        .collect();
    assert_eq!(kinds, ["text", "tool_use", "text"]);
    assert_eq!(blocks[1]["id"], "t1");
    assert_eq!(blocks[2]["text"], "Done.");
}
