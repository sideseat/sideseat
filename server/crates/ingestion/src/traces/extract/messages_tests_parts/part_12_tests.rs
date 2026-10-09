/// The declared counter resolvers find exactly what the retired table found - value **and** presence.
///
/// Presence is half the point: a counter nothing carried and a genuine `0` are different statements about a
/// call, and every framework fallback downstream tests the difference. So this compares `Option<i64>` per
/// counter rather than the stored `i64`, which cannot hold it.
///
/// The shapes that distinguish them: an empty primary alias with a good one behind it (the retired chain
/// selected by presence and then converted, so the empty one ended the *flat* group), a bare scoped name on a
/// span whose name admits it and on one that does not, a reported zero, and each dialect's spelling.
#[test]
fn the_token_rules_reproduce_the_table_they_replaced() {
    let cases: Vec<(&str, &str, HashMap<String, String>)> = vec![
        ("nothing at all", "chat", rule_attrs(&[])),
        (
            "the conventional spellings",
            "chat",
            rule_attrs(&[
                ("gen_ai.usage.input_tokens", "100"),
                ("gen_ai.usage.output_tokens", "50"),
                ("gen_ai.usage.total_tokens", "150"),
                ("gen_ai.usage.cache_read_input_tokens", "20"),
                ("gen_ai.usage.cache_creation_input_tokens", "10"),
                ("gen_ai.usage.output_reasoning_tokens", "5"),
            ]),
        ),
        (
            "a reported zero, which is a count and not a silence",
            "chat",
            rule_attrs(&[
                ("gen_ai.usage.input_tokens", "0"),
                ("gen_ai.usage.output_tokens", "0"),
            ]),
        ),
        (
            "an empty primary with a good alias behind it",
            "chat",
            rule_attrs(&[
                ("gen_ai.usage.input_tokens", ""),
                ("llm.usage.prompt_tokens", "10"),
            ]),
        ),
        (
            "an unreadable primary with a good alias behind it",
            "chat",
            rule_attrs(&[
                ("gen_ai.usage.input_tokens", "many"),
                ("llm.usage.prompt_tokens", "10"),
            ]),
        ),
        (
            "only a later alias",
            "chat",
            rule_attrs(&[
                ("llm.token_count.prompt", "7"),
                ("ai.usage.completionTokens", "3"),
                ("llm.token_count.total", "10"),
            ]),
        ),
        (
            "the dotted cache spellings",
            "chat",
            rule_attrs(&[
                ("gen_ai.usage.cache_read.input_tokens", "11"),
                ("gen_ai.usage.cache_creation.input_tokens", "12"),
                ("gen_ai.usage.reasoning.output_tokens", "13"),
            ]),
        ),
        (
            "one dialect's cache-write spelling",
            "chat",
            rule_attrs(&[("gen_ai.usage.cache_write_input_tokens", "14")]),
        ),
        (
            "bare names on a span whose name admits them",
            "claude_code.api_request",
            rule_attrs(&[
                ("input_tokens", "17"),
                ("output_tokens", "18"),
                ("cache_read_tokens", "19"),
                ("cache_creation_tokens", "20"),
            ]),
        ),
        (
            "the same bare names on a span whose name does not",
            "chat",
            rule_attrs(&[
                ("input_tokens", "17"),
                ("output_tokens", "18"),
                ("cache_read_tokens", "19"),
                ("cache_creation_tokens", "20"),
            ]),
        ),
        (
            "a conventional counter beside a bare one, on a span that admits both",
            "claude_code.api_request",
            rule_attrs(&[("gen_ai.usage.input_tokens", "100"), ("input_tokens", "17")]),
        ),
        (
            "an empty conventional counter beside a bare one",
            "claude_code.api_request",
            rule_attrs(&[("gen_ai.usage.input_tokens", ""), ("input_tokens", "17")]),
        ),
        // The embedded usage objects, which are further sources on the same targets.
        (
            "one dialect's own usage blob, with no flat counters",
            "chat",
            rule_attrs(&[(
                "mlflow.chat.tokenUsage",
                r#"{"prompt_tokens":11,"completion_tokens":22}"#,
            )]),
        ),
        (
            "the same blob's other member names",
            "chat",
            rule_attrs(&[(
                "mlflow.chat.tokenUsage",
                r#"{"input_tokens":11,"output_tokens":22}"#,
            )]),
        ),
        (
            "usage nested in a serialised response",
            "call_llm",
            rule_attrs(&[(
                "gcp.vertex.agent.llm_response",
                r#"{"usage_metadata":{"prompt_token_count":31,"candidates_token_count":32}}"#,
            )]),
        ),
        (
            "only one side flat, the other nested",
            "call_llm",
            rule_attrs(&[
                ("gen_ai.usage.input_tokens", "100"),
                (
                    "gcp.vertex.agent.llm_response",
                    r#"{"usage_metadata":{"prompt_token_count":31,"candidates_token_count":32}}"#,
                ),
            ]),
        ),
        (
            "a third dialect's response object, in each provider's spelling",
            "chat",
            rule_attrs(&[(
                "response_data",
                r#"{"usage":{"input_tokens":41,"output_tokens":42,"cache_read_input_tokens":43,"cache_creation_input_tokens":44}}"#,
            )]),
        ),
        (
            "the same object in the other provider's spelling",
            "chat",
            rule_attrs(&[(
                "response_data",
                r#"{"usage":{"prompt_tokens":41,"completion_tokens":42}}"#,
            )]),
        ),
        (
            "two embedded objects, where the first supplies both",
            "chat",
            rule_attrs(&[
                (
                    "mlflow.chat.tokenUsage",
                    r#"{"prompt_tokens":11,"completion_tokens":22}"#,
                ),
                (
                    "response_data",
                    r#"{"usage":{"input_tokens":99,"output_tokens":88}}"#,
                ),
            ]),
        ),
        (
            "two embedded objects, where the first supplies one side only",
            "chat",
            rule_attrs(&[
                ("mlflow.chat.tokenUsage", r#"{"prompt_tokens":11}"#),
                (
                    "response_data",
                    r#"{"usage":{"input_tokens":99,"output_tokens":88}}"#,
                ),
            ]),
        ),
        (
            "an embedded count written as a quoted number, which is not a number",
            "chat",
            rule_attrs(&[(
                "mlflow.chat.tokenUsage",
                r#"{"prompt_tokens":"11","completion_tokens":22}"#,
            )]),
        ),
        (
            "an unreadable flat counter with an embedded object behind it",
            "chat",
            rule_attrs(&[
                ("gen_ai.usage.input_tokens", "many"),
                (
                    "response_data",
                    r#"{"usage":{"input_tokens":41,"output_tokens":42}}"#,
                ),
            ]),
        ),
        (
            "an embedded zero, which is a count",
            "chat",
            rule_attrs(&[(
                "response_data",
                r#"{"usage":{"input_tokens":0,"output_tokens":0}}"#,
            )]),
        ),
        // The candidates: resolved whatever the chains answered, because the decision reading them needs them.
        (
            "one dialect's embedded object, on a span its gate admits",
            "Crew.kickoff",
            rule_attrs(&[
                ("crew_key", "k"),
                (
                    "output.value",
                    r#"{"token_usage":{"prompt_tokens":500,"completion_tokens":600,"total_tokens":2000,"cached_prompt_tokens":100}}"#,
                ),
            ]),
        ),
        (
            "the same payload with no gate attribute, which is a different framework's key",
            "chat",
            rule_attrs(&[(
                "output.value",
                r#"{"token_usage":{"prompt_tokens":500,"completion_tokens":600}}"#,
            )]),
        ),
        (
            "the embedded object beside flat counters, which must not hide it",
            "Crew.kickoff",
            rule_attrs(&[
                ("crew_key", "k"),
                ("gen_ai.usage.input_tokens", "500"),
                ("gen_ai.usage.output_tokens", "600"),
                (
                    "output.value",
                    r#"{"token_usage":{"prompt_tokens":500,"completion_tokens":600,"total_tokens":2000}}"#,
                ),
            ]),
        ),
        (
            "usage recorded per message, summed across them",
            "chain",
            rule_attrs(&[(
                "output.value",
                r#"{"messages":[{"models_usage":{"prompt_tokens":5,"completion_tokens":6}},{"models_usage":{"prompt_tokens":7,"completion_tokens":8}}]}"#,
            )]),
        ),
        (
            "one message with no usage object among others that have one",
            "chain",
            rule_attrs(&[(
                "output.value",
                r#"{"messages":[{"models_usage":null},{"content":"x"},{"models_usage":{"prompt_tokens":7,"completion_tokens":8}}]}"#,
            )]),
        ),
        (
            "per-message usage on one side only",
            "chain",
            rule_attrs(&[(
                "output.value",
                r#"{"messages":[{"models_usage":{"prompt_tokens":5}}]}"#,
            )]),
        ),
        (
            "a payload with no messages at all",
            "chain",
            rule_attrs(&[("output.value", r#"{"result":"done"}"#)]),
        ),
        (
            "a flat cache counter beside an embedded one",
            "chat",
            rule_attrs(&[
                ("gen_ai.usage.cache_read_input_tokens", "17"),
                (
                    "response_data",
                    r#"{"usage":{"cache_read_input_tokens":100,"cache_creation_input_tokens":5}}"#,
                ),
            ]),
        ),
    ];

    for (what, span_name, attrs) in cases {
        let mut span = SpanData::default();
        let declared = apply_span_fields(
            sideseat_domain::rules::ruleset(),
            &mut span,
            span_name,
            &attrs,
            &[],
        );
        let legacy = crate::traces::extract::attributes::token_readings_legacy(&attrs, span_name);
        let facet = |t: &crate::traces::extract::attributes::TokenReadings| {
            vec![
                format!("input={:?}", t.input),
                format!("output={:?}", t.output),
                format!("total_reported={:?}", t.total_reported),
                format!("cache_read={:?}", t.cache_read),
                format!("cache_write={:?}", t.cache_write),
                format!("reasoning={:?}", t.reasoning),
                format!("candidate_input={:?}", t.candidate_input),
                format!("candidate_output={:?}", t.candidate_output),
                format!("candidate_cache_read={:?}", t.candidate_cache_read),
                format!("candidate_total={:?}", t.candidate_total),
                // The two sums are compared by their **effective** value, because the pairing that reads them
                // cannot distinguish absent from zero: it asks `pt > 0 || ct > 0`. Comparing the `Option`
                // would hold the declared form to a distinction no consumer makes.
                format!("summed_input={}", t.summed_input.unwrap_or(0)),
                format!("summed_output={}", t.summed_output.unwrap_or(0)),
            ]
        };
        assert_eq!(
            facet(&declared),
            facet(&legacy),
            "the declared counter resolvers disagree with the table they replaced: {what}"
        );
    }
}

/// The declared classification plan answers exactly as `detect_observation_type` does.
///
/// A **shadow** comparison: nothing in production reads the plan yet. Classification is a precedence, so the
/// interesting cases are spans that satisfy two rules at once - and each of these was written from a specific
/// arm of the retired sweep, including the three that need a conjunction and the two orderings that are not
/// obvious (a span carrying two dialects' span-kind attributes is what the earlier *key* says, and a name
/// containing both `agent` and `tool` is an agent).
#[test]
fn the_declared_classification_matches_the_sweep_it_shadows() {
    use crate::traces::extract::attributes::detect_observation_type_legacy;
    use sideseat_ports::types::ObservationType;

    let label = |t: ObservationType| t.as_str().to_string();
    let cases: Vec<(&str, &str, HashMap<String, String>)> = vec![
        ("nothing at all", "some span", rule_attrs(&[])),
        (
            "a transport attribute, which outranks everything",
            "chat gpt-4o",
            rule_attrs(&[
                ("http.method", "POST"),
                ("gen_ai.operation.name", "chat"),
                ("gen_ai.request.model", "gpt-4o"),
            ]),
        ),
        (
            "a database call beside a model",
            "select",
            rule_attrs(&[("db.system", "postgresql"), ("gen_ai.request.model", "x")]),
        ),
        ("an rpc call", "grpc", rule_attrs(&[("rpc.system", "grpc")])),
        (
            "a chat completion",
            "chat",
            rule_attrs(&[("gen_ai.operation.name", "chat")]),
        ),
        (
            "a text completion",
            "complete",
            rule_attrs(&[("gen_ai.operation.name", "text_completion")]),
        ),
        (
            "a chat completion whose model is an embedding model",
            "chat",
            rule_attrs(&[
                ("gen_ai.operation.name", "chat"),
                ("gen_ai.request.model", "amazon.titan-embed-text-v2:0"),
            ]),
        ),
        (
            "the same, with the model only on the response side and capitalised",
            "chat",
            rule_attrs(&[
                ("gen_ai.operation.name", "text_completion"),
                ("gen_ai.response.model", "Titan-EMBED-v2"),
            ]),
        ),
        (
            "a chat model answered by a mislabelled response model - the request model applies",
            "chat",
            rule_attrs(&[
                ("gen_ai.operation.name", "chat"),
                ("gen_ai.request.model", "gpt-4o"),
                ("gen_ai.response.model", "text-embedding-3-small"),
            ]),
        ),
        (
            "an empty request model beside an embedding response model",
            "chat",
            rule_attrs(&[
                ("gen_ai.operation.name", "chat"),
                ("gen_ai.request.model", ""),
                ("gen_ai.response.model", "text-embedding-3-small"),
            ]),
        ),
        (
            "an embeddings operation",
            "embed",
            rule_attrs(&[("gen_ai.operation.name", "embeddings")]),
        ),
        (
            "one dialect's agent construction",
            "create_agent",
            rule_attrs(&[
                ("gen_ai.operation.name", "create_agent"),
                ("gen_ai.system", "autogen"),
            ]),
        ),
        (
            "the same operation name from anywhere else, which falls through",
            "agent build",
            rule_attrs(&[
                ("gen_ai.operation.name", "create_agent"),
                ("gen_ai.system", "openai"),
            ]),
        ),
        (
            "an unrecognised operation name, which falls through",
            "some span",
            rule_attrs(&[("gen_ai.operation.name", "rerank")]),
        ),
        (
            "a dialect's span kind",
            "RunnableSequence",
            rule_attrs(&[("openinference.span.kind", "CHAIN")]),
        ),
        (
            "the same in lower case",
            "RunnableSequence",
            rule_attrs(&[("openinference.span.kind", "retriever")]),
        ),
        (
            "the other dialect's span kind",
            "run",
            rule_attrs(&[("langsmith.span.kind", "TOOL")]),
        ),
        (
            "both dialects' span kinds, disagreeing - the earlier key wins",
            "run",
            rule_attrs(&[
                ("openinference.span.kind", "TOOL"),
                ("langsmith.span.kind", "AGENT"),
            ]),
        ),
        (
            "the first key naming a kind nobody recognises, so the second answers",
            "run",
            rule_attrs(&[
                ("openinference.span.kind", "SOMETHING_ELSE"),
                ("langsmith.span.kind", "AGENT"),
            ]),
        ),
        (
            "an operation name beside a span kind - the convention outranks it",
            "run",
            rule_attrs(&[
                ("gen_ai.operation.name", "chat"),
                ("openinference.span.kind", "TOOL"),
            ]),
        ),
        (
            "a dialect that names its model on every span",
            "ai.generateText",
            rule_attrs(&[("ai.model.id", "gpt-4o")]),
        ),
        (
            "the same dialect's embedding call",
            "ai.embed",
            rule_attrs(&[
                ("ai.model.provider", "openai"),
                ("ai.operationId", "ai.embedMany"),
            ]),
        ),
        (
            "the same dialect's operation id in the wrong case, which it does not match",
            "ai.embed",
            rule_attrs(&[
                ("ai.model.provider", "openai"),
                ("ai.operationId", "ai.EmbedMany"),
            ]),
        ),
        (
            "a named agent",
            "run",
            rule_attrs(&[("gen_ai.agent.name", "Researcher")]),
        ),
        (
            "a named tool",
            "run",
            rule_attrs(&[("gen_ai.tool.name", "search")]),
        ),
        (
            "an agent name beside a tool name - the agent wins",
            "run",
            rule_attrs(&[
                ("gen_ai.agent.name", "Researcher"),
                ("gen_ai.tool.name", "search"),
            ]),
        ),
        (
            "a name mentioning embedding",
            "Embedding request",
            rule_attrs(&[]),
        ),
        (
            "a name mentioning an agent",
            "AgentExecutor",
            rule_attrs(&[]),
        ),
        (
            "a name mentioning a tool",
            "execute_tool search",
            rule_attrs(&[]),
        ),
        (
            "a name mentioning both an agent and a tool - the agent wins",
            "agent tool call",
            rule_attrs(&[]),
        ),
        (
            "a name mentioning retrieval",
            "Retriever.get",
            rule_attrs(&[]),
        ),
        (
            "a name mentioning a guardrail",
            "guardrail check",
            rule_attrs(&[]),
        ),
        (
            "a name mentioning evaluation",
            "evaluate answer",
            rule_attrs(&[]),
        ),
        (
            "a tag list mentioning a model call",
            "some span",
            rule_attrs(&[("logfire.tags", r#"["LLM","chat"]"#)]),
        ),
        (
            "a name that mentions a tool beside such a tag list - the name wins",
            "tool run",
            rule_attrs(&[("logfire.tags", r#"["LLM"]"#)]),
        ),
        (
            "only a model",
            "some span",
            rule_attrs(&[("gen_ai.request.model", "gpt-4o")]),
        ),
        (
            "only a response model",
            "some span",
            rule_attrs(&[("gen_ai.response.model", "gpt-4o")]),
        ),
    ];

    let plan = &sideseat_domain::rules::ruleset().observation_types;
    for (what, span_name, attrs) in cases {
        let declared = plan
            .observation_type(span_name, &attrs)
            .map(|verdict| verdict.value.to_string())
            // No rule holding is what "a plain span" means, which the caller names rather than the assets.
            .unwrap_or_else(|| label(ObservationType::Span));
        let swept = label(detect_observation_type_legacy(span_name, &attrs));
        assert_eq!(
            declared, swept,
            "the declared classification disagrees with the sweep it shadows: {what}"
        );
    }
}

/// The declared category plan answers exactly as `categorize_span` does.
///
/// The shadow comparison's precedence half, written from specific arms of the retired sweep - including the two
/// places the two classifications deliberately disagree with each other: a tool name outranks an agent name
/// here and the reverse there, and this one reads only the first dialect's span kind where the other reads two.
#[test]
fn the_declared_category_matches_the_sweep_it_shadows() {
    use crate::traces::extract::attributes::categorize_span_legacy;
    use sideseat_ports::types::SpanCategory;

    let cases: Vec<(&str, &str, HashMap<String, String>)> = vec![
        ("nothing at all", "some span", rule_attrs(&[])),
        (
            "a transport call beside a model, which it duplicates",
            "chat",
            rule_attrs(&[
                ("rpc.system", "aws-api"),
                ("gen_ai.operation.name", "chat"),
                ("gen_ai.request.model", "claude-3"),
            ]),
        ),
        (
            "an http call",
            "POST /v1",
            rule_attrs(&[("http.method", "POST")]),
        ),
        (
            "the newer http spelling",
            "POST",
            rule_attrs(&[("http.request.method", "POST")]),
        ),
        (
            "a database call",
            "select",
            rule_attrs(&[("db.system", "postgresql")]),
        ),
        (
            "a queue",
            "send",
            rule_attrs(&[("messaging.system", "sqs")]),
        ),
        (
            "an object-storage call, recognised by prefix",
            "S3.GetObject",
            rule_attrs(&[("aws.s3.bucket", "b"), ("aws.s3.key", "k")]),
        ),
        (
            "a database call beside a queue - the database wins",
            "select",
            rule_attrs(&[("db.system", "postgresql"), ("messaging.system", "sqs")]),
        ),
        (
            "a chat completion",
            "chat",
            rule_attrs(&[("gen_ai.operation.name", "chat")]),
        ),
        (
            "a chat completion of an embedding model",
            "chat",
            rule_attrs(&[
                ("gen_ai.operation.name", "text_completion"),
                ("gen_ai.request.model", "amazon.titan-EMBED-text-v2:0"),
            ]),
        ),
        (
            "a chat model answered by a mislabelled response model - the request model applies",
            "chat",
            rule_attrs(&[
                ("gen_ai.operation.name", "chat"),
                ("gen_ai.request.model", "gpt-4o"),
                ("gen_ai.response.model", "text-embedding-3-small"),
            ]),
        ),
        (
            "an empty request model beside an embedding response model",
            "chat",
            rule_attrs(&[
                ("gen_ai.operation.name", "chat"),
                ("gen_ai.request.model", ""),
                ("gen_ai.response.model", "text-embedding-3-small"),
            ]),
        ),
        (
            "an embeddings operation",
            "embed",
            rule_attrs(&[("gen_ai.operation.name", "embeddings")]),
        ),
        (
            "a tool execution",
            "execute_tool x",
            rule_attrs(&[("gen_ai.operation.name", "execute_tool")]),
        ),
        (
            "an agent invocation",
            "invoke",
            rule_attrs(&[("gen_ai.operation.name", "invoke_agent")]),
        ),
        (
            "a swarm invocation",
            "invoke",
            rule_attrs(&[("gen_ai.operation.name", "invoke_swarm")]),
        ),
        (
            "one turn of a dialect's own loop",
            "cycle",
            rule_attrs(&[("gen_ai.operation.name", "execute_event_loop_cycle")]),
        ),
        (
            "an unrecognised operation name, which falls through",
            "some span",
            rule_attrs(&[("gen_ai.operation.name", "rerank")]),
        ),
        (
            "a tool name beside an agent name - the tool wins here",
            "run",
            rule_attrs(&[
                ("gen_ai.tool.name", "search"),
                ("gen_ai.agent.name", "Researcher"),
            ]),
        ),
        (
            "a named agent alone",
            "run",
            rule_attrs(&[("gen_ai.agent.name", "Researcher")]),
        ),
        (
            "an agent *id* alone, which this classification does not read",
            "run",
            rule_attrs(&[("gen_ai.agent.id", "a-1")]),
        ),
        (
            "a dialect's span kind",
            "RunnableSequence",
            rule_attrs(&[("openinference.span.kind", "CHAIN")]),
        ),
        (
            "the same in lower case",
            "run",
            rule_attrs(&[("openinference.span.kind", "retriever")]),
        ),
        (
            "a kind with no category of its own",
            "run",
            rule_attrs(&[("openinference.span.kind", "GUARDRAIL")]),
        ),
        (
            "the other dialect's span kind, which this classification does not read",
            "run",
            rule_attrs(&[("langsmith.span.kind", "TOOL")]),
        ),
        (
            "a kind nobody recognises, which falls through to the name",
            "chat request",
            rule_attrs(&[("openinference.span.kind", "SOMETHING_ELSE")]),
        ),
        (
            "a name mentioning a model call",
            "LLM call",
            rule_attrs(&[]),
        ),
        ("a name mentioning chat", "ChatOpenAI", rule_attrs(&[])),
        (
            "a name mentioning both chat and embedding - the model call wins",
            "chat embedding",
            rule_attrs(&[]),
        ),
        (
            "a name mentioning embedding",
            "Embedding request",
            rule_attrs(&[]),
        ),
        (
            "a name mentioning retrieval",
            "Retriever.get",
            rule_attrs(&[]),
        ),
        (
            "a name mentioning a tool, which is not a category signal",
            "execute_tool search",
            rule_attrs(&[]),
        ),
    ];

    let plan = &sideseat_domain::rules::ruleset().observation_types;
    for (what, span_name, attrs) in cases {
        let declared = plan
            .span_category(span_name, &attrs)
            .map(|verdict| verdict.value.to_string())
            .unwrap_or_else(|| SpanCategory::Other.as_str().to_string());
        let swept = categorize_span_legacy(span_name, &attrs)
            .as_str()
            .to_string();
        assert_eq!(
            declared, swept,
            "the declared category disagrees with the sweep it shadows: {what}"
        );
    }
}

/// The usage-details object holds every `gen_ai.usage.*` member **no declared counter reads**, derived.
///
/// It was a list beside the assets, which is the hole this engine exists to close: adding a spelling to a
/// counter would leave the list unchanged, and the same number would then be counted twice - once as the
/// counter it now is, and once as a detail. Derived from the plan, the two cannot drift.
#[test]
fn the_usage_details_are_what_no_declared_counter_reads() {
    let attrs = rule_attrs(&[
        // Read by a counter, so not a detail.
        ("gen_ai.usage.input_tokens", "100"),
        ("gen_ai.usage.cache_read_input_tokens", "20"),
        ("gen_ai.usage.thoughts_token_count", "5"),
        // Read by nothing, so a detail - one integer, one float, one that is neither.
        ("gen_ai.usage.audio_tokens", "7"),
        ("gen_ai.usage.cost_estimate", "0.25"),
        ("gen_ai.usage.tier", "flex"),
    ]);
    let mut span = SpanData::default();
    crate::traces::extract::attributes::tests::extract_genai_as_production_does(
        &mut span, &attrs, "chat",
    );

    let details = span
        .gen_ai_usage_details
        .as_object()
        .cloned()
        .unwrap_or_default();
    let mut names: Vec<&String> = details.keys().collect();
    names.sort();
    assert_eq!(
        names,
        vec!["audio_tokens", "cost_estimate", "tier"],
        "a member a counter reads is not a detail, and one nothing reads is"
    );
    assert_eq!(details["audio_tokens"], serde_json::json!(7));
    assert_eq!(details["cost_estimate"], serde_json::json!(0.25));
    assert_eq!(details["tier"], serde_json::json!("flex"));
}
