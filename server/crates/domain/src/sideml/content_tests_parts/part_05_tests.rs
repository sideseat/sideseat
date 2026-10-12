// The canonical form of a tool result's content: what the tool returned. One part that encodes nothing beyond
// its value is that value; everything else stays the list of blocks it is.

/// A tool result read through the conventions' response part, whose content is decoded from `response`.
fn response_result(response: JsonValue) -> JsonValue {
    normalize_content_block(
        &json!({"type": "tool_call_response", "id": "c1", "response": response}),
    )
    .expect("the result reads")["content"]
        .clone()
}

/// One structured value is that value - where one release wrote the bare object and the next the typed part,
/// the two now read as one result.
#[test]
fn a_single_structured_value_is_the_value() {
    assert_eq!(
        response_result(json!({"city": "Paris", "high_c": 21})),
        json!({"city": "Paris", "high_c": 21})
    );
    // The same value already typed as a part by the producer.
    let typed = normalize_tool_result_content_in(
        &crate::rules::ruleset().content_blocks,
        Some(json!([{"type": "json", "data": {"city": "Paris"}}])),
    );
    assert_eq!(typed, json!({"city": "Paris"}));
}

/// A value spelled like a block keeps its wrapper: unwrapped, a tool returning `{"type": "text", "text": ...}`
/// would be re-read downstream as a text block it never was.
#[test]
fn a_block_shaped_value_keeps_its_wrapper() {
    let shaped = json!({"type": "text", "text": "a value the tool returned"});
    let read = response_result(shaped.clone());
    assert_eq!(read, json!([{"type": "json", "data": shaped}]), "{read}");
    assert!(
        !crate::sideml::content::is_bare_structured_value(
            &crate::rules::ruleset().content_blocks,
            &shaped
        ),
        "a string `type` is block-shaped"
    );
    // So is an object carrying a member the vocabulary reads as a content block.
    assert!(!crate::sideml::content::is_bare_structured_value(
        &crate::rules::ruleset().content_blocks,
        &json!({"thinking": "x"})
    ));
}

/// Several parts, or one typed part that is not json or plain text, stay the list they are: the list is then
/// what the tool returned.
#[test]
fn several_parts_and_typed_parts_stay_a_list() {
    let plan = &crate::rules::ruleset().content_blocks;
    let several = normalize_tool_result_content_in(
        plan,
        Some(json!([{"type": "text", "text": "a"}, {"type": "text", "text": "b"}])),
    );
    assert_eq!(several.as_array().map(Vec::len), Some(2), "{several}");
    let image = normalize_tool_result_content_in(
        plan,
        Some(
            json!([{"type": "image", "source": "url", "data": "https://example.com/a.png",
            "media_type": "image/png"}]),
        ),
    );
    assert!(
        image.is_array(),
        "a typed non-json part stays a block: {image}"
    );
    // A tool returning a list of one value returned that list, not its element.
    let listed = normalize_tool_result_content_in(plan, Some(json!([{"city": "Paris"}])));
    assert!(
        listed.is_array(),
        "the tool's own one-element list stays a list: {listed}"
    );
}

/// One text the tool returned is that string; a text part carrying anything beyond its text - citations - keeps
/// the part, since a string cannot say it.
#[test]
fn a_single_text_part_is_its_string() {
    assert_eq!(response_result(json!("Sunny")), json!("Sunny"));
    let plan = &crate::rules::ruleset().content_blocks;
    assert_eq!(
        normalize_tool_result_content_in(plan, Some(json!([{"type": "text", "text": "Sunny"}]))),
        json!("Sunny")
    );
    assert_eq!(
        crate::sideml::content::single_part_value(
            plan,
            &json!({"type": "text", "text": "Sunny", "citations": [{"kind": "url", "source": "s"}]})
        ),
        None
    );
}

/// The oracles' allowance for the canonical form is source-aware: a collapse is allowed only where the source
/// does not hold the value as the one element of its own list. A negative control - the tool-owned list
/// collapsed anyway - must be rejected, or the allowance would excuse exactly the error the guard prevents.
#[test]
fn the_canonical_allowance_rejects_a_collapsed_tool_list() {
    let value = json!({"city": "Paris"});
    let typed = json!({"type": "tool_call_response", "response": [{"type": "json", "data": {"city": "Paris"}}]});
    let tool_owned = json!({"type": "tool-result", "result": [{"city": "Paris"}]});
    assert!(crate::sideml::test_support::collapse_allowed_by_source(
        &typed, &value
    ));
    assert!(!crate::sideml::test_support::collapse_allowed_by_source(
        &tool_owned,
        &value
    ));
    // And the tool-owned list really stays a list on every path.
    assert!(
        normalize_tool_result_content_in(
            &crate::rules::ruleset().content_blocks,
            Some(json!([{"city": "Paris"}]))
        )
        .is_array()
    );
}

/// Every path that builds a tool result's content holds the same canonical form and the same guards.
#[test]
fn every_result_path_holds_the_canonical_form() {
    let plan = &crate::rules::ruleset().content_blocks;
    // A json part with any member beside its data keeps the part: the bare value would drop it.
    let extra = normalize_tool_result_content_in(
        plan,
        Some(json!([{"type": "json", "data": {"ok": true}, "extra": 1}])),
    );
    assert!(extra.is_array(), "{extra}");

    // The value mode and the tool-role conversion share the guards: a text with citations keeps its part, a
    // json primitive keeps its part, and the tool's own one-element list stays a list.
    let cited =
        json!({"type": "text", "text": "ok", "citations": [{"kind": "url", "source": "s"}]});
    assert!(crate::sideml::content::canonical_inner_content(plan, &[cited], None).is_array());
    let number = json!({"type": "json", "data": 7});
    assert!(crate::sideml::content::canonical_inner_content(plan, &[number], None).is_array());
    let listed = json!({"type": "json", "data": {"city": "Paris"}});
    assert!(
        crate::sideml::content::canonical_inner_content(
            plan,
            std::slice::from_ref(&listed),
            Some(&json!([{"city": "Paris"}]))
        )
        .eq(&json!([{"city": "Paris"}])),
        "the tool's own list, spelled with its bare element"
    );
    assert_eq!(
        crate::sideml::content::canonical_inner_content(plan, &[listed], None),
        json!({"city": "Paris"}),
        "a producer-typed part"
    );

    // A canonical tool result passed through holds the rule too, so it reads as the same result written any
    // other way.
    let passed = normalize_content_block(&json!({"type": "tool_result", "tool_use_id": "c1",
        "content": [{"type": "json", "data": {"city": "Paris"}}]}))
    .expect("the result reads");
    assert_eq!(passed["content"], json!({"city": "Paris"}), "{passed}");
}

/// A tool-role message's `content` is its list of parts, so a bare element is an untyped part and its one value
/// is the result - as a result block's own `result` list, which holds the return value itself, is not.
#[test]
fn a_tool_message_s_parts_are_not_the_tool_s_list() {
    let raw = json!([{"city": "Paris"}]);
    let normalized = normalize_content(Some(&raw));
    let converted = convert_to_tool_result(&normalized, "tool", &Some("c1".into()));
    assert_eq!(
        converted[0]["content"],
        json!({"city": "Paris"}),
        "{converted}"
    );
    let block = normalize_content_block(
        &json!({"type": "tool_result", "tool_use_id": "c1", "content": raw}),
    )
    .expect("the result reads");
    assert_eq!(block["content"], raw, "{block}");
}

/// The canonical form is a fixed point: a stored result read again is the same result. The tool's own list is
/// spelled `[value]`, because `[{"type": "json", "data": value}]` is a producer's typed part and re-reads as the
/// value - which turned the list into its element on a second normalisation.
#[test]
fn the_canonical_form_reads_back_as_itself() {
    let plan = &crate::rules::ruleset().content_blocks;
    for content in [
        json!([{"city": "Paris"}]),
        json!(["Sunny"]),
        json!({"city": "Paris"}),
        json!("Sunny"),
        json!([{"type": "json", "data": {"type": "text", "text": "a value"}}]),
        json!([{"type": "json", "data": {"mime_type": "image/png", "data": "AAAA"}}]),
        json!([{"type": "text", "text": "a"}, {"type": "text", "text": "b"}]),
    ] {
        assert_eq!(
            normalize_tool_result_content_in(plan, Some(content.clone())),
            content,
            "the Normalized reading"
        );
        let block = json!({"type": "tool_result", "tool_use_id": "c1", "content": content.clone()});
        let once = normalize_content_block(&block).expect("the result reads");
        assert_eq!(
            once["content"], content,
            "the canonical passthrough: {once}"
        );
        let twice = normalize_content_block(&once).expect("the result reads again");
        assert_eq!(twice["content"], content, "read twice: {twice}");
    }
}

/// One datum a producer wrote twice, bare and wrapped, is one typed value: it collapses as the wrapping alone
/// does, rather than leaving the one part deduplication kept.
#[test]
fn a_datum_written_twice_collapses_as_once() {
    let plan = &crate::rules::ruleset().content_blocks;
    let datum = json!({"city": "Paris"});
    assert_eq!(
        normalize_tool_result_content_in(
            plan,
            Some(json!([datum.clone(), {"type": "json", "value": datum.clone()}]))
        ),
        normalize_tool_result_content_in(
            plan,
            Some(json!([{"type": "json", "value": datum.clone()}]))
        ),
    );
    assert_eq!(
        normalize_tool_result_content_in(
            plan,
            Some(json!([{"type": "json", "value": datum.clone()}]))
        ),
        datum
    );
}

/// The bare-value guard is the chain's own reading: an untyped object a declared case reads as a block - a MIME
/// type beside data - keeps its json part, since unwrapped it would be re-read as that block.
#[test]
fn a_declared_untyped_block_shape_keeps_its_wrapper() {
    let plan = &crate::rules::ruleset().content_blocks;
    let media = json!({"mime_type": "image/png", "data": "AAAA"});
    assert!(!crate::sideml::content::is_bare_structured_value(
        plan, &media
    ));
    let wrapped = json!([{"type": "json", "data": media}]);
    assert_eq!(
        normalize_tool_result_content_in(plan, Some(wrapped.clone())),
        wrapped
    );
    assert!(crate::sideml::content::is_bare_structured_value(
        plan,
        &json!({"city": "Paris"})
    ));
}

/// Several parts keep the wrapper of a json part around a block-shaped value, as one part does: unwrapped, the
/// returned JSON would become the image it is spelled like.
#[test]
fn several_parts_keep_a_block_shaped_value_s_wrapper() {
    let plan = &crate::rules::ruleset().content_blocks;
    let image = json!({"type": "image", "source": "url", "data": "u"});
    let parts = [
        json!({"type": "json", "data": image.clone()}),
        json!({"type": "text", "text": "done"}),
    ];
    assert_eq!(
        crate::sideml::content::canonical_inner_content(plan, &parts, None),
        json!(parts)
    );
    let plain = [
        json!({"type": "json", "data": {"ok": true}}),
        json!({"type": "text", "text": "done"}),
    ];
    assert_eq!(
        crate::sideml::content::canonical_inner_content(plan, &plain, None),
        json!([{"ok": true}, {"type": "text", "text": "done"}])
    );
}

/// A json string beside other parts keeps its part on every path: bare in a list it would read as text.
#[test]
fn several_parts_keep_a_json_string_s_part() {
    let plan = &crate::rules::ruleset().content_blocks;
    let parts = [
        json!({"type": "json", "data": "ok"}),
        json!({"type": "text", "text": "done"}),
    ];
    assert_eq!(
        crate::sideml::content::canonical_inner_content(plan, &parts, None),
        json!(parts)
    );
    assert_eq!(
        normalize_tool_result_content_in(plan, Some(json!(parts))),
        json!(parts)
    );
}

/// Several parts keep a json part around a list of blocks: the test is plainness at every depth.
#[test]
fn several_parts_keep_a_json_list_of_blocks() {
    let plan = &crate::rules::ruleset().content_blocks;
    let image = json!({"type": "image", "source": "url", "data": "u"});
    let parts = [
        json!({"type": "json", "data": [image]}),
        json!({"type": "text", "text": "done"}),
    ];
    assert_eq!(
        crate::sideml::content::canonical_inner_content(plan, &parts, None),
        json!(parts)
    );
    assert_eq!(
        normalize_tool_result_content_in(plan, Some(json!(parts))),
        json!(parts)
    );
}
