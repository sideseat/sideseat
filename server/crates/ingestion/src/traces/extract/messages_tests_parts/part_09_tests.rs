#[test]
fn two_rules_reading_one_carrier_are_refused() {
    // Not a precedence question: the ingestion claims a carrier once, so the second rule could never
    // emit anything and would look live while doing nothing.
    let contested = br#"{
      "id": "t", "doc": "d",
      "messages": [
        {"id": "a", "doc": "d", "read": {"attribute": "k"}, "parse": "json", "emit": "message",
         "priority": 1},
        {"id": "b", "doc": "d", "read": {"attribute": "k"}, "parse": "json", "emit": "message",
         "priority": 2}
      ]
    }"#;
    let sources = std::collections::BTreeMap::from([("t.json".to_string(), contested.to_vec())]);
    assert!(matches!(
        compile(&ParsedAssets::parse(&sources).expect("the probe assets parse")),
        Err(sideseat_domain::rules::message_rules::MessageCompileError::ContestedCarrier { .. })
    ));
}

#[test]
fn the_two_parse_modes_differ_where_it_matters() {
    // `json` skips what it cannot parse; `json_or_string` keeps it. An extractor using the wrong one
    // either drops a plain-text payload or stores a fragment of JSON as prose.
    let both = br#"{
      "id": "t", "doc": "d",
      "messages": [
        {"id": "strict", "doc": "d", "read": {"attribute": "strict"}, "parse": "json",
         "emit": "message", "priority": 1},
        {"id": "lenient", "doc": "d", "read": {"attribute": "lenient"}, "parse": "json_or_string",
         "emit": "message", "priority": 2}
      ]
    }"#;
    let sources = std::collections::BTreeMap::from([("t.json".to_string(), both.to_vec())]);
    let plan =
        compile(&ParsedAssets::parse(&sources).expect("the probe assets parse")).expect("compiles");
    let span_attrs = rule_attrs(&[("strict", "not json"), ("lenient", "not json")]);
    let emissions = plan.run(&MessageContext::for_span("s", &span_attrs, false));
    let ids: Vec<&str> = emissions.iter().map(|e| e.rule_id).collect();
    assert_eq!(
        ids,
        vec!["lenient"],
        "the strict rule must skip an unparseable value and the lenient one must keep it"
    );
    assert_eq!(emissions[0].value, serde_json::json!("not json"));
}

/// A recognised event must not suppress a tool span's own tool-call attributes.
///
/// The shape the removed orchestration block lost. It read the Vercel tool attributes only when nothing
/// else had produced a message, so any recognised event dropped the call *and* its result - and the
/// equivalence oracle could not see it, because that oracle applies the caller's tool-span exclusion and so
/// compared both implementations after the suppression. No captured fixture carries this shape either.
#[test]
fn an_event_does_not_suppress_a_tool_span_s_own_attributes() {
    let attrs = rule_attrs(&[
        ("ai.toolCall.name", "weather"),
        ("ai.toolCall.id", "call_1"),
        ("ai.toolCall.args", r#"{"city":"NYC"}"#),
        ("ai.toolCall.result", r#"{"temp":21}"#),
    ]);
    assert!(
        is_tool_execution_span(&attrs),
        "name plus id makes this a tool execution span, which is what gated the lost block"
    );

    let mut messages: Vec<RawMessage> = Vec::new();
    let mut tools: Vec<RawToolDefinition> = Vec::new();
    // A message already present, standing for one a recognised event produced.
    messages.push(RawMessage::from_attr(
        "gen_ai.choice",
        Utc::now(),
        serde_json::json!({"role": "assistant", "content": "thinking"}),
    ));
    let found = try_declared_rules(
        &mut messages,
        &mut tools,
        &attrs,
        "ai.toolCall",
        Utc::now(),
        &mut std::collections::HashSet::new(),
    );

    assert!(found);
    let carriers: Vec<String> = messages
        .iter()
        .map(|m| match &m.source {
            MessageSource::Attribute { key, .. } => key.clone(),
            MessageSource::Event { name, .. } => name.clone(),
        })
        .collect();
    assert!(
        carriers.iter().any(|c| c == "ai.toolCall.args"),
        "the call survives an earlier message: {carriers:?}"
    );
    assert!(
        carriers.iter().any(|c| c == "ai.toolCall.result"),
        "and so does its result: {carriers:?}"
    );
}

/// A swept payload is ordered deterministically, whatever the attribute map's own order.
///
/// The property the oracle cannot check, because the baseline it compares against did not have it. An
/// unsorted sweep writes different bytes for the same span on different runs - the map's iteration order is
/// randomised per process and the payload is persisted with its insertion order - which changes the
/// reconstruction cache digest for rows nobody touched.
#[test]
fn a_swept_payload_is_ordered_deterministically() {
    // Enough members that a randomised walk would almost certainly differ between two orders.
    let attrs = rule_attrs(&[
        ("ai.response.text", "answer"),
        ("ai.response.id", "r1"),
        ("ai.response.model", "m"),
        ("ai.response.providerMetadata", "{}"),
        ("ai.response.timestamp", "t"),
        ("ai.response.msgId", "x"),
        ("ai.response.finishReason", "stop"),
    ]);
    let rendered: Vec<String> = (0..8)
        .map(|_| {
            let mut messages: Vec<RawMessage> = Vec::new();
            let mut tools: Vec<RawToolDefinition> = Vec::new();
            try_declared_rules(
                &mut messages,
                &mut tools,
                &attrs,
                "ai.generateText",
                Utc::now(),
                &mut std::collections::HashSet::new(),
            );
            messages
                .iter()
                .map(|m| m.content.to_string())
                .collect::<Vec<_>>()
                .join("|")
        })
        .collect();
    assert!(
        rendered.windows(2).all(|w| w[0] == w[1]),
        "the same span produced different bytes across runs: {rendered:?}"
    );
    // And the members really are in sorted order, which is what makes it stable across *processes* - equal
    // runs inside one process would also pass if the map order happened to be fixed.
    let payload = &rendered[0];
    let finish = payload.find("finishReason").expect("swept member present");
    let model = payload.find("\"model\"").expect("swept member present");
    assert!(
        finish < model,
        "swept members are inserted in sorted order: {payload}"
    );
}

/// The carrier-ownership check catches conflicts a single projection of it missed.
///
/// It used to compare "the carrier a rule reads" and so missed four real shapes: a `tag_as` emitting a name
/// the rule never read, an indexed family against an exact key it generates, a `compose` consuming a carrier
/// another rule emits, and a sweep overlapping an exact source. Each of those is a rule that silently never
/// emits, or two rules writing the same carrier - and either way a reader cannot tell.
#[test]
fn carrier_ownership_conflicts_are_refused() {
    let cases: &[(&str, &str)] = &[
        (
            "tag_as collides with another rule's carrier",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "priority":1},
                {"id":"b","doc":"d","read":{"attribute":"y"},"tag_as":"x","parse":"json",
                 "emit":"message","priority":2}]}"#,
        ),
        (
            "an indexed family covers an exact key it generates",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"indexed_family":"f"},"emit":"message","priority":1},
                {"id":"b","doc":"d","read":{"attribute":"f.0"},"parse":"json","emit":"message",
                 "priority":2}]}"#,
        ),
        (
            "a compose consumes a carrier another rule emits",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute":"r.text"},"parse":"json","emit":"message",
                 "priority":1},
                {"id":"b","doc":"d","compose":{"tag":"r","members":[
                    {"as":"content","from_any_of":["r.text"],"parse":"text"}]},"emit":"message","priority":2}]}"#,
        ),
        (
            "a sweep overlaps an exact source of another rule",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute":"p.one"},"parse":"json","emit":"message",
                 "priority":1},
                {"id":"b","doc":"d","compose":{"tag":"q","members":[
                    {"sweep_prefix":"p."}]},"emit":"message","priority":2}]}"#,
        ),
    ];
    for (what, asset) in cases {
        let sources =
            std::collections::BTreeMap::from([("t.json".to_string(), asset.as_bytes().to_vec())]);
        assert!(
            matches!(
                compile(&ParsedAssets::parse(&sources).expect("the probe assets parse")),
                Err(
                    sideseat_domain::rules::message_rules::MessageCompileError::ContestedCarrier { .. }
                )
            ),
            "should have been refused: {what}"
        );
    }
}

/// A construct the engine cannot execute, or a field it would ignore, is refused rather than accepted.
#[test]
fn inexpressible_rules_are_refused() {
    let cases: &[(&str, &str)] = &[
        (
            "`compose` with `wrap`, which would be ignored",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","compose":{"tag":"q","members":[{"as":"c","from_any_of":["k"],"parse":"text"}]},
                 "wrap":{"role":"user"},"emit":"message","priority":1}]}"#,
        ),
        (
            "an indexed family with `wrap`, which would be ignored",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"indexed_family":"f"},"wrap":{"role":"user"},
                 "emit":"message","priority":1}]}"#,
        ),
        (
            "`entry_member` without an indexed family",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute":"k","entry_member":"m"},"parse":"json",
                 "emit":"message","priority":1}]}"#,
        ),
        (
            "a default section route before another route, which can never match",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute":"k"},"parse":"text","emit":"message",
                 "priority":1,
                 "sections":{"split_on":"|","routes":[
                    {"id":"r.default","role":"user"},{"id":"r.tool","tag_prefix":"T:","role":"tool"}]}}]}"#,
        ),
        (
            "a compose member that is both a sweep and a named source",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","compose":{"tag":"q","members":[
                    {"as":"c","from_any_of":["k"],"sweep_prefix":"p."}]},"emit":"message",
                 "priority":1}]}"#,
        ),
        // Every gate a message rule can carry, through the one validator - three call sites had grown the
        // checks separately, so these were refused for a field source and compiled here.
        (
            "a message gate whose phrase search names a source the probe does not read",
            r#"{"id": "t", "doc": "d", "messages": [{"id": "a", "doc": "d", "read": {"attribute": "k"}, "parse": "json", "emit": "message", "priority": 1, "where": {"source": "span", "contains_ignore_case": "x"}}]}"#,
        ),
        (
            "an `unless` with an empty attribute key",
            r#"{"id": "t", "doc": "d", "messages": [{"id": "a", "doc": "d", "read": {"attribute": "k"}, "parse": "json", "emit": "message", "priority": 1, "where": {"not": {"source": "attr:", "exists": true}}}]}"#,
        ),
        (
            "a compose member's fallback gated on a resource dimension",
            r#"{"id": "t", "doc": "d", "messages": [{"id": "a", "doc": "d", "priority": 1, "compose": {"tag": "q", "members": [{"as": "c", "from_any_of": ["k"], "fallback": {"from": "other", "where": {"source": "resource:service.name", "contains": "svc"}}}]}}]}"#,
        ),
        (
            "a gate on a resource dimension a message rule is never given",
            r#"{"id": "t", "doc": "d", "messages": [{"id": "a", "doc": "d", "read": {"attribute": "k"}, "parse": "json", "emit": "message", "priority": 1, "where": {"source": "resource:service.name", "contains": "svc"}}]}"#,
        ),
        // A branch leaf's own copy of a field only the entry points read. Four spellings, because the
        // parent's no-dead-fields rule had no mirror here and each of these compiled into silence.
        (
            "a branch leaf declaring a stage",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","emit":"message","priority":1,
                 "branch_set":{"primary":[
                    {"id":"a.1","doc":"d","read":{"attribute":"k"},"parse":"json","emit":"message",
                     "source":{"span":{"stage":"fallback"}}}]}}]}"#,
        ),
        (
            "a branch leaf declaring an event",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","emit":"message","priority":1,
                 "branch_set":{"primary":[
                    {"id":"a.1","doc":"d","read":{"attribute":"k"},"parse":"json","emit":"message",
                     "source":{"event":{"names":["some.event"]}}}]}}]}"#,
        ),
        (
            "a branch leaf declaring a rank, which orders nothing - the branch order is positional",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","emit":"message","priority":1,
                 "branch_set":{"primary":[
                    {"id":"a.1","doc":"d","read":{"attribute":"k"},"parse":"json","emit":"message",
                     "priority":2}]}}]}"#,
        ),
        // Presence, not value: each of these writes out the field's own default, which is still a statement
        // the engine reads from somewhere else. A check comparing against the default accepted all three.
        (
            "a branch leaf declaring the default stage explicitly",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","priority":1,
                 "branch_set":{"primary":[
                    {"id":"a.1","doc":"d","read":{"attribute":"k"},"parse":"json","emit":"message",
                     "source":{"span":{"stage":"dialect"}}}]}}]}"#,
        ),
        (
            "a branch leaf declaring an empty event list",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","priority":1,
                 "branch_set":{"primary":[
                    {"id":"a.1","doc":"d","read":{"attribute":"k"},"parse":"json","emit":"message",
                     "source":{"event":{"names":[]}}}]}}]}"#,
        ),
        // The parent's own dead fields. `reads_tool_spans` is the observable one: the permission is read
        // from the leaves, so a parent granting it made the whole branch skipped on a tool span.
        (
            "a branch parent granting tool-span permission its leaves do not have",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","priority":1,"reads_tool_spans":true,
                 "branch_set":{"primary":[
                    {"id":"a.1","doc":"d","read":{"attribute":"k"},"parse":"json","emit":"message"}]}}]}"#,
        ),
        (
            "a branch parent declaring a parse mode for a carrier it does not read",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","priority":1,"parse":"json",
                 "branch_set":{"primary":[
                    {"id":"a.1","doc":"d","read":{"attribute":"k"},"parse":"json","emit":"message"}]}}]}"#,
        ),
        (
            "a branch parent declaring a carrier tag its leaves override",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","priority":1,"tag_as":"q",
                 "branch_set":{"primary":[
                    {"id":"a.1","doc":"d","read":{"attribute":"k"},"parse":"json","emit":"message"}]}}]}"#,
        ),
        (
            "a branch parent requiring a non-empty value it never reads",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","priority":1,"raw_where":{"non_empty":true},
                 "branch_set":{"primary":[
                    {"id":"a.1","doc":"d","read":{"attribute":"k"},"parse":"json","emit":"message"}]}}]}"#,
        ),
        (
            "a branch parent requiring members of an entry it never assembles",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","priority":1,
                 "require_members":{"all_of":[{"name":"role"}]},
                 "branch_set":{"primary":[
                    {"id":"a.1","doc":"d","read":{"attribute":"k"},"parse":"json","emit":"message"}]}}]}"#,
        ),
    ];
    for (what, asset) in cases {
        let sources =
            std::collections::BTreeMap::from([("t.json".to_string(), asset.as_bytes().to_vec())]);
        assert!(
            compile(&ParsedAssets::parse(&sources).expect("the probe assets parse")).is_err(),
            "should have been refused: {what}"
        );
    }
}

/// A `require` that holds of every payload is not a condition, and a tag collision is not a read collision.
///
/// Four shapes, each one a rule that compiles and cannot work:
///
/// - `exists: true` beside `exists: false` on one path holds of every value there is, so a rule whose only
///   reading carries it claims its carrier always - while `rule_is_wholly_conditional` read the non-empty
///   `require` as evidence that it sometimes yields, and a second rule on that carrier was permanently dead.
///   The pair is a tautology on **any** path, unlike the value complements, because `exists` is the predicate
///   that decides presence.
/// - Two rules tagging one carrier from *different* attributes both survive: runtime ownership is over the
///   carrier a rule read, so nothing separates them, and the tag is what carrier semantics, identity and
///   ordering key on. Shared physical ownership is what excuses such a pair - the accepted half below - and a
///   conditional read is not.
/// - A `compose` always emits `compose.tag`, so a `tag_as` beside it was checked for collisions under a name
///   the rule never emits.
#[test]
fn a_tautological_requirement_is_not_a_condition() {
    let refused = [
        (
            "an `exists` complement, which holds of every payload",
            r#"{"id": "t", "doc": "d", "messages": [{"id": "a", "doc": "d", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "priority": 1, "alternatives": [{"id": "probe.alt", "where": {"any": [{"path": "$.v", "exists": true}, {"path": "$.v", "exists": false}]}, "wrap": {"role": "user", "content_from_any_of": ["$.content"]}}]}, {"id": "b", "doc": "d", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "priority": 2}]}"#,
        ),
        (
            "two rules tagging one carrier from different attributes",
            r#"{"id": "t", "doc": "d", "messages": [{"id": "a", "doc": "d", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "tag_as": "shared", "where": {"source": "attr:marker", "exists": true}, "priority": 1}, {"id": "b", "doc": "d", "read": {"attribute": "y"}, "parse": "json", "emit": "message", "tag_as": "shared", "priority": 2}]}"#,
        ),
        (
            "a compose declaring a tag it does not emit",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","tag_as":"declared","priority":1,
                 "compose":{"tag":"actual","members":[{"as":"content","from_any_of":["k"],"parse":"text"}]}}]}"#,
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

    // Accepted, and each for a reason the refusals above depend on. A tag collision between two rules reading
    // *one* carrier is resolved by ownership - which is what lets one dialect claim `input.value` while
    // another reads it - and a genuine `exists` condition on one side is a condition.
    let accepted = [
        (
            "two rules tagging one carrier and reading the same attribute",
            r#"{"id": "t", "doc": "d", "messages": [{"id": "a", "doc": "d", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "tag_as": "shared", "where": {"source": "attr:marker", "exists": true}, "priority": 1}, {"id": "b", "doc": "d", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "tag_as": "shared", "priority": 2}]}"#,
        ),
        (
            "a single `exists` requirement, which is a real condition",
            r#"{"id": "t", "doc": "d", "messages": [{"id": "a", "doc": "d", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "priority": 1, "alternatives": [{"id": "probe.alt", "where": {"path": "$.v", "exists": true}, "wrap": {"role": "user", "content_from_any_of": ["$.content"]}}]}, {"id": "b", "doc": "d", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "priority": 2}]}"#,
        ),
        (
            "an indexed family requiring members, beside a rule reading one of its keys",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"indexed_family":"f"},"emit":"message","priority":1,
                 "require_members":{"all_of":[{"name":"content"}]}},
                {"id":"b","doc":"d","read":{"attribute":"f.0.role"},"parse":"json","emit":"message",
                 "tag_as":"b.own.tag","priority":2}]}"#,
        ),
        (
            "a rule reading a later spelling of a carrier another rule reads first",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"first_present":["first","second"]},"parse":"json",
                 "emit":"message","tag_as":"a.own.tag","priority":1},
                {"id":"b","doc":"d","read":{"attribute":"second"},"parse":"json","emit":"message",
                 "tag_as":"b.own.tag","priority":2}]}"#,
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

/// A condition separates two rules only when it is a *different* condition, and a shared tag needs a proof.
///
/// Four shapes, each a rule that compiled and could never emit:
///
/// - two rules gated on the **same** thing, both reading one carrier: whenever the gate holds the earlier
///   owns the carrier, and otherwise neither runs. A boolean "is conditional" called that pair safe;
/// - a tag collision excused by *any* static overlap between the rules' reads. A rule reading
///   `first_present: ["first", "second"]` owns `first` when both are present, so a rule reading `second`
///   under the same tag is not resolved by ownership at all;
/// - a `one_of`/`none_of` complement on a *member* path, which is a tautology for the same reason the
///   `exists` pair is: a sole `none_of` holds of an absent value, so between them every value and its
///   absence are covered;
/// - the same written `$['v']`, which rendered differently and escaped a same-path check.
#[test]
fn a_condition_separates_two_rules_only_when_it_differs() {
    let refused = [
        (
            "two rules gated on the same thing, reading one carrier",
            r#"{"id": "t", "doc": "d", "messages": [{"id": "a", "doc": "d", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "where": {"source": "attr:marker", "exists": true}, "tag_as": "a.tag", "priority": 1}, {"id": "b", "doc": "d", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "where": {"source": "attr:marker", "exists": true}, "tag_as": "b.tag", "priority": 2}]}"#,
        ),
        (
            "a shared tag where ownership does not resolve the pair",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"first_present":["first","second"]},"parse":"json",
                 "emit":"message","tag_as":"shared","priority":1},
                {"id":"b","doc":"d","read":{"attribute":"second"},"parse":"json","emit":"message",
                 "tag_as":"shared","priority":2}]}"#,
        ),
        (
            "a `one_of`/`none_of` complement on a member path",
            r#"{"id": "t", "doc": "d", "messages": [{"id": "a", "doc": "d", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "priority": 1, "alternatives": [{"id": "probe.alt", "where": {"any": [{"path": "$.v", "one_of": ["a"]}, {"path": "$.v", "none_of": ["a"]}]}, "wrap": {"role": "user", "content_from_any_of": ["$.content"]}}]}, {"id": "b", "doc": "d", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "priority": 2}]}"#,
        ),
        (
            "the same complement with the path written in bracket form",
            r#"{"id": "t", "doc": "d", "messages": [{"id": "a", "doc": "d", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "priority": 1, "alternatives": [{"id": "probe.alt", "where": {"any": [{"path": "$.v", "exists": true}, {"path": "$['v']", "exists": false}]}, "wrap": {"role": "user", "content_from_any_of": ["$.content"]}}]}, {"id": "b", "doc": "d", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "priority": 2}]}"#,
        ),
        (
            "`elements` beside a `tag_as` it never emits",
            r#"{"id":"t","doc":"d","messages":[
                {"id":"a","doc":"d","read":{"attribute":"x"},"parse":"json","emit":"message",
                 "tag_as":"declared","priority":1,
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
            "two rules gated on different things",
            r#"{"id": "t", "doc": "d", "messages": [{"id": "a", "doc": "d", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "where": {"source": "attr:marker.a", "exists": true}, "tag_as": "a.tag", "priority": 1}, {"id": "b", "doc": "d", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "where": {"source": "attr:marker.b", "exists": true}, "tag_as": "b.tag", "priority": 2}]}"#,
        ),
        (
            "a shared tag where both rules necessarily own one carrier",
            r#"{"id": "t", "doc": "d", "messages": [{"id": "a", "doc": "d", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "tag_as": "shared", "where": {"source": "attr:marker", "exists": true}, "priority": 1}, {"id": "b", "doc": "d", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "tag_as": "shared", "priority": 2}]}"#,
        ),
        (
            "a `none_of` that forbids a value nothing else requires",
            r#"{"id": "t", "doc": "d", "messages": [{"id": "a", "doc": "d", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "priority": 1, "alternatives": [{"id": "probe.alt", "where": {"any": [{"path": "$.v", "one_of": ["a"]}, {"path": "$.v", "none_of": ["a", "b"]}]}, "wrap": {"role": "user", "content_from_any_of": ["$.content"]}}]}, {"id": "b", "doc": "d", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "priority": 2}]}"#,
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

/// A branch leaf runs under its parent's gate **and** its own, which is a conjunction neither gate expresses.
///
/// Keeping the leaf's alone claims the rule runs wherever that gate holds - false where the parent's does not,
/// and it convicted a rule live on exactly those spans. Where one gate provably covers the other the
/// conjunction *is* the narrower of the two, which is what keeps this from refusing every nested rule; where
/// neither covers the other nothing here can express it, so the claim is opaque and convicts nothing.
#[test]
fn a_leaf_runs_under_both_gates() {
    let parent_p_leaf_l = |rank_b_gate: &str| {
        format!(
            r#"{{"id":"t","doc":"d","messages":[
                {{"id":"a","doc":"d","priority":1,"where":{{"source":"attr:p","exists":true}},
                 "branch_set":{{"primary":[
                    {{"id":"a.1","doc":"d","read":{{"attribute":"x"}},"parse":"json","emit":"message",
                     "where":{{"source":"attr:l","exists":true}},"tag_as":"a.tag"}}]}}}},
                {{"id":"b","doc":"d","read":{{"attribute":"x"}},"parse":"json","emit":"message",
                 "where":{{"source":{rank_b_gate},"exists":true}},"tag_as":"b.tag","priority":2}}]}}"#
        )
    };
    // `l` without `p` runs only the later rule, so the pair is live and must be accepted.
    let sources = std::collections::BTreeMap::from([(
        "t.json".to_string(),
        parent_p_leaf_l("\"attr:l\"").into_bytes(),
    )]);
    assert!(
        compile(&ParsedAssets::parse(&sources).expect("the probe assets parse")).is_ok(),
        "neither gate covers the other, so nothing is provably dead: {:?}",
        compile(&ParsedAssets::parse(&sources).expect("the probe assets parse")).err()
    );

    // The parent's gate covering the leaf's: the conjunction is the leaf's gate, and a later rule on the same
    // gate really is dead.
    let covered = r#"{"id": "t", "doc": "d", "messages": [{"id": "a", "doc": "d", "priority": 1, "where": {"any": [{"source": "attr:p", "exists": true}, {"source": "attr:l", "exists": true}]}, "branch_set": {"primary": [{"id": "a.1", "doc": "d", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "where": {"source": "attr:l", "exists": true}, "tag_as": "a.tag"}]}}, {"id": "b", "doc": "d", "read": {"attribute": "x"}, "parse": "json", "emit": "message", "where": {"source": "attr:l", "exists": true}, "tag_as": "b.tag", "priority": 2}]}"#;
    let sources =
        std::collections::BTreeMap::from([("t.json".to_string(), covered.as_bytes().to_vec())]);
    assert!(
        compile(&ParsedAssets::parse(&sources).expect("the probe assets parse")).is_err(),
        "the parent admits every span the leaf does, so the leaf's gate is the conjunction and the later \
         rule on that same gate is dead"
    );
}

/// A source that cannot be read stops a **chain** and not a **merge**.
///
/// The two are different questions. In a chain, continuing past a present-but-unreadable value substitutes a
/// later spelling's value for the one this key was meant to carry - `http.status_code = "OK"` beside another
/// key's `503` answered 503. A merge is a union, so one unreadable source leaves the others' contributions
/// intact, which is also what the retired `merge_tags` did: an unparseable value contributed nothing and the
/// loop went on.
///
/// Pinned at the engine level because no shipped asset can reach it: every tag source today is a flat
/// attribute, where an unparseable list reads as *empty* rather than malformed. It takes a JSON list source to
/// produce the outcome, which is exactly the shape the next slice adds.
#[test]
fn an_unreadable_source_stops_a_chain_and_not_a_merge() {
    use sideseat_domain::rules::schema::FieldTarget;
    use sideseat_domain::rules::span_fields::{Reading, compile};

    let asset = br#"{"id":"t","doc":"d","span_fields":[
        {"id":"merged","doc":"d","target":"tags","combine":"merge_all","sources":[
            {"id":"s1","json":{"attribute":"metadata","path":"$.tags"}},
            {"id":"s2","attribute":"tags"}]},
        {"id":"chained","doc":"d","target":"http_status_code","sources":[
            {"id":"s1","attribute":"http.status_code"},
            {"id":"s2","attribute":"http.response.status_code"}]}]}"#;
    let sources = std::collections::BTreeMap::from([("t.json".to_string(), asset.to_vec())]);
    let plan =
        compile(&ParsedAssets::parse(&sources).expect("the probe assets parse")).expect("compiles");

    let attrs = rule_attrs(&[
        ("metadata", r#"{"tags":{}}"#),
        ("tags", r#"["valid"]"#),
        ("http.status_code", "OK"),
        ("http.response.status_code", "503"),
    ]);
    let resolved = plan.resolve("a.span", &attrs, &[]);
    let of = |target: FieldTarget| {
        resolved
            .iter()
            .find(|r| r.target == target)
            .expect("every declared target resolves")
    };
    assert_eq!(
        of(FieldTarget::Tags).reading,
        Reading::StringList(vec!["valid".to_string()]),
        "the merge keeps what the readable source contributed"
    );

    let status = of(FieldTarget::HttpStatusCode);
    assert_eq!(
        status.reading,
        Reading::Absent,
        "the chain stops rather than answering with another key's number"
    );
    // And says which source stopped it. An unfilled column and an unfilled column with a named cause are
    // different facts, which is why the resolution carries both.
    assert!(
        status
            .refused
            .iter()
            .any(|r| r.carrier == "http.status_code"
                && matches!(
                    r.cause,
                    sideseat_domain::rules::refusal::Unusable::Malformed { .. }
                )),
        "the refusal names the source and why: {:?}",
        status.refused
    );

    // A **reduction over nothing** yields nothing, rather than zero. Unobservable through the two candidates
    // that use it today - the pairing that reads them asks `> 0` - and load-bearing wherever a summed source
    // has another behind it: `Integer(0)` answers the chain, `Absent` lets the next source speak.
    let chained = br#"{"id":"t","doc":"d","span_fields":[
        {"id":"summed","doc":"d","target":"usage_input_tokens","sources":[
            {"id":"s1","json":{"attribute":"output.value","path":"$.messages[*].models_usage.prompt_tokens","reduce":"sum"}},
            {"id":"s2","attribute":"gen_ai.usage.input_tokens"}]}]}"#;
    let sources = std::collections::BTreeMap::from([("t.json".to_string(), chained.to_vec())]);
    let plan =
        compile(&ParsedAssets::parse(&sources).expect("the probe assets parse")).expect("compiles");
    let no_messages = rule_attrs(&[
        ("output.value", r#"{"result":"done"}"#),
        ("gen_ai.usage.input_tokens", "42"),
    ]);
    assert_eq!(
        plan.resolve("chain", &no_messages, &[])
            .iter()
            .find(|r| r.target == FieldTarget::UsageInputTokens)
            .map(|r| r.reading.clone()),
        Some(Reading::Integer(42)),
        "a sum over no matches lets the source behind it answer"
    );
    // Counts that sum past what a count can hold are invalid telemetry, not a believable maximum: saturating
    // would report `i64::MAX` as this call's usage and bill it.
    let overflowing = rule_attrs(&[(
        "output.value",
        r#"{"messages":[{"models_usage":{"prompt_tokens":9223372036854775807}},{"models_usage":{"prompt_tokens":1}}]}"#,
    )]);
    let resolved = plan.resolve("chain", &overflowing, &[]);
    let summed = resolved
        .iter()
        .find(|r| r.target == FieldTarget::UsageInputTokens)
        .expect("the target resolves");
    assert_eq!(
        summed.reading,
        Reading::Absent,
        "an overflowing sum fills nothing rather than reporting a believable maximum"
    );
    assert!(
        summed.refused.iter().any(|r| matches!(
            r.cause,
            sideseat_domain::rules::refusal::Unusable::Malformed { .. }
        )),
        "and the refusal says why: {:?}",
        summed.refused
    );

    let with_messages = rule_attrs(&[
        (
            "output.value",
            r#"{"messages":[{"models_usage":{"prompt_tokens":0}}]}"#,
        ),
        ("gen_ai.usage.input_tokens", "42"),
    ]);
    assert_eq!(
        plan.resolve("chain", &with_messages, &[])
            .iter()
            .find(|r| r.target == FieldTarget::UsageInputTokens)
            .map(|r| r.reading.clone()),
        Some(Reading::Integer(0)),
        "a sum that really is zero is an answer, and the source behind it is not consulted"
    );
}

/// A gate that could never hold is refused where a field source declares one.
///
/// Signals are ORed, so an **empty** gate is `false` rather than "anything" - every source carrying one is
/// dead. An empty needle is the opposite mistake: `span_name: [""]` matches every span by prefix. Both
/// compiled silently, because the signal compiler validates nothing.
#[test]
fn a_field_source_may_not_declare_a_gate_that_never_holds() {
    let refused = [
        (
            "an empty span-name prefix, which matches every span",
            r#"{"id": "t", "doc": "d", "span_fields": [{"id": "f", "doc": "d", "target": "user_id", "sources": [{"id": "probe.src", "attribute": "k", "where": {"source": "span_name", "starts_with": ""}}]}]}"#,
        ),
        (
            "an empty attribute key, which nothing writes",
            r#"{"id": "t", "doc": "d", "span_fields": [{"id": "f", "doc": "d", "target": "user_id", "sources": [{"id": "probe.src", "attribute": "k", "where": {"source": "attr:", "exists": true}}]}]}"#,
        ),
        (
            "a reduction on a witness, which asks only whether a member is there",
            r#"{"id":"t","doc":"d","span_fields":[
                {"id":"f","doc":"d","target":"usage_input_tokens",
                 "sources":[{"id":"probe.src","value":"1","when_json":{"attribute":"output.value","path":"$.messages[*].n","reduce":"sum"}}]}]}"#,
        ),
        (
            "a reduction on a field that does not hold a number",
            r#"{"id":"t","doc":"d","span_fields":[
                {"id":"f","doc":"d","target":"user_id",
                 "sources":[{"id":"probe.src","json":{"attribute":"output.value","path":"$.messages[*].n","reduce":"sum"}}]}]}"#,
        ),
        (
            "a reduction over a first-present group, which takes one path of several",
            r#"{"id":"t","doc":"d","span_fields":[
                {"id":"f","doc":"d","target":"usage_input_tokens",
                 "sources":[{"id":"probe.src","json":{"attribute":"output.value","first_present_of":["$.a","$.b"],"reduce":"sum"}}]}]}"#,
        ),
        (
            "a JSON witness naming no member, which always answers false",
            r#"{"id":"t","doc":"d","span_fields":[
                {"id":"f","doc":"d","target":"user_id",
                 "sources":[{"id":"probe.src","value":"x","when_json":{"attribute":"request_data"}}]}]}"#,
        ),
        (
            "a JSON witness naming two ways of naming one member",
            r#"{"id":"t","doc":"d","span_fields":[
                {"id":"f","doc":"d","target":"user_id",
                 "sources":[{"id":"probe.src","value":"x","when_json":{"attribute":"request_data","path":"$.a","first_present_of":["$.b"]}}]}]}"#,
        ),
        (
            "a JSON read naming no member",
            r#"{"id":"t","doc":"d","span_fields":[
                {"id":"f","doc":"d","target":"user_id",
                 "sources":[{"id":"probe.src","json":{"attribute":"request_data"}}]}]}"#,
        ),
        (
            "a phrase search naming a source the probe does not read",
            r#"{"id": "t", "doc": "d", "span_fields": [{"id": "f", "doc": "d", "target": "user_id", "sources": [{"id": "probe.src", "attribute": "k", "where": {"source": "span", "contains_ignore_case": "foo"}}]}]}"#,
        ),
        (
            "a phrase search naming `attr:` with no key",
            r#"{"id": "t", "doc": "d", "span_fields": [{"id": "f", "doc": "d", "target": "user_id", "sources": [{"id": "probe.src", "attribute": "k", "where": {"source": "attr:", "contains_ignore_case": "foo"}}]}]}"#,
        ),
        (
            "a phrase search with no needle",
            r#"{"id": "t", "doc": "d", "span_fields": [{"id": "f", "doc": "d", "target": "user_id", "sources": [{"id": "probe.src", "attribute": "k", "where": {"source": "span_name", "contains_ignore_case": ""}}]}]}"#,
        ),
    ];
    for (what, asset) in refused {
        let sources =
            std::collections::BTreeMap::from([("t.json".to_string(), asset.as_bytes().to_vec())]);
        assert!(
            sideseat_domain::rules::span_fields::compile(
                &ParsedAssets::parse(&sources).expect("the probe assets parse")
            )
            .is_err(),
            "should have been refused: {what}"
        );
    }
    // A real gate compiles, phrase search included.
    let ok = r#"{"id": "t", "doc": "d", "span_fields": [{"id": "f", "doc": "d", "target": "user_id", "sources": [{"id": "probe.src", "attribute": "k", "where": {"source": "attr:marker", "exists": true}}]}, {"id": "g", "doc": "d", "target": "http_method", "sources": [{"id": "probe.src", "attribute": "m", "where": {"source": ["span_name", "attr:k"], "contains_ignore_case": "chat"}}]}, {"id": "h", "doc": "d", "target": "http_url", "sources": [{"id": "probe.src", "attribute": "u", "where": {"source": {"first_of": ["attr:a", "attr:b"]}, "contains_ignore_case": "x"}}]}, {"id": "i", "doc": "d", "target": "db_name", "sources": [{"id": "probe.src", "attribute": "d", "where": {"source": "span_name", "contains_ignore_case": "x"}}]}]}"#;
    let sources =
        std::collections::BTreeMap::from([("t.json".to_string(), ok.as_bytes().to_vec())]);
    assert!(
        sideseat_domain::rules::span_fields::compile(
            &ParsedAssets::parse(&sources).expect("the probe assets parse")
        )
        .is_ok(),
        "a gate naming a real signal must compile: {:?}",
        sideseat_domain::rules::span_fields::compile(
            &ParsedAssets::parse(&sources).expect("the probe assets parse")
        )
        .err()
    );
}
