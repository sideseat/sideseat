/// The parsed probe assets of one body.
fn probe_assets(body: &str) -> ParsedAssets {
    ParsedAssets::parse(&std::collections::BTreeMap::from([(
        "t.json".to_string(),
        body.as_bytes().to_vec(),
    )]))
    .expect("the probe assets parse")
}

/// A probe plan from one asset body, or the compiler's refusal.
fn probe_compile(
    body: &str,
) -> Result<super::message_rules::MessagePlan, super::message_rules::MessageCompileError> {
    super::message_rules::compile(&probe_assets(body))
}

fn probe_attrs(attrs: &[(&str, &str)]) -> std::collections::HashMap<String, String> {
    attrs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// **A branch fallback supplies each kind the primaries did not.** A fallback leaf reading a conversation and
/// the tools it was offered, beside a primary that found only tools, still owes the conversation - and only
/// the conversation: the primary's tools stand and the fallback's do not join them.
#[test]
fn a_branch_fallback_supplies_each_kind_the_primaries_did_not() {
    use crate::rules::message_rules::MessageContext;
    let plan = probe_compile(
        r#"{"id":"t","messages":[{"id":"t.branch","priority":1,"branch_set":{
            "primary":[{"id":"t.tools","read":{"attribute":"tools"},"parse":"json","emit":"tool_definitions"}],
            "fallback_if_primary_empty":[{"id":"t.request","read":{"attribute":"request"},"parse":"json",
              "emit":"message",
              "alternatives":[{"id":"turns","select":"$.messages","each":true,"where":{"path":"$.role"}}],
              "also":[{"id":"offered","select":"$.tools","emit":"tool_definitions"}]}]}}]}"#,
    )
    .expect("the probe compiles");
    let attrs = probe_attrs(&[
        (
            "tools",
            r#"[{"type":"function","function":{"name":"primary"}}]"#,
        ),
        (
            "request",
            r#"{"messages":[{"role":"user","content":"q"}],
                "tools":[{"type":"function","function":{"name":"fallback"}}]}"#,
        ),
    ]);
    let ctx = MessageContext::for_span("span", &attrs, false);
    let messages: Vec<_> = plan.run(&ctx).into_iter().map(|e| e.value).collect();
    assert_eq!(
        messages,
        [serde_json::json!({"role": "user", "content": "q"})],
        "the conversation no primary supplied is read from the fallback"
    );
    let tools: Vec<_> = plan
        .tool_definitions(&ctx)
        .into_iter()
        .map(|e| e.value)
        .collect();
    assert_eq!(
        tools,
        [serde_json::json!([{"type": "function", "function": {"name": "primary"}}])],
        "and the tools the primary supplied are the only ones"
    );
}

/// **An event branch's leaf may read a tool span.** A branch parent states no permission, so the leaves are
/// asked, as on a span - where the parent's own flag, always unset, skipped every leaf.
#[test]
fn an_event_branch_leaf_may_read_a_tool_span() {
    let plan = probe_compile(
        r#"{"id":"t","message_events":[{"id":"t.e","name":"acme.turn"}],
            "messages":[{"id":"t.branch","source":{"event":{"names":["acme.turn"]}},"priority":1,
              "branch_set":{"primary":[{"id":"t.leaf","read":{"attribute":"payload"},"parse":"json",
                "emit":"message","reads_tool_spans":true}]}}]}"#,
    )
    .expect("the probe compiles");
    let event = probe_attrs(&[("payload", r#"{"role":"tool","content":"r"}"#)]);
    let reading = plan.from_event(
        "acme.turn",
        &event,
        "span",
        None,
        &std::collections::HashMap::new(),
        true,
    );
    assert_eq!(
        reading.emissions.len(),
        1,
        "the leaf that may read a tool span reads it"
    );
}

/// **A declaration the engine would discard is refused**, wherever inlining puts it: tool metadata on an event
/// rule, one output member written twice by any envelope, an envelope or target on a selection point that
/// hands its candidates to cases or on any reading of an aggregate, and `parse` beside a traversal into
/// members a string does not have.
#[test]
fn a_declaration_the_engine_would_discard_is_refused_after_inlining() {
    let fragment =
        r#""fragments":{"shapes":{"cases":[{"id":"t.case","where":{"path":"$.role"}}]}}"#;
    let wrapping_fragment = r#""fragments":{"shapes":{"cases":[{"id":"t.case","where":{"path":"$.role"},
        "wrap":{"role":"user"}}]}}"#;
    let event = r#""message_events":[{"id":"t.e","name":"acme.turn"}]"#;
    for (why, body) in [
        (
            "an event rule targeting tool definitions, which are read from the span",
            format!(
                r#"{{"id":"t",{event},"messages":[{{"id":"t.r","source":{{"event":{{"names":["acme.turn"]}}}},
                    "read":{{"attribute":"tools"}},"parse":"json","emit":"tool_definitions","priority":1}}]}}"#
            ),
        ),
        (
            "an alternative's envelope writing the role twice",
            r#"{"id":"t","messages":[{"id":"t.r","read":{"attribute":"x"},"parse":"json","emit":"message",
                "priority":1,"alternatives":[{"id":"a","select":"$.m","wrap":{"role":"user",
                "members":{"role":"assistant"}}}]}]}"#
                .to_string(),
        ),
        (
            "an extra case's envelope writing the role twice",
            r#"{"id":"t","messages":[{"id":"t.r","read":{"attribute":"x"},"parse":"json","emit":"message",
                "priority":1,"alternatives":[{"id":"a","select":"$.m","each":true,"extra_cases":[{"id":"c",
                "where":{"path":"$.role"},"wrap":{"role":"user","members":{"role":"assistant"}}}]}]}]}"#
                .to_string(),
        ),
        (
            "a selection point's envelope beside the fragment cases that build the value",
            format!(
                r#"{{"id":"t",{fragment},"messages":[{{"id":"t.r","read":{{"attribute":"x"}},"parse":"json",
                    "emit":"message","priority":1,"alternatives":[{{"id":"a","select":"$.m","each":true,
                    "then_fragment":"t.shapes","wrap":{{"role":"user"}}}}]}}]}}"#
            ),
        ),
        (
            "a selection point's target beside its extra cases",
            r#"{"id":"t","messages":[{"id":"t.r","read":{"attribute":"x"},"parse":"json","emit":"message",
                "priority":1,"alternatives":[{"id":"a","select":"$.m","each":true,"emit":"tool_definitions",
                "extra_cases":[{"id":"c","where":{"path":"$.role"}}]}]}]}"#
                .to_string(),
        ),
        (
            "an aggregate whose fragment case declares an envelope",
            format!(
                r#"{{"id":"t",{wrapping_fragment},"messages":[{{"id":"t.r","read":{{"attribute":"x"}},
                    "parse":"json","emit":"message","priority":1,"aggregate_into_array":true,
                    "alternatives":[{{"id":"a","select":"$.m","each":true,"then_fragment":"t.shapes"}}]}}]}}"#
            ),
        ),
        (
            "an aggregate whose extra case declares a target",
            r#"{"id":"t","messages":[{"id":"t.r","read":{"attribute":"x"},"parse":"json","emit":"message",
                "priority":1,"aggregate_into_array":true,"alternatives":[{"id":"a","select":"$.m","each":true,
                "extra_cases":[{"id":"c","where":{"path":"$.role"},"emit":"tool_definitions"}]}]}]}"#
                .to_string(),
        ),
        (
            "`parse` beside `descend`, which no string element can answer",
            r#"{"id":"t","messages":[{"id":"t.r","read":{"attribute":"x"},"parse":"json","emit":"message",
                "priority":1,"alternatives":[{"id":"a","select":"$.m","each":true,"parse":"json",
                "descend":"message"}]}]}"#
                .to_string(),
        ),
        (
            "`parse` beside `then_select`, the same",
            r#"{"id":"t","messages":[{"id":"t.r","read":{"attribute":"x"},"parse":"json","emit":"message",
                "priority":1,"alternatives":[{"id":"a","select":"$.m","each":true,"parse":"json",
                "then_select":"$.messages"}]}]}"#
                .to_string(),
        ),
    ] {
        assert!(probe_compile(&body).is_err(), "{why}: compiled - {body}");
    }
    // The same shapes without the defect compile, so each refusal above is about the defect.
    for (why, body) in [
        (
            "a selection point handing its candidates to fragment cases",
            format!(
                r#"{{"id":"t",{fragment},"messages":[{{"id":"t.r","read":{{"attribute":"x"}},"parse":"json",
                    "emit":"message","priority":1,"alternatives":[{{"id":"a","select":"$.m","each":true,
                    "then_fragment":"t.shapes"}}]}}]}}"#
            ),
        ),
        (
            "an alternative's envelope writing each member once",
            r#"{"id":"t","messages":[{"id":"t.r","read":{"attribute":"x"},"parse":"json","emit":"message",
                "priority":1,"alternatives":[{"id":"a","select":"$.m","wrap":{"role":"user"}}]}]}"#
                .to_string(),
        ),
        (
            "`parse` on its own",
            r#"{"id":"t","messages":[{"id":"t.r","read":{"attribute":"x"},"parse":"json","emit":"message",
                "priority":1,"alternatives":[{"id":"a","select":"$.m","each":true,"parse":"json"}]}]}"#
                .to_string(),
        ),
    ] {
        if let Err(error) = probe_compile(&body) {
            panic!("{why}: refused ({error}) - {body}");
        }
    }
}

/// **A scope-name prefix is three-valued.** True or false for a span that reports its instrumentation scope,
/// and unknown for one that reports none - so a `not` over it, the way an asset says "not one of these
/// instrumentors", holds only where a scope is there to be some other one.
#[test]
fn a_scope_name_prefix_is_unknown_where_no_scope_is_reported() {
    use super::expr::Truth::{False as F, True as T, Unknown as U};
    use super::span_conditions::{self, Readable, SpanSubject};
    let everything = Readable {
        span_name: true,
        attributes: true,
        scope: true,
        scope_version: true,
        resource: true,
    };
    let lower = |condition: serde_json::Value, readable: Readable| {
        let parsed: super::schema::SpanWhere =
            serde_json::from_value(condition).expect("the probe condition parses");
        span_conditions::lower(&parsed, readable)
    };
    let prefix =
        serde_json::json!({"source": "scope.name", "starts_with": "acme.instrumentation."});
    let positive = lower(prefix.clone(), everything).expect("a scope prefix lowers");
    let negated = lower(serde_json::json!({"not": prefix}), everything).expect("and its negation");
    let attrs = std::collections::HashMap::new();
    let truth = |condition: &span_conditions::SpanExpr, scope_name: Option<&str>| {
        let subject = SpanSubject {
            span_name: "span",
            attrs: &attrs,
            scope_name,
            scope_version: None,
            resource: None,
        };
        condition.eval(&mut |atom| atom.eval(&subject))
    };
    for (scope, is, is_not) in [
        (Some("acme.instrumentation.openai"), T, F),
        (Some("acme.instrumentation."), T, F),
        (Some("user.application"), F, T),
        (Some(""), F, T),
        (None, U, U),
    ] {
        assert_eq!(
            (truth(&positive, scope), truth(&negated, scope)),
            (is, is_not),
            "{scope:?}"
        );
    }
    // A scope that equals a name under the prefix is under it, so the narrower gate implies the wider.
    let exact = lower(
        serde_json::json!({"source": "scope.name", "equals": "acme.instrumentation.openai"}),
        everything,
    )
    .expect("an exact scope lowers");
    assert!(span_conditions::implies(&exact, &positive));
    assert!(!span_conditions::implies(&positive, &exact));
    // Refused: a section that cannot see the scope, and an empty prefix, which every scope begins with.
    assert!(
        lower(
            prefix.clone(),
            Readable {
                scope: false,
                ..everything
            }
        )
        .is_err(),
        "a section without the scope cannot ask about it"
    );
    let empty = lower(
        serde_json::json!({"source": "scope.name", "starts_with": ""}),
        everything,
    )
    .expect("an empty prefix lowers, and its defect is reported by the atom");
    assert!(
        empty.atoms().iter().any(|atom| atom.defect().is_some()),
        "an empty scope prefix is a defect"
    );
}

/// **An attachment reads, then asks `where`, then lets a literal replace what it read** - for every source.
/// A flag's `value` used to be ignored on an attribute, `parse` on a payload path, and `where` on the span-name
/// fallback, while the other sources honoured each.
#[test]
fn an_attachment_reads_then_asks_where_then_replaces_for_every_source() {
    use crate::rules::message_rules::MessageContext;
    let member = |attach: &str, span_name: &str, attrs: &[(&str, &str)]| {
        let plan = probe_compile(&format!(
            r#"{{"id":"t","messages":[{{"id":"t.r","read":{{"attribute":"x"}},"parse":"json","emit":"message",
                "priority":1,"alternatives":[{{"id":"a","select":"$.text","where":{{"kind":"string"}},
                "wrap":{{"role":"tool","attach":[{attach}]}}}}]}}]}}"#
        ))
        .expect("the probe compiles");
        let attrs = probe_attrs(attrs);
        let ctx = MessageContext::for_span(span_name, &attrs, false);
        let emitted = plan.run(&ctx);
        assert_eq!(emitted.len(), 1, "one message: {attach}");
        emitted[0].value.get("m").cloned()
    };
    let payload = r#"{"text":"r","meta":"{\"a\":1}"}"#;

    // A flag on an attribute attaches the literal, not the attribute's text.
    assert_eq!(
        member(
            r#"{"from":"flag","as":"m","value":true}"#,
            "span",
            &[("x", payload), ("flag", "yes")]
        ),
        Some(serde_json::json!(true))
    );
    // A payload path holding serialised JSON is parsed where declared, as a value path's is.
    assert_eq!(
        member(
            r#"{"from_path":"$.meta","as":"m","parse":"json"}"#,
            "span",
            &[("x", payload)]
        ),
        Some(serde_json::json!({"a": 1}))
    );
    // The span name is read like any source, so `where` is asked of it.
    let named = r#"{"or_span_name":[{"strip_prefix":"execute_tool "},"trim"],"as":"m",
        "where":{"starts_with":"allowed"}}"#;
    assert_eq!(
        member(named, "execute_tool allowed_lookup", &[("x", payload)]),
        Some(serde_json::json!("allowed_lookup"))
    );
    assert_eq!(
        member(named, "execute_tool denied", &[("x", payload)]),
        None,
        "a span name `where` refuses is not attached"
    );
    // A default is not read, so `where` does not ask it.
    assert_eq!(
        member(
            r#"{"from":"absent","as":"m","where":{"starts_with":"allowed"},"default":""}"#,
            "span",
            &[("x", payload)]
        ),
        Some(serde_json::json!(""))
    );

    // Refused: a `where` with nothing read to ask, and two literals for one flag.
    for attach in [
        r#"{"as":"m","value":true,"where":{"kind":"bool"}}"#,
        r#"{"from":"flag","as":"m","pipe":[{"map":{"true":true},"closed":true}],"value":false}"#,
    ] {
        let body = format!(
            r#"{{"id":"t","messages":[{{"id":"t.r","read":{{"attribute":"x"}},"parse":"json","emit":"message",
                "priority":1,"alternatives":[{{"id":"a","select":"$.text","wrap":{{"role":"tool",
                "attach":[{attach}]}}}}]}}]}}"#
        );
        assert!(probe_compile(&body).is_err(), "compiled - {attach}");
    }
}
