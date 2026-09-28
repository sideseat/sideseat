use super::*;

// ============================================================================
// FRAMEWORK DETECTION
// ============================================================================

/// Custom matcher function type for complex framework detection logic
#[cfg(test)]
type CustomMatcher = fn(&str, &HashMap<String, String>, &HashMap<String, String>) -> bool;

/// Framework detection rule for declarative matching
#[cfg(test)]
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
#[cfg(test)]
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
#[cfg(test)]
macro_rules! rule {
    ($framework:expr $(, $field:ident : $value:expr)* $(,)?) => {
        FrameworkRule {
            framework: $framework,
            $($field: $value,)*
            ..DEFAULT_RULE
        }
    };
}

#[cfg(test)]
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
#[cfg(test)]
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
#[cfg(test)]
fn logfire_sdk_matcher(
    _: &str,
    _: &HashMap<String, String>,
    resource_attrs: &HashMap<String, String>,
) -> bool {
    resource_attrs
        .get(keys::TELEMETRY_SDK_NAME)
        .is_some_and(|v| v.contains("logfire"))
}

/// Strands Agents custom matcher — case-insensitive search for "strands" + separator + "agent"
/// in the span name or gen_ai.agent.name attribute.
/// Separators: space, hyphen, underscore (e.g. "Strands Agent", "strands-agent", "strands_agent").
#[cfg(test)]
fn strands_agents_matcher(
    span_name: &str,
    span_attrs: &HashMap<String, String>,
    _: &HashMap<String, String>,
) -> bool {
    let contains_strands_agent = |s: &str| {
        let lower = s.to_lowercase();
        lower.contains("strands agent")
            || lower.contains("strands-agent")
            || lower.contains("strands_agent")
    };
    contains_strands_agent(span_name)
        || span_attrs
            .get("gen_ai.agent.name")
            .is_some_and(|v| contains_strands_agent(v))
}

/// Traceloop SDK name matcher
#[cfg(test)]
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
#[cfg(test)]
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
    // Frameworks instrumented *through* OpenInference. Each emits its own `X.*`
    // attributes alongside `openinference.*`, so these rules must precede the
    // OpenInference rule below or that broader rule claims the spans first.
    //
    // Detection is by attribute prefix only, never service.name: service-name matching is
    // a substring test (`svc.contains(s)`), so `"agno"` would also match a user service
    // called `diagnostics`.
    rule!(Framework::Agno, attr_prefix: &["agno."]),
    rule!(Framework::Smolagents, attr_prefix: &["smolagents."]),
    rule!(Framework::AgentScope, attr_prefix: &["agentscope."]),
    rule!(Framework::Langflow, attr_prefix: &["langflow."]),
    rule!(Framework::Ag2, attr_prefix: &["ag2."]),
    rule!(Framework::Haystack, attr_prefix: &["haystack."]),
    // browser-use sets gen_ai.provider.name unconditionally on every span it emits.
    rule!(Framework::BrowserUse, attr_equals: &[(keys::GEN_AI_PROVIDER_NAME, "browser_use")]),
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
    // Vertex AI — opentelemetry-instrumentation-vertexai (openllmetry) uses vertexai.* span names
    rule!(Framework::VertexAI, span_name_match: &["vertexai."]),
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
    // Microsoft Agent Framework
    rule!(Framework::AgentFramework,
        attr_equals: &[(keys::GEN_AI_PROVIDER_NAME, "microsoft.agent_framework")],
        service_name: &["agent-framework-core"],
    ),
    // AWS Bedrock
    rule!(Framework::AWSBedrock,
        attr_prefix: &["aws.bedrock."],
        attr_equals: &[(keys::GEN_AI_SYSTEM, "aws_bedrock"), (keys::GEN_AI_SYSTEM, "aws.bedrock")],
    ),
    // Claude Agent SDK - the Claude Code CLI subprocess emits claude_code.* spans
    // (interaction, llm_request, tool, tool.execution, tool.blocked_on_user, hook).
    // The prefix covers all of them. Two service names because the CLI reports
    // "claude-code" while the host process wrapping it reports "claude-agent-sdk".
    rule!(Framework::ClaudeAgentSdk,
        span_name_match: &["claude_code."],
        service_name: &["claude-code", "claude-agent-sdk"],
    ),
    // Strands Agents - LAST because service.name="strands-agents" is the sideseat SDK default
    // Only match if gen_ai.system explicitly says "strands-agents" or no other framework matched.
    // Custom matcher does case-insensitive search for "strands" + separator + "agent"
    // in the span name or gen_ai.agent.name attribute (covers space, hyphen, underscore).
    rule!(Framework::StrandsAgents,
        attr_equals: &[
            (keys::GEN_AI_SYSTEM, "strands-agents"),
            (keys::GEN_AI_PROVIDER_NAME, "strands-agents"),
        ],
        service_name: &["strands-agents"],
        custom: Some(strands_agents_matcher),
    ),
];

/// Detect framework from span and resource attributes.
///
/// Evidence from the span wins; a declaration only fills the gap. The SDKs write
/// `sideseat.framework` into the resource, and it is consulted **last** - after every rule has failed -
/// because it is a statement about the *process*, not about this span: a process configured for Strands can
/// still emit LangChain spans from a nested library, and those carry `langchain.*` for a rule to find.
/// Overriding on the declaration would relabel them.
///
/// It is consulted at all because the current OTel GenAI conventions are framework-neutral by design: the
/// Vercel AI SDK's current integration emits pure `gen_ai.*` with no `ai.*` attributes, so no rule can
/// attribute it and no rule should have to. A declaration is the only evidence that exists.
pub(crate) fn detect_framework(
    span_name: &str,
    span_attrs: &HashMap<String, String>,
    resource_attrs: &HashMap<String, String>,
) -> String {
    let ctx = sideseat_domain::rules::DetectContext {
        span_name,
        span_attrs,
        resource_attrs,
    };
    let plan = &sideseat_domain::rules::ruleset().detect;
    if let Some(rule) = plan.resolve(&ctx) {
        return rule.label.clone();
    }
    let declared = resource_attrs
        .get(keys::SIDESEAT_FRAMEWORK)
        .and_then(|declared| plan.label_from_declaration(declared));
    if declared.is_none() {
        // The evidence behind "nothing recognised this producer", which was a bare `None` everywhere. Reported
        // only where the span carries a key some rule reads and the values disagree - which is exactly an
        // unrecognised producer conforming to a convention, and the remedy is to declare the value.
        let near = plan.near_misses(&ctx);
        if !near.is_empty() {
            tracing::debug!(
                target: "sideseat::rules",
                span_name = span_name,
                near_misses = ?near,
                "no rule attributed this span, and these rules read a key it carries with a value they do not \
                 declare"
            );
        }
    }
    declared
        .unwrap_or(sideseat_domain::rules::UNCLAIMED_LABEL)
        .to_string()
}

/// The detection table this engine replaced, kept as the equivalence oracle.
///
/// Compared against the rules over every span of the whole corpus by
/// `the_rules_reproduce_the_legacy_detection`, which is what makes the migration a provable no-op
/// rather than a rewrite trusted because the goldens happened to stay green.
#[cfg(test)]
pub(crate) fn legacy_detect_framework(
    span_name: &str,
    span_attrs: &HashMap<String, String>,
    resource_attrs: &HashMap<String, String>,
) -> &'static str {
    for rule in FRAMEWORK_RULES {
        if rule.matches(span_name, span_attrs, resource_attrs) {
            return rule.framework.as_str();
        }
    }
    declared_framework(resource_attrs)
        .unwrap_or(Framework::Unknown)
        .as_str()
}

/// The framework an SDK declared, when it declared exactly one this server recognises.
///
/// A list is accepted because the SDKs accept one (`framework=[Strands, Bedrock]`), and resolved only when
/// it names a single *framework*: provider slugs return `None` from `from_sdk_slug`, so declaring
/// `[Strands, Bedrock]` still resolves to Strands, while two genuine frameworks resolve to nothing. Two
/// answers is not an answer, and guessing between them would put a label on a span with no evidence for it.
#[cfg(test)]
fn declared_framework(resource_attrs: &HashMap<String, String>) -> Option<Framework> {
    let declared = resource_attrs.get(keys::SIDESEAT_FRAMEWORK)?;
    let mut frameworks = declared
        .split(',')
        .filter_map(Framework::from_sdk_slug)
        .collect::<Vec<_>>();
    frameworks.dedup();
    match frameworks.as_slice() {
        [one] => Some(*one),
        _ => None,
    }
}
