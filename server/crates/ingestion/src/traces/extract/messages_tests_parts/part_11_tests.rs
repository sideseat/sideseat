
/// The convention's inference-details container is read on a tool span too.
///
/// The retired extractor answered this event *before* any tool-span check, so declaring the rules without
/// `reads_tool_spans` narrowed them. The bundled tool result is the opposite case and correctly stays
/// gated: it was guarded by `!is_tool_span` in the code it replaced.
#[test]
fn the_inference_details_container_is_read_on_a_tool_span() {
    let event = Event {
        name: "gen_ai.client.inference.operation.details".to_string(),
        time_unix_nano: 1_702_400_000_000_000_000,
        attributes: vec![make_kv(
            "gen_ai.input.messages",
            r#"[{"role":"user","content":"hi"}]"#,
        )],
        dropped_attributes_count: 0,
    };
    let on_tool_span = extract_message_from_event(&event, "", &HashMap::new(), true);
    // The *carrier*, not the count: without the rule the raw event is emitted instead, which is also one
    // message and would let this pass for the wrong reason.
    let carriers: Vec<String> = on_tool_span
        .iter()
        .map(|m| match &m.source {
            MessageSource::Event { name, .. } => name.clone(),
            MessageSource::Attribute { key, .. } => key.clone(),
        })
        .collect();
    assert_eq!(
        carriers,
        vec!["gen_ai.input.messages".to_string()],
        "the container was narrowed on a tool span, or emitted as its own raw form"
    );
}

/// The fallback stage does not re-read a carrier the dialect stage already read.
///
/// The two stages meet on one path: a generation span whose *answer* is unaccounted for reads the fallback
/// after a dialect produced something. With independent claim sets, a dialect's reading of `output.value`
/// and the fallback's reading of it both survive - the answer twice. That is the shape the compiler permits
/// (a dialect `Claim` on the generic carrier) and no asset currently produces, so it is tested at the
/// mechanism rather than through a fixture.
///
/// Asserted on the **inheritance** itself, because the names carry their kind as an `attr:` / `event:`
/// prefix - the caller's own spelling. Taking them as bare attribute names was a defect that made this do
/// nothing at all, and a span-level test could not see it.
#[test]
fn the_fallback_inherits_what_the_dialect_stage_read() {
    let attrs = make_attrs(&[(
        "output.value",
        r#"{"role":"assistant","content":"the answer"}"#,
    )]);
    let ctx = sideseat_domain::rules::MessageContext::for_span("call_llm", &attrs, false);
    let plan = &ruleset().messages;

    let read_afresh = plan.fallback(&ctx, &std::collections::HashSet::new());
    assert!(
        read_afresh
            .iter()
            .any(|e| e.carrier.name() == "output.value"),
        "the fallback should read the generic carrier when nothing has"
    );

    let inherited = plan.fallback(
        &ctx,
        &std::collections::HashSet::from([sideseat_domain::rules::message_rules::OwnedCarrier {
            is_event: false,
            name: "output.value".to_string(),
        }]),
    );
    assert!(
        !inherited.iter().any(|e| e.carrier.name() == "output.value"),
        "the fallback re-read a carrier the dialect stage had already read: {inherited:?}"
    );
}

/// The retired single-tool triple, frozen as the equivalence oracle's reference.
///
/// Not history: the declared rule must reproduce it, including the identifier test that excludes a
/// synthetic aggregate reported under a parenthesised name, and the position - it ran *after* every other
/// definition had been collected, and merging keeps the first of two equal-quality definitions with one
/// name, so the order decides which description and schema win.
///
/// Copied from `extract_tool_definitions` as of **`08f69b86`**, the commit that declared it. A hand-copy is
/// as trustworthy as any other reviewed data and no less so than a golden - but it cannot be re-derived, so
/// the commit is named here for anyone who needs to check it against the original.
fn legacy_single_tool_definition(
    attrs: &HashMap<String, String>,
    timestamp: DateTime<Utc>,
) -> Option<RawToolDefinition> {
    let tool_name = attrs.get("gen_ai.tool.name")?;
    if !tool_name.starts_with(|c: char| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let mut func = json!({ "name": tool_name });
    if let Some(desc) = attrs.get("gen_ai.tool.description") {
        func["description"] = json!(desc);
    }
    if let Some(schema) = attrs
        .get("gen_ai.tool.json_schema")
        .and_then(|s| serde_json::from_str::<JsonValue>(s).ok())
    {
        func["parameters"] = schema;
    }
    Some(RawToolDefinition::from_attr(
        "gen_ai.tool.name",
        timestamp,
        json!([{"type": "function", "function": func}]),
    ))
}

/// The declared triple reproduces the retired one, and comes last.
#[test]
fn the_declared_single_tool_triple_reproduces_the_retired_one() {
    let cases: Vec<Vec<(&str, &str)>> = vec![
        vec![("gen_ai.tool.name", "weather")],
        vec![
            ("gen_ai.tool.name", "weather"),
            ("gen_ai.tool.description", "looks it up"),
        ],
        vec![
            ("gen_ai.tool.name", "weather"),
            ("gen_ai.tool.description", "looks it up"),
            ("gen_ai.tool.json_schema", r#"{"type":"object"}"#),
        ],
        // The synthetic aggregate one dialect reports: not a tool anyone can call.
        vec![("gen_ai.tool.name", "(merged tools)")],
        vec![("gen_ai.tool.name", "_private")],
        vec![("gen_ai.tool.name", "9lives")],
        // A malformed schema is not a schema.
        vec![
            ("gen_ai.tool.name", "weather"),
            ("gen_ai.tool.json_schema", "{not json"),
        ],
    ];
    for case in cases {
        let attrs = make_attrs(&case);
        let expected = legacy_single_tool_definition(&attrs, Utc::now());
        let (defs, _) = extract_tool_definitions("", &attrs, Utc::now());
        let declared: Vec<&RawToolDefinition> = defs
            .iter()
            .filter(
                |d| matches!(&d.source, ToolDefinitionSource::Attribute { key, .. } if key == "gen_ai.tool.name"),
            )
            .collect();
        match expected {
            None => assert!(
                declared.is_empty(),
                "the declared rule emitted a definition the retired one refused: {case:?} -> {declared:?}"
            ),
            Some(want) => {
                assert_eq!(declared.len(), 1, "{case:?}");
                assert_eq!(declared[0].content, want.content, "{case:?}");
            }
        }
    }
}

/// The triple comes after every other tool definition, which is what decides the merge.
#[test]
fn the_single_tool_triple_is_read_last() {
    let attrs = make_attrs(&[
        ("llm.tools", r#"[{"name":"listed"}]"#),
        ("gen_ai.tool.name", "triple"),
    ]);
    let (defs, _) = extract_tool_definitions("", &attrs, Utc::now());
    let order: Vec<String> = defs
        .iter()
        .map(|d| match &d.source {
            ToolDefinitionSource::Attribute { key, .. } => key.clone(),
        })
        .collect();
    assert_eq!(
        order,
        vec!["llm.tools".to_string(), "gen_ai.tool.name".to_string()],
        "the triple must come last - merging keeps the first of two equal-quality definitions with one name"
    );

    // The consequence, asserted rather than described: with the *same* name in both, the position decides
    // which description survives. Reversed, the triple's would win and the list's would be dropped.
    let same_name = make_attrs(&[
        (
            "llm.tools",
            r#"[{"type":"function","function":{"name":"shared","description":"from the list"}}]"#,
        ),
        ("gen_ai.tool.name", "shared"),
        ("gen_ai.tool.description", "from the triple"),
    ]);
    let (defs, _) = extract_tool_definitions("", &same_name, Utc::now());
    let merged = sideseat_domain::sideml::tools::normalize_tools(&JsonValue::Array(
        defs.iter()
            .flat_map(|d| d.content.as_array().cloned().unwrap_or_default())
            .collect(),
    ));
    let descriptions: Vec<&str> = merged
        .as_array()
        .expect("an array of tools")
        .iter()
        .filter_map(|t| t["function"]["description"].as_str())
        .collect();
    // The order survives normalisation, which is what the position guarantees at this level: the merge that
    // keeps the first of two equal-quality definitions with one name happens later, in the feed, and it sees
    // the list's definition first. Reversed here, the triple's description would be the one it kept.
    assert_eq!(
        descriptions,
        vec!["from the list", "from the triple"],
        "the order the merge will see is wrong: {merged:?}"
    );
}

/// CrewAI's metadata carriers, over the matrix the oracle's two cases do not reach.
///
/// The oracle compares only the shapes its cases carry, and those are two minimal `crew_tasks` payloads.
/// This covers what the retired reader actually did and what a re-derivation gets wrong: **both** carriers,
/// **both** member names *together* (it read `tools_names` and `tools`, not one or the other), a rich
/// definition beside a bare name for the same tool (quality selection, which decides which survives), and a
/// `repr` string (the grammar, reached through the declared vocabulary).
#[test]
fn the_crew_metadata_carriers_yield_what_the_retired_reader_did() {
    let names_of = |defs: &[RawToolDefinition], carrier: &str| -> Vec<String> {
        defs.iter()
            .filter(
                |d| matches!(&d.source, ToolDefinitionSource::Attribute { key, .. } if key == carrier),
            )
            .flat_map(|d| d.content.as_array().cloned().unwrap_or_default())
            .filter_map(|t| {
                t["function"]["name"]
                    .as_str()
                    .or_else(|| t["name"].as_str())
                    .map(str::to_string)
            })
            .collect()
    };

    // Both carriers are read, each as its own observation.
    let attrs = make_attrs(&[
        ("crew_key", "k"),
        ("crew_agents", r#"[{"tools_names":["from_agents"]}]"#),
        ("crew_tasks", r#"[{"tools_names":["from_tasks"]}]"#),
    ]);
    let (defs, _) = extract_tool_definitions("", &attrs, Utc::now());
    assert_eq!(
        names_of(&defs, "crew_agents"),
        vec!["from_agents".to_string()]
    );
    assert_eq!(
        names_of(&defs, "crew_tasks"),
        vec!["from_tasks".to_string()]
    );

    // Both member names in one entry, together - not one or the other.
    let attrs = make_attrs(&[
        ("crew_key", "k"),
        (
            "crew_agents",
            r#"[{"tools_names":["named"],"tools":[{"name":"listed"}]}]"#,
        ),
    ]);
    let (defs, _) = extract_tool_definitions("", &attrs, Utc::now());
    let mut both = names_of(&defs, "crew_agents");
    both.sort();
    assert_eq!(
        both,
        vec!["listed".to_string(), "named".to_string()],
        "both member names are read, which a re-derivation reading one *or* the other misses"
    );

    // A rich definition beside a bare name for one tool: the richer one survives.
    let attrs = make_attrs(&[
        ("crew_key", "k"),
        (
            "crew_agents",
            r#"[{"tools_names":["shared"],"tools":[{"name":"shared","description":"the rich one"}]}]"#,
        ),
    ]);
    let (defs, _) = extract_tool_definitions("", &attrs, Utc::now());
    let described: Vec<String> = defs
        .iter()
        .flat_map(|d| d.content.as_array().cloned().unwrap_or_default())
        .filter_map(|t| t["function"]["description"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        described,
        vec!["the rich one".to_string()],
        "quality selection keeps the richer definition of one name: {defs:?}"
    );
    assert_eq!(names_of(&defs, "crew_agents"), vec!["shared".to_string()]);

    // A `repr` string, reached through the declared vocabulary.
    let attrs = make_attrs(&[
        ("crew_key", "k"),
        (
            "crew_agents",
            r#"[{"tools":["CrewStructuredTool(name='search', description='Tool Arguments: {\"q\": {\"type\": \"str\"}}')"]}]"#,
        ),
    ]);
    let (defs, _) = extract_tool_definitions("", &attrs, Utc::now());
    assert_eq!(names_of(&defs, "crew_agents"), vec!["search".to_string()]);
}

// ============================================================================
// SPAN-FIELD EQUIVALENCE: the declared resolvers against the chains they replaced
// ============================================================================

/// The declared field resolvers produce exactly what the ordered `&[&str]` chains produced.
///
/// Every shape that distinguishes the resolvers: an empty value the chain must step over, a value only a
/// later spelling carries, a value only inside `metadata`, a status code that is not a number, and tag keys
/// that overlap. The oracle is the retired code itself, so this is a provable no-op rather than a hope -
/// which matters because a session id silently lost is a conversation with no session view.
#[test]
fn the_field_rules_reproduce_the_chains_they_replaced() {
    let cases: Vec<(&str, HashMap<String, String>)> = vec![
        ("nothing at all", rule_attrs(&[])),
        (
            "the standard spellings",
            rule_attrs(&[
                ("session.id", "s-1"),
                ("user.id", "u-1"),
                ("http.method", "GET"),
                ("http.url", "https://example/x"),
                ("http.status_code", "200"),
                ("db.system", "postgresql"),
                ("db.name", "main"),
                ("db.operation", "SELECT"),
                ("db.statement", "select 1"),
                ("cloud.provider", "aws"),
                ("aws.s3.bucket", "b"),
                ("aws.s3.key", "k"),
                ("messaging.system", "sqs"),
                ("messaging.destination", "q"),
                ("tags", r#"["a","b"]"#),
            ]),
        ),
        (
            "an empty value the chain steps over, and a real one further down",
            rule_attrs(&[
                ("session.id", ""),
                ("gen_ai.conversation.id", "conv-1"),
                ("user.id", ""),
                ("enduser.id", "u-2"),
            ]),
        ),
        (
            "only the newer spellings",
            rule_attrs(&[
                ("http.request.method", "POST"),
                ("url.full", "https://example/y"),
                ("http.response.status_code", "503"),
                ("messaging.destination.name", "topic"),
                ("gcp.gcs.bucket", "gb"),
                ("gcp.gcs.object", "go"),
            ]),
        ),
        (
            "a session and user only inside metadata",
            rule_attrs(&[(
                "metadata",
                r#"{"thread_id":"t-1","user_id":"u-3","langgraph_step":2}"#,
            )]),
        ),
        (
            "the other metadata spelling of a thread",
            rule_attrs(&[("metadata", r#"{"langgraph_thread_id":"t-2"}"#)]),
        ),
        (
            "a status code that is not a number",
            rule_attrs(&[("http.status_code", "OK")]),
        ),
        (
            "tags under several keys, overlapping",
            rule_attrs(&[
                ("tags", r#"["a","b"]"#),
                ("langsmith.tags", r#"["b","c"]"#),
                ("tag.tags", "d"),
            ]),
        ),
        (
            "framework session and user keys",
            rule_attrs(&[
                ("langsmith.session.id", "ls-1"),
                ("ai.telemetry.metadata.userId", "vu-1"),
            ]),
        ),
        (
            "malformed metadata beside a real session key",
            rule_attrs(&[("session.id", "s-2"), ("metadata", "not json at all")]),
        ),
        // The combinations the first spelling of this oracle missed, and each of them was a real divergence:
        // stepping over a *present* value that does not convert reports a later spelling's number as this
        // field's, and coercing a non-string invents an identifier.
        (
            "a status code that is not a number, beside a second key that is",
            rule_attrs(&[
                ("http.status_code", "OK"),
                ("http.response.status_code", "503"),
            ]),
        ),
        (
            "a thread that is an object, beside one that is a string",
            rule_attrs(&[(
                "metadata",
                r#"{"thread_id":{},"langgraph_thread_id":"t-2"}"#,
            )]),
        ),
        (
            "a thread that is a number",
            rule_attrs(&[("metadata", r#"{"thread_id":123}"#)]),
        ),
        (
            "empty values a producer wrote, on fields that are not chains",
            rule_attrs(&[
                ("db.system", ""),
                ("db.name", ""),
                ("db.operation", ""),
                ("db.statement", ""),
                ("cloud.provider", ""),
                ("messaging.system", ""),
            ]),
        ),
        (
            "an empty value on a field that *is* a chain, with nothing after it",
            rule_attrs(&[("session.id", ""), ("http.method", "")]),
        ),
        (
            "a tag source that cannot be read, beside two that can - a merge is a union",
            rule_attrs(&[
                ("tags", r#"{"not":"a list"}"#),
                ("langsmith.tags", r#"["b"]"#),
                ("tag.tags", "c"),
            ]),
        ),
    ];

    for (what, attrs) in cases {
        let mut declared = SpanData::default();
        crate::traces::extract::attributes::apply_span_fields(&mut declared, "a.span", &attrs, &[]);
        let mut legacy = SpanData::default();
        crate::traces::extract::attributes::extract_semantic_legacy(&mut legacy, &attrs);

        // Named facets rather than a tuple, so a mismatch names the field that moved.
        let facet = |span: &SpanData| {
            vec![
                format!("session_id={:?}", span.session_id),
                format!("user_id={:?}", span.user_id),
                format!("http_method={:?}", span.http_method),
                format!("http_url={:?}", span.http_url),
                format!("http_status_code={:?}", span.http_status_code),
                format!("db_system={:?}", span.db_system),
                format!("db_name={:?}", span.db_name),
                format!("db_operation={:?}", span.db_operation),
                format!("db_statement={:?}", span.db_statement),
                format!("storage_system={:?}", span.storage_system),
                format!("storage_bucket={:?}", span.storage_bucket),
                format!("storage_object={:?}", span.storage_object),
                format!("messaging_system={:?}", span.messaging_system),
                format!("messaging_destination={:?}", span.messaging_destination),
                format!("tags={:?}", span.tags),
            ]
        };
        assert_eq!(
            facet(&declared),
            facet(&legacy),
            "the declared resolvers disagree with the chains they replaced: {what}"
        );
    }
}

/// The declared GenAI resolvers produce exactly what the chains they replaced produced.
///
/// The shapes that distinguish them, and every one of these was a decision in the retired code rather than an
/// accident: a flat parameter written badly falls through to the serialised object (unlike a status code, where
/// a second key's number is a different attribute's answer), `request_data` precedes
/// `llm.invocation_parameters` where both hold a parameter, a provider is implied by the *shape* of a request
/// that names none, an agent list is searched for the first agent that declared a model, and a tool's name
/// comes from the span name when no attribute carries it.
#[test]
fn the_genai_field_rules_reproduce_the_chains_they_replaced() {
    let cases: Vec<(&str, &str, HashMap<String, String>)> = vec![
        ("nothing at all", "plain span", rule_attrs(&[])),
        (
            "the conventional spellings",
            "chat gpt-4o",
            rule_attrs(&[
                ("gen_ai.provider.name", "openai"),
                ("gen_ai.operation.name", "chat"),
                ("gen_ai.request.model", "gpt-4o"),
                ("gen_ai.response.model", "gpt-4o-2024-08-06"),
                ("gen_ai.response.id", "resp-1"),
                ("gen_ai.request.temperature", "0.7"),
                ("gen_ai.request.top_p", "0.95"),
                ("gen_ai.request.top_k", "40"),
                ("gen_ai.request.max_tokens", "1024"),
                ("gen_ai.request.frequency_penalty", "0.1"),
                ("gen_ai.request.presence_penalty", "0.2"),
                ("gen_ai.request.stop_sequences", r#"["\n\n","END"]"#),
                ("gen_ai.response.finish_reasons", r#"["stop"]"#),
                ("gen_ai.agent.id", "agent-1"),
                ("gen_ai.agent.name", "Researcher"),
                ("gen_ai.tool.name", "search"),
                ("gen_ai.tool.call.id", "call-1"),
                ("gen_ai.server.time_to_first_token", "120"),
                ("gen_ai.server.request_duration", "980"),
            ]),
        ),
        (
            "the dialect spellings, none of them conventional",
            "ai.generateText",
            rule_attrs(&[
                ("llm.provider", "anthropic"),
                ("llm.model_name", "claude-3-5-sonnet"),
                ("llm.response.model", "claude-3-5-sonnet-20241022"),
                ("agent_role", "Planner"),
                ("tool_name", "fetch"),
                ("aws.bedrock.agent.id", "bedrock-agent-1"),
            ]),
        ),
        (
            "an embedding span, whose model is the only one it has",
            "embedding",
            rule_attrs(&[("embedding.model_name", "text-embedding-3-small")]),
        ),
        (
            "a reranker span",
            "reranking",
            rule_attrs(&[("reranker.model_name", "rerank-v3")]),
        ),
        (
            "parameters only inside the invocation object",
            "RunnableSequence",
            rule_attrs(&[(
                "llm.invocation_parameters",
                r#"{"temperature":0.3,"top_p":0.8,"top_k":10,"max_tokens":256,"frequency_penalty":0.4,"presence_penalty":0.5}"#,
            )]),
        ),
        (
            "the other max-token spelling in the invocation object",
            "RunnableSequence",
            rule_attrs(&[("llm.invocation_parameters", r#"{"max_output_tokens":512}"#)]),
        ),
        (
            "a flat parameter written badly, with the object holding a real one",
            "RunnableSequence",
            rule_attrs(&[
                ("gen_ai.request.temperature", "hot"),
                ("llm.invocation_parameters", r#"{"temperature":0.7}"#),
            ]),
        ),
        (
            "a request the dialect serialised whole, naming no provider",
            "chat",
            rule_attrs(&[(
                "request_data",
                r#"{"model":"claude-3-haiku","system":"be brief","max_tokens":800}"#,
            )]),
        ),
        (
            "the same, in the other provider's shape",
            "chat",
            rule_attrs(&[(
                "request_data",
                r#"{"model":"gpt-4o-mini","messages":[{"role":"user","content":"hi"}],"max_completion_tokens":600}"#,
            )]),
        ),
        (
            "a serialised request beside a flat model, where only the ceiling is missing",
            "chat",
            rule_attrs(&[
                ("gen_ai.request.model", "gpt-4o"),
                ("gen_ai.provider.name", "openai"),
                ("gen_ai.operation.name", "chat"),
                (
                    "request_data",
                    r#"{"model":"ignored","messages":[],"max_tokens":700}"#,
                ),
            ]),
        ),
        (
            "both objects holding the same parameter",
            "chat",
            rule_attrs(&[
                ("request_data", r#"{"max_tokens":111,"messages":[]}"#),
                ("llm.invocation_parameters", r#"{"max_tokens":222}"#),
            ]),
        ),
        (
            "a flat ceiling beside both objects",
            "chat",
            rule_attrs(&[
                ("gen_ai.request.max_tokens", "999"),
                ("request_data", r#"{"max_tokens":111,"messages":[]}"#),
                ("llm.invocation_parameters", r#"{"max_tokens":222}"#),
            ]),
        ),
        (
            "the model on an agent list, the first agent having none",
            "Crew.kickoff",
            rule_attrs(&[(
                "crew_agents",
                r#"[{"role":"a"},{"role":"b","llm":""},{"role":"c","llm":"gpt-4o-mini"}]"#,
            )]),
        ),
        (
            "a dialect's serialised request naming the model",
            "call_llm",
            rule_attrs(&[(
                "gcp.vertex.agent.llm_request",
                r#"{"model":"gemini-2.0-flash"}"#,
            )]),
        ),
        (
            "a tool name only in the span name",
            "execute_tool temperature_forecast",
            rule_attrs(&[]),
        ),
        (
            "a span named exactly the tool prefix",
            "execute_tool ",
            rule_attrs(&[]),
        ),
        (
            "a descriptive tool name in the message field",
            "agent run",
            rule_attrs(&[("logfire.msg", "running the calculator")]),
        ),
        (
            "empty values on the fields read directly",
            "plain span",
            rule_attrs(&[
                ("gen_ai.operation.name", ""),
                ("gen_ai.response.id", ""),
                ("gen_ai.tool.call.id", ""),
            ]),
        ),
        (
            "a hand-off, which names both sides",
            "transfer",
            rule_attrs(&[
                ("recipient_agent_class", "Writer"),
                ("sender_agent_class", "Planner"),
            ]),
        ),
        (
            "a request naming a tool `system`, which is not a top-level `system` member",
            "chat",
            rule_attrs(&[(
                "request_data",
                r#"{"model":"gpt-4o","messages":[],"tools":[{"name":"system"}]}"#,
            )]),
        ),
        (
            "a carrier that is not JSON at all, beside one that is",
            "call_llm",
            rule_attrs(&[
                ("gcp.vertex.agent.llm_request", "not json"),
                ("request_data", r#"{"model":"gpt-4o","messages":[]}"#),
            ]),
        ),
        (
            "an agent list that is not a list, beside a serialised request that names the model",
            "Crew.kickoff",
            rule_attrs(&[
                ("crew_agents", r#"{"not":"a list"}"#),
                ("request_data", r#"{"model":"gpt-4o-mini","messages":[]}"#),
            ]),
        ),
        (
            "one alias written badly beside its sibling, and another carrier behind both",
            "chat",
            rule_attrs(&[
                (
                    "request_data",
                    r#"{"max_tokens":"bad","max_completion_tokens":600,"messages":[]}"#,
                ),
                ("llm.invocation_parameters", r#"{"max_tokens":222}"#),
            ]),
        ),
        (
            "the same shape in the invocation object",
            "RunnableSequence",
            rule_attrs(&[(
                "llm.invocation_parameters",
                r#"{"max_tokens":"bad","max_output_tokens":700}"#,
            )]),
        ),
        (
            "a parameter in the serialised request only, which the retired chain never read",
            "chat",
            rule_attrs(&[(
                "request_data",
                r#"{"temperature":0.1,"top_p":0.2,"top_k":3,"messages":[]}"#,
            )]),
        ),
        (
            "the serialised request and the invocation object disagreeing about a parameter",
            "chat",
            rule_attrs(&[
                ("request_data", r#"{"temperature":0.1,"messages":[]}"#),
                ("llm.invocation_parameters", r#"{"temperature":0.9}"#),
            ]),
        ),
        (
            "malformed objects, which yield nothing rather than failing",
            "chat",
            rule_attrs(&[
                ("request_data", "not json"),
                ("llm.invocation_parameters", "{{{"),
                ("crew_agents", "[[["),
            ]),
        ),
    ];

    for (what, span_name, attrs) in cases {
        let mut declared = SpanData::default();
        crate::traces::extract::attributes::apply_span_fields(
            &mut declared,
            span_name,
            &attrs,
            &[],
        );
        let mut legacy = SpanData::default();
        crate::traces::extract::attributes::extract_genai_fields_legacy(
            &mut legacy,
            &attrs,
            span_name,
        );

        // Named facets, so a mismatch names the field that moved.
        let facet = |span: &SpanData| {
            vec![
                format!("system={:?}", span.gen_ai_system),
                format!("operation={:?}", span.gen_ai_operation_name),
                format!("request_model={:?}", span.gen_ai_request_model),
                format!("response_model={:?}", span.gen_ai_response_model),
                format!("response_id={:?}", span.gen_ai_response_id),
                format!("temperature={:?}", span.gen_ai_temperature),
                format!("top_p={:?}", span.gen_ai_top_p),
                format!("top_k={:?}", span.gen_ai_top_k),
                format!("max_tokens={:?}", span.gen_ai_max_tokens),
                format!("frequency_penalty={:?}", span.gen_ai_frequency_penalty),
                format!("presence_penalty={:?}", span.gen_ai_presence_penalty),
                format!("stop_sequences={:?}", span.gen_ai_stop_sequences),
                format!("finish_reasons={:?}", span.gen_ai_finish_reasons),
                format!("agent_id={:?}", span.gen_ai_agent_id),
                format!("agent_name={:?}", span.gen_ai_agent_name),
                format!("tool_name={:?}", span.gen_ai_tool_name),
                format!("tool_call_id={:?}", span.gen_ai_tool_call_id),
                format!("ttft={:?}", span.gen_ai_server_ttft_ms),
                format!("duration={:?}", span.gen_ai_server_request_duration_ms),
            ]
        };
        assert_eq!(
            facet(&declared),
            facet(&legacy),
            "the declared GenAI resolvers disagree with the chains they replaced: {what}"
        );
    }
}

/// The declared display-name target produces what the retired resolution produced, and the **raw** name is
/// what every behavioural check still sees.
///
/// The second half is the load-bearing one. Detection, token scoping, classification and every message rule
/// are given the producer's own name; the display name is stored beside it. Resolving one into the other would
/// change what a Claude Code span is scoped as, which is how a bare `input_tokens` is credited to the right
/// producer.
#[test]
fn the_display_name_is_declared_and_the_raw_name_is_untouched() {
    let cases: Vec<(&str, &str, HashMap<String, String>)> = vec![
        ("no attributes at all", "GET /things", rule_attrs(&[])),
        (
            "a template resolved into a sentence",
            "chat {model}",
            rule_attrs(&[
                ("logfire.msg_template", "chat {model}"),
                ("logfire.msg", "chat gpt-4o"),
            ]),
        ),
        (
            "a resolved message with no template, which is not a resolution",
            "agent run",
            rule_attrs(&[("logfire.msg", "running the calculator")]),
        ),
        (
            "a template with no resolved message",
            "chat {model}",
            rule_attrs(&[("logfire.msg_template", "chat {model}")]),
        ),
        (
            "a template whose resolved message is empty",
            "chat {model}",
            rule_attrs(&[
                ("logfire.msg_template", "chat {model}"),
                ("logfire.msg", ""),
            ]),
        ),
        (
            "braces in the name with no template attribute",
            "chat {model}",
            rule_attrs(&[]),
        ),
        ("an empty span name", "", rule_attrs(&[])),
    ];

    for (what, raw_name, attrs) in cases {
        // As `set_core_fields` leaves it: the raw name, which the display resolution then may replace.
        let mut declared = SpanData {
            span_name: raw_name.to_string(),
            ..SpanData::default()
        };
        apply_span_fields(&mut declared, raw_name, &attrs, &[]);

        let mut legacy = SpanData {
            span_name: raw_name.to_string(),
            ..SpanData::default()
        };
        crate::traces::extract::attributes::resolve_span_name(&mut legacy, &attrs);

        assert_eq!(
            declared.span_name, legacy.span_name,
            "the declared display name disagrees with the resolution it replaced: {what}"
        );
    }

    // And the raw name a behavioural check reads is the producer's, whatever the display name became.
    let attrs = rule_attrs(&[
        ("logfire.msg_template", "chat {model}"),
        ("logfire.msg", "chat gpt-4o"),
    ]);
    let mut span = SpanData {
        span_name: "claude_code.api_request".to_string(),
        ..SpanData::default()
    };
    apply_span_fields(&mut span, "claude_code.api_request", &attrs, &[]);
    assert_eq!(
        span.span_name, "chat gpt-4o",
        "the display name is resolved"
    );
    // The scoped token fallback is keyed on the *raw* name, so it still recognises this span.
    let mut scoped = SpanData::default();
    let mut with_tokens = attrs.clone();
    with_tokens.insert("input_tokens".to_string(), "17".to_string());
    crate::traces::extract::attributes::tests::extract_genai_as_production_does(
        &mut scoped,
        &with_tokens,
        "claude_code.api_request",
    );
    assert_eq!(
        scoped.gen_ai_usage_input_tokens, 17,
        "the bare counter is credited because scoping reads the raw span name, not the display name"
    );
}
