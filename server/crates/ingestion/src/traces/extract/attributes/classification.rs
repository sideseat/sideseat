use super::*;

// ============================================================================
// SEMANTIC KIND CLASSIFICATION
// ============================================================================

#[derive(Clone, Copy)]
#[allow(clippy::upper_case_acronyms)]
#[cfg(any(test, feature = "test-support"))]
enum SemanticKind {
    LLM,
    Embedding,
    Agent,
    Tool,
    Chain,
    Retriever,
    Guardrail,
    Evaluator,
}

#[cfg(any(test, feature = "test-support"))]
impl SemanticKind {
    fn parse(kind: &str) -> Option<Self> {
        match kind.to_uppercase().as_str() {
            "LLM" => Some(Self::LLM),
            "EMBEDDING" => Some(Self::Embedding),
            "AGENT" => Some(Self::Agent),
            "TOOL" => Some(Self::Tool),
            "CHAIN" => Some(Self::Chain),
            "RETRIEVER" => Some(Self::Retriever),
            "GUARDRAIL" => Some(Self::Guardrail),
            "EVALUATOR" => Some(Self::Evaluator),
            _ => None,
        }
    }

    fn to_category(self) -> SpanCategory {
        match self {
            Self::LLM => SpanCategory::LLM,
            Self::Embedding => SpanCategory::Embedding,
            Self::Agent => SpanCategory::Agent,
            Self::Tool => SpanCategory::Tool,
            Self::Chain => SpanCategory::Chain,
            Self::Retriever => SpanCategory::Retriever,
            Self::Guardrail | Self::Evaluator => SpanCategory::Other,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    fn to_observation_type(self) -> ObservationType {
        match self {
            Self::LLM => ObservationType::Generation,
            Self::Embedding => ObservationType::Embedding,
            Self::Agent => ObservationType::Agent,
            Self::Tool => ObservationType::Tool,
            Self::Chain => ObservationType::Chain,
            Self::Retriever => ObservationType::Retriever,
            Self::Guardrail => ObservationType::Guardrail,
            Self::Evaluator => ObservationType::Evaluator,
        }
    }
}

// ============================================================================
// SPAN CLASSIFICATION
// ============================================================================

/// Categorize span based on attributes and name patterns.
/// Which category this span falls in, from the declared ordered rules.
///
/// As with the observation type, the one thing left here is which stored value each label means, and that "no
/// rule held" answers `other` - our vocabulary rather than any dialect's.
#[doc(hidden)]
pub fn categorize_span(span_name: &str, attrs: &HashMap<String, String>) -> SpanCategory {
    // The verdict's label; the evidence is what a diagnostic reads, and the enum is what is stored.
    let verdict = sideseat_domain::rules::ruleset()
        .observation_types
        .span_category(span_name, attrs);
    match verdict.as_ref().map(|verdict| verdict.value) {
        Some("llm") => SpanCategory::LLM,
        Some("tool") => SpanCategory::Tool,
        Some("agent") => SpanCategory::Agent,
        Some("chain") => SpanCategory::Chain,
        Some("retriever") => SpanCategory::Retriever,
        Some("embedding") => SpanCategory::Embedding,
        Some("db") => SpanCategory::DB,
        Some("storage") => SpanCategory::Storage,
        Some("http") => SpanCategory::HTTP,
        Some("messaging") => SpanCategory::Messaging,
        Some("other") | None => SpanCategory::Other,
        Some(other) => {
            debug_assert!(
                false,
                "classification rule named an unknown span category `{other}`"
            );
            SpanCategory::Other
        }
    }
}

/// The ordered sweep the declared rules shadow, kept as the equivalence oracle.
#[doc(hidden)]
#[cfg(any(test, feature = "test-support"))]
pub fn categorize_span_legacy(span_name: &str, attrs: &HashMap<String, String>) -> SpanCategory {
    // Priority 0: External service indicators (HTTP/RPC/DB) are NEVER GenAI spans
    // This must be checked FIRST to prevent AWS Bedrock API calls (rpc.system=aws-api)
    // from being classified as LLM even if they have gen_ai.* attributes: when a framework
    // SDK is also instrumenting, the transport span is a duplicate view of the real
    // generation span. Traces whose ONLY span is one of these still appear in the trace
    // list - that is handled by the list filter, not here.
    if attrs.contains_key(keys::HTTP_METHOD)
        || attrs.contains_key(keys::HTTP_REQUEST_METHOD)
        || attrs.contains_key(keys::RPC_SYSTEM)
    {
        return SpanCategory::HTTP;
    }
    if attrs.contains_key(keys::DB_SYSTEM) {
        return SpanCategory::DB;
    }
    if attrs.contains_key(keys::MESSAGING_SYSTEM) {
        return SpanCategory::Messaging;
    }
    if attrs.keys().any(|k| k.starts_with("aws.s3.")) {
        return SpanCategory::Storage;
    }

    // Priority 1: gen_ai.operation.name (with embedding model override)
    if let Some(op) = attrs.get(keys::GEN_AI_OPERATION_NAME) {
        match op.as_str() {
            "chat" | "text_completion" => {
                // Check if model name indicates embedding (e.g., amazon.titan-embed-text-v2:0)
                // Some telemetry incorrectly reports embedding operations as text_completion
                if let Some(model) = attrs
                    .get(keys::GEN_AI_REQUEST_MODEL)
                    .or_else(|| attrs.get(keys::GEN_AI_RESPONSE_MODEL))
                {
                    if contains_ascii_ignore_case(model, "embed") {
                        return SpanCategory::Embedding;
                    }
                }
                return SpanCategory::LLM;
            }
            "embeddings" => return SpanCategory::Embedding,
            "execute_tool" => return SpanCategory::Tool,
            "invoke_agent" | "invoke_swarm" | "execute_event_loop_cycle" => {
                return SpanCategory::Agent;
            }
            _ => {}
        }
    }

    // Priority 2: Tool/Agent indicators
    if attrs.contains_key(keys::GEN_AI_TOOL_NAME) {
        return SpanCategory::Tool;
    }
    if attrs.contains_key(keys::GEN_AI_AGENT_NAME) {
        return SpanCategory::Agent;
    }

    // Priority 3: OpenInference span kind
    if let Some(kind) = attrs
        .get(keys::OPENINFERENCE_SPAN_KIND)
        .and_then(|k| SemanticKind::parse(k))
    {
        return kind.to_category();
    }

    // Priority 4: Span name patterns
    let name_lower = span_name.to_lowercase();
    if name_lower.contains("llm") || name_lower.contains("chat") {
        return SpanCategory::LLM;
    }
    if name_lower.contains("embed") {
        return SpanCategory::Embedding;
    }
    if name_lower.contains("retriev") {
        return SpanCategory::Retriever;
    }

    SpanCategory::Other
}

/// Detect observation type from span attributes.
/// What kind of observation a span is, from the declared ordered rules.
///
/// The precedence and every condition are in `rules/vocabulary/observation-types.json`; this is the one thing about it
/// that is not a producer's business - which stored value each label means. An unrecognised label would be a
/// build defect (the assets ship inside the binary), and the answer for "no rule held" is a plain span, which
/// is our vocabulary rather than any dialect's.
#[doc(hidden)]
pub fn detect_observation_type(
    span_name: &str,
    attrs: &HashMap<String, String>,
) -> ObservationType {
    let verdict = sideseat_domain::rules::ruleset()
        .observation_types
        .observation_type(span_name, attrs);
    match verdict.as_ref().map(|verdict| verdict.value) {
        Some("generation") => ObservationType::Generation,
        Some("embedding") => ObservationType::Embedding,
        Some("agent") => ObservationType::Agent,
        Some("tool") => ObservationType::Tool,
        Some("chain") => ObservationType::Chain,
        Some("retriever") => ObservationType::Retriever,
        Some("guardrail") => ObservationType::Guardrail,
        Some("evaluator") => ObservationType::Evaluator,
        Some("span") | None => ObservationType::Span,
        Some(other) => {
            debug_assert!(
                false,
                "classification rule named an unknown observation type `{other}`"
            );
            ObservationType::Span
        }
    }
}

/// The ordered sweep the declared rules replaced, kept as the equivalence oracle.
#[doc(hidden)]
#[cfg(any(test, feature = "test-support"))]
pub fn detect_observation_type_legacy(
    span_name: &str,
    attrs: &HashMap<String, String>,
) -> ObservationType {
    // Priority 0: External service calls (HTTP/RPC/DB) are NEVER GenAI observations, for
    // the same reason as in categorize_span - with a framework SDK present the transport
    // span duplicates the real generation. Visibility of transport-only traces is handled
    // by the trace-list filter.
    if attrs.contains_key(keys::HTTP_METHOD)
        || attrs.contains_key(keys::HTTP_REQUEST_METHOD)
        || attrs.contains_key(keys::RPC_SYSTEM)
        || attrs.contains_key(keys::DB_SYSTEM)
    {
        return ObservationType::Span;
    }

    // Priority 1: gen_ai.operation.name (with embedding model override)
    if let Some(op) = attrs.get(keys::GEN_AI_OPERATION_NAME) {
        match op.as_str() {
            "chat" | "text_completion" => {
                // Check if model name indicates embedding (e.g., amazon.titan-embed-text-v2:0)
                // Some telemetry incorrectly reports embedding operations as text_completion
                if let Some(model) = attrs
                    .get(keys::GEN_AI_REQUEST_MODEL)
                    .or_else(|| attrs.get(keys::GEN_AI_RESPONSE_MODEL))
                {
                    if contains_ascii_ignore_case(model, "embed") {
                        return ObservationType::Embedding;
                    }
                }
                return ObservationType::Generation;
            }
            "embeddings" => return ObservationType::Embedding,
            "create_agent"
                if attrs
                    .get(keys::GEN_AI_SYSTEM)
                    .is_some_and(|s| s == "autogen") =>
            {
                return ObservationType::Span;
            }
            _ => {}
        }
    }

    // Priority 2: SDK span kinds
    for key in [keys::OPENINFERENCE_SPAN_KIND, keys::LANGSMITH_SPAN_KIND] {
        if let Some(kind) = attrs.get(key).and_then(|k| SemanticKind::parse(k)) {
            return kind.to_observation_type();
        }
    }

    // Priority 3: Vercel AI SDK
    if attrs.contains_key(keys::AI_MODEL_ID) || attrs.contains_key(keys::AI_MODEL_PROVIDER) {
        if attrs
            .get(keys::AI_OPERATION_ID)
            .is_some_and(|v| v.contains("embed"))
        {
            return ObservationType::Embedding;
        }
        return ObservationType::Generation;
    }

    // Priority 4: Attribute presence
    if attrs.contains_key(keys::GEN_AI_AGENT_NAME) || attrs.contains_key(keys::GEN_AI_AGENT_ID) {
        return ObservationType::Agent;
    }
    if attrs.contains_key(keys::GEN_AI_TOOL_NAME) || attrs.contains_key(keys::GEN_AI_TOOL_CALL_ID) {
        return ObservationType::Tool;
    }

    // Priority 5: Span name patterns
    let name_lower = span_name.to_lowercase();
    for (pattern, obs_type) in [
        ("embed", ObservationType::Embedding),
        ("agent", ObservationType::Agent),
        ("tool", ObservationType::Tool),
        ("retriev", ObservationType::Retriever),
        ("guardrail", ObservationType::Guardrail),
        ("eval", ObservationType::Evaluator),
    ] {
        if name_lower.contains(pattern) {
            return obs_type;
        }
    }

    // Priority 6: Logfire tags (logfire.tags: ["LLM"])
    if let Some(tags) = attrs.get("logfire.tags") {
        let tags_lower = tags.to_lowercase();
        if tags_lower.contains("llm") {
            return ObservationType::Generation;
        }
    }

    // Priority 7: Has model = Generation
    if attrs.contains_key(keys::GEN_AI_REQUEST_MODEL)
        || attrs.contains_key(keys::GEN_AI_RESPONSE_MODEL)
    {
        return ObservationType::Generation;
    }

    ObservationType::Span
}

/// Sum `models_usage.prompt_tokens` / `completion_tokens` from AutoGen `output.value`.
/// Only extracts from chain spans (`output.value.messages[]`) to avoid double-counting —
/// the same message appears in multiple routing (process) spans.
/// Retired: the two sums are declared (`usage_summed_input`/`_output`, `reduce: sum`). The oracle's copy.
#[cfg(test)]
pub(super) fn extract_autogen_tokens(attrs: &HashMap<String, String>) -> (i64, i64) {
    let output = match extract_json::<JsonValue>(attrs, keys::OUTPUT_VALUE) {
        Some(v) => v,
        None => return (0, 0),
    };
    let msgs = match output.get("messages").and_then(|m| m.as_array()) {
        Some(m) => m,
        None => return (0, 0),
    };
    let mut pt: i64 = 0;
    let mut ct: i64 = 0;
    for m in msgs {
        if let Some(mu) = m.get("models_usage").filter(|v| v.is_object()) {
            pt += mu
                .get("prompt_tokens")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            ct += mu
                .get("completion_tokens")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
        }
    }
    (pt, ct)
}
