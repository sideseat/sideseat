/// A branch leaf keeps its own gate, and one runtime signal can imply another across dimensions.
///
/// Two shapes flattening got wrong. A leaf's condition was replaced by its parent's, so an **ungated** parent
/// made a `when`-gated leaf look unconditional - and that leaf then convicted a rule live on spans its gate
/// excludes. And a `fallback_if_primary_empty` leaf reads only where every primary came up empty, which is a
/// payload condition nothing here can compare, so it must not convict anything either.
///
/// The cross-dimension case is the mirror: `attr_equals(k, v)` cannot hold unless `attr_exists(k)` does, so a
/// wide `attr_exists` rule at the earlier rank really does suppress a narrow `attr_equals` one - and comparing
/// each dimension only with itself saw two unrelated gates.
#[test]
fn a_branch_leaf_keeps_its_own_gate() {
    let accepted = [
        (
            "a gated leaf under an ungated parent, beside a rule its gate excludes",
            r#"{"id": "t", "doc": "d", "messages": [{"id": "a", "doc": "d", "priority": 1, "branch_set": {"primary": [{"id": "a.1", "doc": "d", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "where": {"source": "attr:only.a", "exists": true}, "tag_as": "a.tag"}]}}, {"id": "b", "doc": "d", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "where": {"source": "attr:only.b", "exists": true}, "tag_as": "b.tag", "priority": 2}]}"#,
        ),
        (
            "a fallback-group leaf, which reads only where the primaries found nothing",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","priority":1,
                 "branch_set":{"primary":[
                    {"id":"a.1","doc":"d","read":{"attribute":"p"},"parse":"json","emit":"message",
                     "tag_as":"a.1.tag"}],
                  "fallback_if_primary_empty":[
                    {"id":"a.2","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                     "tag_as":"a.2.tag"}]}},
                {"id":"b","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "tag_as":"b.tag","priority":2}]}"#,
        ),
    ];
    for (what, asset) in accepted {
        let sources =
            std::collections::BTreeMap::from([("t.json".to_string(), asset.as_bytes().to_vec())]);
        assert!(
            compile(&ParsedAssets::parse(&sources).expect("the probe assets parse")).is_ok(),
            "should have been accepted: {what} - {:?}",
            compile(&ParsedAssets::parse(&sources).expect("the probe assets parse")).err()
        );
    }

    let refused = [
        (
            "an `attr_exists` rule ahead of an `attr_equals` one on the same key",
            r#"{"id": "t", "doc": "d", "messages": [{"id": "a", "doc": "d", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "where": {"source": "attr:marker", "exists": true}, "tag_as": "a.tag", "priority": 1}, {"id": "b", "doc": "d", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "where": {"source": "attr:marker", "equals": "yes"}, "tag_as": "b.tag", "priority": 2}]}"#,
        ),
        (
            "an `attr_prefix` rule ahead of an exact key beneath that prefix",
            r#"{"id": "t", "doc": "d", "messages": [{"id": "a", "doc": "d", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "where": {"source": "attr_keys", "starts_with": "dialect."}, "tag_as": "a.tag", "priority": 1}, {"id": "b", "doc": "d", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "where": {"source": "attr:dialect.node", "exists": true}, "tag_as": "b.tag", "priority": 2}]}"#,
        ),
        (
            "an ungated leaf under an ungated parent, which really does suppress",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","priority":1,
                 "branch_set":{"primary":[
                    {"id":"a.1","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                     "tag_as":"a.tag"}]}},
                {"id":"b","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "tag_as":"b.tag","priority":2}]}"#,
        ),
    ];
    for (what, asset) in refused {
        let sources =
            std::collections::BTreeMap::from([("t.json".to_string(), asset.as_bytes().to_vec())]);
        assert!(
            compile(&ParsedAssets::parse(&sources).expect("the probe assets parse")).is_err(),
            "should have been refused: {what}"
        );
    }
}

/// A gate is a **disjunction**, so one gate can hold wherever another does without being the same gate.
///
/// Equality was the wrong relation. `attr_exists: ["a", "b"]` holds everywhere `attr_exists: ["a"]` does, so
/// the wider-gated rule suppresses the narrower one whenever it comes first - and their declared forms differ,
/// which is all an equality test could see. The relation is directional and rank-aware: the question is
/// whether the *earlier* rule leaves anything for the later one.
///
/// It also has to be two facets rather than one label. A rule can be gated **and** payload-narrowed, and
/// folding them together made "same gate, mutually exclusive payloads" - a working pair - look dead.
#[test]
fn a_wider_gate_suppresses_a_narrower_one() {
    let refused = [
        (
            // This was an **accepted** case, on the claim that "every emission owns [its compose tag]". It does
            // not: a synthetic tag is the name the engine gives an assembled result, not something the span
            // carried. Owning it let one compose suppress another that read entirely different carriers, using
            // a name no producer wrote - so the tag left `owns`, and the emitted-carrier ambiguity check then
            // says what is actually wrong here: two emissions under one tag that **nothing resolves**, so both
            // survive and nothing downstream tells them apart. Which is the same reasoning that check already
            // applies to a gated `tag_as` beside an ungated one.
            "two composes sharing a tag while reading different carriers",
            r#"{"id": "t", "doc": "d", "messages": [{"id": "a", "doc": "d", "priority": 1, "where": {"source": "attr:m.a", "exists": true}, "compose": {"tag": "shared", "members": [{"as": "content", "from_any_of": ["k1"], "parse": "text"}]}}, {"id": "b", "doc": "d", "priority": 2, "where": {"source": "attr:m.b", "exists": true}, "compose": {"tag": "shared", "members": [{"as": "content", "from_any_of": ["k2"], "parse": "text"}]}}]}"#,
        ),
        (
            "a superset gate at the earlier rank",
            r#"{"id": "t", "doc": "d", "messages": [{"id": "a", "doc": "d", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "where": {"any": [{"source": "attr:a", "exists": true}, {"source": "attr:b", "exists": true}]}, "tag_as": "a.tag", "priority": 1}, {"id": "b", "doc": "d", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "where": {"source": "attr:a", "exists": true}, "tag_as": "b.tag", "priority": 2}]}"#,
        ),
        (
            "a shorter span-name prefix, which covers every longer one",
            r#"{"id": "t", "doc": "d", "messages": [{"id": "a", "doc": "d", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "where": {"source": "span_name", "starts_with": "chat"}, "tag_as": "a.tag", "priority": 1}, {"id": "b", "doc": "d", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "where": {"source": "span_name", "starts_with": "chat.completions"}, "tag_as": "b.tag", "priority": 2}]}"#,
        ),
        (
            "a root `starts_with` beside its own negation, which holds of every value",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message","priority":1,
                 "alternatives":[{"id":"probe.alt","require":{"any":[
                    {"starts_with":"a"},{"lacks_prefix":"a"}]},
                  "wrap":{"role":"user","content_from_any_of":["$.content"]}}]},
                {"id":"b","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "priority":2}]}"#,
        ),
        (
            "`elements` beside an explicitly false aggregate, which it also ignores",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "aggregate_into_array":false,"priority":1,
                 "elements":{"passes":[{"id":"probe.pass","tag_from":"$.name"}]}}]}"#,
        ),
    ];
    for (what, asset) in refused {
        let sources =
            std::collections::BTreeMap::from([("t.json".to_string(), asset.as_bytes().to_vec())]);
        assert!(
            compile(&ParsedAssets::parse(&sources).expect("the probe assets parse")).is_err(),
            "should have been refused: {what}"
        );
    }

    let accepted = [
        (
            "the subset gate at the earlier rank, which leaves spans for the wider one",
            r#"{"id": "t", "doc": "d", "messages": [{"id": "a", "doc": "d", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "where": {"source": "attr:a", "exists": true}, "tag_as": "a.tag", "priority": 1}, {"id": "b", "doc": "d", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "where": {"any": [{"source": "attr:a", "exists": true}, {"source": "attr:b", "exists": true}]}, "tag_as": "b.tag", "priority": 2}]}"#,
        ),
        (
            "the same gate, where the earlier rule may read nothing on a span it runs on",
            r#"{"id": "t", "doc": "d", "messages": [{"id": "a", "doc": "d", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "where": {"source": "attr:marker", "exists": true}, "tag_as": "a.tag", "priority": 1, "alternatives": [{"id": "probe.alt", "require": {"any": [{"path": "$.kind", "one_of": ["first"]}]}, "wrap": {"role": "user", "content_from_any_of": ["$.content"]}}]}, {"id": "b", "doc": "d", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "where": {"source": "attr:marker", "exists": true}, "tag_as": "b.tag", "priority": 2}]}"#,
        ),
        (
            "a reading narrowed only by `require_parent`, which narrows as `require` does",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "tag_as":"a.tag","priority":1,
                 "alternatives":[{"id":"probe.alt","require_parent":{"any":[{"path":"$.kind","one_of":["k"]}]},
                  "wrap":{"role":"user","content_from_any_of":["$.content"]}}]},
                {"id":"b","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "tag_as":"b.tag","priority":2}]}"#,
        ),
        (
            "a single-spelling `first_present`, which is not a choice",
            r#"{"id": "t", "doc": "d", "messages": [{"id": "a", "doc": "d", "read": {"first_present": ["only"]}, "parse": "json", "emit": "message", "tag_as": "shared", "where": {"source": "attr:m", "exists": true}, "priority": 1}, {"id": "b", "doc": "d", "read": {"attribute": "only"}, "parse": "json", "emit": "message", "tag_as": "shared", "priority": 2}]}"#,
        ),
    ];
    for (what, asset) in accepted {
        let sources =
            std::collections::BTreeMap::from([("t.json".to_string(), asset.as_bytes().to_vec())]);
        assert!(
            compile(&ParsedAssets::parse(&sources).expect("the probe assets parse")).is_ok(),
            "should have been accepted: {what} - {:?}",
            compile(&ParsedAssets::parse(&sources).expect("the probe assets parse")).err()
        );
    }
}

/// A conditional claim is conditional about **one carrier**, not about every carrier its rule reads.
///
/// Two shapes, each a rule that would be permanently dead while compilation called the pair conditional:
///
/// - a witnessed overlay made its whole rule conditional, so a second rule reading one of the *family's* own
///   keys was accepted - and on a span carrying the family and no side payload, the family rule reads and
///   owns that key on every span, so the second could never emit;
/// - "every alternative carries a `require`" ignored `also` and `fallback`, which `all_readings` also emits
///   through - so a rule with one required alternative and an unconditional fallback claimed its carrier
///   always and still counted as conditional.
///
/// The accepted halves are what keep this from being an over-refusal: the same overlay against a rule reading
/// the *side payload* is a genuine pair, and so is a rule whose every reading is required.
#[test]
fn conditionality_is_a_property_of_the_carrier_not_of_the_rule() {
    // Rule A reads family `f` always and `side` only where the witness holds; rule B reads `f.0.content`.
    let family_key_conflict = r#"{"id":"t","doc":"d","messages":[
        {"id":"a","doc":"d","priority":1,
         "read":{"indexed_family":"f","overlay":{
            "from":"side","parse":"json","select_any_of":["$"],
            "witness":{"any":[{"path":"$[*].id","kind":"array"}]},
            "when_member_prefix":"contents.","content_any_of":["$.content"],
            "as_member":"content"}},
         "emit":"message"},
        {"id":"b","doc":"d","read":{"attribute":"f.0.content"},"parse":"json","emit":"message",
         "tag_as":"b.own.tag","priority":2}]}"#;
    // The same overlay, against a rule reading the payload the overlay joins against. Genuinely conditional:
    // A consumes `side` only where the witness holds, and yields it elsewhere.
    let side_payload_pair = r#"{"id":"t","doc":"d","messages":[
        {"id":"a","doc":"d","priority":1,
         "read":{"indexed_family":"f","overlay":{
            "from":"side","parse":"json","select_any_of":["$"],
            "witness":{"any":[{"path":"$[*].id","kind":"array"}]},
            "when_member_prefix":"contents.","content_any_of":["$.content"],
            "as_member":"content"}},
         "emit":"message"},
        {"id":"b","doc":"d","read":{"attribute":"side"},"parse":"json","emit":"message",
         "priority":2}]}"#;
    // Rule A reads `x` through one required alternative *and* an unconditional fallback, so it claims `x` on
    // every span; rule B reads `x` too.
    let unconditional_fallback = r#"{"id":"t","doc":"d","messages":[
        {"id":"a","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message","priority":1,
         "alternatives":[{"id":"probe.alt","require":{"any":[{"path":"$.marker","exists":true}]},
                          "wrap":{"role":"user","content_from_any_of":["$.content"]}}],
         "fallback":[{"id":"probe.fallback","wrap":{"role":"user","content_from_any_of":["$.content"]}}]},
        {"id":"b","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
         "priority":2}]}"#;
    // The same rule with no unconditional path: every reading is required, so it yields on a payload none
    // recognises and the pair is genuine.
    let all_readings_required = r#"{"id":"t","doc":"d","messages":[
        {"id":"a","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message","priority":1,
         "alternatives":[{"id":"probe.alt","require":{"any":[{"path":"$.marker","exists":true}]},
                          "wrap":{"role":"user","content_from_any_of":["$.content"]}}]},
        {"id":"b","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
         "priority":2}]}"#;

    for (what, asset) in [
        (
            "a family key another rule reads, beside a witnessed overlay",
            family_key_conflict,
        ),
        (
            "an unconditional fallback beside a required alternative",
            unconditional_fallback,
        ),
    ] {
        let sources =
            std::collections::BTreeMap::from([("t.json".to_string(), asset.as_bytes().to_vec())]);
        assert!(
            compile(&ParsedAssets::parse(&sources).expect("the probe assets parse")).is_err(),
            "the second rule is permanently dead and should have been refused: {what}"
        );
    }
    for (what, asset) in [
        (
            "the overlay's own payload, which it reads only where witnessed",
            side_payload_pair,
        ),
        (
            "a rule whose every reading is required",
            all_readings_required,
        ),
    ] {
        let sources =
            std::collections::BTreeMap::from([("t.json".to_string(), asset.as_bytes().to_vec())]);
        assert!(
            compile(&ParsedAssets::parse(&sources).expect("the probe assets parse")).is_ok(),
            "this claim really is conditional and must be permitted: {what} - {:?}",
            compile(&ParsedAssets::parse(&sources).expect("the probe assets parse")).err()
        );
    }
}

/// An event rule's gate asks about the **span**, and its `read` draws from the **event**.
///
/// Both dimensions were unavailable at that entry point: the span name was passed as `""`, so a `span_name`
/// gate compiled and could only ever fail, and the event's own attribute map stood in for the span's, so an
/// `attr_exists` gate asked about the wrong map. Neither was an error - the rule simply never fired, which is
/// the shape this whole engine exists to make impossible.
///
/// One dimension per rule, because gate signals are ORed: a rule naming two would be satisfied by either and
/// could not tell which one the engine actually consulted.
#[test]
fn an_event_rules_gate_asks_about_its_span_not_about_the_event() {
    let asset = br#"{"id": "t", "doc": "d", "message_events": [{"id": "probe.some_event", "name": "some.event", "doc": "a probe event"}], "messages": [{"id": "by_name", "doc": "d", "source": {"event": {"names": ["some.event"]}}, "where": {"source": "span_name", "starts_with": "chat "}, "read": {"attribute": "payload"}, "parse": "json", "emit": "message", "priority": 1}, {"id": "by_attr", "doc": "d", "source": {"event": {"names": ["some.event"]}}, "where": {"source": "attr:framework.marker", "exists": true}, "read": {"attribute": "other"}, "parse": "json", "emit": "message", "priority": 2}]}"#;
    let sources = std::collections::BTreeMap::from([("t.json".to_string(), asset.to_vec())]);
    let plan = compile(&ParsedAssets::parse(&sources).expect("the probe assets parse"))
        .expect("an event rule may be gated on the span that carries it");

    let event_attrs = rule_attrs(&[
        ("payload", r#"{"role":"user","content":"q"}"#),
        ("other", r#"{"role":"user","content":"r"}"#),
    ]);
    let marked = rule_attrs(&[("framework.marker", "yes")]);
    let bare = rule_attrs(&[]);
    let fired = |span_name: &str, span_attrs: &HashMap<String, String>| -> Vec<String> {
        plan.from_event(
            "some.event",
            &event_attrs,
            span_name,
            None,
            span_attrs,
            false,
        )
        .emissions
        .iter()
        .map(|e| e.rule_id.to_string())
        .collect()
    };

    assert_eq!(
        fired("chat model", &bare),
        vec!["by_name"],
        "the span name is the real one, so the name-gated rule reads its event"
    );
    assert_eq!(
        fired("tool execution", &marked),
        vec!["by_attr"],
        "the attribute gate asks about the span's map, which carries the marker"
    );
    assert!(
        fired("tool execution", &bare).is_empty(),
        "neither gate holds of this span"
    );
    // And the gate is not satisfiable from the event's own map, which used to stand in for the span's.
    let self_marked = rule_attrs(&[
        ("payload", r#"{"role":"user","content":"q"}"#),
        ("other", r#"{"role":"user","content":"r"}"#),
        ("framework.marker", "yes"),
    ]);
    assert!(
        plan.from_event(
            "some.event",
            &self_marked,
            "tool execution",
            None,
            &bare,
            false
        )
        .emissions
        .is_empty(),
        "an event carrying the marker is not a span carrying it"
    );
}

/// The declared evidence answers "is this a tool running" exactly as the retired list of branches did.
///
/// Written as the table it replaced, so a dialect whose signal is deleted from an asset fails here rather
/// than silently letting rules read a tool span as a model's turn.
#[test]
fn the_declared_span_facts_reproduce_the_legacy_tool_span_test() {
    /// A span's attributes, and whether the branches this replaced called it a tool execution.
    type ToolSpanCase = (&'static str, Vec<(&'static str, &'static str)>, bool);
    // Each case, and what the branches it replaced answered.
    let cases: Vec<ToolSpanCase> = vec![
        ("nothing at all", vec![], false),
        (
            "the operation the span reports",
            vec![("gen_ai.operation.name", "execute_tool")],
            true,
        ),
        (
            "a different operation",
            vec![("gen_ai.operation.name", "chat")],
            false,
        ),
        (
            "a span kind, in the capitals the convention writes",
            vec![("openinference.span.kind", "TOOL")],
            true,
        ),
        (
            "the same, lowercase",
            vec![("openinference.span.kind", "tool")],
            true,
        ),
        (
            "another kind",
            vec![("openinference.span.kind", "LLM")],
            false,
        ),
        // The conjunction, both ways round: a name alone sits on a model span that mentions a tool.
        (
            "a tool name alone",
            vec![("gen_ai.tool.name", "search")],
            false,
        ),
        (
            "a call id alone",
            vec![("gen_ai.tool.call.id", "c1")],
            false,
        ),
        (
            "a tool name and a call id",
            vec![
                ("gen_ai.tool.name", "search"),
                ("gen_ai.tool.call.id", "c1"),
            ],
            true,
        ),
        (
            "a tool response, which only the span that ran it records",
            vec![("gcp.vertex.agent.tool_response", "{}")],
            true,
        ),
        (
            "one dialect's spelling of the pair",
            vec![("ai.toolCall.name", "search"), ("ai.toolCall.id", "c1")],
            true,
        ),
        ("half of it", vec![("ai.toolCall.name", "search")], false),
    ];
    for (what, attrs, expected) in cases {
        let attrs = make_attrs(&attrs);
        assert_eq!(
            is_tool_execution_span(&attrs),
            expected,
            "{what}: the declared evidence disagrees with the branches it replaced"
        );
    }
}

/// Two dialects that both read one carrier must not both emit from it.
///
/// The ranks exist to decide which is tried first, and the ownership check permits a collision only when a
/// condition separates them - so the *first* rule to read a carrier owns it, as one extractor claiming a
/// carrier used to mean. Without that, consolidating extractors into one entry turned "the earlier one
/// won" into "both emit", which a reader sees as the same turn twice.
#[test]
fn one_carrier_is_read_by_one_rule() {
    let attrs = make_attrs(&[
        ("langgraph.node", "agent"),
        ("message", r#"{"type":"HumanMessage","content":"hi"}"#),
    ]);
    let mut messages = Vec::new();
    let mut tools = Vec::new();
    try_declared_rules(
        &mut messages,
        &mut tools,
        &attrs,
        "span",
        Utc::now(),
        &mut std::collections::HashSet::new(),
    );
    let from_message: Vec<&RawMessage> = messages
        .iter()
        .filter(|m| matches!(&m.source, MessageSource::Attribute { key, .. } if key == "message"))
        .collect();
    assert_eq!(
        from_message.len(),
        1,
        "the `message` carrier produced {} observations: {:?}",
        from_message.len(),
        from_message
            .iter()
            .map(|m| m.content.to_string())
            .collect::<Vec<_>>()
    );
}

/// A declared tool definition is read on every span, including one that is a tool running.
///
/// `reads_tool_spans` asks whether a rule may read a tool span **as a conversation** - its messages are
/// that tool's input and result, not a model's turn. That is not a question about tool *definitions*, and
/// the path reading them has always run on every span. Routing them through the message evaluator applied
/// the message gate, so a framework's tool list vanished on any span that also carried a tool-execution
/// signal.
#[test]
fn declared_tool_definitions_survive_a_tool_execution_span() {
    let agents = r#"[{"role":"Weather Expert","tools_names":["get_weather"]}]"#;
    let attrs = make_attrs(&[
        ("crew_agents", agents),
        ("crew_key", "k"),
        // Any declared tool-execution signal.
        ("gen_ai.operation.name", "execute_tool"),
    ]);
    assert!(
        is_tool_execution_span(&attrs),
        "the case has to be a tool span for this to mean anything"
    );
    let (tool_defs, _) = extract_tool_definitions("", &attrs, Utc::now());
    assert_eq!(
        tool_defs.len(),
        1,
        "a tool span lost its declared tool definitions"
    );
}

#[test]
fn openinference_tool_output_is_a_tool_result() {
    let attrs = make_attrs(&[
        ("openinference.span.kind", "TOOL"),
        ("tool.id", "call-weather"),
        ("tool.name", "get_weather"),
        ("tool.parameters", r#"{"location":"Paris"}"#),
        ("input.value", r#"{"location":"Paris"}"#),
        ("output.value", "Sunny, 22°C, light breeze in Paris."),
        ("output.mime_type", "text/plain"),
    ]);
    assert!(
        is_tool_execution_span(&attrs),
        "the OpenInference span kind must establish the tool-execution gate"
    );

    let mut messages = Vec::new();
    let mut tools = Vec::new();
    extract_messages_from_context(
        &mut messages,
        &mut tools,
        SpanExtraction {
            name: "weather_assistant.get_weather",
            attrs: &attrs,
            scope_name: Some("openinference.instrumentation.generic"),
            is_tool_span: true,
        },
        Utc::now(),
        ExtractionMode::PerCarrier,
    );

    assert_eq!(
        messages.len(),
        1,
        "the execution output must not be dropped"
    );
    assert!(matches!(
        &messages[0].source,
        MessageSource::Attribute { key, .. } if key == "output.value"
    ));
    assert_eq!(messages[0].content["role"].as_str(), Some("tool"));
    assert_eq!(
        messages[0].content["content"][0]["type"].as_str(),
        Some("tool_result")
    );
    assert_eq!(
        messages[0].content["content"][0]["name"].as_str(),
        Some("get_weather")
    );
    assert_eq!(
        messages[0].content["content"][0]["tool_use_id"].as_str(),
        Some("call-weather")
    );
    assert_eq!(
        messages[0].content["content"][0]["content"].as_str(),
        Some("Sunny, 22°C, light breeze in Paris.")
    );
}

/// OpenTelemetry's Google Gen AI instrumentation writes a called function's arguments as
/// `code.function.parameters.<name>.value` beside each argument's Python type. Read as written, the
/// call's input matched no copy of the same call on the model span, so a streamed tool loop showed
/// the call twice; the arguments are the members that pattern picks out.
#[test]
fn google_genai_flattened_tool_arguments_are_the_arguments_the_model_sent() {
    let flattened = r#"{"code.function.parameters.city.type": "str", "code.function.parameters.city.value": "Rome", "code.function.parameters.days.type": "int", "code.function.parameters.days.value": 1}"#;
    let extract = |scope: &str, arguments: &str| {
        let attrs = make_attrs(&[
            ("gen_ai.operation.name", "execute_tool"),
            ("gen_ai.tool.name", "get_weather"),
            ("gen_ai.tool.call.arguments", arguments),
        ]);
        let mut messages = Vec::new();
        let mut tools = Vec::new();
        extract_messages_from_context(
            &mut messages,
            &mut tools,
            SpanExtraction {
                name: "execute_tool get_weather",
                attrs: &attrs,
                scope_name: Some(scope),
                is_tool_span: true,
            },
            Utc::now(),
            ExtractionMode::PerCarrier,
        );
        assert_eq!(messages.len(), 1);
        messages[0].content["content"][0].clone()
    };

    let call = extract("opentelemetry.instrumentation.google_genai", flattened);
    assert_eq!(call["type"].as_str(), Some("tool_use"));
    assert_eq!(call["name"].as_str(), Some("get_weather"));
    assert_eq!(
        call["input"],
        serde_json::json!({"city": "Rome", "days": 1})
    );

    // Arguments in the conventions' own shape fall through to the generic rule unchanged.
    let plain = extract(
        "opentelemetry.instrumentation.google_genai",
        r#"{"city": "Rome"}"#,
    );
    assert_eq!(plain["input"], serde_json::json!({"city": "Rome"}));
    // Another instrumentation's flattened-looking keys are its own business.
    let other = extract("another.instrumentation", flattened);
    assert_eq!(
        other["input"]["code.function.parameters.city.value"].as_str(),
        Some("Rome")
    );
}
/// A carrier whose whole content is a tool list is not also a conversation.
///
/// The repr grammar runs on the metadata axis, outside claiming, which is right - a tool definition is not
/// a message. But it still *reads* a carrier, and when that carrier holds nothing but the tool list, the
/// generic fallback would go on to present the same Python `repr` as a user turn. So a repr rule that
/// recognised the carrier claims it on the message axis too, saying "mine, and no conversation".
#[test]
fn a_carrier_holding_only_a_tool_list_is_not_read_as_a_conversation() {
    let repr = r#"{"tools": ["CrewStructuredTool(name='search', description='Tool Arguments: {\"q\": {\"type\": \"str\"}}')"]}"#;
    let attrs = make_attrs(&[("crew_key", "k"), ("input.value", repr)]);
    let (tool_defs, _) = extract_tool_definitions("", &attrs, Utc::now());
    assert_eq!(tool_defs.len(), 1, "the tool list should be read");

    let mut messages = Vec::new();
    let mut defs = Vec::new();
    extract_messages_from_attrs(
        &mut messages,
        &mut defs,
        &attrs,
        "span",
        Utc::now(),
        ExtractionMode::PerCarrier,
        is_tool_execution_span(&attrs),
    );
    let from_input: Vec<&RawMessage> = messages
        .iter()
        .filter(
            |m| matches!(&m.source, MessageSource::Attribute { key, .. } if key == "input.value"),
        )
        .collect();
    assert!(
        from_input.is_empty(),
        "the tool list was also presented as a conversation: {:?}",
        from_input
            .iter()
            .map(|m| m.content.to_string())
            .collect::<Vec<_>>()
    );
}

/// A message rule that also declares tools on the same carrier yields both.
///
/// AutoGen's logging channel carries the conversation, the reply and the tools offered, in one carrier via
/// `also`. The whole rule is on the message axis (its target is `Message`), so it runs through `run` - and
/// its tool-definition reading has to survive that path, or a framework that co-locates tools and
/// conversation loses its tools. This is the shape `is_metadata_rule` must not mishandle by routing the
/// whole rule one way.
#[test]
fn a_message_rule_may_also_emit_tool_definitions() {
    let body = r#"{"type": "LLMCall", "messages": [{"role": "user", "content": "q"}], "response": {"content": "a"}, "tools": [{"name": "search"}]}"#;
    let attrs = make_attrs(&[("body", body)]);
    // The conversation is read on the message axis, the tools on the metadata axis - two entry points,
    // routed by each emission's own target, from the one rule.
    let mut messages = Vec::new();
    let mut unused = Vec::new();
    try_declared_rules(
        &mut messages,
        &mut unused,
        &attrs,
        "span",
        Utc::now(),
        &mut std::collections::HashSet::new(),
    );
    assert!(!messages.is_empty(), "the conversation and reply were lost");
    let (tools, _) = extract_tool_definitions("", &attrs, Utc::now());
    assert_eq!(
        tools.len(),
        1,
        "the tool list co-located with the conversation was lost: {tools:?}"
    );
}

/// The tools-only claim must not swallow a conversation that sits beside the tools.
///
/// For `{tools: [...], messages: [...]}`, the tool parser reads the list on the metadata axis,
/// but the carrier also holds a real conversation, so claiming it as "mine and empty" loses the turn.
/// Excluding only `context` did not establish "holds only tools" - `messages` is a conversation too.
#[test]
fn a_tools_list_beside_a_conversation_does_not_claim_the_carrier() {
    let mixed = r#"{"tools": ["search"], "messages": [{"role": "user", "content": "hello"}]}"#;
    let attrs = make_attrs(&[("crew_key", "k"), ("input.value", mixed)]);
    let mut messages = Vec::new();
    let mut unused = Vec::new();
    extract_messages_from_attrs(
        &mut messages,
        &mut unused,
        &attrs,
        "span",
        Utc::now(),
        ExtractionMode::PerCarrier,
        is_tool_execution_span(&attrs),
    );
    let from_input: Vec<&RawMessage> = messages
        .iter()
        .filter(
            |m| matches!(&m.source, MessageSource::Attribute { key, .. } if key == "input.value"),
        )
        .collect();
    assert!(
        !from_input.is_empty(),
        "the conversation beside the tools was claimed away"
    );
}

/// A present-but-empty declaration wrapper has declared no tools, and is not itself a tool.
///
/// The retired code chose the wrapper member by *presence* and extended by its elements, so an empty
/// `function_declarations: []` contributed nothing. Choosing "the first path that yielded something"
/// instead skips the empty member and falls through to emitting the wrapper object as a tool - a tool
/// called nothing, with the wrapper's own shape.
#[test]
fn an_empty_declaration_wrapper_yields_no_tool() {
    let request = r#"{"tools":[{"function_declarations":[]}]}"#;
    let attrs = make_attrs(&[("gcp.vertex.agent.llm_request", request)]);
    let (tools, _) = extract_tool_definitions("", &attrs, Utc::now());
    assert!(
        tools.is_empty(),
        "an empty declaration wrapper produced a tool: {tools:?}"
    );
}

/// Both spellings present: the first *declared* one wins, whichever is non-empty.
#[test]
fn the_first_declared_wrapper_spelling_wins() {
    let request = r#"{"tools":[{"function_declarations":[{"name":"snake"}],"functionDeclarations":[{"name":"camel"}]}]}"#;
    let attrs = make_attrs(&[("gcp.vertex.agent.llm_request", request)]);
    let (tools, _) = extract_tool_definitions("", &attrs, Utc::now());
    let names: Vec<String> = tools
        .iter()
        .flat_map(|t| t.content.as_array().cloned().unwrap_or_default())
        .filter_map(|t| t.get("name").and_then(|n| n.as_str()).map(str::to_string))
        .collect();
    assert_eq!(names, vec!["snake".to_string()]);
}

/// An indexed entry that wraps one serialised payload *is* that payload.
///
/// The family's members arrive as `<prefix>.<n>.tool.json_schema`, so stripping the entry prefix leaves a
/// member literally named `tool.json_schema` - a dot in the key, not a nesting - which is why the projection
/// is bracket-quoted. Both rules over the family are pinned here: the schema list and the names projected
/// out of it, which are different observations.
#[test]
fn an_indexed_entry_projects_its_leaf_payload() {
    let attrs = make_attrs(&[
        (
            "llm.tools.0.tool.json_schema",
            r#"{"type":"function","function":{"name":"search","parameters":{"type":"object"}}}"#,
        ),
        (
            "llm.tools.1.tool.json_schema",
            r#"{"type":"function","function":{"name":"calculator"}}"#,
        ),
    ]);
    let (defs, names) = extract_tool_definitions("", &attrs, Utc::now());
    assert_eq!(defs.len(), 1, "the family is one observation: {defs:?}");
    let schemas = defs[0].content.as_array().expect("an array of schemas");
    assert_eq!(schemas.len(), 2);
    assert_eq!(
        schemas[0]["function"]["name"].as_str(),
        Some("search"),
        "the projection returned the wrapper rather than the schema: {schemas:?}"
    );
    assert_eq!(names.len(), 1, "the names are their own observation");
    let listed: Vec<&str> = names[0]
        .content
        .as_array()
        .expect("an array of names")
        .iter()
        .filter_map(|n| n.as_str())
        .collect();
    assert_eq!(listed, vec!["search", "calculator"]);
}

/// The single-tool triple yields to each of its two precedences independently.
///
/// Its gate is one `unless` naming two dimensions - this dialect's own list carriers, and the convention's
/// single-tool attribute. A `DetectMatch` holds when **any** dimension does, which is what makes one gate
/// express two independent precedences; were it all-of, neither alone would suppress the triple and the same
/// tools would be reported twice. The retired code expressed both as a global "has anything produced tools
/// yet" flag, which any unrelated dialect could satisfy first.
#[test]
fn the_single_tool_triple_yields_to_each_precedence_alone() {
    let triple: &[(&str, &str)] = &[
        ("tool.name", "secondary"),
        ("tool.description", "a tool"),
        ("tool.parameters", r#"{"type":"object"}"#),
    ];
    let names_of = |attrs: &HashMap<String, String>| -> Vec<String> {
        extract_tool_definitions("", attrs, Utc::now())
            .0
            .iter()
            .flat_map(|t| t.content.as_array().cloned().unwrap_or_default())
            .filter_map(|t| {
                t["function"]["name"]
                    .as_str()
                    .or_else(|| t["name"].as_str())
                    .map(str::to_string)
            })
            .collect()
    };

    // Alone, the triple is read.
    let mut only = triple.to_vec();
    assert!(names_of(&make_attrs(&only)).contains(&"secondary".to_string()));

    // The dialect's own list carrier suppresses it - same tools, described once.
    only.push(("llm.tools", r#"[{"name":"listed"}]"#));
    assert!(!names_of(&make_attrs(&only)).contains(&"secondary".to_string()));

    // And so does the convention's own single-tool attribute, on its own.
    let mut with_convention = triple.to_vec();
    with_convention.push(("gen_ai.tool.name", "primary"));
    let seen = names_of(&make_attrs(&with_convention));
    assert!(seen.contains(&"primary".to_string()));
    assert!(!seen.contains(&"secondary".to_string()));
}

/// A malformed list carrier does not suppress a perfectly good single-tool triple.
///
/// The precedence is "the triple is read if the lists produced nothing" - which is what the retired code's
/// flag tested. Expressed as an `attr_exists` gate it became "the triple is read if no list carrier is
/// *present*", so an unparseable `llm.tools` took the tools away entirely rather than yielding to the
/// triple. A branch set tests production, which is the condition that was always meant.
#[test]
fn a_malformed_list_carrier_yields_to_the_single_tool_triple() {
    let attrs = make_attrs(&[
        ("llm.tools", "not json at all"),
        ("tool.name", "secondary"),
        ("tool.description", "a tool"),
    ]);
    let (defs, _) = extract_tool_definitions("", &attrs, Utc::now());
    let names: Vec<String> = defs
        .iter()
        .flat_map(|t| t.content.as_array().cloned().unwrap_or_default())
        .filter_map(|t| t["function"]["name"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        names,
        vec!["secondary".to_string()],
        "a malformed list carrier suppressed the triple: {defs:?}"
    );
}

/// A malformed indexed schema is dropped, not reported as a tool.
///
/// An indexed member is sniffed, so an unparseable schema arrives as the string it is. Projected without a
/// declared parse mode that string became a "tool definition" - junk where the retired code, which required
/// `serde_json::from_str` to succeed, reported nothing.
#[test]
fn a_malformed_indexed_schema_is_not_a_tool() {
    let attrs = make_attrs(&[
        ("llm.tools.0.tool.json_schema", "{not json"),
        (
            "llm.tools.1.tool.json_schema",
            r#"{"type":"function","function":{"name":"good"}}"#,
        ),
    ]);
    let (defs, _) = extract_tool_definitions("", &attrs, Utc::now());
    let schemas: Vec<JsonValue> = defs
        .iter()
        .flat_map(|t| t.content.as_array().cloned().unwrap_or_default())
        .collect();
    assert_eq!(
        schemas.len(),
        1,
        "the malformed schema was reported as a tool: {schemas:?}"
    );
    assert_eq!(schemas[0]["function"]["name"].as_str(), Some("good"));
}

/// A present wrapper that is not a list has not declared its contents.
///
/// A wrapper member *is* a list of declarations. Present but scalar, it says nothing - and the retired code,
/// which required the member to be an array, fell back to the enclosing tool group. Emitting the scalar
/// itself would report a "tool" that is a string.
#[test]
fn a_non_array_declaration_wrapper_falls_back_to_the_group() {
    let request = r#"{"tools":[{"name":"bare","function_declarations":"not a list"}]}"#;
    let attrs = make_attrs(&[("gcp.vertex.agent.llm_request", request)]);
    let (defs, _) = extract_tool_definitions("", &attrs, Utc::now());
    let tools: Vec<JsonValue> = defs
        .iter()
        .flat_map(|t| t.content.as_array().cloned().unwrap_or_default())
        .collect();
    assert_eq!(tools.len(), 1, "expected the enclosing group: {tools:?}");
    assert_eq!(tools[0]["name"].as_str(), Some("bare"));
}

/// A tool with no name is not a tool definition.
///
/// `composed` emits as soon as *any* member resolved, so a span carrying only `tool.description` produced a
/// canonical definition with no `name` - unusable, and the retired branch required the name to be there.
#[test]
fn a_nameless_single_tool_is_not_emitted() {
    let attrs = make_attrs(&[("tool.description", "a tool with no name")]);
    let (defs, _) = extract_tool_definitions("", &attrs, Utc::now());
    assert!(
        defs.is_empty(),
        "a nameless tool definition was emitted: {defs:?}"
    );
}

/// The declared fallback stage, in the retired extractor's shape.
///
/// `try_raw_io` is gone: its four carriers are declared with `stage: fallback`, and the stage is evaluated by
/// the plan. These call sites assert the same behaviour through the declaration.
fn try_raw_io(
    messages: &mut Vec<RawMessage>,
    _tool_definitions: &mut Vec<RawToolDefinition>,
    attrs: &HashMap<String, String>,
    span_name: &str,
    timestamp: DateTime<Utc>,
) -> bool {
    let produced: Vec<RawMessage> = sideseat_domain::rules::ruleset()
        .messages
        .fallback(
            &sideseat_domain::rules::MessageContext::for_span(
                span_name,
                attrs,
                is_tool_execution_span(attrs),
            ),
            &std::collections::HashSet::new(),
        )
        .into_iter()
        .map(|e| RawMessage::from_attr(e.carrier.name(), timestamp, e.value))
        .collect();
    let found = !produced.is_empty();
    messages.extend(produced);
    found
}
