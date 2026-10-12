/// **A declaration the engine would ignore is refused** - cycle 2's compile-time findings, one probe each.
#[test]
fn a_declaration_the_engine_would_ignore_is_refused() {
    // `sections` splits raw text, so any other `parse` is ignored.
    let sections = |parse: &str| {
        probe_compile(&format!(
            r#"{{"id":"t","messages":[{{"id":"t.s","read":{{"attribute":"x"}}{parse},"emit":"message",
                "priority":1,"sections":{{"split_on":"|","routes":[{{"id":"r","role":"system"}}]}}}}]}}"#
        ))
    };
    assert!(sections(r#","parse":"text""#).is_ok());
    assert!(sections(r#","parse":"json""#).is_err());

    // A wrap with neither `role` nor `role_from` emits a roleless message - allowed only where the rule's
    // name carries a declared event role, which decides the role at read time.
    let roleless = |tag: &str, roles: &str| {
        probe_compile(&format!(
            r#"{{"id":"t"{roles},"messages":[{{"id":"t.r","read":{{"attribute":"x"}},"parse":"json",
                "emit":"message","priority":1{tag},"wrap":{{"content_from":"$.text"}}}}]}}"#
        ))
    };
    assert!(roleless("", "").is_err(), "a roleless wrap is refused");
    assert!(
        roleless(
            r#","tag_as":"t.result""#,
            r#","event_roles":[{"id":"t.role","name":"t.result","role":"tool"}]"#
        )
        .is_ok(),
        "a declared event role decides it at read time"
    );
    // A `content_default` with no `content_from` is never read.
    assert!(
        probe_compile(
            r#"{"id":"t","messages":[{"id":"t.r","read":{"attribute":"x"},"parse":"json","emit":"message",
                "priority":1,"wrap":{"role":"user","content_default":"d"}}]}"#,
        )
        .is_err()
    );
    // Content construction beside `tool_calls_from` is discarded.
    assert!(
        probe_compile(
            r#"{"id":"t","messages":[{"id":"t.r","read":{"attribute":"x"},"parse":"json","emit":"message",
                "priority":1,"wrap":{"role":"assistant","content_from":"$.text",
                "tool_calls_from":{"select":"$.calls","id":"$.id","name":"$.name","arguments":"$.args",
                "on_invalid_item":"skip"}}}]}"#,
        )
        .is_err()
    );
    // A nested block constructor writing one member twice.
    assert!(
        probe_compile(
            r#"{"id":"t","messages":[{"id":"t.r","read":{"attribute":"x"},"parse":"json","emit":"message",
                "priority":1,"wrap":{"role":"assistant","block":{"type":"tool_use","content_as":"type"}}}]}"#,
        )
        .is_err()
    );
    // A reading filing what the rule's envelope wraps under a target that holds no messages.
    assert!(
        probe_compile(
            r#"{"id":"t","messages":[{"id":"t.r","read":{"attribute":"x"},"parse":"json","emit":"message",
                "priority":1,"wrap":{"role":"user","content_from":"$.text"},
                "alternatives":[{"id":"a","where":{"path":"$.tools"},"emit":"tool_definitions"}]}]}"#,
        )
        .is_err()
    );
    // A per-reading `direction` beside an aggregate is discarded.
    assert!(
        probe_compile(
            r#"{"id":"t","messages":[{"id":"t.r","read":{"attribute":"x"},"parse":"json",
                "emit":"tool_definitions","priority":1,"aggregate_into_array":true,
                "alternatives":[{"id":"a","select":"$.tools","each":true,"direction":"input"}]}]}"#,
        )
        .is_err()
    );
    // A compose sweep's `parse`/`fallback`, and a named member's `except`, are never read.
    let compose = |member: &str| {
        probe_compile(&format!(
            r#"{{"id":"t","messages":[{{"id":"t.c","emit":"message","priority":1,
                "compose":{{"tag":"t.composed","members":[{member}]}}}}]}}"#
        ))
    };
    assert!(compose(r#"{"sweep_prefix":"p."}"#).is_ok());
    assert!(compose(r#"{"sweep_prefix":"p.","parse":"json"}"#).is_err());
    assert!(compose(r#"{"as":"m","from":"k","except":["x"]}"#).is_err());

    // A span condition asserting absence and testing the value.
    let detect = |condition: &str| {
        super::detect_rules::compile(&probe_assets(&format!(
            r#"{{"id":"t","detect":[{{"id":"t.d","label":"Probe","priority":1,"where":{condition}}}]}}"#
        )))
    };
    assert!(detect(r#"{"source":"attr:k","exists":false,"equals":"x"}"#).is_err());
    assert!(
        detect(r#"{"not":{"source":"attr:k","exists":false,"equals":"x"}}"#).is_ok(),
        "under a not it is a condition"
    );
}

/// **A version suffix past u64 is a version this scheme cannot say**: saturated, two distinct bounds compared
/// equal.
#[test]
fn an_oversized_version_suffix_fails_the_parse() {
    use crate::rules::versions::Version;
    use crate::rules::versions::VersionScheme;
    assert!(Version::parse(VersionScheme::Pep440, "1.0rc1").is_some());
    assert!(Version::parse(VersionScheme::Pep440, "1.0rc99999999999999999999999").is_none());
}

/// **A structured tool output is its data, whole**: `{"type":"json","data":{"text":"answer","confidence":0.9}}`
/// is the object, not its `text` member - and a merged result keeps the `name` it pairs by and the
/// `provider_executed` that says whose execution it was.
#[test]
fn a_structured_tool_output_keeps_its_members() {
    use crate::sideml::content::create_inner_content;
    let structured =
        serde_json::json!({"type": "json", "data": {"text": "answer", "confidence": 0.9}});
    assert_eq!(
        create_inner_content(std::slice::from_ref(&structured)),
        serde_json::json!({"text": "answer", "confidence": 0.9})
    );
    let text = serde_json::json!({"type": "text", "text": "answer"});
    assert_eq!(
        create_inner_content(std::slice::from_ref(&text)),
        serde_json::json!("answer")
    );
}

/// **An aggregate honours the per-carrier ceiling too.** It is one emission, but its entries are built per
/// index, and bypassing the cap made this the one path a family's size went unreported: a family past the
/// ceiling keeps the first entries in index order and the stop is reported, exactly as the per-entry path
/// behaves.
#[test]
fn an_aggregated_family_past_the_ceiling_keeps_the_ceiling_in_index_order() {
    use crate::rules::message_rules::MessageContext;
    use sideseat_core::constants::RULE_MAX_EMISSIONS_PER_CARRIER;

    let plan = probe_compile(
        r#"{"id":"t","messages":[{"id":"t.a","read":{"indexed_family":"fam"},"emit":"tool_definitions",
            "aggregate_into_array":true,"priority":1}]}"#,
    )
    .expect("the aggregate probe compiles");
    let attrs: Vec<(String, String)> = (0..RULE_MAX_EMISSIONS_PER_CARRIER + 10)
        .map(|i| (format!("fam.{i}.name"), format!("tool{i}")))
        .collect();
    let attrs_ref: Vec<(&str, &str)> = attrs
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let attrs = probe_attrs(&attrs_ref);
    let ctx = MessageContext::for_span("span", &attrs, false);
    let emitted = plan.tool_definitions(&ctx);
    assert_eq!(emitted.len(), 1, "an aggregate is one emission");
    let array = emitted[0].value.as_array().expect("an array");
    assert_eq!(array.len(), RULE_MAX_EMISSIONS_PER_CARRIER);
    assert_eq!(
        array[0]["name"], "tool0",
        "the first entries, in index order"
    );

    // The attribute path with per-element readings honours the same ceiling - it used to return before the
    // ordinary check and keep every reading.
    let plan = probe_compile(
        r#"{"id":"t","messages":[{"id":"t.a","read":{"attribute":"x"},"parse":"json",
            "emit":"tool_definitions","aggregate_into_array":true,"priority":1,
            "alternatives":[{"id":"a","select":"$.tools","each":true}]}]}"#,
    )
    .expect("the attribute aggregate compiles");
    let tools: Vec<String> = (0..RULE_MAX_EMISSIONS_PER_CARRIER + 10)
        .map(|i| format!(r#"{{"name":"tool{i}"}}"#))
        .collect();
    let attrs = probe_attrs(&[("x", &format!(r#"{{"tools":[{}]}}"#, tools.join(",")))]);
    let ctx = MessageContext::for_span("span", &attrs, false);
    let emitted = plan.tool_definitions(&ctx);
    assert_eq!(emitted.len(), 1, "{}", emitted.len());
    let array = emitted[0].value.as_array().expect("an array");
    assert_eq!(array.len(), RULE_MAX_EMISSIONS_PER_CARRIER);
    assert_eq!(array[0]["name"], "tool0");
}

/// **`only_plain_data` beside a constructor is contradictory wherever the wrap sits.** The declaration passes
/// an already-message-shaped value through untouched, so a block or tool-call constructor beside it is
/// skipped for exactly the values it exists to recognise - refused on the rule's own wrap, on a reading's,
/// and on a fragment case's, where it used to compile.
#[test]
fn only_plain_data_beside_a_constructor_is_refused_on_every_wrap() {
    let contradictory = r#""wrap":{"role":"user","content_from":"$.c","only_plain_data":true,
            "block":{"type":"tool_result"}}"#;
    let on_rule = probe_compile(&format!(
        r#"{{"id":"t","messages":[{{"id":"t.r","read":{{"attribute":"x"}},"parse":"json",
            "emit":"message","priority":1,{contradictory}}}]}}"#
    ));
    assert!(on_rule.is_err(), "the rule's own wrap");
    let on_reading = probe_compile(&format!(
        r#"{{"id":"t","messages":[{{"id":"t.r","read":{{"attribute":"x"}},"parse":"json",
            "emit":"message","priority":1,
            "alternatives":[{{"id":"a","select":"$.m","each":true,{contradictory}}}]}}]}}"#
    ));
    assert!(on_reading.is_err(), "a reading's wrap");
    let on_fragment = probe_compile(&format!(
        r#"{{"id":"t","fragments":{{"turn":{{"doc":"d","cases":[
            {{"id":"c","where":{{"path":"$.role","kind":"string","non_empty":true}},{contradictory}}}]}}}},
            "messages":[{{"id":"t.r","read":{{"attribute":"x"}},"parse":"json","emit":"message","priority":1,
            "alternatives":[{{"id":"a","select":"$.m","each":true,"then_fragment":"t.turn"}}]}}]}}"#
    ));
    assert!(on_fragment.is_err(), "a fragment case's wrap");
}

/// **An aggregate's entries may lift from the carrier.** The aggregate path passes no rule envelope - the
/// array is wrapped once - but the carrier context still travels: a `lift` with `from: "carrier"` reads the
/// span's attributes, which used to compile and copy nothing there.
#[test]
fn an_aggregate_s_entries_still_lift_from_the_carrier() {
    use crate::rules::message_rules::MessageContext;
    let plan = probe_compile(
        r#"{"id":"t","messages":[{"id":"t.a","read":{"attribute":"x"},"parse":"json",
            "emit":"tool_definitions","aggregate_into_array":true,"priority":1,
            "alternatives":[{"id":"a","select":"$.tools","each":true,
            "lift":[{"from":"carrier","members":["origin"],"on_conflict":"keep_target"}]}]}]}"#,
    )
    .expect("the probe compiles");
    let attrs = probe_attrs(&[
        ("x", r#"{"tools": [{"name": "weather"}]}"#),
        ("origin", "the-span"),
    ]);
    let ctx = MessageContext::for_span("span", &attrs, false);
    let emitted = plan.tool_definitions(&ctx);
    assert_eq!(emitted.len(), 1, "{emitted:?}");
    assert_eq!(
        emitted[0].value[0]["origin"], "the-span",
        "the carrier lift copied the span's attribute: {:?}",
        emitted[0].value
    );
}

/// **The named-role exemption proves a role for the names the emissions land under.** A direction-only
/// declaration says nothing about the role; a rule reading several events is exempt only when every one is
/// declared; and an overriding tag replaces the event's name, so the tag alone must carry the role.
#[test]
fn the_named_role_exemption_proves_a_role_per_effective_name() {
    let probe = |head: &str, roles: &str| {
        probe_compile(&format!(
            r#"{{"id":"t"{roles},
                "message_events":[{{"id":"t.ev1","name":"t.result","doc":"d"}},
                    {{"id":"t.ev2","name":"t.other","doc":"d"}}],
                "messages":[{{"id":"t.r",{head},"parse":"json",
                "emit":"message","priority":1,"wrap":{{"content_from":"$.text"}}}}]}}"#
        ))
    };
    // A direction-only declaration exempts no envelope.
    assert!(
        probe(
            r#""source":{"event":{"names":["t.result"]}},"read":{"attribute":"x"}"#,
            r#","event_roles":[{"id":"t.role","name":"t.result","direction":"output"}]"#
        )
        .is_err(),
        "direction is not a role"
    );
    // Two events, one declared: the other's messages land roleless under their own name.
    assert!(
        probe(
            r#""source":{"event":{"names":["t.result","t.other"]}},"read":{"attribute":"x"}"#,
            r#","event_roles":[{"id":"t.role","name":"t.result","role":"tool"}]"#
        )
        .is_err(),
        "every event the rule reads must be declared"
    );
    assert!(
        probe(
            r#""source":{"event":{"names":["t.result","t.other"]}},"read":{"attribute":"x"}"#,
            r#","event_roles":[{"id":"t.role","name":"t.result","role":"tool"},
                {"id":"t.role2","name":"t.other","role":"tool"}]"#
        )
        .is_ok(),
        "all declared is exempt"
    );
    // An overriding tag is the name everything lands under: a named source event proves nothing about it.
    assert!(
        probe(
            r#""source":{"event":{"names":["t.result"]}},"read":{"attribute":"x"},"tag_as":"t.renamed""#,
            r#","event_roles":[{"id":"t.role","name":"t.result","role":"tool"}]"#
        )
        .is_err(),
        "the tag replaces the name"
    );
}

/// **A merge keeps an accepted empty.** `accept_empty` records the empty value a producer wrote as the
/// answer, with its source as the witness - and a `merge_all` of nothing else used to overwrite it with
/// `Absent`, discarding the acceptance its witness claimed.
#[test]
fn a_merge_of_nothing_else_keeps_an_accepted_empty() {
    let assets = probe_assets(
        r#"{"id":"t","span_fields":[{"id":"t.tags","target":"tags","combine":"merge_all",
            "sources":[{"id":"t.src","attribute":"tags","accept_empty":true}]}]}"#,
    );
    let plan = crate::rules::span_fields::compile(&assets).expect("the probe compiles");
    let resolved = plan.resolve("span", &probe_attrs(&[("tags", "[]")]), &[]);
    let field = resolved.first().expect("the field resolves");
    assert_ne!(
        field.reading,
        crate::rules::span_fields::Reading::Absent,
        "the accepted empty is the answer, not absence"
    );
}

/// **A list converts whole or not at all.** A member no conversion supports makes the list malformed - read
/// as a shorter list, `["a", true]` silently dropped what the producer wrote, and a wholly unsupported list
/// read as `Empty` claimed the producer wrote none.
#[test]
fn a_list_with_an_unsupported_member_is_malformed_not_shorter() {
    let assets = probe_assets(
        r#"{"id":"t","span_fields":[{"id":"t.tags","target":"tags","combine":"merge_all",
            "sources":[{"id":"t.src","json":{"attribute":"meta","path":"$.tags"}}]}]}"#,
    );
    let plan = crate::rules::span_fields::compile(&assets).expect("the probe compiles");
    let read = |raw: &str| {
        plan.resolve(
            "span",
            &probe_attrs(&[("meta", &format!(r#"{{"tags": {raw}}}"#))]),
            &[],
        )
        .into_iter()
        .next()
        .expect("the field resolves")
    };
    let mixed = read(r#"["a", true]"#);
    assert_eq!(
        mixed.reading,
        crate::rules::span_fields::Reading::Absent,
        "a mixed list contributes nothing, never a shorter list"
    );
    assert!(!mixed.refused.is_empty(), "and the refusal is reported");
    let unsupported = read(r#"[true, false]"#);
    assert_eq!(
        unsupported.reading,
        crate::rules::span_fields::Reading::Absent,
        "a wholly unsupported list is refused, not read as the producer writing none"
    );
    assert!(!unsupported.refused.is_empty());
    assert_eq!(
        read(r#"["a", 2]"#).reading,
        crate::rules::span_fields::Reading::StringList(vec!["a".into(), "2".into()]),
        "numbers stay deliberately convertible"
    );

    // The direct attribute read is held to the same rule: a JSON list converts whole or not at all, where
    // the comma fallback read `["a", true]` as two texts and split a quoted member on its own comma. A
    // bracketed value that is not JSON keeps the fallback - `[a, b]` unquoted is two texts.
    let assets = probe_assets(
        r#"{"id":"t","span_fields":[{"id":"t.tags","target":"tags","combine":"merge_all",
            "sources":[{"id":"t.src","attribute":"tags"}]}]}"#,
    );
    let plan = crate::rules::span_fields::compile(&assets).expect("the direct probe compiles");
    let direct = |raw: &str| {
        plan.resolve("span", &probe_attrs(&[("tags", raw)]), &[])
            .into_iter()
            .next()
            .expect("the field resolves")
    };
    let mixed = direct(r#"["a", true]"#);
    assert_eq!(
        mixed.reading,
        crate::rules::span_fields::Reading::Absent,
        "{mixed:?}"
    );
    assert!(!mixed.refused.is_empty());
    assert_eq!(
        direct("[a, b]").reading,
        crate::rules::span_fields::Reading::StringList(vec!["a".into(), "b".into()]),
        "an unquoted bracket list keeps the fallback"
    );
}

/// **Indexed assembly never lets two inputs land silently on one destination.** With a nested entry member,
/// the subject's own member wins over the entry's metadata of the same name; and overlay renumbering reads
/// indices canonically, so `contents.01` is not a block of the list and never renames onto `contents.1`'s
/// destination.
#[test]
fn indexed_assembly_keeps_distinct_inputs_distinct() {
    use crate::rules::message_rules::MessageContext;
    // The nested member is the message's own statement; the entry's `role` beside it is metadata.
    let plan = probe_compile(
        r#"{"id":"t","messages":[{"id":"t.m","read":{"indexed_family":"msgs","entry_member":"message"},
            "emit":"message","priority":1}]}"#,
    )
    .expect("the probe compiles");
    let attrs = probe_attrs(&[
        ("msgs.0.message.role", "user"),
        ("msgs.0.message.content", "hello"),
        ("msgs.0.role", "assistant"),
    ]);
    let ctx = MessageContext::for_span("span", &attrs, false);
    let emitted = plan.run(&ctx);
    assert_eq!(emitted.len(), 1, "{emitted:?}");
    assert_eq!(
        emitted[0].value["role"], "user",
        "the subject's own role is the message's: {:?}",
        emitted[0].value
    );

    // Renumbering after a prepended block: canonical indices move, `01` keeps its name.
    let plan = probe_compile(
        r#"{"id":"t","messages":[{"id":"t.m","read":{"indexed_family":"msgs",
            "overlay":{"from":"raw","parse":"json","select":"$","witness":{"path":"$[*].pre","exists":true},
            "when_member_prefix":"contents.","prepend_from":"$.pre"}},
            "emit":"message","priority":1}]}"#,
    )
    .expect("the overlay probe compiles");
    let attrs = probe_attrs(&[
        ("msgs.0.contents.0.text", "first"),
        ("msgs.0.contents.1.text", "second"),
        ("msgs.0.contents.01.text", "sneak"),
        ("raw", r#"[{"pre": [{"type": "thinking", "text": "t"}]}]"#),
    ]);
    let ctx = MessageContext::for_span("span", &attrs, false);
    let emitted = plan.run(&ctx);
    assert_eq!(emitted.len(), 1, "{emitted:?}");
    let value = &emitted[0].value;
    assert_eq!(value["contents.1.text"], "first", "{value}");
    assert_eq!(value["contents.2.text"], "second", "{value}");
    assert_eq!(
        value["contents.01.text"], "sneak",
        "a non-canonical index is not a block of the list and keeps its name: {value}"
    );
}

/// **Cycle 4's compile-time refusals and diagnoses, one probe each.**
#[test]
fn cycle_four_declarations_that_say_nothing_are_refused() {
    // A source-less attachment is a literal member: a `default` would never attach, a `parse` would decode
    // nothing.
    let attach = |extra: &str| {
        probe_compile(&format!(
            r#"{{"id":"t","messages":[{{"id":"t.r","read":{{"attribute":"x"}},"parse":"json",
                "emit":"message","priority":1,"wrap":{{"role":"user","content_from":"$.c",
                "attach":[{{"as":"name","value":"n"{extra}}}]}}}}]}}"#
        ))
    };
    assert!(attach("").is_ok(), "a literal member is a statement");
    assert!(
        attach(r#","default":"fallback""#).is_err(),
        "a default with no source"
    );
    assert!(
        attach(r#","parse":"json""#).is_err(),
        "a parse with no source"
    );

    // A compose member naming an output with no way to fill it.
    assert!(
        probe_compile(
            r#"{"id":"t","messages":[{"id":"t.r","read":{"attribute":"x"},"emit":"message","priority":1,
                "compose":{"tag":"probe","members":[{"as":"content"}],"trailing":{"role":"user"}}}]}"#,
        )
        .is_err(),
        "a member with no from, fallback or sweep can never emit"
    );

    // Two mutually exclusive carrier forms are diagnosed as such, not as constraining nothing.
    let error = crate::rules::carrier_rules::compile(&probe_assets(
        r#"{"id":"t","carriers":[{"id":"t.c","doc":"d",
            "match":{"event":"e","attribute":"a"},"facts":{"preset":"emission"}}]}"#,
    ))
    .expect_err("two carrier forms are refused");
    assert!(
        error.to_string().contains("mutually exclusive"),
        "the diagnosis names the defect: {error}"
    );

    // A double-quoted member name holding bracket spellings is not folded into them: the two paths differ,
    // so an `exists: true` beside `exists: false` over them is not a tautology and compiles.
    assert!(
        probe_compile(
            r##"{"id":"t","messages":[{"id":"t.r","read":{"attribute":"x"},"parse":"json","emit":"message",
                "priority":1,"wrap":{"role":"user","content_from":"$.c"},
                "alternatives":[{"id":"a","where":{"any":[
                    {"path":"$[\"['v']\"]","exists":true},
                    {"path":"$[\".v\"]","exists":false}]}}]}]}"##,
        )
        .is_ok(),
        "distinct quoted names are distinct paths"
    );
}

/// **A walk reads a message beside more state.** The node-itself clause reads each member at its own visit
/// and stops below it, so a message in one member never hides its siblings' - a `$.*` clause in `stop_on`
/// used to stop the whole node when any member matched, and the sibling's conversation was never visited.
#[test]
fn a_walk_reads_a_message_beside_more_state() {
    use crate::rules::message_rules::MessageContext;
    let plan = probe_compile(
        r#"{"id":"t","messages":[{"id":"t.w","read":{"attribute":"x"},"parse":"json","emit":"message",
            "priority":1,"walk":{"max_depth":3,"stop_on":["m"],
                "prune":[{"member":"messages","taken_by":"mm"}]},
            "also":[
                {"id":"m","where":{"all":[{"path":"$.role"},{"path":"$.content"}]},
                 "wrap":{"role_from":"$.role","content_from":"$.content"}},
                {"id":"mm","select":"$.messages[*]","where":{"path":"$.role"},
                 "wrap":{"role_from":"$.role","content_from":"$.content"}}]}]}"#,
    )
    .expect("the walk probe compiles");
    let attrs = probe_attrs(&[(
        "x",
        r#"{"direct":{"role":"user","content":"q"},
            "nested":{"messages":[{"role":"assistant","content":"a"}]}}"#,
    )]);
    let ctx = MessageContext::for_span("span", &attrs, false);
    let emitted = plan.run(&ctx);
    let contents: Vec<&str> = emitted
        .iter()
        .filter_map(|e| e.value["content"].as_str())
        .collect();
    assert!(contents.contains(&"q"), "{contents:?}");
    assert!(
        contents.contains(&"a"),
        "the sibling member's conversation is read too: {contents:?}"
    );
    assert_eq!(emitted.len(), 2, "and nothing twice: {contents:?}");

    // A conversation list beside nested state: the list is pruned once read, and the sibling is still
    // visited - a stop on the list clause stopped the whole node and the nested answer was never read.
    let attrs = probe_attrs(&[(
        "x",
        r#"{"messages":[{"role":"user","content":"q"}],
            "nested":{"messages":[{"role":"assistant","content":"a"}]}}"#,
    )]);
    let ctx = MessageContext::for_span("span", &attrs, false);
    let contents: Vec<String> = plan
        .run(&ctx)
        .iter()
        .filter_map(|e| e.value["content"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        contents,
        vec!["q".to_string(), "a".to_string()],
        "{contents:?}"
    );
}

/// **Literal text is a statement the language can make.** A message whose content is a bare string is
/// normalised like any provider's - double-encoded JSON decodes - while a rule that means *exactly these
/// characters* builds the canonical text block, which passes through verbatim: `"[]"` stays two characters.
#[test]
fn a_built_text_block_keeps_its_characters_verbatim() {
    use crate::rules::message_rules::MessageContext;
    let plan = probe_compile(
        r#"{"id":"t","messages":[{"id":"t.r","read":{"attribute":"x"},"parse":"text","emit":"message",
            "priority":1,"wrap":{"role":"user","content_from":"$",
            "block":{"type":"text","content_as":"text"}}}]}"#,
    )
    .expect("the probe compiles");
    let attrs = probe_attrs(&[("x", "[]")]);
    let ctx = MessageContext::for_span("span", &attrs, false);
    let emitted = plan.run(&ctx);
    assert_eq!(emitted.len(), 1);
    let message = crate::sideml::normalize(&emitted[0].value);
    assert_eq!(message.content.len(), 1, "{message:?}");
    let crate::sideml::ContentBlock::Text { text, .. } = &message.content[0] else {
        panic!("a text block: {:?}", message.content);
    };
    assert_eq!(text, "[]", "the characters survive normalisation");
}

/// **A named object with a producer's `function` member is still a named tool.** The named-object shape
/// excluded anything carrying `function` because the canonical nesting uses it - but one producer writes
/// `function` as a string naming the Python callable, and every one of its tools was silently unnameable, so
/// the views showed no definitions at all. The exclusion is now the canonical nesting itself: `function.name`.
#[test]
fn a_named_tool_with_a_producer_function_member_is_still_named() {
    assert_eq!(
        crate::sideml::tools::extract_tool_name(&serde_json::json!({
            "name": "get_weather",
            "description": "d",
            "parameters": {"type": "object"},
            "function": "pkg.get_weather"
        }))
        .as_deref(),
        Some("get_weather")
    );
    // The canonical nesting keeps its own clause.
    assert_eq!(
        crate::sideml::tools::extract_tool_name(&serde_json::json!({
            "type": "function",
            "function": {"name": "nested", "parameters": {}}
        }))
        .as_deref(),
        Some("nested")
    );
}

/// **The shipped LangGraph walks read a conversation beside more state, in order**, on both carriers: what a
/// clause read at a node is pruned there, never a stop for the whole node - which hid the nested state's
/// conversation - and a message in a member precedes the answer `raw` reads, as the state declares them.
#[test]
fn the_shipped_graph_walks_read_beside_a_read_member() {
    use crate::rules::message_rules::MessageContext;
    for carrier in ["input.value", "output.value"] {
        for (shape, expected) in [
            (
                r#"{"messages":[{"role":"user","content":"q"}],
                    "nested":{"messages":[{"role":"assistant","content":"a"}]}}"#,
                vec!["q", "a"],
            ),
            (
                r#"{"raw":"r","nested":{"messages":[{"role":"assistant","content":"a"}]}}"#,
                vec!["r", "a"],
            ),
            (
                r#"{"direct":{"role":"user","content":"q"},"raw":"a"}"#,
                vec!["q", "a"],
            ),
            (
                r#"{"direct":{"role":"user","content":"q"},
                    "nested":{"messages":[{"role":"assistant","content":"a"}]}}"#,
                vec!["q", "a"],
            ),
            // Document order across a node: a message nested in an earlier element precedes one a clause read
            // from a later element - emitting the node's readings before visiting any element reversed them.
            (
                r#"{"state":[{"nested":{"role":"user","content":"q"}},
                    {"role":"assistant","content":"a"}]}"#,
                vec!["q", "a"],
            ),
        ] {
            let attrs = probe_attrs(&[(carrier, shape), ("langgraph.node", "agent")]);
            let ctx = MessageContext::for_span("span", &attrs, false);
            let contents: Vec<String> = crate::rules::ruleset()
                .messages
                .run(&ctx)
                .iter()
                .filter(|e| e.rule_id.starts_with("langgraph."))
                .filter_map(|e| {
                    let content = &e.value["content"];
                    content
                        .as_str()
                        .map(str::to_string)
                        .or_else(|| content[0]["text"].as_str().map(str::to_string))
                })
                .collect();
            assert_eq!(
                contents, expected,
                "{carrier}: every message once, in the state's order, from {shape}"
            );
        }
    }
}

/// **`prune_read_by` names clauses of its own rule.** An id nothing declares never reads a member, so the
/// prune would never happen - a misspelling indistinguishable from a deliberate "prune nothing".
#[test]
fn a_walk_prunes_only_by_a_clause_it_declares() {
    let walk = |prune_read_by: &str| {
        probe_compile(&format!(
            r#"{{"id":"t","messages":[{{"id":"t.w","read":{{"attribute":"x"}},"parse":"json",
                "emit":"message","priority":1,
                "walk":{{"max_depth":2,"stop_on":["m"],"prune_read_by":["{prune_read_by}"]}},
                "also":[{{"id":"m","where":{{"path":"$.role"}},
                    "wrap":{{"role_from":"$.role","content_from":"$.content"}}}},
                    {{"id":"any","select":"$.*","where":{{"path":"$.role"}},
                    "wrap":{{"role_from":"$.role","content_from":"$.content"}}}}]}}]}}"#
        ))
    };
    assert!(walk("any").is_ok(), "a declared clause");
    assert!(walk("anything").is_err(), "an undeclared one");

    // A pruning clause must read its members whole: one that descends into a member reads part of it, and
    // pruning the member hides the parts it did not read - `{"box": {"picked": q, "other": a}}` lost `a`.
    let partial = probe_compile(
        r#"{"id":"t","messages":[{"id":"t.w","read":{"attribute":"x"},"parse":"json",
            "emit":"message","priority":1,
            "walk":{"max_depth":3,"stop_on":["m"],"prune_read_by":["d"]},
            "also":[{"id":"m","where":{"path":"$.role"},
                "wrap":{"role_from":"$.role","content_from":"$.content"}},
                {"id":"d","select":"$.*","descend":"picked","where":{"path":"$.role"},
                "wrap":{"role_from":"$.role","content_from":"$.content"}}]}]}"#,
    );
    assert!(partial.is_err(), "a descending pruning clause is refused");
    // The same through a fragment case it hands its elements to.
    let through_fragment = probe_compile(
        r#"{"id":"t","fragments":{"pick":{"doc":"d","cases":[
            {"id":"c","descend":"picked","where":{"path":"$.role"},
             "wrap":{"role_from":"$.role","content_from":"$.content"}}]}},
            "messages":[{"id":"t.w","read":{"attribute":"x"},"parse":"json",
            "emit":"message","priority":1,
            "walk":{"max_depth":3,"stop_on":["m"],"prune_read_by":["d"]},
            "also":[{"id":"m","where":{"path":"$.role"},
                "wrap":{"role_from":"$.role","content_from":"$.content"}},
                {"id":"d","select":"$.*","then_fragment":"t.pick"}]}]}"#,
    );
    assert!(
        through_fragment.is_err(),
        "a descending fragment case is refused too"
    );

    // A case selecting inside the element, or iterating it, reads only some of its members.
    for case in [
        r#"{"id":"c","select":"$.picked","where":{"path":"$.role"},
            "wrap":{"role_from":"$.role","content_from":"$.content"}}"#,
        r#"{"id":"c","select":"$.items","each":true,"where":{"path":"$.role"},
            "wrap":{"role_from":"$.role","content_from":"$.content"}}"#,
    ] {
        let refused = probe_compile(&format!(
            r#"{{"id":"t","fragments":{{"pick":{{"doc":"d","cases":[{case}]}}}},
                "messages":[{{"id":"t.w","read":{{"attribute":"x"}},"parse":"json",
                "emit":"message","priority":1,
                "walk":{{"max_depth":3,"stop_on":["m"],"prune_read_by":["d"]}},
                "also":[{{"id":"m","where":{{"path":"$.role"}},
                    "wrap":{{"role_from":"$.role","content_from":"$.content"}}}},
                    {{"id":"d","select":"$.*","then_fragment":"t.pick"}}]}}]}}"#
        ));
        assert!(
            refused.is_err(),
            "a case reading inside its element is refused: {case}"
        );
    }
}

/// **A walk's readings follow the payload's document order, whatever clause read them.** A clause selecting a
/// nested member, or iterating a root array, read its messages at the node, ahead of earlier members a
/// descent reads - each source is placed where the payload has it.
#[test]
fn a_walk_orders_every_reading_by_its_source() {
    use crate::rules::message_rules::MessageContext;
    let run = |rule: &str, payload: &str| -> Vec<String> {
        let plan = probe_compile(&format!(
            r#"{{"id":"t","messages":[{{"id":"t.w","read":{{"attribute":"x"}},"parse":"json",
                "emit":"message","priority":1,{rule}}}]}}"#
        ))
        .expect("the probe compiles");
        let attrs = probe_attrs(&[("x", payload)]);
        let ctx = MessageContext::for_span("span", &attrs, false);
        plan.run(&ctx)
            .iter()
            .filter_map(|e| e.value["content"].as_str().map(str::to_string))
            .collect()
    };
    let message = r#"{"id":"m","where":{"all":[{"path":"$.role"},{"path":"$.content"}]},
        "wrap":{"role_from":"$.role","content_from":"$.content"}}"#;
    // A clause selecting a nested member.
    assert_eq!(
        run(
            &format!(
                r#""walk":{{"max_depth":3,"stop_on":["m"]}},"also":[{message},
                {{"id":"later","select":"$.box.later","where":{{"path":"$.role"}},
                 "wrap":{{"role_from":"$.role","content_from":"$.content"}}}}]"#
            ),
            r#"{"box":{"earlier":{"role":"user","content":"q"},
                "later":{"role":"assistant","content":"a"}}}"#,
        ),
        vec!["q", "a", "a"],
        "the earlier member's message first: the nested selection is placed at its member (the descent \
         finds `a` again under `box`, which this probe's own declaration does not prune)"
    );
    // A root array's elements iterated by a clause the walk prunes by.
    assert_eq!(
        run(
            &format!(
                r#""walk":{{"max_depth":3,"stop_on":["m"],"prune_read_by":["each"]}},"also":[{message},
                {{"id":"each","select":"$","each":true,"where":{{"all":[{{"path":"$.role"}},
                    {{"path":"$.content"}}]}},
                 "wrap":{{"role_from":"$.role","content_from":"$.content"}}}}]"#
            ),
            r#"[{"nested":{"role":"user","content":"q"}},{"role":"assistant","content":"a"}]"#,
        ),
        vec!["q", "a"]
    );
}
