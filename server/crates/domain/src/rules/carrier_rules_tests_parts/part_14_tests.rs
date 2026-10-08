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

/// **Starvation is judged in execution order, between rules that can meet, over everything a reading owns.**
/// The dialect stage runs before the fallback stage whatever the ranks say; two rules whose gates exclude each
/// other never run on one span; and a single sweep owns every key under its prefix, less its `except`.
#[test]
fn starvation_is_judged_in_execution_order_between_rules_that_can_meet() {
    use crate::rules::message_rules::MessageCompileError;
    let starved = |rules: &str| {
        matches!(
            probe_compile(&format!(r#"{{"id":"t","messages":[{rules}]}}"#)),
            Err(MessageCompileError::StarvedReading { .. })
        )
    };
    let compiles = |rules: &str| {
        let body = format!(r#"{{"id":"t","messages":[{rules}]}}"#);
        if let Err(error) = probe_compile(&body) {
            panic!("refused ({error}) - {body}");
        }
    };
    let take_x = |rank: u32, extra: &str| {
        format!(
            r#"{{"id":"t.take_x","where":{{"source":"attr:marker","exists":true}},"read":{{"attribute":"x"}},
                "parse":"text","tag_as":"taken","emit":"message","priority":{rank}{extra}}}"#
        )
    };
    let compose_xy = |rank: u32, extra: &str| {
        format!(
            r#"{{"id":"t.compose","compose":{{"tag":"joined","members":[{{"as":"a","from":"x","parse":"text"}},
                {{"as":"b","from":"y","parse":"text"}}]}},"emit":"message","priority":{rank}{extra}}}"#
        )
    };
    let fallback = r#","source":{"span":{"stage":"fallback"}}"#;

    // A fallback rule runs after every dialect rule, so its rank does not make it the taker...
    compiles(&format!("{},{}", take_x(1, fallback), compose_xy(2, "")));
    // ...and a dialect rule of a later rank still runs first, and starves a fallback composition.
    assert!(starved(&format!(
        "{},{}",
        compose_xy(1, fallback),
        take_x(2, "")
    )));

    // Gates that exclude each other never meet: two scopes.
    let scope = |name: &str| format!(r#","where":{{"source":"scope.name","equals":"{name}"}}"#);
    let take_in = |name: &str| {
        format!(
            r#"{{"id":"t.take_x","where":{{"source":"scope.name","equals":"{name}"}},"read":{{"attribute":"x"}},
                "parse":"text","tag_as":"taken","emit":"message","priority":1}}"#
        )
    };
    compiles(&format!(
        "{},{}",
        take_in("one"),
        compose_xy(2, &scope("two"))
    ));
    assert!(
        starved(&format!(
            "{},{}",
            take_in("one"),
            compose_xy(2, &scope("one"))
        )),
        "the same scope meets"
    );

    // A sweep owns the keys under its prefix, so an earlier reader of one starves it - unless it excepts it.
    let take_name = r#"{"id":"t.take_name","where":{"source":"attr:marker","exists":true},
        "read":{"attribute":"p.name"},"parse":"text","tag_as":"taken","emit":"message","priority":1}"#;
    let sweep = |except: &str| {
        format!(
            r#"{{"id":"t.sweep","compose":{{"tag":"swept","members":[{{"sweep_prefix":"p."{except}}}]}},
                "emit":"message","priority":2}}"#
        )
    };
    assert!(starved(&format!("{take_name},{}", sweep(""))));
    compiles(&format!("{take_name},{}", sweep(r#","except":["name"]"#)));
}

/// **A compose's fallback gives way to the carrier's owner, and the compose keeps the rest.** The two shipped
/// readings of `output.value` meet on this span: OpenInference's output family, whose overlay joins the
/// serialised LangChain generation in `output.value` (ranked first), and the Vercel response compose, whose
/// content falls back to `output.value` on Vercel evidence when no response text was written. The overlay keeps
/// `output.value`; the compose gives up its content member and still reports the tool calls only it read -
/// where it used to be dropped whole.
#[test]
fn a_compose_fallback_gives_way_to_the_carrier_s_owner_and_keeps_the_rest() {
    use crate::rules::message_rules::{MessageContext, OwnedCarrier};
    let plan = &super::ruleset().messages;
    let attrs = probe_attrs(&[
        ("llm.output_messages.0.message.role", "assistant"),
        (
            "llm.output_messages.0.message.contents.0.message_content.type",
            "text",
        ),
        (
            "llm.output_messages.0.message.contents.0.message_content.text",
            "the answer",
        ),
        (
            "output.value",
            r#"{"generations":[[{"message":{"id":["langchain","schema","messages","AIMessage"],
                "kwargs":{"content":[{"type":"text","text":"the whole answer"}]}}}]]}"#,
        ),
        (
            "ai.response.toolCalls",
            r#"[{"toolCallType":"function","toolCallId":"call_1","toolName":"lookup","args":"{\"q\":1}"}]"#,
        ),
        ("ai.response.finishReason", "tool-calls"),
    ]);
    let ctx = MessageContext::for_span("ai.generateText.doGenerate", &attrs, false);
    let emissions = plan.run(&ctx);
    let attribute = |name: &str| OwnedCarrier {
        is_event: false,
        name: name.to_string(),
    };
    let output_value = attribute("output.value");
    let family: Vec<_> = emissions
        .iter()
        .filter(|e| e.rule_id == "openinference.output_messages")
        .collect();
    assert_eq!(family.len(), 1, "{emissions:#?}");
    assert!(
        family[0].owns.contains(&output_value),
        "the overlay joined `output.value`, so it owns it"
    );
    assert!(family[0].value.to_string().contains("the whole answer"));
    let response: Vec<_> = emissions
        .iter()
        .filter(|e| e.rule_id == "vercel-ai.response")
        .collect();
    assert_eq!(
        response.len(),
        1,
        "the compose is kept, not dropped whole: {emissions:#?}"
    );
    let members = response[0].value.as_object().expect("a composed message");
    assert!(
        members.get("content").is_none(),
        "the content it read through the fallback is given up: {members:?}"
    );
    assert!(members.contains_key("tool_calls"), "{members:?}");
    assert_eq!(
        members.keys().map(String::as_str).collect::<Vec<_>>(),
        ["tool_calls", "finishReason", "role"],
        "the members it kept, in the order it wrote them"
    );
    assert!(
        !response[0].owns.contains(&output_value),
        "and it does not own the carrier it gave up"
    );
    assert!(
        response[0]
            .owns
            .contains(&attribute("ai.response.toolCalls"))
    );
}

/// **Only a message compose's member may fall back.** A fallback gives way to its carrier's owner, which only the
/// message arena arbitrates: a tool definition is wrapped before any claim is asked and tool metadata is claimed
/// per axis, so a fallback there would drop the whole reading again.
#[test]
fn only_a_message_compose_s_member_falls_back() {
    let compose = |extra: &str, emit: &str| {
        format!(
            r#"{{"id":"t","messages":[{{"id":"t.c","priority":1,"emit":"{emit}","compose":{{"tag":"t.tag"{extra},
                "members":[{{"as":"name","from":"t.name","parse":"text",
                  "fallback":{{"from":"t.other","where":{{"source":"attr:t.evidence","exists":true}},"parse":"text"}}}},
                  {{"as":"description","from":"t.description","parse":"text"}}]}}}}]}}"#
        )
    };
    assert!(probe_compile(&compose("", "message")).is_ok());
    for (why, body) in [
        (
            "a tool definition",
            compose(r#","as_tool_definition":true"#, "tool_definitions"),
        ),
        ("a name list", compose("", "tool_names")),
    ] {
        assert!(probe_compile(&body).is_err(), "{why}: compiled - {body}");
    }
}

/// **A provider asset's keys are a producer's keys.** Both attribute-key sweeps count them: pricing may name a
/// provider, which spares the spellings the catalogue maps inside the pricing module and nothing else.
#[test]
fn a_provider_asset_s_keys_are_caught_by_the_key_sweeps() {
    let keys = framework_attribute_keys();
    let line = |literal: &str| format!("fn probe() {{ let key = \"{literal}\"; }}");
    for key in ["llm.provider", "llm.system"] {
        assert!(
            !attribute_key_offenders("server/crates/domain/src/probe.rs", &line(key), &keys)
                .is_empty(),
            "`{key}`, which only a provider asset declares, passes the attribute-key sweep"
        );
    }
    assert!(
        attribute_key_offenders(
            "server/crates/domain/src/pricing/matching.rs",
            &line("aws.bedrock"),
            &keys
        )
        .is_empty(),
        "a provider's name, where it is priced"
    );
    assert!(
        !attribute_key_offenders(
            "server/crates/domain/src/probe.rs",
            &line("aws.bedrock"),
            &keys
        )
        .is_empty(),
        "the same spelling anywhere else is a provider asset's value"
    );
}

/// **Ordering and ownership follow what each path actually runs.** A dotted family owns its keys together, so an
/// earlier reader of one starves it; the metadata path runs every stage and claims per axis, so two stages'
/// definition readers contend and a definition reader beside a name reader does not; and a `raw_where` is a
/// condition on the payload, so two readers it separates take turns.
#[test]
fn arenas_and_claims_follow_what_each_path_runs() {
    use crate::rules::message_rules::MessageCompileError;
    let compiled = |rules: &str| probe_compile(&format!(r#"{{"id":"t","messages":[{rules}]}}"#));
    // A family read whole, after a gated reader of one of its keys.
    assert!(matches!(
        compiled(
            r#"{"id":"t.take","where":{"source":"attr:marker","exists":true},"read":{"attribute":"f.a"},
                "parse":"text","tag_as":"t.taken","emit":"message","priority":1},
               {"id":"t.family","read":{"family":"f."},"emit":"message","priority":2}"#
        ),
        Err(MessageCompileError::StarvedReading { .. })
    ));
    // Two definition readers of one carrier at different stages: one rank between them decides, so a shared
    // priority is refused and an unconditional earlier one leaves the later dead.
    let definitions = |priority_b: u32| {
        format!(
            r#"{{"id":"t.d","read":{{"attribute":"tools"}},"parse":"json","emit":"tool_definitions","priority":1}},
               {{"id":"t.f","source":{{"span":{{"stage":"fallback"}}}},"read":{{"attribute":"tools"}},"parse":"json",
                 "emit":"tool_definitions","priority":{priority_b}}}"#
        )
    };
    assert!(
        compiled(&definitions(1)).is_err(),
        "a shared priority across stages"
    );
    assert!(
        compiled(&definitions(2)).is_err(),
        "an unconditional earlier reader leaves the fallback-stage one dead on the metadata path"
    );
    // A definition list and a name list are two arenas: one priority is no tie.
    if let Err(error) = compiled(
        r#"{"id":"t.d","read":{"attribute":"tools"},"parse":"json","emit":"tool_definitions","priority":1},
           {"id":"t.n","read":{"attribute":"names"},"parse":"json","emit":"tool_names","priority":1}"#,
    ) {
        panic!("definitions and names were put in one arena: {error}");
    }
    // Two readers one carrier's raw text separates.
    if let Err(error) = compiled(
        r#"{"id":"t.paren","read":{"attribute":"x"},"raw_where":{"starts_with":"("},"parse":"text",
            "tag_as":"t.paren","emit":"message","priority":1,"wrap":{"role":"user"}},
           {"id":"t.plain","read":{"attribute":"x"},"raw_where":{"lacks_prefix":"("},"parse":"text",
            "tag_as":"t.plain","emit":"message","priority":2,"wrap":{"role":"user"}}"#,
    ) {
        panic!("raw_where separates them, and they were refused as contenders: {error}");
    }
}
