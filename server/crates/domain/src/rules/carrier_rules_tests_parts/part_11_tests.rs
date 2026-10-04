
/// Every detection refusal fires.
///
/// Three of the ten were exercised by nothing, `UselessSupersedes` among them - and that one changed meaning when
/// `supersedes` began to order, so what it still refuses is worth pinning: a target nothing declares, a self-edge,
/// and the same target twice.
#[test]
fn every_detection_refusal_fires() {
    use crate::rules::detect_rules::{DetectCompileError as E, compile};
    let compiled = |asset: &str| {
        compile(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            asset.as_bytes().to_vec(),
        )]))
    };
    type Case = (&'static str, &'static str, fn(&E) -> bool);
    let cases: Vec<Case> = vec![
        ("not JSON at all", "{", |e| matches!(e, E::Parse { .. })),
        (
            "an empty attribute prefix, which matches everything",
            r#"{"id":"t","doc":"d","detect":[{"id":"a","doc":"d","label":"A","legacy_rank":1,
               "match":{"attr_prefix":[""]}}]}"#,
            |e| matches!(e, E::EmptyLiteral { .. }),
        ),
        (
            "two rules sharing an id",
            r#"{"id":"t","doc":"d","detect":[
               {"id":"a","doc":"d","label":"A","legacy_rank":1,"match":{"attr_prefix":["one."]}},
               {"id":"a","doc":"d","label":"B","legacy_rank":2,"match":{"attr_prefix":["two."]}}]}"#,
            |e| matches!(e, E::DuplicateRuleId { .. }),
        ),
        (
            "a supersedes edge naming a rule nothing declares",
            r#"{"id":"t","doc":"d","detect":[{"id":"a","doc":"d","label":"A","legacy_rank":1,
               "match":{"attr_prefix":["one."]},"supersedes":["absent"]}]}"#,
            |e| matches!(e, E::UselessSupersedes { .. }),
        ),
        (
            // The two sections fill the label independently, so a typo is a second framework name for one
            // producer - and which one a span gets depends on whether its signals were detected or its SDK
            // declared itself, which a reader filtering on the label sees as one producer split in two.
            "an SDK slug resolving to a label no rule produces",
            r#"{"id":"t","doc":"d","detect":[{"id":"a","doc":"d","label":"Acme","legacy_rank":1,
               "match":{"attr_prefix":["one."]}}],
               "sdk_slugs":[{"slug":"acme","label":"Acmee"}]}"#,
            |e| matches!(e, E::SlugLabelNoRuleProduces { .. }),
        ),
        (
            "a supersedes edge to the rule itself",
            r#"{"id":"t","doc":"d","detect":[{"id":"a","doc":"d","label":"A","legacy_rank":1,
               "match":{"attr_prefix":["one."]},"supersedes":["a"]}]}"#,
            |e| matches!(e, E::UselessSupersedes { .. }),
        ),
    ];
    for (what, asset, expected) in cases {
        let error = compiled(asset)
            .err()
            .unwrap_or_else(|| panic!("should have been refused: {what}"));
        assert!(expected(&error), "wrong refusal for {what}: {error}");
    }
    // A slug naming a label its own asset's rule produces compiles, which is the shape every asset uses.
    assert!(
        compiled(
            r#"{"id":"t","doc":"d","detect":[{"id":"a","doc":"d","label":"Acme","legacy_rank":1,
               "match":{"attr_prefix":["one."]}}],
               "sdk_slugs":[{"slug":"acme","label":"Acme"}]}"#
        )
        .is_ok()
    );
}

/// **Every refusal the rules engine declares is exercised by some test.** Read off the source, so a new one
/// cannot arrive unexercised.
///
/// This is the meta-rule the series kept citing and nothing enforced: 67 refusals across seven compile modules,
/// and **21 of them were exercised nowhere** when this was written - each a claim rather than a guard, any of them
/// deletable or narrowable with the suite green. Several were one edit from unreachable, which is the shape that
/// matters: the span-field exclusivity check *counts* seven reader forms, and a count that drifted to six would
/// silently admit the form it forgot.
///
/// The check is on the **source text**, because the refusals are seven unrelated enums with no common trait, and a
/// runtime inventory would need every module to opt in - which is the thing that gets forgotten. It looks for the
/// variant named in a test context: a `*_tests.rs` file, or a module's own `#[cfg(test)] mod tests`.
///
/// What it cannot see, stated because it is a real limit: whether the probe that names a variant actually *causes*
/// that refusal, or merely mentions it. That is what mutation-verifying each one is for, and each of the
/// `every_*_refusal_fires` tests matches on the variant rather than the message so a reworded diagnostic does not
/// quietly stop testing it.
#[test]
fn every_declared_refusal_is_exercised_by_a_test() {
    /// The seven compile modules and the error enum each declares.
    const MODULES: &[(&str, &str)] = &[
        ("carrier_rules.rs", "CompileError"),
        ("detect_rules.rs", "DetectCompileError"),
        ("classify.rs", "ClassifyCompileError"),
        ("members.rs", "MemberCompileError"),
        ("message_rules.rs", "MessageCompileError"),
        ("tool_shapes.rs", "ToolShapeError"),
        ("span_fields.rs", "FieldCompileError"),
    ];
    /// Every production module's source, so the enum bodies and inline test contexts come from one place.
    const SOURCES: &[(&str, &str)] = &[
        ("carrier_rules.rs", include_str!("../carrier_rules.rs")),
        ("detect_rules.rs", include_str!("../detect_rules.rs")),
        ("classify.rs", include_str!("../classify.rs")),
        ("members.rs", include_str!("../members.rs")),
        ("message_rules.rs", include_str!("../message_rules.rs")),
        ("tool_shapes.rs", include_str!("../tool_shapes.rs")),
        ("span_fields.rs", include_str!("../span_fields.rs")),
    ];

    let source_of = |name: &str| {
        SOURCES
            .iter()
            .find(|(found, _)| *found == name)
            .map(|(_, text)| *text)
            .unwrap_or_else(|| panic!("`{name}` is not in SOURCES"))
    };
    // Every inline test context.
    let mut contexts = String::new();
    for (_, text) in SOURCES {
        if let Some(at) = text.find("#[cfg(test)]\nmod tests {") {
            contexts.push_str(&text[at..]);
        }
    }
    // Dedicated test modules may be split into sibling part directories. Read the whole tree so
    // dividing a large suite cannot make a refusal disappear from this audit.
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for path in [
        manifest.join("src/rules/carrier_rules_tests.rs"),
        manifest.join("src/rules/detect_rules_tests.rs"),
        manifest.join("../ingestion/src/traces/extract/attributes_tests.rs"),
        manifest.join("../ingestion/src/traces/extract/messages_tests.rs"),
    ] {
        contexts.push_str(&test_module_source(&path));
    }

    let mut unexercised: Vec<String> = Vec::new();
    let mut counted = 0_usize;
    for (module, enum_name) in MODULES {
        let text = source_of(module);
        let start = text
            .find(&format!("pub enum {enum_name} {{"))
            .unwrap_or_else(|| panic!("`{enum_name}` is not declared in `{module}`"));
        let body = &text[start..];
        let end = body.find("\n}\n").expect("the enum body ends");
        for line in body[..end].lines() {
            // A variant: four-space indented, capitalised, and opening its payload or ending the entry.
            let trimmed = line.trim_start();
            let indent = line.len() - trimmed.len();
            let name: String = trimmed
                .chars()
                .take_while(|c| c.is_alphanumeric())
                .collect();
            let after = trimmed[name.len()..].trim_start();
            if indent != 4
                || name.is_empty()
                || !name.starts_with(char::is_uppercase)
                || !(after.starts_with('{') || after.starts_with('(') || after == ",")
            {
                continue;
            }
            counted += 1;
            if !contexts.contains(&format!("::{name}")) {
                unexercised.push(format!("{module}: {enum_name}::{name}"));
            }
        }
    }

    assert!(
        counted > 60,
        "only {counted} refusals were found, so the parse is not reading the enums"
    );
    assert!(
        unexercised.is_empty(),
        "{} of {counted} declared refusals are exercised by no test, so each is a claim rather than a \
         guard:\n{}",
        unexercised.len(),
        unexercised.join("\n")
    );
}

fn test_module_source(path: &std::path::Path) -> String {
    let mut source = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("cannot read `{}`: {error}", path.display()));
    let stem = path
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .expect("a Rust test module has a UTF-8 stem");
    let parts = path.with_file_name(format!("{stem}_parts"));
    if !parts.is_dir() {
        return source;
    }
    let mut paths: Vec<std::path::PathBuf> = std::fs::read_dir(&parts)
        .unwrap_or_else(|error| panic!("cannot read `{}`: {error}", parts.display()))
        .map(|entry| entry.expect("test-part directory entry is readable").path())
        .filter(|part| part.extension().is_some_and(|extension| extension == "rs"))
        .collect();
    paths.sort();
    for part in paths {
        source.push_str(
            &std::fs::read_to_string(&part)
                .unwrap_or_else(|error| panic!("cannot read `{}`: {error}", part.display())),
        );
    }
    source
}

#[test]
fn a_scoped_constructor_repr_decoder_yields_to_the_general_carrier_reader() {
    use crate::rules::message_rules::{MessageContext, compile};

    let plan = compile(&std::collections::BTreeMap::from([(
        "t.json".to_string(),
        br#"{"id":"t","messages":[
          {"id":"t.constructor","read":{"attribute":"result"},
           "parse":"python_constructor_repr","instrumentation_scope":{"name":"specific"},
           "wrap":{"role":"tool","content_from_any_of":["$.content"],"block":{
             "type":"tool_result","attach":[
               {"from_value_any_of":["$.state"],"require":{"all":[
                 {"one_of":["error","denied","interrupted"]}]},
                "as":"is_error","value":true,"after_content":true}]}},
           "emit":"message","reads_tool_spans":true,"legacy_rank":1},
          {"id":"t.general","read":{"attribute":"result"},"parse":"json_or_string",
           "wrap":{"role":"tool","block":{"type":"tool_result"}},
           "emit":"message","reads_tool_spans":true,"legacy_rank":2}]}"#
            .to_vec(),
    )]))
    .expect("an exact instrumentation scope makes the first reading conditional");

    let attrs = std::collections::HashMap::from([(
        "result".to_string(),
        serde_json::to_string(
            "Response(content=[TextBlock(type='text', text='failed', id='volatile')], \
             state=<State.ERROR: 'error'>)",
        )
        .expect("the OTLP string layer serialises"),
    )]);
    let scoped = MessageContext::for_scoped_span("tool", Some("specific"), Some("1.0"), &attrs, true);
    let read = plan.run(&scoped);
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].rule_id, "t.constructor");
    assert_eq!(read[0].value["content"][0]["is_error"], true);
    assert_eq!(read[0].value["content"][0]["content"][0]["text"], "failed");

    let other = MessageContext::for_scoped_span("tool", Some("other"), Some("1.0"), &attrs, true);
    let read = plan.run(&other);
    assert_eq!(read.len(), 1);
    assert_eq!(
        read[0].rule_id, "t.general",
        "outside the declared scope the conventional reader still owns the carrier"
    );
}

/// A JSON list of constructor reprs is read element by element, and only when every element is one.
///
/// An agent assigned work through AgentScope's team pipeline returns its content blocks rather than a
/// ToolResponse, serialised as such a list. Read as plain JSON strings, the result kept each block's generated
/// id and timestamp, so two runs of one conversation reported different results.
#[test]
fn a_list_of_constructor_reprs_is_read_as_blocks_and_only_whole() {
    use crate::rules::message_rules::{MessageContext, compile};

    let plan = compile(&std::collections::BTreeMap::from([(
        "t.json".to_string(),
        br#"{"id":"t","messages":[
          {"id":"t.blocks","read":{"attribute":"result"},"parse":"python_constructor_repr_array",
           "instrumentation_scope":{"name":"specific"},
           "wrap":{"role":"tool","content_from_any_of":["$"],"block":{"type":"tool_result"}},
           "emit":"message","reads_tool_spans":true,"legacy_rank":1},
          {"id":"t.general","read":{"attribute":"result"},"parse":"json_or_string",
           "wrap":{"role":"tool","block":{"type":"tool_result"}},
           "emit":"message","reads_tool_spans":true,"legacy_rank":2}]}"#
            .to_vec(),
    )]))
    .expect("the array decoder compiles");

    let run = |value: serde_json::Value| {
        let attrs = std::collections::HashMap::from([("result".to_string(), value.to_string())]);
        let context =
            MessageContext::for_scoped_span("tool", Some("specific"), Some("1.0"), &attrs, true);
        plan.run(&context)
    };

    let read = run(serde_json::json!([
        "TextBlock(type='text', text='Sunny', id='volatile', created_at='2026-10-04T23:39:42')"
    ]));
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].rule_id, "t.blocks");
    assert_eq!(read[0].value["content"][0]["content"][0]["text"], "Sunny");

    let read = run(serde_json::json!(["TextBlock(type='text', text='Sunny')", "plain"]));
    assert_eq!(read.len(), 1);
    assert_eq!(
        read[0].rule_id, "t.general",
        "a list with an element that is not a repr is some other shape"
    );
}
