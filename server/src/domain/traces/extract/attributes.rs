//! Attribute extraction for spans.
//!
//! Extracts GenAI attributes, semantic conventions, and classifies spans.

#![allow(clippy::collapsible_if)]

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use opentelemetry_proto::tonic::trace::v1::Span;
use serde_json::{Value as JsonValue, json};

use crate::core::constants;
use crate::data::types::{Framework, ObservationType, SpanCategory};
use crate::utils::string::parse_string_array;
use crate::utils::time::nanos_to_datetime;

use super::truncate_bytes;

use super::{extract_json, keys};

// ============================================================================
// SHARED HELPER FUNCTIONS
// ============================================================================

/// Check if haystack contains needle (case-insensitive, ASCII only).
/// Zero-allocation alternative to `haystack.to_lowercase().contains(needle)`.
#[inline]
fn contains_ascii_ignore_case(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if haystack.len() < needle.len() {
        return false;
    }
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

/// Merge tags from multiple attribute keys, deduplicating
pub(super) fn merge_tags(attrs: &HashMap<String, String>, tag_keys: &[&str]) -> Vec<String> {
    let mut tags = Vec::new();
    let mut seen = HashSet::new();
    for key in tag_keys {
        if let Some(val) = attrs.get(*key) {
            for tag in parse_string_array(val) {
                if seen.insert(tag.clone()) {
                    tags.push(tag);
                }
            }
        }
    }
    tags
}

/// Get first matching value from attribute keys.
pub(super) fn get_first(attrs: &HashMap<String, String>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| attrs.get(*k).cloned())
}

/// Parse a value from attributes.
pub(super) fn parse_opt<T: std::str::FromStr>(
    attrs: &HashMap<String, String>,
    key: &str,
) -> Option<T> {
    attrs.get(key).and_then(|v| v.parse().ok())
}

// ============================================================================
// OTLP CORE FIELD EXTRACTION
// ============================================================================

pub(super) fn set_core_fields(s: &mut SpanData, span: &Span) {
    s.trace_id = hex::encode(&span.trace_id);
    s.span_id = hex::encode(&span.span_id);
    s.parent_span_id = if span.parent_span_id.is_empty() {
        None
    } else {
        Some(hex::encode(&span.parent_span_id))
    };
    s.trace_state = if span.trace_state.is_empty() {
        None
    } else {
        Some(span.trace_state.clone())
    };
    s.span_name = span.name.clone();
    s.span_kind = Some(span_kind_to_string(span.kind).to_string());
    s.status_code = span
        .status
        .as_ref()
        .map(|st| status_code_to_string(st.code).to_string());
    s.status_message = span.status.as_ref().and_then(|st| {
        if st.message.is_empty() {
            None
        } else if st.message.len() > constants::ERROR_MESSAGE_MAX_LEN {
            Some(format!(
                "{}...",
                truncate_bytes(&st.message, constants::ERROR_MESSAGE_MAX_LEN)
            ))
        } else {
            Some(st.message.clone())
        }
    });
    s.timestamp_start = nanos_to_datetime(span.start_time_unix_nano);
    s.timestamp_end = if span.end_time_unix_nano > 0 {
        Some(nanos_to_datetime(span.end_time_unix_nano))
    } else {
        None
    };
    s.duration_ms = if span.end_time_unix_nano > span.start_time_unix_nano {
        ((span.end_time_unix_nano - span.start_time_unix_nano) / 1_000_000) as i64
    } else {
        0
    };
}

fn span_kind_to_string(kind: i32) -> &'static str {
    match kind {
        0 => "UNSPECIFIED",
        1 => "INTERNAL",
        2 => "SERVER",
        3 => "CLIENT",
        4 => "PRODUCER",
        5 => "CONSUMER",
        _ => "UNKNOWN",
    }
}

fn status_code_to_string(code: i32) -> &'static str {
    match code {
        0 => "UNSET",
        1 => "OK",
        2 => "ERROR",
        _ => "UNKNOWN",
    }
}

// ============================================================================
// SPAN DATA
// ============================================================================

/// Extracted span data for pipeline processing.
#[derive(Debug, Clone, Default)]
pub struct SpanData {
    // Identity
    pub project_id: Option<String>,
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub trace_state: Option<String>,

    // Session/User
    pub session_id: Option<String>,
    pub user_id: Option<String>,

    // Classification
    pub span_name: String,
    pub span_kind: Option<String>,
    pub span_category: Option<SpanCategory>,
    pub observation_type: Option<ObservationType>,
    pub framework: Option<Framework>,
    pub status_code: Option<String>,
    pub status_message: Option<String>,
    pub exception_type: Option<String>,
    pub exception_message: Option<String>,
    pub exception_stacktrace: Option<String>,

    // Time
    pub timestamp_start: DateTime<Utc>,
    pub timestamp_end: Option<DateTime<Utc>>,
    pub duration_ms: i64,

    // Environment
    pub environment: Option<String>,

    // GenAI Core
    pub gen_ai_system: Option<String>,
    pub gen_ai_operation_name: Option<String>,
    pub gen_ai_request_model: Option<String>,
    pub gen_ai_response_model: Option<String>,
    pub gen_ai_response_id: Option<String>,

    // GenAI Parameters
    pub gen_ai_temperature: Option<f64>,
    pub gen_ai_top_p: Option<f64>,
    pub gen_ai_top_k: Option<i64>,
    pub gen_ai_max_tokens: Option<i64>,
    pub gen_ai_frequency_penalty: Option<f64>,
    pub gen_ai_presence_penalty: Option<f64>,
    pub gen_ai_stop_sequences: Vec<String>,
    pub gen_ai_finish_reasons: Vec<String>,

    // GenAI Agent/Tool
    pub gen_ai_agent_id: Option<String>,
    pub gen_ai_agent_name: Option<String>,
    pub gen_ai_tool_name: Option<String>,
    pub gen_ai_tool_call_id: Option<String>,

    // GenAI Performance
    pub gen_ai_server_ttft_ms: Option<i64>,
    pub gen_ai_server_request_duration_ms: Option<i64>,

    // Token Usage
    pub gen_ai_usage_input_tokens: i64,
    pub gen_ai_usage_output_tokens: i64,
    pub gen_ai_usage_total_tokens: i64,
    pub gen_ai_usage_cache_read_tokens: i64,
    pub gen_ai_usage_cache_write_tokens: i64,
    pub gen_ai_usage_reasoning_tokens: i64,
    pub gen_ai_usage_details: JsonValue,

    // Pre-calculated costs (from OpenInference llm.cost.* or other sources)
    // These are used as fallback when pricing service cannot calculate costs
    pub extracted_cost_total: Option<f64>,
    pub extracted_cost_input: Option<f64>,
    pub extracted_cost_output: Option<f64>,

    // External Services
    pub http_method: Option<String>,
    pub http_url: Option<String>,
    pub http_status_code: Option<i64>,
    pub db_system: Option<String>,
    pub db_name: Option<String>,
    pub db_operation: Option<String>,
    pub db_statement: Option<String>,
    pub storage_system: Option<String>,
    pub storage_bucket: Option<String>,
    pub storage_object: Option<String>,
    pub messaging_system: Option<String>,
    pub messaging_destination: Option<String>,

    // Tags/Metadata
    pub tags: Vec<String>,
    pub metadata: JsonValue,
}

// ============================================================================
// TOKEN USAGE CONFIGURATION
// ============================================================================

/// Token count extraction configuration with fallback keys.
struct TokenConfig {
    primary: &'static str,
    fallbacks: &'static [&'static str],
}

impl TokenConfig {
    const fn new(primary: &'static str, fallbacks: &'static [&'static str]) -> Self {
        Self { primary, fallbacks }
    }

    pub(super) fn extract(&self, attrs: &HashMap<String, String>) -> i64 {
        attrs
            .get(self.primary)
            .or_else(|| self.fallbacks.iter().find_map(|k| attrs.get(*k)))
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    }
}

const INPUT_TOKENS: TokenConfig = TokenConfig::new(
    "gen_ai.usage.input_tokens",
    &[
        "gen_ai.usage.prompt_tokens",
        "llm.usage.prompt_tokens",
        "llm.token_count.prompt",
        "ai.usage.promptTokens",
    ],
);

const OUTPUT_TOKENS: TokenConfig = TokenConfig::new(
    "gen_ai.usage.output_tokens",
    &[
        "gen_ai.usage.completion_tokens",
        "llm.usage.completion_tokens",
        "llm.token_count.completion",
        "ai.usage.completionTokens",
    ],
);

const TOTAL_TOKENS: TokenConfig =
    TokenConfig::new("gen_ai.usage.total_tokens", &["llm.token_count.total"]);

const CACHE_READ_TOKENS: TokenConfig = TokenConfig::new(
    "gen_ai.usage.cache_read_input_tokens",
    &[
        "gen_ai.usage.cache_read_tokens",
        "llm.usage.cache_read_input_tokens",
        "ai.usage.cachedInputTokens",
    ],
);

const CACHE_WRITE_TOKENS: TokenConfig = TokenConfig::new(
    "gen_ai.usage.cache_creation_input_tokens",
    &[
        "gen_ai.usage.cache_write_input_tokens", // Strands
        "gen_ai.usage.cache_write_tokens",
        "llm.usage.cache_creation_input_tokens",
    ],
);

const REASONING_TOKENS: TokenConfig = TokenConfig::new(
    "gen_ai.usage.output_reasoning_tokens",
    &[
        "gen_ai.usage.thoughts_token_count",
        "ai.usage.reasoningTokens",
    ],
);

const KNOWN_USAGE_FIELDS: &[&str] = &[
    "input_tokens",
    "output_tokens",
    "total_tokens",
    "prompt_tokens",
    "completion_tokens",
    "cache_read_input_tokens",
    "cache_read_tokens",
    "cache_creation_input_tokens",
    "cache_write_tokens",
    "output_reasoning_tokens",
    "thoughts_token_count",
];

// ============================================================================
// FRAMEWORK DETECTION
// ============================================================================

/// Custom matcher function type for complex framework detection logic
type CustomMatcher = fn(&str, &HashMap<String, String>, &HashMap<String, String>) -> bool;

/// Framework detection rule for declarative matching
struct FrameworkRule {
    framework: Framework,
    /// Match if span name equals or starts with any of these
    span_name_match: &'static [&'static str],
    /// Match if any attribute key starts with any of these prefixes
    attr_prefix: &'static [&'static str],
    /// Match if attribute equals (key, value)
    attr_equals: &'static [(&'static str, &'static str)],
    /// Match if service.name equals or contains any of these
    service_name: &'static [&'static str],
    /// Match if any of these attribute keys exist
    attr_exists: &'static [&'static str],
    /// Match if metadata JSON contains any of these strings
    metadata_contains: &'static [&'static str],
    /// Custom matcher for complex logic (return true to match)
    custom: Option<CustomMatcher>,
}

/// Default rule for struct update syntax in const context
const DEFAULT_RULE: FrameworkRule = FrameworkRule {
    framework: Framework::Unknown,
    span_name_match: &[],
    attr_prefix: &[],
    attr_equals: &[],
    service_name: &[],
    attr_exists: &[],
    metadata_contains: &[],
    custom: None,
};

/// Macro to create FrameworkRule with defaults for unspecified fields
macro_rules! rule {
    ($framework:expr $(, $field:ident : $value:expr)* $(,)?) => {
        FrameworkRule {
            framework: $framework,
            $($field: $value,)*
            ..DEFAULT_RULE
        }
    };
}

impl FrameworkRule {
    fn matches(
        &self,
        span_name: &str,
        span_attrs: &HashMap<String, String>,
        resource_attrs: &HashMap<String, String>,
    ) -> bool {
        // Span name match
        if !self.span_name_match.is_empty()
            && self
                .span_name_match
                .iter()
                .any(|p| span_name == *p || span_name.starts_with(p))
        {
            return true;
        }

        // Attribute prefix match
        if !self.attr_prefix.is_empty()
            && self
                .attr_prefix
                .iter()
                .any(|p| span_attrs.keys().any(|k| k.starts_with(p)))
        {
            return true;
        }

        // Attribute equals match
        if !self.attr_equals.is_empty()
            && self
                .attr_equals
                .iter()
                .any(|(k, v)| span_attrs.get(*k).is_some_and(|val| val == *v))
        {
            return true;
        }

        // Service name match
        if !self.service_name.is_empty() {
            if let Some(svc) = resource_attrs.get(keys::SERVICE_NAME) {
                if self
                    .service_name
                    .iter()
                    .any(|s| svc == *s || svc.contains(s))
                {
                    return true;
                }
            }
        }

        // Attribute exists match
        if !self.attr_exists.is_empty()
            && self.attr_exists.iter().any(|k| span_attrs.contains_key(*k))
        {
            return true;
        }

        // Metadata contains match
        if !self.metadata_contains.is_empty() {
            if let Some(metadata) = span_attrs.get(keys::METADATA) {
                if self.metadata_contains.iter().any(|s| metadata.contains(s)) {
                    return true;
                }
            }
        }

        // Custom matcher
        if let Some(f) = self.custom {
            if f(span_name, span_attrs, resource_attrs) {
                return true;
            }
        }

        false
    }
}

/// Vercel AI SDK custom matcher - has complex prefix matching
fn vercel_ai_matcher(
    _: &str,
    span_attrs: &HashMap<String, String>,
    _: &HashMap<String, String>,
) -> bool {
    span_attrs.keys().any(|k| {
        k.starts_with("ai.prompt.")
            || k.starts_with("ai.completion.")
            || k.starts_with("ai.settings.")
            || k.starts_with("ai.telemetry.")
            || k.starts_with("ai.stream.")
            || k.starts_with("ai.finishReason")
            || k.starts_with("ai.usage.")
    })
}

/// Logfire SDK name matcher
fn logfire_sdk_matcher(
    _: &str,
    _: &HashMap<String, String>,
    resource_attrs: &HashMap<String, String>,
) -> bool {
    resource_attrs
        .get(keys::TELEMETRY_SDK_NAME)
        .is_some_and(|v| v.contains("logfire"))
}

/// Traceloop SDK name matcher
fn traceloop_sdk_matcher(
    _: &str,
    _: &HashMap<String, String>,
    resource_attrs: &HashMap<String, String>,
) -> bool {
    resource_attrs
        .get(keys::TELEMETRY_SDK_NAME)
        .is_some_and(|v| v.contains("traceloop"))
}

/// Framework detection rules in priority order (first match wins)
///
/// IMPORTANT: All specific attribute-based rules come BEFORE generic service-name fallbacks.
/// The sideseat SDK defaults service.name to "strands-agents", so service_name-based detection
/// must be the LAST check to avoid misidentifying other frameworks.
const FRAMEWORK_RULES: &[FrameworkRule] = &[
    // AutoGen - check gen_ai.system and span name prefix
    // (OpenInference AutoGen sets gen_ai.system="autogen" but service.name may be default)
    rule!(Framework::AutoGen,
        span_name_match: &["autogen ", "autogen."],
        attr_prefix: &["autogen."],
        attr_equals: &[(keys::GEN_AI_SYSTEM, "autogen")],
    ),
    // Google ADK - check gcp.vertex.agent.* attributes BEFORE service name fallback
    rule!(Framework::GoogleAdk,
        attr_prefix: &["google.adk.", "gcp.vertex.agent."],
        attr_equals: &[(keys::GEN_AI_SYSTEM, "gcp.vertex.agent")],
    ),
    // CrewAI - check specific attributes
    rule!(Framework::CrewAI,
        service_name: &["crewAI-telemetry"],
        attr_exists: &["crewai_version", "crew_key", "crew_id", "crew_fingerprint", "task_key"],
    ),
    // LangGraph (before LangChain - more specific)
    rule!(Framework::LangGraph,
        span_name_match: &["LangGraph", "LangGraph."],
        attr_prefix: &["langgraph."],
        metadata_contains: &["langgraph_", "\"langgraph_"],
    ),
    // LangChain
    rule!(Framework::LangChain, attr_prefix: &["langchain.", "langsmith."]),
    // LlamaIndex
    rule!(Framework::LlamaIndex, attr_prefix: &["llama_index."]),
    // OpenInference
    rule!(Framework::OpenInference, attr_prefix: &["openinference."]),
    // Semantic Kernel
    rule!(Framework::SemanticKernel, attr_prefix: &["semantic_kernel."]),
    // Azure OpenAI (before AzureAIFoundry - more specific)
    rule!(Framework::AzureOpenAI,
        attr_equals: &[
            (keys::GEN_AI_SYSTEM, "azure_openai"),
            (keys::GEN_AI_SYSTEM, "azure.openai"),
            (keys::GEN_AI_PROVIDER_NAME, "azure_openai"),
        ],
        attr_prefix: &["azure.openai."],
    ),
    // Azure AI Foundry
    rule!(Framework::AzureAIFoundry, attr_prefix: &["az.ai."]),
    // Vertex AI
    rule!(Framework::VertexAI, attr_prefix: &["gcp.vertex_ai."]),
    // Vercel AI SDK
    rule!(Framework::VercelAISdk,
        attr_exists: &["ai.operationId", "ai.telemetry.functionId", "ai.telemetry.metadata"],
        custom: Some(vercel_ai_matcher),
    ),
    // Logfire
    rule!(Framework::Logfire, attr_prefix: &["logfire."], custom: Some(logfire_sdk_matcher)),
    // MLflow
    rule!(Framework::MLFlow, attr_prefix: &["mlflow."]),
    // TraceLoop
    rule!(Framework::TraceLoop, attr_prefix: &["traceloop."], custom: Some(traceloop_sdk_matcher)),
    // LiveKit
    rule!(Framework::LiveKit, attr_prefix: &["livekit.", "lk."]),
    // OpenAI Agents SDK
    rule!(Framework::OpenAIAgents,
        attr_prefix: &["openai.agents."],
        service_name: &["openai-agents", "openai_agents"],
    ),
    // AWS Bedrock
    rule!(Framework::AWSBedrock,
        attr_prefix: &["aws.bedrock."],
        attr_equals: &[(keys::GEN_AI_SYSTEM, "aws_bedrock"), (keys::GEN_AI_SYSTEM, "aws.bedrock")],
    ),
    // Strands Agents - LAST because service.name="strands-agents" is the sideseat SDK default
    // Only match if gen_ai.system explicitly says "strands-agents" or no other framework matched
    rule!(Framework::StrandsAgents,
        attr_equals: &[(keys::GEN_AI_SYSTEM, "strands-agents"), (keys::GEN_AI_PROVIDER_NAME, "strands-agents")],
        service_name: &["strands-agents"],
    ),
];

/// Detect framework from span and resource attributes.
pub(crate) fn detect_framework(
    span_name: &str,
    span_attrs: &HashMap<String, String>,
    resource_attrs: &HashMap<String, String>,
) -> Framework {
    for rule in FRAMEWORK_RULES {
        if rule.matches(span_name, span_attrs, resource_attrs) {
            return rule.framework;
        }
    }
    Framework::Unknown
}

// ============================================================================
// SEMANTIC KIND CLASSIFICATION
// ============================================================================

#[derive(Clone, Copy)]
#[allow(clippy::upper_case_acronyms)]
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
pub(crate) fn categorize_span(span_name: &str, attrs: &HashMap<String, String>) -> SpanCategory {
    // Priority 0: External service indicators (HTTP/RPC/DB) are NEVER GenAI spans
    // This must be checked FIRST to prevent AWS Bedrock API calls (rpc.system=aws-api)
    // from being classified as LLM even if they have gen_ai.* attributes.
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
pub(crate) fn detect_observation_type(
    span_name: &str,
    attrs: &HashMap<String, String>,
) -> ObservationType {
    // Priority 0: External service calls (HTTP/RPC/DB) are NEVER GenAI spans
    // This must be checked FIRST to prevent AWS Bedrock API calls (rpc.system=aws-api)
    // from being classified as Generation even if they have gen_ai.* attributes.
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

    // Priority 6: Has model = Generation
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
fn extract_autogen_tokens(attrs: &HashMap<String, String>) -> (i64, i64) {
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

// ============================================================================
// ATTRIBUTE EXTRACTION
// ============================================================================

pub(crate) fn extract_semantic(span: &mut SpanData, attrs: &HashMap<String, String>) {
    let metadata: Option<JsonValue> = extract_json(attrs, keys::METADATA);

    // Session ID with framework fallbacks (including Vercel AI telemetry metadata)
    span.session_id = get_first(
        attrs,
        &[
            keys::SESSION_ID,
            keys::LANGSMITH_SESSION_ID,
            keys::LANGSMITH_TRACE_SESSION_ID, // LangSmith OTEL exporter
            keys::GCP_VERTEX_SESSION_ID,
            keys::AI_TELEMETRY_SESSION_ID, // Vercel AI SDK
            keys::LANGGRAPH_THREAD_ID,     // LangGraph
            keys::MLFLOW_TRACE_SESSION,    // MLflow
        ],
    )
    .or_else(|| {
        // Try thread_id or langgraph_thread_id from metadata
        metadata.as_ref().and_then(|m| {
            m.get("thread_id")
                .or_else(|| m.get("langgraph_thread_id"))
                .and_then(|v| v.as_str())
                .map(String::from)
        })
    });

    // User ID (including Vercel AI telemetry metadata)
    span.user_id = get_first(
        attrs,
        &[
            keys::USER_ID,
            keys::ENDUSER_ID,
            keys::AI_TELEMETRY_USER_ID, // Vercel AI SDK
            keys::MLFLOW_TRACE_USER,    // MLflow
        ],
    )
    .or_else(|| {
        metadata
            .as_ref()?
            .get("user_id")?
            .as_str()
            .map(String::from)
    });

    // HTTP
    span.http_method = get_first(attrs, &[keys::HTTP_METHOD, keys::HTTP_REQUEST_METHOD]);
    span.http_url = get_first(attrs, &[keys::HTTP_URL, keys::URL_FULL]);
    span.http_status_code = get_first(
        attrs,
        &[keys::HTTP_STATUS_CODE, keys::HTTP_RESPONSE_STATUS_CODE],
    )
    .and_then(|v| v.parse().ok());

    // Database
    span.db_system = attrs.get(keys::DB_SYSTEM).cloned();
    span.db_name = attrs.get(keys::DB_NAME).cloned();
    span.db_operation = attrs.get(keys::DB_OPERATION).cloned();
    span.db_statement = attrs.get(keys::DB_STATEMENT).cloned();

    // Storage
    span.storage_system = attrs.get(keys::CLOUD_PROVIDER).cloned();
    span.storage_bucket = get_first(attrs, &[keys::AWS_S3_BUCKET, keys::GCP_GCS_BUCKET]);
    span.storage_object = get_first(attrs, &[keys::AWS_S3_KEY, keys::GCP_GCS_OBJECT]);

    // Messaging
    span.messaging_system = attrs.get(keys::MESSAGING_SYSTEM).cloned();
    span.messaging_destination = get_first(
        attrs,
        &[
            keys::MESSAGING_DESTINATION,
            keys::MESSAGING_DESTINATION_NAME,
        ],
    );

    // Tags (merge and dedupe from multiple sources)
    span.tags = merge_tags(attrs, &[keys::TAGS, keys::LANGSMITH_TAGS, keys::TAG_TAGS]);
}

pub(crate) fn extract_genai(span: &mut SpanData, attrs: &HashMap<String, String>, span_name: &str) {
    // System and operation
    span.gen_ai_system = get_first(
        attrs,
        &[
            keys::GEN_AI_SYSTEM,
            "az.ai.inference.model_provider",
            "ai.model.provider",
            "llm.provider",
        ],
    );
    span.gen_ai_operation_name = attrs.get(keys::GEN_AI_OPERATION_NAME).cloned();

    // Models (including embedding/reranker model names as fallback)
    span.gen_ai_request_model = get_first(
        attrs,
        &[
            keys::GEN_AI_REQUEST_MODEL,
            "ai.model.id",
            "llm.model_name",
            keys::EMBEDDING_MODEL_NAME,
            keys::RERANKER_MODEL_NAME,
        ],
    );
    span.gen_ai_response_model =
        get_first(attrs, &[keys::GEN_AI_RESPONSE_MODEL, "llm.response.model"]);
    span.gen_ai_response_id = attrs.get(keys::GEN_AI_RESPONSE_ID).cloned();

    // Google ADK: model from llm_request JSON
    if span.gen_ai_request_model.is_none() {
        if let Some(req) = extract_json::<JsonValue>(attrs, keys::GCP_VERTEX_LLM_REQUEST) {
            if let Some(model) = req.get("model").and_then(|v| v.as_str()) {
                if !model.is_empty() {
                    span.gen_ai_request_model = Some(model.to_string());
                }
            }
        }
    }

    // CrewAI: model from crew_agents JSON (agent.llm field)
    if span.gen_ai_request_model.is_none() {
        if let Some(agents) = extract_json::<JsonValue>(attrs, "crew_agents") {
            if let Some(arr) = agents.as_array() {
                for agent in arr {
                    if let Some(model) = agent.get("llm").and_then(|v| v.as_str()) {
                        if !model.is_empty() {
                            span.gen_ai_request_model = Some(model.to_string());
                            break;
                        }
                    }
                }
            }
        }
    }

    // Request parameters
    span.gen_ai_temperature = parse_opt(attrs, keys::GEN_AI_TEMPERATURE);
    span.gen_ai_top_p = parse_opt(attrs, keys::GEN_AI_TOP_P);
    span.gen_ai_top_k = parse_opt(attrs, keys::GEN_AI_TOP_K);
    span.gen_ai_max_tokens = parse_opt(attrs, keys::GEN_AI_MAX_TOKENS);
    span.gen_ai_frequency_penalty = parse_opt(attrs, keys::GEN_AI_FREQUENCY_PENALTY);
    span.gen_ai_presence_penalty = parse_opt(attrs, keys::GEN_AI_PRESENCE_PENALTY);

    // OpenInference llm.invocation_parameters fallback
    if let Some(params_json) = attrs.get(keys::LLM_INVOCATION_PARAMETERS) {
        if let Ok(params) = serde_json::from_str::<JsonValue>(params_json) {
            if span.gen_ai_temperature.is_none() {
                span.gen_ai_temperature = params.get("temperature").and_then(|v| v.as_f64());
            }
            if span.gen_ai_top_p.is_none() {
                span.gen_ai_top_p = params.get("top_p").and_then(|v| v.as_f64());
            }
            if span.gen_ai_top_k.is_none() {
                span.gen_ai_top_k = params.get("top_k").and_then(|v| v.as_i64());
            }
            if span.gen_ai_max_tokens.is_none() {
                span.gen_ai_max_tokens = params
                    .get("max_tokens")
                    .or_else(|| params.get("max_output_tokens"))
                    .and_then(|v| v.as_i64());
            }
            if span.gen_ai_frequency_penalty.is_none() {
                span.gen_ai_frequency_penalty =
                    params.get("frequency_penalty").and_then(|v| v.as_f64());
            }
            if span.gen_ai_presence_penalty.is_none() {
                span.gen_ai_presence_penalty =
                    params.get("presence_penalty").and_then(|v| v.as_f64());
            }
        }
    }

    if let Some(stops) = attrs.get(keys::GEN_AI_STOP_SEQUENCES) {
        span.gen_ai_stop_sequences = parse_string_array(stops);
    }
    if let Some(reasons) = attrs.get(keys::GEN_AI_FINISH_REASONS) {
        span.gen_ai_finish_reasons = parse_string_array(reasons);
    }

    // Agent fields
    span.gen_ai_agent_id = get_first(attrs, &[keys::GEN_AI_AGENT_ID, keys::AWS_BEDROCK_AGENT_ID]);
    span.gen_ai_agent_name = get_first(
        attrs,
        &[
            keys::GEN_AI_AGENT_NAME,
            "agent_role",
            "recipient_agent_class",
            "sender_agent_class",
        ],
    );

    // Tool fields - logfire.msg is used by Pydantic AI for descriptive tool names
    span.gen_ai_tool_name = get_first(
        attrs,
        &[
            keys::GEN_AI_TOOL_NAME,
            "tool.name",
            "tool_name",
            keys::LOGFIRE_MSG,
        ],
    )
    .or_else(|| span_name.strip_prefix("execute_tool ").map(String::from));
    span.gen_ai_tool_call_id = attrs.get(keys::GEN_AI_TOOL_CALL_ID).cloned();

    // Performance
    span.gen_ai_server_ttft_ms = parse_opt(attrs, keys::GEN_AI_TTFT);
    span.gen_ai_server_request_duration_ms = parse_opt(attrs, keys::GEN_AI_REQUEST_DURATION);

    // Token usage
    span.gen_ai_usage_input_tokens = INPUT_TOKENS.extract(attrs);
    span.gen_ai_usage_output_tokens = OUTPUT_TOKENS.extract(attrs);

    // MLflow token usage from JSON blob (if not already extracted)
    if span.gen_ai_usage_input_tokens == 0 || span.gen_ai_usage_output_tokens == 0 {
        if let Some(usage) = extract_json::<JsonValue>(attrs, keys::MLFLOW_CHAT_TOKEN_USAGE) {
            if span.gen_ai_usage_input_tokens == 0 {
                span.gen_ai_usage_input_tokens = usage
                    .get("prompt_tokens")
                    .or_else(|| usage.get("input_tokens"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
            }
            if span.gen_ai_usage_output_tokens == 0 {
                span.gen_ai_usage_output_tokens = usage
                    .get("completion_tokens")
                    .or_else(|| usage.get("output_tokens"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
            }
        }
    }

    // Google ADK: tokens from llm_response JSON
    if span.gen_ai_usage_input_tokens == 0 && span.gen_ai_usage_output_tokens == 0 {
        if let Some(resp) = extract_json::<JsonValue>(attrs, keys::GCP_VERTEX_LLM_RESPONSE) {
            if let Some(usage) = resp.get("usage_metadata") {
                if span.gen_ai_usage_input_tokens == 0 {
                    span.gen_ai_usage_input_tokens = usage
                        .get("prompt_token_count")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                }
                if span.gen_ai_usage_output_tokens == 0 {
                    span.gen_ai_usage_output_tokens = usage
                        .get("candidates_token_count")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                }
            }
        }
    }

    span.gen_ai_usage_total_tokens = TOTAL_TOKENS
        .extract(attrs)
        .max(span.gen_ai_usage_input_tokens + span.gen_ai_usage_output_tokens);
    span.gen_ai_usage_cache_read_tokens = CACHE_READ_TOKENS.extract(attrs);
    span.gen_ai_usage_cache_write_tokens = CACHE_WRITE_TOKENS.extract(attrs);
    span.gen_ai_usage_reasoning_tokens = REASONING_TOKENS.extract(attrs);

    // CrewAI: tokens from output.value JSON (CrewOutput.token_usage)
    // CrewAI embeds token usage in the serialized CrewOutput object, not as flat attributes.
    if span.gen_ai_usage_input_tokens == 0 && span.gen_ai_usage_output_tokens == 0 {
        let is_crewai = attrs.contains_key("crew_key")
            || attrs.contains_key("crew_id")
            || attrs.contains_key("crew_tasks")
            || attrs.contains_key("task_key");
        if is_crewai {
            if let Some(output) = extract_json::<JsonValue>(attrs, keys::OUTPUT_VALUE) {
                if let Some(usage) = output.get("token_usage") {
                    span.gen_ai_usage_input_tokens = usage
                        .get("prompt_tokens")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    span.gen_ai_usage_output_tokens = usage
                        .get("completion_tokens")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    if span.gen_ai_usage_cache_read_tokens == 0 {
                        span.gen_ai_usage_cache_read_tokens = usage
                            .get("cached_prompt_tokens")
                            .and_then(|v| v.as_i64())
                            .unwrap_or(0);
                    }
                    // Recompute total: honor reported total if larger than sum
                    let crewai_total = usage
                        .get("total_tokens")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    span.gen_ai_usage_total_tokens = crewai_total
                        .max(span.gen_ai_usage_input_tokens + span.gen_ai_usage_output_tokens);
                }
            }
        }
    }

    // AutoGen: tokens from output.value.messages[].models_usage on chain spans.
    // The models_usage field is AutoGen-specific; safe to check without framework guard.
    if span.gen_ai_usage_input_tokens == 0 && span.gen_ai_usage_output_tokens == 0 {
        let (pt, ct) = extract_autogen_tokens(attrs);
        if pt > 0 || ct > 0 {
            span.gen_ai_usage_input_tokens = pt;
            span.gen_ai_usage_output_tokens = ct;
            span.gen_ai_usage_total_tokens = pt + ct;
        }
    }

    // Usage details (remaining gen_ai.usage.* fields)
    let mut details = serde_json::Map::new();
    for (key, value) in attrs {
        if let Some(field) = key.strip_prefix("gen_ai.usage.")
            && !KNOWN_USAGE_FIELDS.contains(&field)
        {
            let json_val = value
                .parse::<i64>()
                .map(|n| json!(n))
                .or_else(|_| value.parse::<f64>().map(|n| json!(n)))
                .unwrap_or_else(|_| json!(value));
            details.insert(field.to_string(), json_val);
        }
    }
    span.gen_ai_usage_details = if details.is_empty() {
        JsonValue::Null
    } else {
        JsonValue::Object(details)
    };

    // Pre-calculated costs (OpenInference llm.cost.* attributes)
    span.extracted_cost_total = parse_opt(attrs, keys::LLM_COST_TOTAL);
    span.extracted_cost_input = parse_opt(attrs, keys::LLM_COST_PROMPT);
    span.extracted_cost_output = parse_opt(attrs, keys::LLM_COST_COMPLETION);
}

#[cfg(test)]
#[path = "attributes_tests.rs"]
mod tests;
