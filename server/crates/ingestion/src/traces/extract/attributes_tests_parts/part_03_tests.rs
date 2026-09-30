
/// And a reported zero is a fact the fallback must not overwrite.
#[test]
fn a_reported_zero_survives_a_fallback() {
    let mut attrs = HashMap::new();
    attrs.insert("crew_key".to_string(), "crew-1".to_string());
    attrs.insert("gen_ai.usage.input_tokens".to_string(), "0".to_string());
    attrs.insert("gen_ai.usage.output_tokens".to_string(), "0".to_string());
    attrs.insert(
        "output.value".to_string(),
        serde_json::json!({
            "token_usage": {"prompt_tokens": 999, "completion_tokens": 999}
        })
        .to_string(),
    );

    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "crew");

    assert_eq!(
        (
            span.gen_ai_usage_input_tokens,
            span.gen_ai_usage_output_tokens
        ),
        (0, 0),
        "the provider stated zero; an aggregate elsewhere in the payload does not override it"
    );
}

/// A reported cache counter of zero survives a framework fallback, like the two sides do.
///
/// The per-side gate fixed input and output but left the cache counter testing its *value*, so an explicit
/// `cache_read=0` was replaced by whatever the embedded payload reported - inflating stored usage, and the
/// cost with it.
#[test]
fn a_reported_zero_cache_counter_survives_a_fallback() {
    let mut attrs = HashMap::new();
    attrs.insert("crew_key".to_string(), "crew-1".to_string());
    attrs.insert("gen_ai.usage.input_tokens".to_string(), "10".to_string());
    attrs.insert(
        "gen_ai.usage.cache_read_input_tokens".to_string(),
        "0".to_string(),
    );
    attrs.insert(
        "output.value".to_string(),
        serde_json::json!({
            "token_usage": {"completion_tokens": 5, "cached_prompt_tokens": 100}
        })
        .to_string(),
    );

    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "crew");

    assert_eq!(span.gen_ai_usage_input_tokens, 10);
    assert_eq!(
        span.gen_ai_usage_output_tokens, 5,
        "the output side was absent, so the fallback supplies it"
    );
    assert_eq!(
        span.gen_ai_usage_cache_read_tokens, 0,
        "the provider stated zero cache reads; the embedded payload does not override that"
    );
}

/// An imported total must describe the parts that were imported with it.
///
/// With a flat `input_tokens=0` and no output attribute, the output came from the embedded payload while the
/// input kept its reported zero - and taking the embedded *total* regardless left the row claiming 1,099
/// tokens for 0 + 100.
#[test]
fn an_imported_total_is_only_used_when_it_describes_the_imported_parts() {
    let mut attrs = HashMap::new();
    attrs.insert("crew_key".to_string(), "crew-1".to_string());
    attrs.insert("gen_ai.usage.input_tokens".to_string(), "0".to_string());
    attrs.insert(
        "output.value".to_string(),
        serde_json::json!({
            "token_usage": {"prompt_tokens": 999, "completion_tokens": 100, "total_tokens": 1099}
        })
        .to_string(),
    );

    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "crew");

    assert_eq!(
        (
            span.gen_ai_usage_input_tokens,
            span.gen_ai_usage_output_tokens
        ),
        (0, 100),
        "the reported zero stands; only the absent side is filled"
    );
    assert_eq!(
        span.gen_ai_usage_total_tokens, 100,
        "the total must account for the input and output actually stored"
    );
}

/// The embedded cache counter and total are reachable even when both flat sides were reported.
///
/// The CrewAI block was gated on a *side* being missing, so a payload reporting both sides flatly could
/// never contribute its cache counter or its total: `{prompt:500, completion:600, total:2000, cached:100}`
/// with flat `500/600` stored a total of 1,100 and no cache at all.
#[test]
fn an_embedded_cache_counter_is_read_even_when_both_sides_were_reported() {
    let mut attrs = HashMap::new();
    attrs.insert("crew_key".to_string(), "crew-1".to_string());
    attrs.insert("gen_ai.usage.input_tokens".to_string(), "500".to_string());
    attrs.insert("gen_ai.usage.output_tokens".to_string(), "600".to_string());
    attrs.insert(
        "output.value".to_string(),
        serde_json::json!({
            "token_usage": {
                "prompt_tokens": 500,
                "completion_tokens": 600,
                "total_tokens": 2000,
                "cached_prompt_tokens": 100
            }
        })
        .to_string(),
    );

    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "crew");

    assert_eq!(span.gen_ai_usage_cache_read_tokens, 100);
    assert_eq!(
        span.gen_ai_usage_total_tokens, 2_000,
        "the payload's parts match what is stored, so its reported total applies"
    );
}

/// A later source does not overwrite a counter an earlier one supplied.
///
/// `cache_read_supplied` recorded only whether a *flat attribute* existed, so a value the Logfire
/// `response_data` path filled in was not protected: CrewAI's `cached_prompt_tokens` overwrote it, and the
/// cache charge followed the wrong number. The flag describes the span, not one source's view of it.
#[test]
fn a_framework_fallback_does_not_overwrite_an_earlier_fallback_s_cache_count() {
    let mut attrs = HashMap::new();
    attrs.insert("crew_key".to_string(), "crew-1".to_string());
    attrs.insert(
        "response_data".to_string(),
        serde_json::json!({ "usage": { "cache_read_input_tokens": 17 } }).to_string(),
    );
    attrs.insert(
        "output.value".to_string(),
        serde_json::json!({ "token_usage": { "cached_prompt_tokens": 100 } }).to_string(),
    );

    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "crew");

    assert_eq!(
        span.gen_ai_usage_cache_read_tokens, 17,
        "the first source to supply the counter owns it"
    );
}

/// A framework's embedded total does not raise a total the provider stated itself.
///
/// The embedded total is taken through `max`, which against an explicit flat total can only replace the
/// provider's own statement with the framework's: flat `500/600` with a flat total of 1,100 became 2,000.
#[test]
fn an_embedded_total_does_not_override_an_explicit_flat_total() {
    let mut attrs = HashMap::new();
    attrs.insert("crew_key".to_string(), "crew-1".to_string());
    attrs.insert("gen_ai.usage.input_tokens".to_string(), "500".to_string());
    attrs.insert("gen_ai.usage.output_tokens".to_string(), "600".to_string());
    attrs.insert("gen_ai.usage.total_tokens".to_string(), "1100".to_string());
    attrs.insert(
        "output.value".to_string(),
        serde_json::json!({
            "token_usage": {
                "prompt_tokens": 500,
                "completion_tokens": 600,
                "total_tokens": 2000
            }
        })
        .to_string(),
    );

    let mut span = SpanData::default();
    extract_genai_as_production_does(&mut span, &attrs, "crew");

    assert_eq!(
        span.gen_ai_usage_total_tokens, 1_100,
        "the provider stated the total; the framework's does not raise it"
    );
}

/// A declared framework fills the gap the conventions leave, and never overrides span evidence.
///
/// The current OTel GenAI conventions are framework-neutral on purpose, so a producer that follows them -
/// the Vercel AI SDK's current integration is pure `gen_ai.*`, with no `ai.*` at all - offers nothing to
/// sniff and arrived as `Unknown`. The SDKs declare what they were configured for, and that is consulted
/// **after** every rule: a process configured for one framework can still emit another's spans from a nested
/// library, and those carry their own attributes for a rule to find.
#[test]
fn a_declared_framework_is_a_fallback_and_not_an_override() {
    let declared = |slug: &str| {
        let mut resource = HashMap::new();
        resource.insert("sideseat.framework".to_string(), slug.to_string());
        resource
    };

    // Pure semantic conventions: no rule matches, so the declaration answers.
    let mut semconv = HashMap::new();
    semconv.insert("gen_ai.operation.name".to_string(), "chat".to_string());
    semconv.insert("gen_ai.provider.name".to_string(), "anthropic".to_string());
    semconv.insert("gen_ai.request.model".to_string(), "claude".to_string());
    assert_eq!(
        detect_framework("chat claude", &semconv, &declared("vercel-ai")),
        Framework::VercelAISdk.as_str(),
        "a framework-neutral span is attributed by what the SDK declared"
    );
    assert_eq!(
        detect_framework("chat claude", &semconv, &HashMap::new()),
        Framework::Unknown.as_str(),
        "and with no declaration it stays unknown rather than guessing"
    );

    // Span evidence wins: a nested library's spans keep their own framework.
    let mut langchain = semconv.clone();
    langchain.insert("langchain.version".to_string(), "0.3".to_string());
    assert_eq!(
        detect_framework("RunnableSequence", &langchain, &declared("strands")),
        Framework::LangChain.as_str(),
        "the span says LangChain, so the process-level declaration must not relabel it"
    );

    // A provider slug is not a framework, so `[strands, bedrock]` still resolves to Strands...
    assert_eq!(
        detect_framework("chat claude", &semconv, &declared("strands,bedrock")),
        Framework::StrandsAgents.as_str()
    );
    // ...while two genuine frameworks resolve to nothing: two answers is not an answer.
    assert_eq!(
        detect_framework("chat claude", &semconv, &declared("strands,langgraph")),
        Framework::Unknown.as_str()
    );
    // An unknown slug claims nothing.
    assert_eq!(
        detect_framework("chat claude", &semconv, &declared("something-else")),
        Framework::Unknown.as_str()
    );
}

/// Attributes for the detection cases below.
fn detect_attrs(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Detection over one span, both ways: the table first, then the rules.
fn both(
    span_name: &str,
    span_attrs: &HashMap<String, String>,
    resource_attrs: &HashMap<String, String>,
) -> (String, String) {
    (
        legacy_detect_framework(span_name, span_attrs, resource_attrs).to_string(),
        detect_framework(span_name, span_attrs, resource_attrs),
    )
}

// ============================================================================
// DETECTION EQUIVALENCE: the rules against the table they replaced
// ============================================================================

/// Spans covering every rule, every dimension, and the orderings that matter.
///
/// Hand-written rather than derived from the assets: a case list generated from the thing under test
/// would stop covering a rule the moment that rule was deleted, and the equivalence claim would still
/// pass. The corpus-wide comparison in `message_goldens_tests` covers the real payloads; this covers the
/// shapes the corpus does not contain, which is most of the 28 rules.
#[test]
fn the_rules_reproduce_the_legacy_detection() {
    let sdk_default = detect_attrs(&[("service.name", "strands-agents")]);
    let empty = detect_attrs(&[]);

    /// One detection case: the span name, its attributes, and the resource's.
    type DetectCase = (
        &'static str,
        HashMap<String, String>,
        HashMap<String, String>,
    );

    let cases: Vec<DetectCase> = vec![
        // Every rule, by its own signal.
        ("autogen run", empty.clone(), empty.clone()),
        ("s", detect_attrs(&[("autogen.foo", "1")]), empty.clone()),
        (
            "s",
            detect_attrs(&[("gen_ai.system", "autogen")]),
            empty.clone(),
        ),
        (
            "s",
            detect_attrs(&[("gcp.vertex.agent.x", "1")]),
            empty.clone(),
        ),
        ("s", detect_attrs(&[("google.adk.x", "1")]), empty.clone()),
        ("s", detect_attrs(&[("crew_key", "k")]), empty.clone()),
        (
            "s",
            empty.clone(),
            detect_attrs(&[("service.name", "crewAI-telemetry")]),
        ),
        ("LangGraph", empty.clone(), empty.clone()),
        ("s", detect_attrs(&[("langgraph.step", "1")]), empty.clone()),
        (
            "s",
            detect_attrs(&[("metadata", "{\"langgraph_step\":1}")]),
            empty.clone(),
        ),
        ("s", detect_attrs(&[("langchain.x", "1")]), empty.clone()),
        ("s", detect_attrs(&[("langsmith.x", "1")]), empty.clone()),
        ("s", detect_attrs(&[("llama_index.x", "1")]), empty.clone()),
        ("s", detect_attrs(&[("agno.x", "1")]), empty.clone()),
        ("s", detect_attrs(&[("smolagents.x", "1")]), empty.clone()),
        ("s", detect_attrs(&[("agentscope.x", "1")]), empty.clone()),
        ("s", detect_attrs(&[("langflow.x", "1")]), empty.clone()),
        ("s", detect_attrs(&[("ag2.x", "1")]), empty.clone()),
        ("s", detect_attrs(&[("haystack.x", "1")]), empty.clone()),
        (
            "s",
            detect_attrs(&[("gen_ai.provider.name", "browser_use")]),
            empty.clone(),
        ),
        (
            "s",
            detect_attrs(&[("openinference.span.kind", "LLM")]),
            empty.clone(),
        ),
        (
            "s",
            detect_attrs(&[("semantic_kernel.x", "1")]),
            empty.clone(),
        ),
        (
            "s",
            detect_attrs(&[("gen_ai.system", "azure_openai")]),
            empty.clone(),
        ),
        (
            "s",
            detect_attrs(&[("gen_ai.system", "azure.openai")]),
            empty.clone(),
        ),
        ("s", detect_attrs(&[("azure.openai.x", "1")]), empty.clone()),
        ("s", detect_attrs(&[("az.ai.x", "1")]), empty.clone()),
        ("vertexai.generate", empty.clone(), empty.clone()),
        ("s", detect_attrs(&[("ai.operationId", "x")]), empty.clone()),
        (
            "s",
            detect_attrs(&[("ai.prompt.messages", "[]")]),
            empty.clone(),
        ),
        (
            "s",
            detect_attrs(&[("ai.usage.promptTokens", "1")]),
            empty.clone(),
        ),
        (
            "s",
            detect_attrs(&[("ai.finishReason", "stop")]),
            empty.clone(),
        ),
        ("s", detect_attrs(&[("logfire.msg", "x")]), empty.clone()),
        (
            "s",
            empty.clone(),
            detect_attrs(&[("telemetry.sdk.name", "logfire-python")]),
        ),
        ("s", detect_attrs(&[("mlflow.x", "1")]), empty.clone()),
        (
            "s",
            detect_attrs(&[("traceloop.entity.input", "x")]),
            empty.clone(),
        ),
        (
            "s",
            empty.clone(),
            detect_attrs(&[("telemetry.sdk.name", "traceloop-sdk")]),
        ),
        ("s", detect_attrs(&[("livekit.x", "1")]), empty.clone()),
        ("s", detect_attrs(&[("lk.x", "1")]), empty.clone()),
        (
            "s",
            detect_attrs(&[("openai.agents.x", "1")]),
            empty.clone(),
        ),
        (
            "s",
            empty.clone(),
            detect_attrs(&[("service.name", "openai-agents")]),
        ),
        (
            "s",
            detect_attrs(&[("gen_ai.provider.name", "microsoft.agent_framework")]),
            empty.clone(),
        ),
        (
            "s",
            empty.clone(),
            detect_attrs(&[("service.name", "agent-framework-core")]),
        ),
        ("s", detect_attrs(&[("aws.bedrock.x", "1")]), empty.clone()),
        (
            "s",
            detect_attrs(&[("gen_ai.system", "aws_bedrock")]),
            empty.clone(),
        ),
        ("claude_code.interaction", empty.clone(), empty.clone()),
        (
            "s",
            empty.clone(),
            detect_attrs(&[("service.name", "claude-code")]),
        ),
        (
            "s",
            detect_attrs(&[("gen_ai.system", "strands-agents")]),
            empty.clone(),
        ),
        ("Strands Agent loop", empty.clone(), empty.clone()),
        ("strands-agent", empty.clone(), empty.clone()),
        (
            "s",
            detect_attrs(&[("gen_ai.agent.name", "My STRANDS_AGENT")]),
            empty.clone(),
        ),
        // Nothing at all.
        ("plain span", empty.clone(), empty.clone()),
        // The declaration fallback, including a provider slug that must contribute nothing and two
        // genuine frameworks that must resolve to nothing.
        (
            "s",
            empty.clone(),
            detect_attrs(&[("sideseat.framework", "strands")]),
        ),
        (
            "s",
            empty.clone(),
            detect_attrs(&[("sideseat.framework", "strands,bedrock")]),
        ),
        (
            "s",
            empty.clone(),
            detect_attrs(&[("sideseat.framework", "strands,crewai")]),
        ),
        (
            "s",
            empty.clone(),
            detect_attrs(&[("sideseat.framework", "vercel-ai")]),
        ),
        (
            "s",
            empty.clone(),
            detect_attrs(&[("sideseat.framework", "openai")]),
        ),
        (
            "s",
            empty.clone(),
            detect_attrs(&[("sideseat.framework", "pydantic-ai")]),
        ),
        (
            "s",
            empty.clone(),
            detect_attrs(&[("sideseat.framework", "not-a-framework")]),
        ),
        // `dedup` runs on an *unsorted* list, so it removes only consecutive repeats - preserved from
        // the table this replaced. It cannot change an answer, and these cases are why: a repeat becomes
        // non-consecutive only when a different label sits between it, and two distinct labels already
        // resolve to nothing; a slug that resolves to nothing (a provider) is filtered out before the
        // dedup, so it cannot separate them either.
        (
            "s",
            empty.clone(),
            detect_attrs(&[("sideseat.framework", "strands,bedrock,strands")]),
        ),
        (
            "s",
            empty.clone(),
            detect_attrs(&[("sideseat.framework", "strands,crewai,strands")]),
        ),
        (
            "s",
            empty.clone(),
            detect_attrs(&[("sideseat.framework", "strands,crewai,crewai")]),
        ),
        (
            "s",
            empty.clone(),
            detect_attrs(&[("sideseat.framework", " strands , bedrock ")]),
        ),
        (
            "s",
            empty.clone(),
            detect_attrs(&[("sideseat.framework", "")]),
        ),
        // The ordering that matters most: the SDK's default service name must not claim a span whose own
        // attributes name a different framework.
        (
            "s",
            detect_attrs(&[("langgraph.step", "1")]),
            sdk_default.clone(),
        ),
        ("s", detect_attrs(&[("crew_key", "k")]), sdk_default.clone()),
        (
            "s",
            detect_attrs(&[("openinference.span.kind", "LLM")]),
            sdk_default.clone(),
        ),
        ("s", empty.clone(), sdk_default.clone()),
        // Overlaps the ranks exist to resolve.
        (
            "s",
            detect_attrs(&[("langgraph.step", "1"), ("langchain.x", "1")]),
            empty.clone(),
        ),
        (
            "s",
            detect_attrs(&[("agno.x", "1"), ("openinference.span.kind", "LLM")]),
            empty.clone(),
        ),
        (
            "s",
            detect_attrs(&[("azure.openai.x", "1"), ("az.ai.x", "1")]),
            empty.clone(),
        ),
    ];

    let mut disagreements = Vec::new();
    for (span_name, span_attrs, resource_attrs) in &cases {
        let (legacy, rules) = both(span_name, span_attrs, resource_attrs);
        if legacy != rules {
            disagreements.push(format!(
                "  span `{span_name}` attrs={span_attrs:?} resource={resource_attrs:?}: table said \
                 `{legacy}`, rules said `{rules}`"
            ));
        }
    }
    assert!(
        disagreements.is_empty(),
        "detection changed for {} case(s):\n{}",
        disagreements.len(),
        disagreements.join("\n")
    );
}

#[test]
fn a_span_nothing_claims_is_labelled_unclaimed() {
    let (legacy, rules) = both("plain", &detect_attrs(&[]), &detect_attrs(&[]));
    assert_eq!(rules, sideseat_domain::rules::UNCLAIMED_LABEL);
    assert_eq!(legacy, rules);
}

/// How far detection is from being order-independent, measured rather than assumed.
///
/// `legacy_rank` exists because the rules were transcribed from a first-match table whose order is
/// load-bearing. The target is that no span has two candidates, and this reports which of the corpus's
/// own shapes still do - so retiring the rank is a measurable job rather than a hope. Reported, not
/// asserted to be zero: making it zero means narrowing predicates, which is its own reviewed change.
#[test]
fn detection_overlaps_are_reported() {
    let plan = &sideseat_domain::rules::ruleset().detect;
    let empty = HashMap::new();

    // Shapes that really co-occur: a framework riding OpenInference, and any framework using the SDK
    // (whose default service name is one framework's own).
    let sdk_default = detect_attrs(&[("service.name", "strands-agents")]);
    /// One overlap probe: span name, its attributes, and the resource's.
    type OverlapProbe<'a> = (
        &'a str,
        HashMap<String, String>,
        &'a HashMap<String, String>,
    );

    let probes: Vec<OverlapProbe<'_>> = vec![
        (
            "s",
            detect_attrs(&[("agno.x", "1"), ("openinference.span.kind", "LLM")]),
            &empty,
        ),
        (
            "s",
            detect_attrs(&[("langgraph.step", "1"), ("langchain.x", "1")]),
            &empty,
        ),
        ("s", detect_attrs(&[("langgraph.step", "1")]), &sdk_default),
        ("s", detect_attrs(&[("crew_key", "k")]), &sdk_default),
        (
            "s",
            detect_attrs(&[("openinference.span.kind", "LLM")]),
            &sdk_default,
        ),
        (
            "s",
            detect_attrs(&[("azure.openai.x", "1"), ("az.ai.x", "1")]),
            &empty,
        ),
    ];

    let mut overlaps = Vec::new();
    for (span_name, span_attrs, resource_attrs) in &probes {
        let ctx = sideseat_domain::rules::DetectContext {
            span_name,
            scope_name: None,
            span_attrs,
            resource_attrs,
        };
        let candidates = plan.overlapping_candidates(&ctx);
        if !candidates.is_empty() {
            let names: Vec<&str> = candidates.iter().map(|c| c.rule_id.as_str()).collect();
            overlaps.push(format!("  {span_attrs:?} -> {}", names.join(", ")));
        }
    }
    println!(
        "detection: {} of {} probed shapes have more than one candidate, so `legacy_rank` still \
         decides them:\n{}",
        overlaps.len(),
        probes.len(),
        overlaps.join("\n")
    );
    // The bound that must hold: whatever the overlaps, the *winner* is the one the table chose. That is
    // what `the_rules_reproduce_the_legacy_detection` asserts; this test exists to name the residue.
    assert!(
        overlaps.len() <= probes.len(),
        "sanity: cannot overlap on more shapes than were probed"
    );
}

/// The declared finish-reason chain answers exactly as the four Rust blocks it replaced did, and in their
/// order.
///
/// Four attribute sources moved (`ff0d3cdf`); only the `gen_ai.choice` **event** stayed, which field
/// resolution cannot see. Three things this pins that the goldens cannot, because no captured span carries
/// the shapes:
///
/// - **The order**, spelled out: the flat attribute, then a serialised completion's choices, then a
///   serialised output-message list, then one dialect's serialised response, then another's. A reordering of
///   the asset is a silent change in which producer's statement is believed.
/// - **The array requirement.** Two of the retired readers decoded a *list* and fell through when the payload
///   was not one. A JSONPath wildcard matches an object's members too, so `{"x": {"finish_reason": …}}`
///   answered where the retired chain moved on - and answered with a different producer's value.
/// - **Scalar strings only.** The retired readers took `as_str()`, so a member holding `["stop", "length"]`
///   was ignored; collected as a string *list* it contributed two reasons the producer never stated.
#[test]
fn the_declared_finish_reason_chain_reproduces_the_retired_blocks() {
    use sideseat_domain::rules::ruleset;
    use std::collections::HashMap;

    let resolve = |attrs: &HashMap<String, String>| -> Vec<String> {
        ruleset()
            .span_fields
            .resolve("some.span", attrs, &[])
            .into_iter()
            .find(|r| {
                matches!(
                    r.target,
                    sideseat_domain::rules::schema::FieldTarget::GenAiFinishReasons
                )
            })
            .and_then(|r| match r.reading {
                sideseat_domain::rules::span_fields::Reading::StringList(items) => Some(items),
                sideseat_domain::rules::span_fields::Reading::Text(text) => Some(vec![text]),
                _ => None,
            })
            .unwrap_or_default()
    };

    /// The four retired blocks, in their order, verbatim in behaviour.
    fn retired(attrs: &HashMap<String, String>) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        if let Some(flat) = attrs.get("gen_ai.response.finish_reasons") {
            // The flat attribute, read as the stored list form.
            if let Ok(items) = serde_json::from_str::<Vec<String>>(flat) {
                out = items;
            } else if !flat.is_empty() {
                out = vec![flat.clone()];
            }
        }
        if out.is_empty()
            && let Some(completion) = attrs.get("gen_ai.completion")
            && let Ok(json) = serde_json::from_str::<serde_json::Value>(completion)
            && let Some(choices) = json.get("choices").and_then(|c| c.as_array())
        {
            for choice in choices {
                if let Some(reason) = choice.get("finish_reason").and_then(|r| r.as_str()) {
                    out.push(reason.to_string());
                }
            }
        }
        if out.is_empty()
            && let Some(messages) = attrs.get("gen_ai.output.messages")
            && let Ok(list) = serde_json::from_str::<Vec<serde_json::Value>>(messages)
        {
            for message in &list {
                if let Some(reason) = message.get("finish_reason").and_then(|r| r.as_str()) {
                    out.push(reason.to_string());
                    break;
                }
            }
        }
        if out.is_empty()
            && let Some(response) = attrs.get("gcp.vertex.agent.llm_response")
            && let Ok(json) = serde_json::from_str::<serde_json::Value>(response)
            && let Some(reason) = json.get("finish_reason").and_then(|r| r.as_str())
        {
            out = vec![reason.to_lowercase()];
        }
        if out.is_empty()
            && let Some(response) = attrs.get("response_data")
            && let Ok(json) = serde_json::from_str::<serde_json::Value>(response)
            && let Some(reason) = json.get("finish_reason").and_then(|r| r.as_str())
        {
            out = vec![reason.to_string()];
        }
        out
    }

    // Each case names every attribute it sets, so the order is exercised by conflicting values rather than
    // asserted about. Every shape the retired readers refused is here, because a refusal *is* the order:
    // falling through is how the next producer's statement gets believed.
    let cases: &[&[(&str, &str)]] = &[
        // One source at a time.
        &[("gen_ai.response.finish_reasons", "tool_use")],
        &[(
            "gen_ai.completion",
            r#"{"choices":[{"finish_reason":"stop"}]}"#,
        )],
        &[(
            "gen_ai.completion",
            r#"{"choices":[{"finish_reason":"stop"},{"finish_reason":"length"}]}"#,
        )],
        &[(
            "gen_ai.output.messages",
            r#"[{"finish_reason":"stop"},{"finish_reason":"length"}]"#,
        )],
        &[(
            "gcp.vertex.agent.llm_response",
            r#"{"finish_reason":"STOP"}"#,
        )],
        &[("response_data", r#"{"finish_reason":"length"}"#)],
        // The order, by conflict: each earlier source must win.
        &[
            ("gen_ai.response.finish_reasons", "tool_use"),
            (
                "gen_ai.completion",
                r#"{"choices":[{"finish_reason":"stop"}]}"#,
            ),
            ("gen_ai.output.messages", r#"[{"finish_reason":"length"}]"#),
            (
                "gcp.vertex.agent.llm_response",
                r#"{"finish_reason":"STOP"}"#,
            ),
            ("response_data", r#"{"finish_reason":"content_filter"}"#),
        ],
        &[
            (
                "gen_ai.completion",
                r#"{"choices":[{"finish_reason":"stop"}]}"#,
            ),
            ("gen_ai.output.messages", r#"[{"finish_reason":"length"}]"#),
            ("response_data", r#"{"finish_reason":"content_filter"}"#),
        ],
        &[
            ("gen_ai.output.messages", r#"[{"finish_reason":"length"}]"#),
            (
                "gcp.vertex.agent.llm_response",
                r#"{"finish_reason":"STOP"}"#,
            ),
        ],
        &[
            (
                "gcp.vertex.agent.llm_response",
                r#"{"finish_reason":"STOP"}"#,
            ),
            ("response_data", r#"{"finish_reason":"content_filter"}"#),
        ],
        // The shapes a refusal must let through. An **object** where a list was required: the retired reader
        // decoded a `Vec` and fell through, and a JSONPath wildcard would have answered here instead.
        &[
            (
                "gen_ai.output.messages",
                r#"{"x":{"finish_reason":"stop"}}"#,
            ),
            ("response_data", r#"{"finish_reason":"length"}"#),
        ],
        &[
            (
                "gen_ai.completion",
                r#"{"choices":{"a":{"finish_reason":"stop"}}}"#,
            ),
            ("response_data", r#"{"finish_reason":"length"}"#),
        ],
        // A member that is not a scalar string, on **every** source. The retired readers all took `as_str()`,
        // so an array-valued member was ignored and the chain moved to the next producer - a different
        // statement about why the model stopped, not a formatting difference. `collect_all` fixed only the
        // completion source; the other three read the field's own *list* type and answered here instead.
        &[(
            "gen_ai.output.messages",
            r#"[{"finish_reason":["stop"]},{"finish_reason":"length"}]"#,
        )],
        &[
            ("gen_ai.output.messages", r#"[{"finish_reason":["stop"]}]"#),
            ("response_data", r#"{"finish_reason":"length"}"#),
        ],
        &[
            (
                "gcp.vertex.agent.llm_response",
                r#"{"finish_reason":["STOP"]}"#,
            ),
            ("response_data", r#"{"finish_reason":"length"}"#),
        ],
        &[("response_data", r#"{"finish_reason":["length"]}"#)],
        &[
            ("gen_ai.response.finish_reasons", "[]"),
            ("response_data", r#"{"finish_reason":"length"}"#),
        ],
        // A member that is not a scalar string. `as_str()` ignored it; collected as a list it became two.
        &[(
            "gen_ai.completion",
            r#"{"choices":[{"finish_reason":["stop","length"]}]}"#,
        )],
        &[
            (
                "gen_ai.completion",
                r#"{"choices":[{"finish_reason":["stop","length"]}]}"#,
            ),
            ("response_data", r#"{"finish_reason":"content_filter"}"#),
        ],
        // An **empty** reason is a value, and it ends the chain. `as_str()` returned `Some("")`, so the
        // retired reader pushed it and stopped looking - discarding it lost the value *and* let a later
        // producer's reason answer in its place.
        &[("gen_ai.completion", r#"{"choices":[{"finish_reason":""}]}"#)],
        &[
            ("gen_ai.completion", r#"{"choices":[{"finish_reason":""}]}"#),
            ("response_data", r#"{"finish_reason":"length"}"#),
        ],
        &[
            ("gen_ai.output.messages", r#"[{"finish_reason":""}]"#),
            ("response_data", r#"{"finish_reason":"length"}"#),
        ],
        &[
            ("gcp.vertex.agent.llm_response", r#"{"finish_reason":""}"#),
            ("response_data", r#"{"finish_reason":"length"}"#),
        ],
        &[("response_data", r#"{"finish_reason":""}"#)],
        // Nothing readable anywhere.
        &[("gen_ai.completion", "not json")],
        &[("response_data", r#"{"finish_reason":null}"#)],
        &[("gcp.vertex.agent.llm_response", "[]")],
        &[],
    ];

    for case in cases {
        let attrs: HashMap<String, String> = case
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        assert_eq!(
            resolve(&attrs),
            retired(&attrs),
            "the declared chain disagrees with the retired blocks on {case:?}"
        );
    }
}
