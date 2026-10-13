use serde_json::json;

use super::super::python_repr::PYTHON_REPR_MAX_DEPTH;
use super::{try_parse_python_fields, try_parse_python_object_repr};

/// Strings and objects as CPython printed them (`fixtures/python_repr_cases.json`, written by `repr()` itself),
/// each with the tree its reading must give: the escape decoder is held to Python's own output, not to
/// hand-written expectations.
#[test]
fn every_value_cpython_printed_reads_as_its_tree() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/python_repr_cases.json"))
            .expect("the fixture is JSON");
    let cases = fixture["cases"].as_array().expect("cases");
    assert!(cases.len() >= 20, "the fixture holds {} cases", cases.len());
    for case in cases {
        let printed = case["repr"].as_str().expect("a repr");
        assert_eq!(
            try_parse_python_object_repr(printed).as_ref(),
            Some(&case["value"]),
            "{printed}"
        );
    }
}

/// An object `repr` cannot print is an opaque leaf holding nothing to read, nested angle brackets and all, and an
/// enum repr keeps its member and its value - neither is read as anything else.
#[test]
fn unprintable_objects_and_enums_are_explicit_leaves() {
    let text = "{'ctx': <workflows.context.context.Context object at 0x11054c490>, \
                'function': <function MCPToolset._connect_and_load_tools.<locals>.create_invoke_tool.<locals>.invoke_tool at 0x10db9a1f0>, \
                'role': <ChatRole.SYSTEM: 'system'>}";
    assert_eq!(
        try_parse_python_object_repr(text),
        Some(json!({
            "ctx": {"__python_opaque": true},
            "function": {"__python_opaque": true},
            "role": {"__python_enum": "ChatRole.SYSTEM", "__python_value": "system"},
        }))
    );
}

/// The shapes the readers exist for, as their producers print them: a list of constructors, a constructor with
/// positional arguments, a dict of objects.
#[test]
fn the_recorded_shapes_read() {
    assert_eq!(
        try_parse_python_object_repr(
            "[TextContent(inner_content=TextContent(type='text', text='395.0', annotations=None, meta=None), \
             ai_model_id=None, metadata={}, content_type='text', text='395.0', encoding=None)]"
        ),
        Some(json!([{
            "__python_constructor": "TextContent",
            "inner_content": {"__python_constructor": "TextContent", "type": "text", "text": "395.0", "annotations": null, "meta": null},
            "ai_model_id": null, "metadata": {}, "content_type": "text", "text": "395.0", "encoding": null,
        }]))
    );
    // A flag with no member name prints as its class and value.
    assert_eq!(
        try_parse_python_object_repr("<F: 0>"),
        Some(json!({"__python_enum": "F", "__python_value": 0}))
    );
    assert_eq!(
        try_parse_python_object_repr("1000000000000000.2"),
        Some(json!(1_000_000_000_000_000.2_f64))
    );
    assert_eq!(
        try_parse_python_object_repr("-1000000000000000.2"),
        Some(json!(-1_000_000_000_000_000.2_f64))
    );
    assert_eq!(
        try_parse_python_object_repr("Point(1, -2.5, label='p')"),
        Some(json!({"__python_constructor": "Point", "__python_args": [1, -2.5], "label": "p"}))
    );
    assert_eq!(
        try_parse_python_object_repr("{'args': (), 'kwargs': {'expression': '(17 * 23) + 4'}}"),
        Some(json!({"args": [], "kwargs": {"expression": "(17 * 23) + 4"}}))
    );
}

/// A data model's `str()` reads as its fields, with no class invented.
#[test]
fn a_model_printed_by_str_reads_as_its_fields() {
    assert_eq!(
        try_parse_python_fields(
            "meta={'fastmcp': {'wrap_result': True}} content=[TextContent(type='text', text='395.0', \
             annotations=None, meta=None)] structured_content={'result': 395.0} is_error=False result_type='complete'"
        ),
        Some(json!({
            "meta": {"fastmcp": {"wrap_result": true}},
            "content": [{"__python_constructor": "TextContent", "type": "text", "text": "395.0", "annotations": null, "meta": null}],
            "structured_content": {"result": 395.0},
            "is_error": false,
            "result_type": "complete",
        }))
    );
    // Two spaces between fields, or prose, is no model `str()` printed.
    assert_eq!(try_parse_python_fields("a=1  b=2"), None);
    assert_eq!(try_parse_python_fields("the weather is fine"), None);
    // A value is still `repr`'s: a bare word is not one.
    assert_eq!(try_parse_python_fields("state=sunny"), None);
}

/// A text `repr` could not have printed stays text: other spacing, a trailing comma it never writes, a bare
/// name, a key or keyword the tree cannot hold, an escape it never writes, a positional argument after a keyword.
#[test]
fn a_text_repr_did_not_print_stays_text() {
    for text in [
        "[1,2]",
        "[1, 2,]",
        "Text( text='a')",
        "Text(text = 'a')",
        "{'a':1}",
        "sunny",
        "Weather(state=sunny)",
        "{1: 'one'}",
        "{'a': 1, 'a': 2}",
        "Text(text='a', text='b')",
        "Text(__python_constructor='Other')",
        "'\\N{SNOWMAN}'",
        "'bell\\a'",
        "'\\101'",
        "Call(a=1, 2)",
        "<Role.USER: 'user'",
        "<object with 'a quote' at 0x1>",
        "'unterminated",
        "'line\nbreak'",
        "None None",
        // Escapes with a sign, and raw control characters, which `repr` always escapes.
        "'\\x+1'",
        "'\\u+041'",
        "'\\U+0000041'",
        "'a\tb'",
        "'nul\u{0}'",
        // Numbers `repr` spells otherwise, or not at all.
        "01",
        "1.",
        "-0",
        "1E+20",
        "1e5",
        "1e999",
        "1.50",
        "inf",
        "18446744073709551616",
        // An enum whose value fails, or whose spacing is not repr's, is no opaque object.
        "<E.INF: inf>",
        "<E.X:1>",
        // A float spelt other than CPython rounds it: `1000000000000000.25` prints `...0.2`, ties to even.
        "1000000000000000.3",
    ] {
        assert_eq!(try_parse_python_object_repr(text), None, "{text}");
    }
}

/// Nesting is bounded by the same depth as the other Python readers.
#[test]
fn nesting_is_bounded_as_the_other_python_readers_bound_it() {
    let nested = |depth: usize| format!("{}{}", "[".repeat(depth), "]".repeat(depth));
    assert!(try_parse_python_object_repr(&nested(PYTHON_REPR_MAX_DEPTH - 1)).is_some());
    assert_eq!(
        try_parse_python_object_repr(&nested(PYTHON_REPR_MAX_DEPTH + 1)),
        None
    );
}

/// Every float CPython printed - random bit patterns across every magnitude, exact halves and quarters at the
/// magnitudes where the shortest digits tie, rounded decimals - reads as the value it printed, and its
/// neighbouring spelling with the last digit moved is refused: the reader accepts `repr`'s spelling and no other.
#[test]
fn every_float_cpython_printed_is_its_only_spelling() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/python_float_cases.json"))
            .expect("the fixture is JSON");
    let mut checked_neighbours = 0;
    let reprs = fixture["reprs"].as_array().expect("reprs");
    assert!(
        reprs.len() > 3000,
        "the fixture holds {} floats",
        reprs.len()
    );
    for printed in reprs {
        let printed = printed.as_str().expect("a repr");
        let value: f64 = printed.parse().expect("CPython printed a float");
        assert_eq!(
            try_parse_python_object_repr(printed).and_then(|v| v.as_f64()),
            Some(value),
            "{printed}"
        );
        // The same value spelt with its last digit moved by one is not how `repr` spells it.
        let mantissa = printed.split('e').next().unwrap_or(printed);
        let digit = mantissa
            .rfind(|c: char| c.is_ascii_digit())
            .expect("a digit");
        for step in [1u8, 9] {
            let moved = (printed.as_bytes()[digit] - b'0' + step) % 10;
            let mut neighbour = printed.to_string();
            neighbour.replace_range(digit..=digit, &moved.to_string());
            if neighbour.parse::<f64>().ok() == Some(value) {
                assert_eq!(
                    try_parse_python_object_repr(&neighbour),
                    None,
                    "{neighbour} for {printed}"
                );
                checked_neighbours += 1;
            }
        }
    }
    assert!(
        checked_neighbours > 1000,
        "{checked_neighbours} same-value neighbours checked"
    );
}
