/// Every detection refusal fires.
///
/// Three of the ten were exercised by nothing, `UselessSupersedes` among them - and that one changed meaning when
/// `supersedes` began to order, so what it still refuses is worth pinning: a target nothing declares, a self-edge,
/// and the same target twice.
#[test]
fn every_detection_refusal_fires() {
    use crate::rules::detect_rules::{DetectCompileError as E, compile};
    let compiled = |asset: &str| {
        compile(
            &ParsedAssets::parse(&std::collections::BTreeMap::from([(
                "t.json".to_string(),
                asset.as_bytes().to_vec(),
            )]))
            .expect("the probe assets parse"),
        )
    };
    type Case = (&'static str, &'static str, fn(&E) -> bool);
    let cases: Vec<Case> = vec![
        (
            "an empty attribute prefix, which matches everything",
            r#"{"id": "t", "doc": "d", "detect": [{"id": "a", "doc": "d", "label": "A", "priority": 1, "where": {"source": "attr_keys", "starts_with": ""}}]}"#,
            |e| matches!(e, E::Condition { .. }),
        ),
        (
            "two rules sharing an id",
            r#"{"id": "t", "doc": "d", "detect": [{"id": "a", "doc": "d", "label": "A", "priority": 1, "where": {"source": "attr_keys", "starts_with": "one."}}, {"id": "a", "doc": "d", "label": "B", "priority": 2, "where": {"source": "attr_keys", "starts_with": "two."}}]}"#,
            |e| matches!(e, E::DuplicateRuleId { .. }),
        ),
        (
            "a supersedes edge naming a rule nothing declares",
            r#"{"id": "t", "doc": "d", "detect": [{"id": "a", "doc": "d", "label": "A", "priority": 1, "where": {"source": "attr_keys", "starts_with": "one."}, "supersedes": ["absent"]}]}"#,
            |e| matches!(e, E::UselessSupersedes { .. }),
        ),
        (
            // The two sections fill the label independently, so a typo is a second framework name for one
            // producer - and which one a span gets depends on whether its signals were detected or its SDK
            // declared itself, which a reader filtering on the label sees as one producer split in two.
            "an SDK slug resolving to a label no rule produces",
            r#"{"id": "t", "doc": "d", "detect": [{"id": "a", "doc": "d", "label": "Acme", "priority": 1, "where": {"source": "attr_keys", "starts_with": "one."}}], "sdk_slugs": [{"slug": "acme", "label": "Acmee"}]}"#,
            |e| matches!(e, E::SlugLabelNoRuleProduces { .. }),
        ),
        (
            "a supersedes edge to the rule itself",
            r#"{"id": "t", "doc": "d", "detect": [{"id": "a", "doc": "d", "label": "A", "priority": 1, "where": {"source": "attr_keys", "starts_with": "one."}, "supersedes": ["a"]}]}"#,
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
            r#"{"id": "t", "doc": "d", "detect": [{"id": "a", "doc": "d", "label": "Acme", "priority": 1, "where": {"source": "attr_keys", "starts_with": "one."}}], "sdk_slugs": [{"slug": "acme", "label": "Acme"}]}"#
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
        counted > 50,
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

    let plan = compile(
        &ParsedAssets::parse(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            br#"{"id": "t", "messages": [{"id": "t.constructor", "read": {"attribute": "result"}, "parse": "python_constructor_repr", "where": {"source": "scope.name", "equals": "specific"}, "wrap": {"role": "tool", "content_from_any_of": ["$.content"], "block": {"type": "tool_result", "attach": [{"from_value_any_of": ["$.state"], "where": {"one_of": ["error", "denied", "interrupted"]}, "as": "is_error", "value": true, "after_content": true}]}}, "emit": "message", "reads_tool_spans": true, "priority": 1}, {"id": "t.general", "read": {"attribute": "result"}, "parse": "json_or_string", "wrap": {"role": "tool", "block": {"type": "tool_result"}}, "emit": "message", "reads_tool_spans": true, "priority": 2}]}"#
                .to_vec(),
        )]))
        .expect("the probe assets parse"),
    )
    .expect("an exact instrumentation scope makes the first reading conditional");

    let attrs = std::collections::HashMap::from([(
        "result".to_string(),
        serde_json::to_string(
            "Response(content=[TextBlock(type='text', text='failed', id='volatile')], \
             state=<State.ERROR: 'error'>)",
        )
        .expect("the OTLP string layer serialises"),
    )]);
    let scoped = MessageContext::for_scoped_span("tool", Some("specific"), &attrs, true);
    let read = plan.run(&scoped);
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].rule_id, "t.constructor");
    assert_eq!(read[0].value["content"][0]["is_error"], true);
    assert_eq!(read[0].value["content"][0]["content"][0]["text"], "failed");

    let other = MessageContext::for_scoped_span("tool", Some("other"), &attrs, true);
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

    let plan = compile(
        &ParsedAssets::parse(&std::collections::BTreeMap::from([(
            "t.json".to_string(),
            br#"{"id": "t", "messages": [{"id": "t.blocks", "read": {"attribute": "result"}, "parse": "python_constructor_repr_array", "where": {"source": "scope.name", "equals": "specific"}, "wrap": {"role": "tool", "content_from_any_of": ["$"], "block": {"type": "tool_result"}}, "emit": "message", "reads_tool_spans": true, "priority": 1}, {"id": "t.general", "read": {"attribute": "result"}, "parse": "json_or_string", "wrap": {"role": "tool", "block": {"type": "tool_result"}}, "emit": "message", "reads_tool_spans": true, "priority": 2}]}"#
                .to_vec(),
        )]))
        .expect("the probe assets parse"),
    )
    .expect("the array decoder compiles");

    let run = |value: serde_json::Value| {
        let attrs = std::collections::HashMap::from([("result".to_string(), value.to_string())]);
        let context = MessageContext::for_scoped_span("tool", Some("specific"), &attrs, true);
        plan.run(&context)
    };

    let read = run(serde_json::json!([
        "TextBlock(type='text', text='Sunny', id='volatile', created_at='2026-10-04T23:39:42')"
    ]));
    assert_eq!(read.len(), 1);
    assert_eq!(read[0].rule_id, "t.blocks");
    assert_eq!(read[0].value["content"][0]["content"][0]["text"], "Sunny");

    let read = run(serde_json::json!([
        "TextBlock(type='text', text='Sunny')",
        "plain"
    ]));
    assert_eq!(read.len(), 1);
    assert_eq!(
        read[0].rule_id, "t.general",
        "a list with an element that is not a repr is some other shape"
    );
}

/// OpenInference's OpenAI instrumentor drops a Chat Completions `file` part from the flattened family but
/// keeps it in the serialised request, so a PDF a user sent was missing from the conversation.
///
/// The request is joined by position only when its own list holds the system turn: a body whose system
/// prompt sits beside the list is indexed one off from the family, and joining it would hand the user's
/// content to the system message.
#[test]
fn a_chat_completions_request_restores_the_file_part_the_family_dropped() {
    let plan = &super::ruleset().messages;
    let family = |system_key: &str| {
        std::collections::HashMap::from([
            ("openinference.span.kind".to_string(), "LLM".to_string()),
            (
                "llm.input_messages.0.message.role".to_string(),
                "system".to_string(),
            ),
            (system_key.to_string(), "be brief".to_string()),
            (
                "llm.input_messages.1.message.role".to_string(),
                "user".to_string(),
            ),
            (
                "llm.input_messages.1.message.contents.0.message_content.type".to_string(),
                "text".to_string(),
            ),
            (
                "llm.input_messages.1.message.contents.0.message_content.text".to_string(),
                "read this".to_string(),
            ),
        ])
    };
    let read = |attrs: &std::collections::HashMap<String, String>| {
        let ctx = super::message_rules::MessageContext::for_span("ChatCompletion", attrs, false);
        plan.run(&ctx)
            .iter()
            .map(|e| {
                (
                    e.value["role"].as_str().unwrap_or("").to_string(),
                    e.value.to_string(),
                )
            })
            .collect::<Vec<_>>()
    };

    let mut chat = family("llm.input_messages.0.message.content");
    chat.insert(
        "input.value".to_string(),
        serde_json::json!({"model": "m", "messages": [
            {"role": "system", "content": "be brief"},
            {"role": "user", "content": [
                {"type": "text", "text": "read this"},
                {"type": "file", "file": {
                    "filename": "task.pdf",
                    "file_data": "data:application/pdf;base64,JVBERi0x"
                }}
            ]}
        ]})
        .to_string(),
    );
    let messages = read(&chat);
    assert!(
        messages
            .iter()
            .any(|(role, value)| role == "user" && value.contains("task.pdf")),
        "the user turn must carry the file part: {messages:?}"
    );

    let mut converse = family("llm.input_messages.0.message.contents.0.message_content.text");
    converse.insert(
        "input.value".to_string(),
        serde_json::json!({"system": [{"text": "be brief"}], "messages": [
            {"role": "user", "content": [
                {"text": "read this"},
                {"document": {"name": "task.pdf", "format": "pdf", "source": {"bytes": "JVBERi0x"}}}
            ]}
        ]})
        .to_string(),
    );
    let messages = read(&converse);
    assert!(
        !messages
            .iter()
            .any(|(role, value)| role == "system" && value.contains("task.pdf")),
        "a list without its system turn is one position off the family: {messages:?}"
    );
}

/// No production Rust spells an attribute key that only a framework's own asset declares.
///
/// `no_production_module_names_a_framework` catches a framework's *name*; this catches its *vocabulary*. The two
/// leaks it was written for named no framework at all - a constant `"llm.cost.total"` read beside the declared
/// fields, and a filter keyed on one dialect's output attribute - and the name sweep passed over both. A key that
/// a convention asset also declares is a published convention, which code may name; a key that a provider asset
/// declares is the provider's identity, which the pricing catalogue may name.
#[test]
fn no_production_module_spells_a_framework_attribute_key() {
    fn strings<'a>(value: &'a serde_json::Value, out: &mut Vec<&'a str>) {
        match value {
            serde_json::Value::String(s) => out.push(s),
            serde_json::Value::Array(items) => items.iter().for_each(|v| strings(v, out)),
            serde_json::Value::Object(map) => map.iter().for_each(|(k, v)| {
                out.push(k);
                strings(v, out);
            }),
            _ => {}
        }
    }
    let is_key = |s: &str| {
        let mut parts = s.split('.');
        parts.next().is_some_and(|head| {
            !head.is_empty()
                && head
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        }) && s.contains('.')
            && s.split('.').all(|part| {
                !part.is_empty()
                    && part
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            })
    };
    let sources = crate::rules::schema::embedded_sources();
    let mut framework_keys: std::collections::BTreeMap<String, String> = Default::default();
    let mut shared: std::collections::BTreeSet<String> = Default::default();
    for (path, bytes) in &sources {
        let id = path
            .rsplit('/')
            .next()
            .unwrap_or(path)
            .trim_end_matches(".json");
        let value: serde_json::Value = serde_json::from_slice(bytes).expect("the asset parses");
        let mut found = Vec::new();
        strings(&value, &mut found);
        let keys = found.into_iter().filter(|s| is_key(s)).map(str::to_string);
        // Shared means a published convention. The vocabulary assets are cross-framework *tables*, but each
        // entry in them is still one framework's spelling - `llm.token_count.prompt` is not semconv because
        // the usage table lists it beside `gen_ai.usage.input_tokens`.
        if path.starts_with("conventions/") || PROVIDERS.contains(&id) {
            shared.extend(keys);
        } else {
            for key in keys {
                framework_keys.entry(key).or_insert_with(|| id.to_string());
            }
        }
    }
    // Published OpenTelemetry attributes that no convention asset happens to list, because the reading of them
    // is a span field rather than a carrier: the session, user and conversation identity conventions.
    const PUBLISHED: &[&str] = &[
        "session.id",
        "user.id",
        "enduser.id",
        "gen_ai.conversation.id",
    ];
    shared.extend(PUBLISHED.iter().map(|key| key.to_string()));
    framework_keys.retain(|key, _| !shared.contains(key));
    assert!(
        framework_keys.len() > 100,
        "only {} framework-specific keys were derived from the assets, so the derivation is wrong",
        framework_keys.len()
    );

    let repository = repository_root();
    let mut offenders = Vec::new();
    let mut checked = 0_usize;
    walk_production_rust_sources(&mut |path, source| {
        let relative = path
            .strip_prefix(&repository)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        if relative.contains("_tests.rs") || relative.ends_with("/tests.rs") {
            return;
        }
        checked += 1;
        for (line, text) in production_names(source, &relative) {
            if let Some(asset) = framework_keys.get(literal_text(&text)) {
                offenders.push(format!(
                    "  {relative}:{line}: {text} <- declared by `{asset}`"
                ));
            }
        }
    });
    assert!(checked > 50, "the sweep checked only {checked} files");
    assert!(
        offenders.is_empty(),
        "production Rust spells attribute keys that only a framework's asset declares. Move the reading into \
         the asset, or into a shared vocabulary asset if it is not one framework's:\n{}",
        offenders.join("\n")
    );
}
