use serde_json::json;

use super::try_parse_java_tostring as parse;

/// **A message object's `toString` reads as the tree it prints**: nested objects under their class names, a
/// list of them, a map, and every string exactly as written - including where the class leaves the comma
/// between two members out, as LangChain4j's `ImageContent` does.
#[test]
fn a_message_objects_tostring_reads_as_the_tree_it_prints() {
    let text = r#"UserMessage { name = null, contents = [TextContent { text = "Describe the image." }, ImageContent { image = Image { url = null, base64Data = "/9j/4AAQ", mimeType = "image/jpeg", revisedPrompt = null } detailLevel = LOW }, PdfFileContent { pdfFile = PdfFile { url = null, base64Data = "JVBERi0x", mimeType = "application/pdf" } }], attributes = {} }"#;
    assert_eq!(
        parse(text),
        Some(json!({
            "__java_class": "UserMessage",
            "name": null,
            "contents": [
                {"__java_class": "TextContent", "text": "Describe the image."},
                {"__java_class": "ImageContent",
                 "image": {"__java_class": "Image", "url": null, "base64Data": "/9j/4AAQ",
                           "mimeType": "image/jpeg", "revisedPrompt": null},
                 "detailLevel": "LOW"},
                {"__java_class": "PdfFileContent",
                 "pdfFile": {"__java_class": "PdfFile", "url": null, "base64Data": "JVBERi0x",
                             "mimeType": "application/pdf"}}
            ],
            "attributes": {}
        }))
    );
}

/// **Java does not escape a quote inside a string, so where one ends is found, not read.** The earliest quote
/// the surrounding structure can follow is taken, and a later one where the rest then fails to read: a tool's
/// JSON result, prose quoting a word, and prose that looks like the next member all keep their whole text.
#[test]
fn a_string_ends_at_the_quote_the_structure_can_follow() {
    let result = r#"ToolExecutionResultMessage { id = "call_1", toolName = "weather", text = "{"city": "Paris", "forecast": [{"day": 1, "sky": "sun"}], "note": {"a": "b"}}" }"#;
    let parsed = parse(result).expect("a JSON result inside a string");
    assert_eq!(
        parsed["text"],
        r#"{"city": "Paris", "forecast": [{"day": 1, "sky": "sun"}], "note": {"a": "b"}}"#
    );
    assert_eq!(parsed["id"], "call_1");

    let quoting = r#"TextContent { text = "She said "yes", then left." }"#;
    assert_eq!(
        parse(quoting).expect("prose quoting a word")["text"],
        r#"She said "yes", then left."#
    );

    // `"x", y = 1` looks like a second member until ` ok"` does not read: the next end is taken.
    let lookalike = r#"TextContent { text = "He wrote "x", y = 1 ok" }"#;
    assert_eq!(
        parse(lookalike),
        Some(json!({"__java_class": "TextContent", "text": r#"He wrote "x", y = 1 ok"#}))
    );

    // Whitespace, newlines and an empty string are kept as written.
    assert_eq!(
        parse("Note { text = \"  two\nlines \", empty = \"\" }"),
        Some(json!({"__java_class": "Note", "text": "  two\nlines ", "empty": ""}))
    );
}

/// A bare token is the value it prints - `null`, a boolean, a number only where it reads back as written -
/// and its text otherwise; a map's entries and a list's elements are bare, and a class name may be qualified.
#[test]
fn a_bare_token_is_the_value_it_prints() {
    assert_eq!(
        parse(
            "dev.acme.Turn$Part { index = 12, ratio = 0.50, big = 1e3, done = true, kind = TOOL, \
               ref = dev.acme.Ref@6d06d69c, tags = [a, b], options = {temperature=0.2, mode=FAST, nested=Inner { x = 1 }}, \
               empty = Empty { } }"
        ),
        Some(json!({
            "__java_class": "dev.acme.Turn$Part",
            "index": 12,
            "ratio": "0.50",
            "big": "1e3",
            "done": true,
            "kind": "TOOL",
            "ref": "dev.acme.Ref@6d06d69c",
            "tags": ["a", "b"],
            "options": {"temperature": 0.2, "mode": "FAST", "nested": {"__java_class": "Inner", "x": 1}},
            "empty": {"__java_class": "Empty"}
        }))
    );
    assert_eq!(
        parse("[A { x = 1 }, B { y = \"two\" }]"),
        Some(json!([{"__java_class": "A", "x": 1}, {"__java_class": "B", "y": "two"}])),
        "a list of objects at the top"
    );
}

/// **What is not such a `toString` is refused, so the carrier stays text.**
#[test]
fn what_is_not_a_tostring_is_refused() {
    for (why, text) in [
        ("prose", "The model said hello."),
        ("JSON", r#"{"role": "user", "content": "hi"}"#),
        ("a bare token", "LOW"),
        ("a map at the top", "{a=1}"),
        ("trailing text", "A { x = 1 } and more"),
        ("a member named twice", "A { x = 1, x = 2 }"),
        ("a member shadowing the class", "A { __java_class = B }"),
        (
            "a quoted list element, which Java never prints",
            r#"A { xs = ["a"] }"#,
        ),
        (
            "a quoted map value, which Java never prints",
            r#"A { m = {k="v"} }"#,
        ),
        ("an unterminated string", r#"A { text = "never closed }"#),
        ("an unclosed object", "A { x = 1"),
        ("a member without a value", "A { x = }"),
        ("a list closed by a brace", "A { xs = [1, 2} }"),
        ("a Python repr", "A(x=1)"),
    ] {
        assert_eq!(parse(text), None, "{why}: {text}");
    }

    let deep = format!("{}1{}", "A { x = ".repeat(70), " }".repeat(70));
    assert_eq!(parse(&deep), None, "nesting past the depth bound");
    let within = format!("{}1{}", "A { x = ".repeat(60), " }".repeat(60));
    assert!(parse(&within).is_some(), "and within it");

    // Twenty strings whose every later member could be part of an earlier one, and text after the end: the
    // readings multiply, and the bound refuses the text rather than searching all of them.
    let members: Vec<String> = (0..20).map(|i| format!("m{i} = \"a\"")).collect();
    let adversarial = format!("A {{ {} }} trailing", members.join(", "));
    assert_eq!(parse(&adversarial), None);
    let started = std::time::Instant::now();
    let _ = parse(&adversarial);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "the bound keeps the search short"
    );
}

/// **A reading is accepted only where printing it again gives back the text, byte for byte.** A quote inside a
/// string is not escaped, so a wrong end can still read as a well-formed tree; printed again with the format's
/// own spacing, it differs from the text unless the text could have been printed from it.
#[test]
fn a_reading_is_accepted_only_where_printing_it_again_gives_back_the_text() {
    // The earliest end reads `b` as a second member, but this format never prints a comma without a space after
    // it: only the reading that keeps `",b = "` inside the string reproduces the text.
    assert_eq!(
        parse(r#"T { text = "a",b = "c" }"#),
        Some(json!({"__java_class": "T", "text": r#"a",b = "c"#}))
    );
    // Ambiguous, and neither reading reproduces it - the closing brace has no space before it - so it stays text.
    assert_eq!(parse(r#"T { text = "a", b = "c"}"#), None);
    // Nor is spacing this format does not print accepted where nothing is ambiguous.
    for text in [
        r#"T {text = "x"}"#,
        r#"T { text="x" }"#,
        "T { xs = [a,b] }",
        "T { m = {k = v} }",
        " T { x = 1 }",
        "T { x = 1 } ",
        "T { x = 1, }",
        "T { x = 1  y = 2 }",
    ] {
        assert_eq!(parse(text), None, "{text}");
    }
}

/// A rule reads a carrier with `parse: java_tostring`, and three answers follow: a `toString` is read as its
/// tree, a text that is not one is skipped - the carrier is left to the next rule, not read as something else -
/// and an absent carrier reads nothing.
#[test]
fn a_rule_reads_a_tostring_carrier_and_skips_one_that_is_not() {
    use crate::rules::assets::ParsedAssets;
    use crate::rules::message_rules::{MessageContext, compile};

    let plan = compile(
        &ParsedAssets::parse(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            br#"{"id": "t", "messages": [{"id": "t.tostring", "read": {"attribute": "probe.message"}, "parse": "java_tostring", "wrap": {"role": "user", "content_from": "$.text"}, "emit": "message", "priority": 1}]}"#
                .to_vec(),
        )]))
        .expect("the probe assets parse"),
    )
    .expect("the probe rule compiles");
    let read = |value: Option<&str>| {
        let attrs: std::collections::HashMap<String, String> = value
            .map(|text| ("probe.message".to_string(), text.to_string()))
            .into_iter()
            .collect();
        plan.run(&MessageContext::for_scoped_span(
            "chat", None, &attrs, false,
        ))
        .into_iter()
        .map(|emission| emission.value)
        .collect::<Vec<_>>()
    };
    assert_eq!(
        read(Some(r#"TextContent { text = "She said "yes"." }"#)),
        vec![json!({"role": "user", "content": r#"She said "yes"."#})]
    );
    assert!(
        read(Some(r#"TextContent {text = "hi"}"#)).is_empty(),
        "not a toString this format prints"
    );
    assert!(read(Some("hi")).is_empty(), "prose");
    assert!(read(None).is_empty(), "no carrier");
}
