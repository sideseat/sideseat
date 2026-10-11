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
