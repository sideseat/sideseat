#[cfg(test)]
use std::collections::HashMap;

#[cfg(test)]
use serde_json::Value as JsonValue;

#[cfg(test)]
use super::{TokenReadings, extract_autogen_tokens, extract_json, keys};

// ============================================================================
// TOKEN USAGE CONFIGURATION
// ============================================================================

/// Token count extraction configuration with fallback keys.
///
/// Retired: where each counter is written is declared in `rules/vocabulary/span-fields-usage.json`. Kept as the
/// equivalence oracle - the arithmetic that reads the counters was never here.
#[cfg(test)]
pub(super) struct TokenConfig {
    primary: &'static str,
    fallbacks: &'static [&'static str],
    /// Fallbacks whose names are too generic to consult globally (e.g. a bare
    /// `input_tokens`). Only read for spans that `scope` accepts, so a framework that
    /// happens to use the same name is never credited with tokens or cost.
    scoped_fallbacks: &'static [&'static str],
}

/// Every counter as the retired code found it: the flat table, then the three embedded usage objects in the
/// order their fallbacks ran. The equivalence oracle for `rules/vocabulary/span-fields-usage.json`.
///
/// Each embedded block is entered only for a counter nothing before it supplied - which is what an ordered
/// chain means, and is why the declared form needs no flags threaded through it.
#[cfg(test)]
pub(in crate::traces::extract) fn token_readings_legacy(
    attrs: &HashMap<String, String>,
    span_name: &str,
) -> TokenReadings {
    let mut input = INPUT_TOKENS.extract_opt_for_span(attrs, span_name);
    let mut output = OUTPUT_TOKENS.extract_opt_for_span(attrs, span_name);
    let mut cache_read = CACHE_READ_TOKENS.extract_opt_for_span(attrs, span_name);
    let mut cache_write = CACHE_WRITE_TOKENS.extract_opt_for_span(attrs, span_name);

    if input.is_none() || output.is_none() {
        if let Some(usage) = extract_json::<JsonValue>(attrs, keys::MLFLOW_CHAT_TOKEN_USAGE) {
            if input.is_none() {
                input = usage
                    .get("prompt_tokens")
                    .or_else(|| usage.get("input_tokens"))
                    .and_then(|v| v.as_i64());
            }
            if output.is_none() {
                output = usage
                    .get("completion_tokens")
                    .or_else(|| usage.get("output_tokens"))
                    .and_then(|v| v.as_i64());
            }
        }
    }
    if input.is_none() || output.is_none() {
        if let Some(resp) = extract_json::<JsonValue>(attrs, keys::GCP_VERTEX_LLM_RESPONSE) {
            if let Some(usage) = resp.get("usage_metadata") {
                if input.is_none() {
                    input = usage.get("prompt_token_count").and_then(|v| v.as_i64());
                }
                if output.is_none() {
                    output = usage.get("candidates_token_count").and_then(|v| v.as_i64());
                }
            }
        }
    }
    if input.is_none() || output.is_none() || cache_read.is_none() || cache_write.is_none() {
        if let Some(resp) = extract_json::<JsonValue>(attrs, keys::RESPONSE_DATA) {
            if let Some(usage) = resp.get("usage") {
                if input.is_none() {
                    input = usage
                        .get("input_tokens")
                        .or_else(|| usage.get("prompt_tokens"))
                        .and_then(|v| v.as_i64());
                }
                if output.is_none() {
                    output = usage
                        .get("output_tokens")
                        .or_else(|| usage.get("completion_tokens"))
                        .and_then(|v| v.as_i64());
                }
                if cache_read.is_none() {
                    cache_read = usage
                        .get("cache_read_input_tokens")
                        .and_then(|v| v.as_i64());
                }
                if cache_write.is_none() {
                    cache_write = usage
                        .get("cache_creation_input_tokens")
                        .and_then(|v| v.as_i64());
                }
            }
        }
    }
    // Another dialect's embedded object, resolved whatever the chains answered.
    let embedded = |member: &str| -> Option<i64> {
        let gated = attrs.contains_key("crew_key")
            || attrs.contains_key("crew_id")
            || attrs.contains_key("crew_tasks")
            || attrs.contains_key("task_key");
        if !gated {
            return None;
        }
        extract_json::<JsonValue>(attrs, keys::OUTPUT_VALUE)?
            .get("token_usage")?
            .get(member)?
            .as_i64()
    };

    TokenReadings {
        input,
        output,
        total_reported: TOTAL_TOKENS.extract_opt_for_span(attrs, span_name),
        cache_read,
        cache_write,
        reasoning: REASONING_TOKENS.extract_opt_for_span(attrs, span_name),
        candidate_input: embedded("prompt_tokens"),
        candidate_output: embedded("completion_tokens"),
        candidate_cache_read: embedded("cached_prompt_tokens"),
        candidate_total: embedded("total_tokens"),
        summed_input: Some(extract_autogen_tokens(attrs).0),
        summed_output: Some(extract_autogen_tokens(attrs).1),
    }
}

#[cfg(test)]
impl TokenConfig {
    const fn new(primary: &'static str, fallbacks: &'static [&'static str]) -> Self {
        Self {
            primary,
            fallbacks,
            scoped_fallbacks: &[],
        }
    }

    const fn with_scoped(
        primary: &'static str,
        fallbacks: &'static [&'static str],
        scoped_fallbacks: &'static [&'static str],
    ) -> Self {
        Self {
            primary,
            fallbacks,
            scoped_fallbacks,
        }
    }

    pub(super) fn extract(&self, attrs: &HashMap<String, String>) -> i64 {
        self.extract_for_span(attrs, "")
    }

    pub(super) fn extract_for_span(&self, attrs: &HashMap<String, String>, span_name: &str) -> i64 {
        self.extract_opt_for_span(attrs, span_name).unwrap_or(0)
    }

    /// The counter if any key in the chain carried one, distinguishing **absent** from a genuine `0`.
    ///
    /// The stored column is never NULL - `extract_for_span` still defaults to 0 - but the framework fallbacks
    /// need the difference: they treated `0` as "missing", so a completion that genuinely produced no output
    /// tokens had its 0 replaced by whatever the framework's JSON happened to report. Presence is a fact about
    /// the payload and cannot be recovered from the value.
    pub(super) fn extract_opt_for_span(
        &self,
        attrs: &HashMap<String, String>,
        span_name: &str,
    ) -> Option<i64> {
        let scoped = if is_claude_code_span(span_name) {
            self.scoped_fallbacks
        } else {
            &[][..]
        };
        attrs
            .get(self.primary)
            .or_else(|| self.fallbacks.iter().find_map(|k| attrs.get(*k)))
            .or_else(|| scoped.iter().find_map(|k| attrs.get(*k)))
            .and_then(|v| v.parse().ok())
    }
}

/// Spans emitted by the Claude Code CLI, which names token attributes outside the
/// `gen_ai.usage.*` conventions.
///
/// Retired with the table it scoped: the same gate is `when: {span_name: ["claude_code."]}` on the source that
/// needs it.
#[cfg(test)]
fn is_claude_code_span(span_name: &str) -> bool {
    span_name.starts_with("claude_code.")
}

#[cfg(test)]
pub(super) const INPUT_TOKENS: TokenConfig = TokenConfig::with_scoped(
    "gen_ai.usage.input_tokens",
    &[
        "gen_ai.usage.prompt_tokens",
        "llm.usage.prompt_tokens",
        "llm.token_count.prompt",
        "ai.usage.promptTokens",
    ],
    &["input_tokens"], // Claude Code CLI (Claude Agent SDK)
);

#[cfg(test)]
const OUTPUT_TOKENS: TokenConfig = TokenConfig::with_scoped(
    "gen_ai.usage.output_tokens",
    &[
        "gen_ai.usage.completion_tokens",
        "llm.usage.completion_tokens",
        "llm.token_count.completion",
        "ai.usage.completionTokens",
    ],
    &["output_tokens"], // Claude Code CLI (Claude Agent SDK)
);

#[cfg(test)]
const TOTAL_TOKENS: TokenConfig =
    TokenConfig::new("gen_ai.usage.total_tokens", &["llm.token_count.total"]);

#[cfg(test)]
const CACHE_READ_TOKENS: TokenConfig = TokenConfig::with_scoped(
    "gen_ai.usage.cache_read_input_tokens",
    &[
        "gen_ai.usage.cache_read_tokens",
        // Dotted semconv spelling, used by Pipecat among others.
        "gen_ai.usage.cache_read.input_tokens",
        "llm.usage.cache_read_input_tokens",
        "ai.usage.cachedInputTokens",
    ],
    &["cache_read_tokens"], // Claude Code CLI (Claude Agent SDK)
);

#[cfg(test)]
const CACHE_WRITE_TOKENS: TokenConfig = TokenConfig::with_scoped(
    "gen_ai.usage.cache_creation_input_tokens",
    &[
        "gen_ai.usage.cache_write_input_tokens", // Strands
        "gen_ai.usage.cache_write_tokens",
        "gen_ai.usage.cache_creation.input_tokens",
        "llm.usage.cache_creation_input_tokens",
    ],
    &["cache_creation_tokens"], // Claude Code CLI (Claude Agent SDK)
);

#[cfg(test)]
const REASONING_TOKENS: TokenConfig = TokenConfig::new(
    "gen_ai.usage.output_reasoning_tokens",
    &[
        "gen_ai.usage.thoughts_token_count",
        // Dotted and bare semconv spellings.
        "gen_ai.usage.reasoning.output_tokens",
        "gen_ai.usage.reasoning_tokens",
        "ai.usage.reasoningTokens",
    ],
);

/// The `gen_ai.usage.*` members a declared counter already reads, so the leftovers are what is left.
///
/// **Derived** from the assets rather than listed beside them: a hand-maintained mirror of a declaration is
/// the hole this engine exists to close, and adding a spelling to a counter would otherwise silently change
/// what the details object contains - the same value counted twice, once as a counter and once as a detail.
pub(super) fn counters_already_read() -> std::collections::BTreeSet<&'static str> {
    use sideseat_domain::rules::schema::FieldTarget::*;
    sideseat_domain::rules::ruleset()
        .span_fields
        .attributes_read(&[
            UsageInputTokens,
            UsageOutputTokens,
            UsageTotalTokensReported,
            UsageCacheReadTokens,
            UsageCacheWriteTokens,
            UsageReasoningTokens,
        ])
        .into_iter()
        .filter_map(|key| key.strip_prefix("gen_ai.usage."))
        .collect()
}
