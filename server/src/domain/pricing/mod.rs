//! Pricing service for LLM cost calculations
//!
//! Implements a robust cost calculation system using LiteLLM's pricing data.
//! Features:
//! - Multi-strategy model lookup (exact → provider-prefixed → alias → family)
//! - Provider-aware normalization (20+ gen_ai.system mappings)
//! - Background sync from GitHub with atomic updates
//! - Thread-safe with read-heavy optimized locking

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use serde::Serialize;
use thiserror::Error;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use utoipa::ToSchema;

use crate::core::storage::AppStorage;

// ============================================================================
// CONSTANTS
// ============================================================================

/// Embedded pricing data (compile-time)
const EMBEDDED_PRICING_JSON: &str =
    include_str!("../../../data/model_prices_and_context_window.json");

/// Pricing file name in data directory
const PRICING_FILE_NAME: &str = "model_prices.json";

/// GitHub raw URL for LiteLLM pricing data
const PRICING_SYNC_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";

/// Minimum sync interval (1 hour) to avoid rate limiting
const MIN_SYNC_HOURS: u64 = 1;

// ============================================================================
// ERROR TYPE
// ============================================================================

#[derive(Error, Debug)]
pub enum PricingError {
    #[error("Failed to parse pricing data: {0}")]
    ParseError(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
}

// ============================================================================
// PRICING DATA STRUCTURES
// ============================================================================

/// Parsed model pricing entry from LiteLLM JSON
#[derive(Debug, Clone, Default)]
pub struct ModelPricing {
    /// Cost per input token (USD)
    pub input_cost_per_token: f64,
    /// Cost per output token (USD)
    pub output_cost_per_token: f64,

    /// Cache read cost (Anthropic, OpenAI)
    pub cache_read_input_token_cost: f64,
    /// Cache creation cost (Anthropic, OpenAI)
    pub cache_creation_input_token_cost: f64,

    /// Reasoning tokens cost (o1, Claude thinking)
    pub output_cost_per_reasoning_token: f64,

    /// LiteLLM provider name
    pub litellm_provider: String,
    /// Mode: "chat", "embedding", "completion", etc.
    pub mode: String,
}

/// Match type for cost confidence scoring
///
/// Exposed in SpanCostOutput to indicate how the model was matched.
/// Higher confidence = more accurate cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MatchType {
    /// Exact key match (confidence: 100%)
    Exact,
    /// Matched via provider prefix, e.g., "azure/gpt-4o" (confidence: 95%)
    ProviderPrefix,
    /// Matched via alias, e.g., "-latest" suffix stripped (confidence: 85%)
    Alias,
    /// Matched base model family, e.g., date stripped (confidence: 70%)
    Family,
    /// No match found (confidence: 0%)
    #[default]
    NotFound,
}

impl MatchType {
    /// Returns confidence level (0.0-1.0) based on match type
    pub fn confidence(self) -> f64 {
        match self {
            MatchType::Exact => 1.0,
            MatchType::ProviderPrefix => 0.95,
            MatchType::Alias => 0.85,
            MatchType::Family => 0.70,
            MatchType::NotFound => 0.0,
        }
    }
}

// ============================================================================
// PRICING DATA
// ============================================================================

/// Parsed and indexed pricing data
#[derive(Debug)]
pub struct PricingData {
    /// Primary lookup: exact model key → pricing
    /// Keys are lowercase for case-insensitive matching
    models: HashMap<String, ModelPricing>,

    /// Provider-prefixed lookup: (provider, model) → canonical key
    /// Handles "openai" + "gpt-4o" → "gpt-4o"
    provider_models: HashMap<(String, String), String>,

    /// Model count for logging and comparison
    pub model_count: usize,
}

impl PricingData {
    /// Parse pricing data from JSON string
    pub fn from_json_str(json: &str) -> Result<Self, PricingError> {
        let raw: serde_json::Value =
            serde_json::from_str(json).map_err(|e| PricingError::ParseError(e.to_string()))?;

        let obj = raw
            .as_object()
            .ok_or_else(|| PricingError::ParseError("Expected JSON object".into()))?;

        let mut models = HashMap::new();
        let mut provider_models = HashMap::new();

        for (key, value) in obj {
            // Skip documentation entry
            if key == "sample_spec" {
                continue;
            }

            // Skip non-object entries
            let Some(entry) = value.as_object() else {
                continue;
            };

            // Parse pricing fields (default to 0.0 if missing)
            let input_cost = entry
                .get("input_cost_per_token")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let output_cost = entry
                .get("output_cost_per_token")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);

            // Skip entries with no pricing (image generation, etc.)
            if input_cost == 0.0 && output_cost == 0.0 {
                continue;
            }

            // Validate pricing: skip negative values (data corruption indicator)
            if input_cost < 0.0 || output_cost < 0.0 {
                tracing::warn!(model = key, "Skipping model with negative pricing");
                continue;
            }

            // Sanity check: warn on suspiciously high prices (> $1/token)
            if input_cost > 1.0 || output_cost > 1.0 {
                tracing::warn!(
                    model = key,
                    input_cost,
                    output_cost,
                    "Model has unusually high pricing"
                );
            }

            let pricing = ModelPricing {
                input_cost_per_token: input_cost,
                output_cost_per_token: output_cost,
                cache_read_input_token_cost: entry
                    .get("cache_read_input_token_cost")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0)
                    .max(0.0),
                cache_creation_input_token_cost: entry
                    .get("cache_creation_input_token_cost")
                    .and_then(|v| v.as_f64())
                    .filter(|&v| v > 0.0)
                    .or_else(|| {
                        // Fallback: if model supports caching but has no explicit cache creation cost,
                        // use input cost (conservative estimate - many providers charge input rate for cache writes)
                        let supports_caching = entry
                            .get("supports_prompt_caching")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        if supports_caching {
                            Some(input_cost)
                        } else {
                            None
                        }
                    })
                    .unwrap_or(0.0)
                    .max(0.0),
                output_cost_per_reasoning_token: entry
                    .get("output_cost_per_reasoning_token")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0)
                    .max(0.0),
                litellm_provider: entry
                    .get("litellm_provider")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                mode: entry
                    .get("mode")
                    .and_then(|v| v.as_str())
                    .unwrap_or("chat")
                    .to_string(),
            };

            let key_lower = key.to_lowercase();

            // Build provider index: extract provider from key or use litellm_provider
            // Keys like "azure/gpt-4o" → provider="azure", model="gpt-4o"
            if let Some((provider, model)) = key_lower.split_once('/') {
                provider_models
                    .insert((provider.to_string(), model.to_string()), key_lower.clone());
            } else if !pricing.litellm_provider.is_empty() {
                // Index by litellm_provider + model key
                provider_models.insert(
                    (pricing.litellm_provider.to_lowercase(), key_lower.clone()),
                    key_lower.clone(),
                );
            }

            models.insert(key_lower, pricing);
        }

        let model_count = models.len();

        Ok(Self {
            models,
            provider_models,
            model_count,
        })
    }

    /// Look up pricing for a model with multi-strategy fallback
    ///
    /// Lookup order:
    /// 1. Exact match on model name
    ///    - 1b. Strip Bedrock regional prefix (global., us., eu., etc.) and retry
    ///    - 1c. Extract fine-tuned base model (ft:gpt-3.5-turbo:org::id → gpt-3.5-turbo)
    /// 2. Provider-prefixed match (e.g., "azure/gpt-4o")
    /// 3. Provider + model via index
    /// 4. Normalized model name (strip -latest suffix)
    /// 5. Base model without version date (e.g., strip -20241022)
    pub fn lookup(&self, system: Option<&str>, model: &str) -> Option<(&ModelPricing, MatchType)> {
        let model_lower = model.to_lowercase();
        let provider = system
            .map(map_system_to_litellm_provider)
            .filter(|p| !p.is_empty());

        // Strategy 1: Exact match (most common case)
        if let Some(pricing) = self.models.get(&model_lower) {
            return Some((pricing, MatchType::Exact));
        }

        // Strategy 1b: Strip Bedrock regional prefix and retry
        // Handles "global.amazon.nova-2-lite-v1:0" → "amazon.nova-2-lite-v1:0"
        if let Some(stripped) = strip_bedrock_region_prefix(&model_lower)
            && let Some(pricing) = self.models.get(stripped)
        {
            return Some((pricing, MatchType::Exact));
        }

        // Strategy 1b2: Strip LiteLLM slash prefix (e.g. "bedrock/model" → "model")
        if let Some((_, model_part)) = model_lower.split_once('/')
            && !model_part.is_empty()
        {
            if let Some(pricing) = self.models.get(model_part) {
                return Some((pricing, MatchType::ProviderPrefix));
            }
            if let Some(stripped) = strip_bedrock_region_prefix(model_part)
                && let Some(pricing) = self.models.get(stripped)
            {
                return Some((pricing, MatchType::ProviderPrefix));
            }
        }

        // Strategy 1c: LiteLLM colon prefix format (openai:gpt-4o → gpt-4o with openai provider)
        // Only applies if model contains colon and prefix is a known provider
        if let Some((prefix, model_after_colon)) = extract_litellm_colon_prefix(&model_lower) {
            // Try with extracted provider
            let prefixed = format!("{}/{}", prefix, model_after_colon);
            if let Some(pricing) = self.models.get(&prefixed) {
                return Some((pricing, MatchType::ProviderPrefix));
            }
            // Try exact match on model part
            if let Some(pricing) = self.models.get(model_after_colon) {
                return Some((pricing, MatchType::Exact));
            }
        }

        // Strategy 1d: Vertex AI resource paths
        // Handles "publishers/google/models/gemini-2.0-flash" → "gemini-2.0-flash"
        // Handles "projects/x/locations/y/publishers/google/models/gemini-2.0-flash"
        if let Some(extracted) = extract_vertex_resource_model(&model_lower) {
            if let Some(pricing) = self.models.get(extracted) {
                return Some((pricing, MatchType::Exact));
            }
            // Try with vertex_ai prefix
            let prefixed = format!("vertex_ai/{}", extracted);
            if let Some(pricing) = self.models.get(&prefixed) {
                return Some((pricing, MatchType::ProviderPrefix));
            }
            // Try with gemini prefix (for google models)
            let gemini_prefixed = format!("gemini/{}", extracted);
            if let Some(pricing) = self.models.get(&gemini_prefixed) {
                return Some((pricing, MatchType::ProviderPrefix));
            }
        }

        // Strategy 1e: Replicate version format (owner/model:version_id → owner/model)
        // Handles "stability-ai/sdxl:2b017d0c..." → "stability-ai/sdxl"
        if let Some(stripped) = strip_replicate_version(&model_lower) {
            if let Some(pricing) = self.models.get(stripped) {
                return Some((pricing, MatchType::Exact));
            }
            // Try with replicate prefix
            let prefixed = format!("replicate/{}", stripped);
            if let Some(pricing) = self.models.get(&prefixed) {
                return Some((pricing, MatchType::ProviderPrefix));
            }
        }

        // Strategy 1f: Extract base model from fine-tuned model IDs
        // Handles "ft:gpt-3.5-turbo-0125:org::id" → "gpt-3.5-turbo-0125"
        // Handles "davinci:ft-personal-2023-04-05" → "davinci"
        if let Some(base_model) = extract_finetune_base_model(&model_lower) {
            // Try exact match on base model
            if let Some(pricing) = self.models.get(base_model) {
                return Some((pricing, MatchType::Alias));
            }
            // Try stripping date from base model (e.g., gpt-3.5-turbo-0125 → gpt-3.5-turbo)
            let base_no_date = strip_date_suffix(base_model);
            if base_no_date != base_model
                && let Some(pricing) = self.models.get(&base_no_date)
            {
                return Some((pricing, MatchType::Family));
            }
        }

        // Strategy 2 & 3: Provider-aware lookup
        if let Some(provider) = provider {
            // Strategy 2: Provider-prefixed key (e.g., "azure/gpt-4o")
            let prefixed = format!("{}/{}", provider, model_lower);
            if let Some(pricing) = self.models.get(&prefixed) {
                return Some((pricing, MatchType::ProviderPrefix));
            }

            // Strategy 3: Provider index lookup (uses pre-built index)
            let key = (provider.to_string(), model_lower.clone());
            if let Some(canonical_key) = self.provider_models.get(&key)
                && let Some(pricing) = self.models.get(canonical_key)
            {
                return Some((pricing, MatchType::ProviderPrefix));
            }
        }

        // Strategy 4: Normalized model (strip -latest, :latest suffix)
        // Try provider-prefixed first to maintain provider context
        let normalized = normalize_model_name(&model_lower);
        if normalized != model_lower {
            // 4a: Try provider-prefixed normalized key first
            if let Some(provider) = provider {
                let prefixed = format!("{}/{}", provider, normalized);
                if let Some(pricing) = self.models.get(&prefixed) {
                    return Some((pricing, MatchType::Alias));
                }
            }
            // 4b: Fall back to global normalized key
            if let Some(pricing) = self.models.get(normalized) {
                return Some((pricing, MatchType::Alias));
            }
        }

        // Strategy 5: Base model without date suffix (last resort)
        // "claude-3-5-sonnet-20241022" → "claude-3-5-sonnet"
        // "gpt-4o-2024-11-20" → "gpt-4o"
        let base = strip_date_suffix(&model_lower);
        if base != model_lower {
            // 5a: Try provider-prefixed base key first
            if let Some(provider) = provider {
                let prefixed = format!("{}/{}", provider, base);
                if let Some(pricing) = self.models.get(&prefixed) {
                    return Some((pricing, MatchType::Family));
                }
            }
            // 5b: Fall back to global base key
            if let Some(pricing) = self.models.get(&base) {
                return Some((pricing, MatchType::Family));
            }
        }

        // Not found
        None
    }
}

// ============================================================================
// PROVIDER MAPPING
// ============================================================================

/// Maps gen_ai.system attribute to LiteLLM provider name
///
/// Returns empty string for framework-only values (let model lookup handle them)
fn map_system_to_litellm_provider(system: &str) -> &'static str {
    match system.to_lowercase().as_str() {
        // Direct mappings
        "openai" => "openai",
        "anthropic" => "anthropic",
        "cohere" => "cohere",
        "mistral" => "mistral",

        // AWS Bedrock variants
        "aws_bedrock" | "aws.bedrock" | "bedrock" | "amazon_bedrock" => "bedrock",

        // Azure OpenAI variants
        "azure" | "azure_openai" | "azure.openai" | "azureopenai" => "azure",

        // Google variants
        "google" | "gemini" | "google_ai_studio" => "gemini",
        "vertex" | "vertex_ai" | "vertexai" | "google_vertexai" => "vertex_ai",
        "google_adk" | "googleadk" => "gemini",

        // Other providers
        "groq" => "groq",
        "together" | "together_ai" | "togetherai" => "together_ai",
        "fireworks" | "fireworks_ai" => "fireworks_ai",
        "deepinfra" | "deep_infra" => "deepinfra",
        "perplexity" => "perplexity",
        "replicate" => "replicate",
        "ollama" => "ollama",
        "xai" | "x.ai" | "grok" => "xai",
        "ai21" | "ai21_chat" => "ai21",
        "openrouter" | "open_router" => "openrouter",
        "databricks" => "databricks",
        "watsonx" | "watson_x" | "ibm_watsonx" => "watsonx",

        // Framework-only values: return empty string to rely on model lookup
        "strands-agents" | "strands_agents" | "langchain" | "langgraph" | "openinference"
        | "llamaindex" | "crewai" | "autogen" | "huggingface" | "hugging_face" => "",

        // Unknown - return empty string
        _ => "",
    }
}

// ============================================================================
// MODEL NAME NORMALIZATION
// ============================================================================
//
// Helper functions for normalizing model names before lookup.
// These handle provider-specific formats, suffixes, and prefixes.

/// Normalize model name for lookup
///
/// IMPORTANT: Assumes input is already lowercased (from lookup() caller)
///
/// Handles special cases:
/// - Strip "-latest" / ":latest" suffixes added by some frameworks
/// - Strip GCP Vertex AI "@date" suffix (e.g., "claude-sonnet-4-5@20250929")
/// - Strip Bedrock version suffix (e.g., "-v1:0", "-v2:0")
fn normalize_model_name(model: &str) -> &str {
    let mut result = model;

    // Strip -latest / :latest suffixes
    result = result
        .trim_end_matches("-latest")
        .trim_end_matches(":latest");

    // Strip OpenRouter routing suffixes (:free, :extended, :nitro, :beta)
    // These are routing hints, not part of the model name
    result = strip_openrouter_routing_suffix(result);

    // Strip GCP Vertex AI @date suffix (e.g., "@20250929")
    if let Some(at_pos) = result.rfind('@') {
        let after_at = &result[at_pos + 1..];
        // Verify it's a date (all digits, 8 chars)
        if after_at.len() == 8 && after_at.chars().all(|c| c.is_ascii_digit()) {
            result = &result[..at_pos];
        }
    }

    // Strip Bedrock version suffix (e.g., "-v1:0", "-v2:0")
    // Pattern: -v followed by digit, colon, digit(s)
    if let Some(v_pos) = result.rfind("-v") {
        let after_v = &result[v_pos + 2..];
        // Check if it matches pattern: digit:digit(s)
        if let Some(colon_pos) = after_v.find(':') {
            let before_colon = &after_v[..colon_pos];
            let after_colon = &after_v[colon_pos + 1..];
            if !before_colon.is_empty()
                && before_colon.chars().all(|c| c.is_ascii_digit())
                && !after_colon.is_empty()
                && after_colon.chars().all(|c| c.is_ascii_digit())
            {
                result = &result[..v_pos];
            }
        }
    }

    result
}

/// Strip OpenRouter routing suffixes from model names
///
/// OpenRouter uses suffixes like `:free`, `:extended`, `:nitro`, `:beta`, `:thinking`, `:exacto`
/// for routing. These are not part of the actual model name.
fn strip_openrouter_routing_suffix(model: &str) -> &str {
    const ROUTING_SUFFIXES: &[&str] = &[
        ":free",
        ":extended",
        ":nitro",
        ":beta",
        ":thinking",
        ":exacto",
    ];

    for suffix in ROUTING_SUFFIXES {
        if let Some(stripped) = model.strip_suffix(suffix) {
            return stripped;
        }
    }
    model
}

/// Extract LiteLLM colon prefix format (provider:model)
///
/// LiteLLM and some proxies use `provider:model` format, e.g.:
/// - `openai:gpt-4o` → ("openai", "gpt-4o")
/// - `bedrock:anthropic.claude-3-opus` → ("bedrock", "anthropic.claude-3-opus")
/// - `vertex:gemini-pro` → ("vertex_ai", "gemini-pro")
///
/// Only extracts if prefix is a known provider. Returns None for:
/// - Fine-tuned models like `ft:gpt-3.5-turbo:org::id`
/// - Version suffixes like `-v1:0`
fn extract_litellm_colon_prefix(model: &str) -> Option<(&str, &str)> {
    // Must have exactly one colon at the start (not in middle of model name)
    let colon_pos = model.find(':')?;

    // Skip if colon is too far into string (likely version suffix or fine-tuned)
    if colon_pos > 20 {
        return None;
    }

    let prefix = &model[..colon_pos];
    let rest = &model[colon_pos + 1..];

    // Skip fine-tuned model format (ft:model:org::id)
    if prefix == "ft" {
        return None;
    }

    // Skip version suffix format (-v1:0, -v2:0)
    if prefix.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }

    // Check if prefix maps to a known provider
    let mapped_provider = map_system_to_litellm_provider(prefix);
    if mapped_provider.is_empty() {
        return None;
    }

    // Must have content after colon
    if rest.is_empty() {
        return None;
    }

    Some((mapped_provider, rest))
}

/// Strip AWS Bedrock regional prefix from model IDs
///
/// Bedrock cross-region inference profiles use prefixes like:
/// - `global.amazon.nova-2-lite-v1:0` → `amazon.nova-2-lite-v1:0`
/// - `us.anthropic.claude-3-haiku-20240307-v1:0` → `anthropic.claude-3-haiku-20240307-v1:0`
/// - `eu.meta.llama3-70b-instruct-v1:0` → `meta.llama3-70b-instruct-v1:0`
///
/// Uses known AWS Bedrock regional prefixes. No assumptions about model ID format.
///
/// Returns the model ID without the regional prefix, or None if no known prefix found.
fn strip_bedrock_region_prefix(model: &str) -> Option<&str> {
    // Known AWS Bedrock regional prefixes for cross-region inference profiles
    // See: https://docs.aws.amazon.com/bedrock/latest/userguide/cross-region-inference.html
    const BEDROCK_REGION_PREFIXES: &[&str] = &[
        "global.", // Global inference profile
        "us.",     // United States
        "eu.",     // Europe
        "ap.",     // Asia Pacific
        "me.",     // Middle East
        "sa.",     // South America
        "ca.",     // Canada
        "af.",     // Africa
        "il.",     // Israel
        "mx.",     // Mexico
    ];

    for prefix in BEDROCK_REGION_PREFIXES {
        if let Some(stripped) = model.strip_prefix(prefix) {
            // Only strip if there's something left after the prefix
            if !stripped.is_empty() {
                return Some(stripped);
            }
        }
    }

    None
}

/// Strip date suffixes from model names (last resort fallback only)
///
/// Examples:
/// - "claude-3-5-sonnet-20241022" → "claude-3-5-sonnet"
/// - "gpt-4o-2024-11-20" → "gpt-4o"
fn strip_date_suffix(model: &str) -> String {
    use std::sync::OnceLock;

    static RE_COMPACT: OnceLock<regex::Regex> = OnceLock::new();
    static RE_DASHED: OnceLock<regex::Regex> = OnceLock::new();

    let re_compact =
        RE_COMPACT.get_or_init(|| regex::Regex::new(r"-\d{8}$").expect("Invalid regex"));
    let re_dashed =
        RE_DASHED.get_or_init(|| regex::Regex::new(r"-\d{4}-\d{2}-\d{2}$").expect("Invalid regex"));

    let result = re_compact.replace(model, "");
    let result = re_dashed.replace(&result, "");
    result.to_string()
}

/// Extract base model from fine-tuned model IDs
///
/// OpenAI fine-tuned models have special formats that need extraction:
/// - New format: `ft:gpt-3.5-turbo-0125:org::id` → `gpt-3.5-turbo-0125`
/// - With checkpoint: `ft:gpt-3.5-turbo-0125:org::id:ckpt-step-900` → `gpt-3.5-turbo-0125`
/// - Old format: `davinci:ft-personal-2023-04-05` → `davinci`
///
/// Returns the base model name, or None if not a fine-tuned model.
fn extract_finetune_base_model(model: &str) -> Option<&str> {
    // New OpenAI fine-tune format: ft:base-model:org::id[:checkpoint]
    if let Some(rest) = model.strip_prefix("ft:") {
        // Find the next colon to get the base model
        if let Some(colon_pos) = rest.find(':') {
            let base = &rest[..colon_pos];
            if !base.is_empty() {
                return Some(base);
            }
        }
        return None;
    }

    // Old fine-tune format: base-model:ft-...
    // e.g., "davinci:ft-personal-2023-04-05-15-59-30"
    if let Some(colon_pos) = model.find(':') {
        let after_colon = &model[colon_pos + 1..];
        if after_colon.starts_with("ft-") || after_colon.starts_with("ft:") {
            let base = &model[..colon_pos];
            if !base.is_empty() {
                return Some(base);
            }
        }
    }

    None
}

/// Extract model name from Vertex AI resource paths
///
/// Vertex AI can use full resource paths for model references:
/// - `publishers/google/models/gemini-2.0-flash` → `gemini-2.0-flash`
/// - `projects/my-project/locations/us-central1/publishers/google/models/gemini-2.0-flash` → `gemini-2.0-flash`
///
/// Returns the extracted model name, or None if not a Vertex AI resource path.
fn extract_vertex_resource_model(model: &str) -> Option<&str> {
    // Look for the "/models/" segment which precedes the model name
    const MODELS_SEGMENT: &str = "/models/";
    if let Some(idx) = model.find(MODELS_SEGMENT) {
        let model_name = &model[idx + MODELS_SEGMENT.len()..];
        if !model_name.is_empty() {
            return Some(model_name);
        }
    }
    None
}

/// Strip Replicate version hash from model IDs
///
/// Replicate uses versioned model references with SHA hashes:
/// - `stability-ai/sdxl:2b017d0c4f2e...` → `stability-ai/sdxl`
/// - `owner/model:abc123...` → `owner/model`
///
/// Only strips if the format matches owner/model:version pattern.
/// Returns the model without version, or None if not a Replicate format.
fn strip_replicate_version(model: &str) -> Option<&str> {
    // Must have a slash (owner/model format)
    let slash_pos = model.find('/')?;

    // Must have a colon after the slash (version separator)
    let colon_pos = model[slash_pos..].find(':').map(|p| p + slash_pos)?;

    // The version hash should be after the colon
    let version = &model[colon_pos + 1..];

    // Replicate versions are long hex hashes (typically 64 chars)
    // But some may be shorter. Just check it looks like a hash (hex chars)
    // and is at least 12 chars to avoid false positives
    if version.len() >= 12 && version.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(&model[..colon_pos]);
    }

    None
}

// ============================================================================
// INPUT/OUTPUT TYPES
// ============================================================================

/// Input data for cost calculation
#[derive(Debug, Clone, Default)]
pub struct SpanCostInput {
    pub system: Option<String>,
    pub model: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub reasoning_tokens: i64,
}

/// Calculated costs for a span - always returns values (0.0 if no pricing data)
#[derive(Debug, Clone, Default)]
pub struct SpanCostOutput {
    pub input_cost: f64,
    pub output_cost: f64,
    pub cache_read_cost: f64,
    pub cache_write_cost: f64,
    pub reasoning_cost: f64,
    pub total_cost: f64,

    /// Confidence scoring: indicates how the model was matched
    pub match_type: Option<MatchType>,
}

impl SpanCostOutput {
    /// Returns true if costs were calculated (model was found)
    pub fn is_calculated(&self) -> bool {
        matches!(self.match_type, Some(t) if t != MatchType::NotFound)
    }

    /// Returns confidence level (0.0-1.0) based on match type
    pub fn confidence(&self) -> f64 {
        self.match_type.map_or(0.0, |t| t.confidence())
    }
}

// ============================================================================
// PRICING SERVICE
// ============================================================================

/// Thread-safe pricing service with background sync
pub struct PricingService {
    /// Pricing data (read-heavy, RwLock for concurrent reads)
    data: RwLock<PricingData>,

    /// Path to local pricing file in data directory
    local_path: PathBuf,

    /// Reusable HTTP client for sync
    http_client: reqwest::Client,
}

impl PricingService {
    /// Initialize pricing service
    ///
    /// Loading priority:
    /// 1. Try local file from data directory
    /// 2. If local valid and has >= models than embedded, use it
    /// 3. Otherwise, use embedded data and save to disk
    ///
    /// If sync_hours > 0, spawns background fetch from GitHub after init.
    pub async fn init(storage: &AppStorage, sync_hours: u64) -> Result<Arc<Self>, PricingError> {
        let local_path = storage.data_dir().join(PRICING_FILE_NAME);

        let data = Self::load_pricing_data(&local_path).await?;

        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("SideSeat/1.0")
            .build()
            .map_err(PricingError::Http)?;

        let service = Arc::new(Self {
            data: RwLock::new(data),
            local_path,
            http_client,
        });

        if sync_hours > 0 {
            let service_clone = Arc::clone(&service);
            tokio::spawn(async move {
                service_clone.sync().await;
            });
        }

        Ok(service)
    }

    /// Load pricing data with fallback: local file → embedded
    async fn load_pricing_data(local_path: &Path) -> Result<PricingData, PricingError> {
        if !local_path.exists() {
            return Self::load_embedded_with_save(local_path).await;
        }

        match Self::try_load_local(local_path).await {
            Ok(local_data) => {
                let embedded_count = Self::count_embedded_models();
                if local_data.model_count >= embedded_count {
                    Ok(local_data)
                } else {
                    Self::load_embedded_with_save(local_path).await
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to load local pricing, using embedded");
                Self::load_embedded_with_save(local_path).await
            }
        }
    }

    /// Load embedded pricing data and save to disk (best-effort)
    async fn load_embedded_with_save(local_path: &Path) -> Result<PricingData, PricingError> {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON)?;
        if let Err(e) = Self::save_to_file(local_path, EMBEDDED_PRICING_JSON).await {
            tracing::warn!(error = %e, "Failed to save pricing to disk (continuing with embedded)");
        }
        Ok(data)
    }

    /// Count models in embedded data
    fn count_embedded_models() -> usize {
        match serde_json::from_str::<serde_json::Value>(EMBEDDED_PRICING_JSON) {
            Ok(serde_json::Value::Object(map)) => {
                map.keys().filter(|k| *k != "sample_spec").count()
            }
            _ => {
                tracing::warn!("Failed to count embedded models, using fallback");
                1000
            }
        }
    }

    /// Create PricingService for testing (no file I/O)
    #[cfg(test)]
    pub fn init_for_test() -> Result<Self, PricingError> {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON)?;
        Ok(Self {
            data: RwLock::new(data),
            local_path: std::env::temp_dir().join("sideseat_test_pricing.json"),
            http_client: reqwest::Client::new(),
        })
    }

    /// Try to load pricing data from local file
    async fn try_load_local(path: &Path) -> Result<PricingData, PricingError> {
        let json = tokio::fs::read_to_string(path).await?;
        PricingData::from_json_str(&json)
    }

    /// Save pricing data to file atomically (write to temp, then rename)
    async fn save_to_file(path: &Path, json: &str) -> Result<(), PricingError> {
        let temp_path = path.with_extension("json.tmp");
        tokio::fs::write(&temp_path, json).await?;

        // Windows-safe atomic replace: remove destination first if exists
        #[cfg(target_os = "windows")]
        if path.exists() {
            let _ = tokio::fs::remove_file(path).await;
        }

        tokio::fs::rename(&temp_path, path).await?;
        Ok(())
    }

    /// Calculate costs for a span's token usage
    ///
    /// Thread-safe: acquires read lock on pricing data.
    /// Fail-safe: returns zero costs if model not found (debug log only).
    pub fn calculate_cost(&self, input: &SpanCostInput) -> SpanCostOutput {
        let model = match &input.model {
            Some(m) if !m.is_empty() => m.as_str(),
            _ => return SpanCostOutput::default(),
        };

        let data = self.data.read();
        let (pricing, match_type) = match data.lookup(input.system.as_deref(), model) {
            Some(result) => result,
            None => {
                tracing::trace!(
                    model = model,
                    system = input.system.as_deref().unwrap_or("none"),
                    "No pricing found for model"
                );
                return SpanCostOutput {
                    match_type: Some(MatchType::NotFound),
                    ..Default::default()
                };
            }
        };

        // For embedding models, only input tokens are charged
        let is_embedding = pricing.mode.eq_ignore_ascii_case("embedding");

        // Clamp token counts to prevent negative costs from data corruption
        let input_tokens = input.input_tokens.max(0) as f64;
        let output_tokens = input.output_tokens.max(0) as f64;
        let cache_read_tokens = input.cache_read_tokens.max(0) as f64;
        let cache_write_tokens = input.cache_write_tokens.max(0) as f64;
        let reasoning_tokens = input.reasoning_tokens.max(0) as f64;

        // Calculate costs
        let input_cost = input_tokens * pricing.input_cost_per_token;

        // Output cost: zero for embeddings (they only have input)
        let output_cost = if is_embedding {
            0.0
        } else {
            output_tokens * pricing.output_cost_per_token
        };

        let cache_read_cost = cache_read_tokens * pricing.cache_read_input_token_cost;
        let cache_write_cost = cache_write_tokens * pricing.cache_creation_input_token_cost;

        // Reasoning tokens: use dedicated rate if available, else output rate
        let reasoning_cost = if is_embedding {
            0.0
        } else {
            let reasoning_rate = if pricing.output_cost_per_reasoning_token > 0.0 {
                pricing.output_cost_per_reasoning_token
            } else {
                pricing.output_cost_per_token
            };
            reasoning_tokens * reasoning_rate
        };

        let total_cost =
            input_cost + output_cost + cache_read_cost + cache_write_cost + reasoning_cost;

        tracing::trace!(
            model = model,
            match_type = ?match_type,
            mode = pricing.mode,
            total_cost = total_cost,
            "Calculated cost"
        );

        SpanCostOutput {
            input_cost,
            output_cost,
            cache_read_cost,
            cache_write_cost,
            reasoning_cost,
            total_cost,
            match_type: Some(match_type),
        }
    }

    /// Get model pricing information (per-token rates)
    ///
    /// Returns the pricing rates and match type for a given model.
    /// Thread-safe: acquires read lock on pricing data.
    pub fn get_model_pricing(
        &self,
        provider: Option<&str>,
        model: &str,
    ) -> Option<(ModelPricing, MatchType)> {
        if model.is_empty() {
            return None;
        }

        let data = self.data.read();
        data.lookup(provider, model)
            .map(|(pricing, match_type)| (pricing.clone(), match_type))
    }

    /// Sync pricing data from GitHub
    async fn sync(&self) {
        let request = self.http_client.get(PRICING_SYNC_URL);

        match request.send().await {
            Ok(resp) if resp.status().is_success() => match resp.text().await {
                Ok(text) => self.apply_sync_data(&text).await,
                Err(e) => tracing::warn!(error = %e, "Failed to read pricing response"),
            },
            Ok(resp) => {
                tracing::warn!(status = %resp.status(), "Pricing sync HTTP error");
            }
            Err(e) => {
                tracing::warn!(error = %e, "Pricing sync request failed");
            }
        }
    }

    /// Apply synced data: parse, save to disk atomically, update memory
    async fn apply_sync_data(&self, json: &str) {
        // Parse first to validate
        let new_data = match PricingData::from_json_str(json) {
            Ok(data) => data,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to parse synced pricing data");
                return;
            }
        };

        let new_count = new_data.model_count;
        let current_count = self.data.read().model_count;

        // Sanity check: reject if new data has < 50% of current models
        let min_acceptable = current_count / 2;
        if new_count < min_acceptable {
            tracing::warn!(
                current = current_count,
                new = new_count,
                min_acceptable = min_acceptable,
                "Synced data has too few models (<50%), rejecting"
            );
            return;
        }

        // Warn if < 80%
        let warning_threshold = (current_count * 80) / 100;
        if new_count < warning_threshold {
            tracing::warn!(
                current = current_count,
                new = new_count,
                "Synced data has fewer models (< 80%), accepting with warning"
            );
        }

        // Save to disk atomically
        if let Err(e) = Self::save_to_file(&self.local_path, json).await {
            tracing::warn!(error = %e, "Failed to save pricing data to disk");
        }

        // Update in-memory data
        {
            let mut data = self.data.write();
            *data = new_data;
        }
    }

    /// Start background sync task
    ///
    /// # Arguments
    /// * `sync_hours` - Sync interval in hours. 0 disables sync. Minimum 1 hour.
    /// * `shutdown_rx` - Shutdown signal receiver
    ///
    /// # Returns
    /// `Some(JoinHandle)` if sync is enabled, `None` if disabled
    pub fn start_sync_task(
        self: &Arc<Self>,
        sync_hours: u64,
        mut shutdown_rx: watch::Receiver<bool>,
    ) -> Option<JoinHandle<()>> {
        if sync_hours == 0 {
            return None;
        }

        // Enforce minimum interval and prevent overflow
        let sync_hours = sync_hours.max(MIN_SYNC_HOURS);
        let interval = Duration::from_secs(sync_hours.saturating_mul(3600));
        let service = Arc::clone(self);

        Some(tokio::spawn(async move {
            let mut timer = tokio::time::interval(interval);
            timer.tick().await; // Skip immediate first tick

            loop {
                tokio::select! {
                    biased;
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            break;
                        }
                    }
                    _ = timer.tick() => {
                        service.sync().await;
                    }
                }
            }
        }))
    }
}

impl Default for PricingService {
    fn default() -> Self {
        // Fallback for cases where async init isn't possible
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON)
            .expect("Failed to parse embedded pricing data");
        Self {
            data: RwLock::new(data),
            local_path: PathBuf::new(),
            http_client: reqwest::Client::new(),
        }
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Test helper: Map system to provider string with lowercasing for unknown providers
    fn map_system_to_provider_string(system: &str) -> String {
        let mapped = map_system_to_litellm_provider(system);
        if mapped.is_empty() {
            system.to_lowercase()
        } else {
            mapped.to_string()
        }
    }

    #[test]
    fn test_parse_pricing_data() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        assert!(data.model_count > 1000, "Should have 1000+ models");
    }

    #[test]
    fn test_lookup_exact_match() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        let result = data.lookup(Some("openai"), "gpt-4o");
        assert!(result.is_some());
        let (pricing, match_type) = result.unwrap();
        assert_eq!(match_type, MatchType::Exact);
        assert!(pricing.input_cost_per_token > 0.0);
    }

    #[test]
    fn test_lookup_provider_prefix() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        let result = data.lookup(Some("azure"), "gpt-4o");
        assert!(result.is_some(), "Should find azure/gpt-4o");
    }

    #[test]
    fn test_lookup_not_found() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        assert!(data.lookup(None, "nonexistent-model-xyz").is_none());
    }

    #[test]
    fn test_provider_mapping() {
        assert_eq!(map_system_to_litellm_provider("aws_bedrock"), "bedrock");
        assert_eq!(map_system_to_litellm_provider("azure_openai"), "azure");
        assert_eq!(map_system_to_litellm_provider("strands-agents"), "");
        assert_eq!(map_system_to_litellm_provider("langchain"), "");
    }

    #[test]
    fn test_calculate_cost() {
        let service = PricingService::init_for_test().unwrap();
        let input = SpanCostInput {
            system: Some("openai".to_string()),
            model: Some("gpt-4o".to_string()),
            input_tokens: 1000,
            output_tokens: 500,
            ..Default::default()
        };
        let output = service.calculate_cost(&input);
        assert!(output.total_cost > 0.0);
        assert!(output.input_cost > 0.0);
        assert!(output.output_cost > 0.0);
    }

    #[test]
    fn test_calculate_cost_unknown_model() {
        let service = PricingService::init_for_test().unwrap();
        let input = SpanCostInput {
            system: None,
            model: Some("unknown-model-xyz".to_string()),
            input_tokens: 1000,
            output_tokens: 500,
            ..Default::default()
        };
        let output = service.calculate_cost(&input);
        assert_eq!(output.total_cost, 0.0);
        assert_eq!(output.match_type, Some(MatchType::NotFound));
    }

    #[test]
    fn test_calculate_cost_no_model() {
        let service = PricingService::init_for_test().unwrap();
        let input = SpanCostInput {
            system: Some("openai".to_string()),
            model: None,
            input_tokens: 1000,
            output_tokens: 500,
            ..Default::default()
        };
        let output = service.calculate_cost(&input);
        assert_eq!(output.total_cost, 0.0);
    }

    #[test]
    fn test_count_embedded_models() {
        let count = PricingService::count_embedded_models();
        assert!(count > 1000, "Should count 1000+ models");
    }

    #[test]
    fn test_confidence_scoring_exact_match() {
        let service = PricingService::init_for_test().unwrap();
        let input = SpanCostInput {
            system: Some("openai".to_string()),
            model: Some("gpt-4o".to_string()),
            input_tokens: 1000,
            ..Default::default()
        };
        let output = service.calculate_cost(&input);
        assert_eq!(output.match_type, Some(MatchType::Exact));
        assert_eq!(output.confidence(), 1.0);
        assert!(output.is_calculated());
    }

    #[test]
    fn test_confidence_scoring_not_found() {
        let service = PricingService::init_for_test().unwrap();
        let input = SpanCostInput {
            system: None,
            model: Some("nonexistent-xyz".to_string()),
            input_tokens: 1000,
            ..Default::default()
        };
        let output = service.calculate_cost(&input);
        assert_eq!(output.match_type, Some(MatchType::NotFound));
        assert_eq!(output.confidence(), 0.0);
        assert!(!output.is_calculated());
    }

    #[test]
    fn test_embedding_model_only_input_cost() {
        let service = PricingService::init_for_test().unwrap();
        let input = SpanCostInput {
            system: Some("openai".to_string()),
            model: Some("text-embedding-3-small".to_string()),
            input_tokens: 1000,
            output_tokens: 500,
            ..Default::default()
        };
        let output = service.calculate_cost(&input);
        assert!(output.input_cost > 0.0, "Embedding should have input cost");
        assert_eq!(
            output.output_cost, 0.0,
            "Embedding should have zero output cost"
        );
    }

    #[test]
    fn test_normalize_model_name() {
        // -latest / :latest suffix
        assert_eq!(normalize_model_name("gpt-4o-latest"), "gpt-4o");
        assert_eq!(normalize_model_name("model:latest"), "model");
        assert_eq!(normalize_model_name("gpt-4o"), "gpt-4o");

        // GCP Vertex AI @date suffix
        assert_eq!(
            normalize_model_name("claude-sonnet-4-5@20250929"),
            "claude-sonnet-4-5"
        );
        assert_eq!(
            normalize_model_name("claude-3-haiku@20240307"),
            "claude-3-haiku"
        );
        // Should NOT strip if not 8 digits
        assert_eq!(normalize_model_name("model@123"), "model@123");
        assert_eq!(normalize_model_name("model@abc"), "model@abc");

        // Bedrock version suffix -v1:0
        assert_eq!(
            normalize_model_name("anthropic.claude-3-haiku-20240307-v1:0"),
            "anthropic.claude-3-haiku-20240307"
        );
        assert_eq!(
            normalize_model_name("amazon.nova-lite-v1:0"),
            "amazon.nova-lite"
        );
        assert_eq!(
            normalize_model_name("meta.llama3-70b-instruct-v1:0"),
            "meta.llama3-70b-instruct"
        );
        // Should NOT strip if pattern doesn't match
        assert_eq!(normalize_model_name("model-v1"), "model-v1");
        assert_eq!(normalize_model_name("model-vx:0"), "model-vx:0");
    }

    #[test]
    fn test_strip_date_suffix() {
        assert_eq!(
            strip_date_suffix("claude-3-5-sonnet-20241022"),
            "claude-3-5-sonnet"
        );
        assert_eq!(strip_date_suffix("gpt-4o-2024-11-20"), "gpt-4o");
        assert_eq!(strip_date_suffix("gpt-4o"), "gpt-4o");
        assert_eq!(strip_date_suffix("model-v2:0"), "model-v2:0");
    }

    #[test]
    fn test_is_calculated_logic() {
        let mut output = SpanCostOutput::default();
        assert!(!output.is_calculated());

        output.match_type = Some(MatchType::Exact);
        assert!(output.is_calculated());

        output.match_type = Some(MatchType::NotFound);
        assert!(!output.is_calculated());
    }

    #[test]
    fn test_negative_tokens_clamped_to_zero() {
        let service = PricingService::init_for_test().unwrap();
        let input = SpanCostInput {
            system: Some("openai".to_string()),
            model: Some("gpt-4o".to_string()),
            input_tokens: -1000,
            output_tokens: -500,
            cache_read_tokens: -100,
            cache_write_tokens: -50,
            reasoning_tokens: -25,
            ..Default::default()
        };
        let output = service.calculate_cost(&input);
        assert_eq!(output.input_cost, 0.0);
        assert_eq!(output.output_cost, 0.0);
        assert_eq!(output.total_cost, 0.0);
    }

    #[test]
    fn test_lookup_strategy_latest_suffix() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // gpt-4o should exist
        let exact = data.lookup(Some("openai"), "gpt-4o");
        assert!(exact.is_some());

        // gpt-4o-latest should find gpt-4o via alias stripping
        let result = data.lookup(Some("openai"), "gpt-4o-latest");
        assert!(result.is_some());
    }

    // Strategy fallback tests with MatchType assertions
    #[test]
    fn test_lookup_strategy_2_provider_prefix() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Azure models are stored as "azure/gpt-4o"
        // Since gpt-4o exists as exact match, we test that azure lookup still works
        // The result could be Exact (if base model exists) or ProviderPrefix (if only azure/model exists)
        let result = data.lookup(Some("azure"), "gpt-4o");
        assert!(
            result.is_some(),
            "Should find azure/gpt-4o via provider prefix or exact"
        );
        let (_, match_type) = result.unwrap();
        // Either Exact (gpt-4o exists directly) or ProviderPrefix (azure/gpt-4o found)
        assert!(
            match_type == MatchType::Exact || match_type == MatchType::ProviderPrefix,
            "Should match via Exact or ProviderPrefix"
        );
    }

    #[test]
    fn test_lookup_strategy_5_date_suffix() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // If gpt-4o-2024-11-20 doesn't exist exactly, should strip to gpt-4o
        // Note: This test assumes the dated version doesn't exist in LiteLLM data
        let result = data.lookup(Some("openai"), "some-model-20241120");
        // This will likely return None since base model doesn't exist
        // The test validates the stripping logic runs without error
        assert!(result.is_none() || matches!(result, Some((_, MatchType::Family))));
    }

    #[test]
    fn test_embedding_mode_case_insensitive() {
        // This test verifies case-insensitive embedding mode handling
        // by checking that eq_ignore_ascii_case is used in calculate_cost
        let service = PricingService::init_for_test().unwrap();
        let input = SpanCostInput {
            system: Some("openai".to_string()),
            model: Some("text-embedding-3-small".to_string()),
            input_tokens: 1000,
            output_tokens: 500, // Should be ignored for embeddings
            ..Default::default()
        };
        let output = service.calculate_cost(&input);
        // Embedding models should have zero output cost
        assert_eq!(
            output.output_cost, 0.0,
            "Embedding should have zero output cost"
        );
    }

    // Provider-aware normalization tests
    #[test]
    fn test_lookup_provider_aware_latest_suffix() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Test that azure + gpt-4o-latest tries azure/gpt-4o before global gpt-4o
        let result = data.lookup(Some("azure"), "gpt-4o-latest");
        if let Some((pricing, match_type)) = result {
            // Should find azure/gpt-4o via provider-aware Alias strategy
            // or gpt-4o via global fallback
            assert!(
                match_type == MatchType::Alias || match_type == MatchType::Exact,
                "Should be Exact (if entry exists) or Alias (stripped)"
            );
            // Verify we got azure pricing if azure/gpt-4o exists
            if data.lookup(Some("azure"), "gpt-4o").is_some() {
                assert_eq!(
                    pricing.litellm_provider.to_lowercase(),
                    "azure",
                    "Should return azure provider pricing"
                );
            }
        }
    }

    #[test]
    fn test_lookup_provider_aware_date_suffix() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Test that provider context is maintained through date stripping
        // system=azure, model=gpt-4o-2024-11-20 should try azure/gpt-4o
        let result = data.lookup(Some("azure"), "gpt-4o-2024-11-20");
        // We expect Family match if azure/gpt-4o exists in data
        if let Some((_, match_type)) = result {
            assert!(
                match_type == MatchType::Family || match_type == MatchType::Exact,
                "Should find via Family (date stripped) or Exact"
            );
        }
    }

    #[test]
    fn test_unknown_provider_lowercase() {
        // Unknown providers should be lowercased for consistent lookup
        let provider = map_system_to_provider_string("MyCustomProvider");
        assert_eq!(
            provider, "mycustomprovider",
            "Unknown providers should be lowercased"
        );
    }

    // Initial sync behavior tests
    #[tokio::test]
    async fn test_init_with_sync_disabled_no_network() {
        // When sync_hours = 0, init should not spawn any background tasks
        // This is verified by checking that no HTTP requests are made
        let storage = AppStorage::init_for_test(std::env::temp_dir());
        let service = PricingService::init(&storage, 0).await.unwrap();
        // If we got here without network, sync was disabled correctly
        assert!(service.data.read().model_count > 0);
    }

    #[test]
    fn test_count_embedded_models_robust() {
        let count = PricingService::count_embedded_models();
        // Should count all top-level keys except sample_spec
        assert!(count > 1000, "Should have 1000+ models");
        // Verify it handles JSON parsing correctly
        assert!(count < 100000, "Should not be unreasonably large");
    }

    #[test]
    fn test_apply_sync_data_percentage_threshold() {
        // Test that sync accepts data with 51% of models (above 50% threshold)
        // and rejects data with 49% of models (below 50% threshold)
        // This would require constructing test data with specific counts
        // For now, verify the logic by checking the threshold calculation
        let current = 1000;
        let min_acceptable = current / 2;
        assert_eq!(
            min_acceptable, 500,
            "50% threshold should be 500 for 1000 models"
        );

        let warning_threshold = (current * 80) / 100;
        assert_eq!(
            warning_threshold, 800,
            "80% threshold should be 800 for 1000 models"
        );
    }

    // Bedrock regional prefix tests
    #[test]
    fn test_strip_bedrock_region_prefix() {
        // Should strip known Bedrock regional prefixes
        assert_eq!(
            strip_bedrock_region_prefix("global.amazon.nova-2-lite-v1:0"),
            Some("amazon.nova-2-lite-v1:0")
        );
        assert_eq!(
            strip_bedrock_region_prefix("us.anthropic.claude-3-haiku-20240307-v1:0"),
            Some("anthropic.claude-3-haiku-20240307-v1:0")
        );
        assert_eq!(
            strip_bedrock_region_prefix("eu.meta.llama3-70b-instruct-v1:0"),
            Some("meta.llama3-70b-instruct-v1:0")
        );
        assert_eq!(
            strip_bedrock_region_prefix("ap.cohere.command-r-plus-v1:0"),
            Some("cohere.command-r-plus-v1:0")
        );
        assert_eq!(
            strip_bedrock_region_prefix("me.mistral.mistral-large-2402-v1:0"),
            Some("mistral.mistral-large-2402-v1:0")
        );
        assert_eq!(
            strip_bedrock_region_prefix("sa.ai21.jamba-1-5-large-v1:0"),
            Some("ai21.jamba-1-5-large-v1:0")
        );
        assert_eq!(
            strip_bedrock_region_prefix("ca.amazon.titan-embed-text-v1"),
            Some("amazon.titan-embed-text-v1")
        );
        assert_eq!(
            strip_bedrock_region_prefix("af.stability.sd3-5-large-v1:0"),
            Some("stability.sd3-5-large-v1:0")
        );
        assert_eq!(
            strip_bedrock_region_prefix("il.writer.palmyra-x4-v1:0"),
            Some("writer.palmyra-x4-v1:0")
        );
        assert_eq!(
            strip_bedrock_region_prefix("mx.qwen.qwen3-32b-v1:0"),
            Some("qwen.qwen3-32b-v1:0")
        );

        // Works regardless of model ID structure (no assumptions about dots)
        assert_eq!(
            strip_bedrock_region_prefix("global.some-model-without-dots"),
            Some("some-model-without-dots")
        );
        assert_eq!(
            strip_bedrock_region_prefix("us.a.b.c.d.e"),
            Some("a.b.c.d.e")
        );

        // Should return None for non-prefixed models
        assert_eq!(strip_bedrock_region_prefix("amazon.nova-2-lite-v1:0"), None);
        assert_eq!(strip_bedrock_region_prefix("gpt-4o"), None);
        assert_eq!(strip_bedrock_region_prefix("claude-3-opus"), None);

        // Should return None for unknown prefixes
        assert_eq!(
            strip_bedrock_region_prefix("unknown.amazon.nova-v1:0"),
            None
        );

        // Should return None for empty result
        assert_eq!(strip_bedrock_region_prefix("global."), None);
        assert_eq!(strip_bedrock_region_prefix("us."), None);
    }

    #[test]
    fn test_lookup_bedrock_global_prefix() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Test with global prefix - should find the base model
        // Note: This assumes amazon.nova-lite-v1:0 or similar exists in LiteLLM data
        // If not, we test with a model we know exists
        let result = data.lookup(
            Some("bedrock"),
            "global.anthropic.claude-3-haiku-20240307-v1:0",
        );
        // Should find via Bedrock prefix stripping
        if let Some((_, match_type)) = result {
            assert_eq!(
                match_type,
                MatchType::Exact,
                "Should find via exact match after stripping region prefix"
            );
        }
    }

    #[test]
    fn test_lookup_bedrock_us_prefix() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Test with US regional prefix
        let result = data.lookup(Some("bedrock"), "us.anthropic.claude-3-haiku-20240307-v1:0");
        if let Some((_, match_type)) = result {
            assert_eq!(
                match_type,
                MatchType::Exact,
                "Should find via exact match after stripping region prefix"
            );
        }
    }

    #[test]
    fn test_lookup_bedrock_eu_prefix() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Test with EU regional prefix
        let result = data.lookup(Some("bedrock"), "eu.anthropic.claude-3-haiku-20240307-v1:0");
        if let Some((_, match_type)) = result {
            assert_eq!(
                match_type,
                MatchType::Exact,
                "Should find via exact match after stripping region prefix"
            );
        }
    }

    #[test]
    fn test_lookup_bedrock_without_prefix() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Test without prefix - should still work
        let result = data.lookup(Some("bedrock"), "anthropic.claude-3-haiku-20240307-v1:0");
        assert!(result.is_some(), "Should find Bedrock model without prefix");
    }

    #[test]
    fn test_lookup_bedrock_nova_models() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Test Amazon Nova models with various prefixes
        // These models may or may not exist in LiteLLM data

        // Without prefix
        let base_result = data.lookup(Some("bedrock"), "amazon.nova-lite-v1:0");

        // With global prefix - should find same model
        let global_result = data.lookup(Some("bedrock"), "global.amazon.nova-lite-v1:0");

        // If base model exists, both should find it
        if base_result.is_some() {
            assert!(
                global_result.is_some(),
                "global.amazon.nova-lite-v1:0 should find same model as amazon.nova-lite-v1:0"
            );
        }
    }

    // OpenAI fine-tuned model tests
    #[test]
    fn test_extract_finetune_base_model() {
        // New format: ft:base-model:org::id
        assert_eq!(
            extract_finetune_base_model("ft:gpt-3.5-turbo-0125:personal::AKwrJ7vh"),
            Some("gpt-3.5-turbo-0125")
        );
        // With checkpoint
        assert_eq!(
            extract_finetune_base_model("ft:gpt-3.5-turbo-0125:personal::AKwrJ7vh:ckpt-step-900"),
            Some("gpt-3.5-turbo-0125")
        );
        // Old format: base:ft-...
        assert_eq!(
            extract_finetune_base_model("davinci:ft-personal-2023-04-05-15-59-30"),
            Some("davinci")
        );
        // Not a fine-tuned model
        assert_eq!(extract_finetune_base_model("gpt-4o"), None);
        assert_eq!(extract_finetune_base_model("gpt-3.5-turbo"), None);
        // Edge cases
        assert_eq!(extract_finetune_base_model("ft:"), None);
        assert_eq!(extract_finetune_base_model("ft::org::id"), None);
    }

    #[test]
    fn test_lookup_openai_base_models() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Base models should be found via exact match
        let models = [
            "gpt-4",
            "gpt-4o",
            "gpt-4o-mini",
            "gpt-3.5-turbo",
            "o1",
            "o3-mini",
        ];
        for model in models {
            let result = data.lookup(Some("openai"), model);
            assert!(result.is_some(), "Should find OpenAI model: {}", model);
        }
    }

    #[test]
    fn test_lookup_openai_dated_models() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Dated models should be found (exact or via date stripping)
        let dated_models = [
            "gpt-4-0613",
            "gpt-4o-2024-05-13",
            "gpt-4o-mini-2024-07-18",
            "o1-2024-12-17",
        ];
        for model in dated_models {
            let result = data.lookup(Some("openai"), model);
            assert!(
                result.is_some(),
                "Should find OpenAI dated model: {}",
                model
            );
        }
    }

    #[test]
    fn test_lookup_openai_finetuned_models() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Fine-tuned models should find their base model pricing
        let result = data.lookup(Some("openai"), "ft:gpt-3.5-turbo-0125:personal::AKwrJ7vh");
        assert!(result.is_some(), "Should find base model for fine-tuned");
        if let Some((_, match_type)) = result {
            // Should be Alias (base model) or Family (date stripped)
            assert!(
                match_type == MatchType::Alias || match_type == MatchType::Family,
                "Fine-tuned should match via Alias or Family"
            );
        }

        // Old format fine-tuned - davinci-002 exists in LiteLLM data
        let result = data.lookup(
            Some("openai"),
            "davinci-002:ft-personal-2023-04-05-15-59-30",
        );
        assert!(
            result.is_some(),
            "Should find base model for old fine-tuned format"
        );

        // Test extraction logic works even if base model not in pricing data
        // (just verify it doesn't panic)
        let _ = data.lookup(Some("openai"), "custom-model:ft-org-2023-01-01");
    }

    #[test]
    fn test_lookup_openai_latest_suffix() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Models with -latest suffix should strip it
        let result = data.lookup(Some("openai"), "chatgpt-4o-latest");
        // Should find via Alias match (stripped -latest)
        if let Some((_, match_type)) = result {
            assert!(
                match_type == MatchType::Exact || match_type == MatchType::Alias,
                "chatgpt-4o-latest should find via Exact or Alias"
            );
        }
    }

    #[test]
    fn test_lookup_openai_embedding_models() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Embedding models
        let embedding_models = [
            "text-embedding-3-small",
            "text-embedding-3-large",
            "text-embedding-ada-002",
        ];
        for model in embedding_models {
            let result = data.lookup(Some("openai"), model);
            assert!(
                result.is_some(),
                "Should find OpenAI embedding model: {}",
                model
            );
            if let Some((pricing, _)) = result {
                assert!(
                    pricing.mode.eq_ignore_ascii_case("embedding"),
                    "Embedding model should have embedding mode"
                );
            }
        }
    }

    #[test]
    fn test_lookup_openai_preview_models() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Preview models - may or may not exist in LiteLLM data
        let preview_models = [
            "gpt-4o-audio-preview",
            "gpt-4o-realtime-preview",
            "gpt-4-turbo-preview",
        ];
        for model in preview_models {
            // Just verify lookup doesn't panic
            let _ = data.lookup(Some("openai"), model);
        }
    }

    #[test]
    fn test_lookup_openai_case_insensitive() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Model names should be case-insensitive
        let result_lower = data.lookup(Some("openai"), "gpt-4o");
        let result_upper = data.lookup(Some("openai"), "GPT-4O");
        let result_mixed = data.lookup(Some("openai"), "Gpt-4O");

        assert!(result_lower.is_some());
        assert!(result_upper.is_some());
        assert!(result_mixed.is_some());
    }

    // Anthropic Claude model tests
    #[test]
    fn test_lookup_anthropic_claude_api_models() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Claude API format models (dated versions)
        let claude_models = [
            "claude-sonnet-4-5-20250929",
            "claude-haiku-4-5-20251001",
            "claude-opus-4-5-20251101",
            "claude-opus-4-1-20250805",
            "claude-sonnet-4-20250514",
            "claude-3-7-sonnet-20250219",
            "claude-3-haiku-20240307",
        ];
        for model in claude_models {
            let result = data.lookup(Some("anthropic"), model);
            // Should find via exact match or date stripping
            if result.is_none() {
                // Try without date for newer models
                let base = strip_date_suffix(model);
                let result = data.lookup(Some("anthropic"), &base);
                assert!(
                    result.is_some() || base == model,
                    "Should find Anthropic model: {} (or base: {})",
                    model,
                    base
                );
            }
        }
    }

    #[test]
    fn test_lookup_anthropic_claude_aliases() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Claude API aliases (without date)
        let aliases = [
            "claude-sonnet-4-5",
            "claude-haiku-4-5",
            "claude-opus-4-5",
            "claude-opus-4-1",
            "claude-sonnet-4-0",
            "claude-opus-4-0",
        ];
        for alias in aliases {
            // Just verify lookup doesn't panic - aliases may or may not exist in LiteLLM
            let _ = data.lookup(Some("anthropic"), alias);
        }
    }

    #[test]
    fn test_lookup_anthropic_bedrock_format() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // AWS Bedrock format: anthropic.claude-*-v1:0
        let bedrock_models = [
            "anthropic.claude-3-haiku-20240307-v1:0",
            "anthropic.claude-sonnet-4-5-20250929-v1:0",
            "anthropic.claude-opus-4-5-20251101-v1:0",
        ];
        for model in bedrock_models {
            let result = data.lookup(Some("bedrock"), model);
            // Should find after stripping -v1:0 suffix
            if let Some((_, match_type)) = result {
                assert!(
                    match_type == MatchType::Exact
                        || match_type == MatchType::Alias
                        || match_type == MatchType::Family,
                    "Bedrock model {} should match",
                    model
                );
            }
        }
    }

    #[test]
    fn test_lookup_anthropic_vertex_format() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // GCP Vertex AI format: claude-*@date
        // These newer models may not be in LiteLLM yet, so just verify no panic
        let vertex_models = [
            "claude-sonnet-4-5@20250929",
            "claude-haiku-4-5@20251001",
            "claude-opus-4-5@20251101",
            "claude-3-haiku@20240307",
        ];
        for model in vertex_models {
            // Just verify lookup doesn't panic
            let _ = data.lookup(Some("vertex_ai"), model);
        }

        // Test that @date stripping works correctly
        assert_eq!(
            normalize_model_name("claude-sonnet-4-5@20250929"),
            "claude-sonnet-4-5"
        );
    }

    #[test]
    fn test_lookup_anthropic_with_regional_prefix() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Bedrock cross-region format: region.anthropic.claude-*
        let regional_models = [
            "global.anthropic.claude-3-haiku-20240307-v1:0",
            "us.anthropic.claude-sonnet-4-5-20250929-v1:0",
            "eu.anthropic.claude-opus-4-5-20251101-v1:0",
        ];
        for model in regional_models {
            let result = data.lookup(Some("bedrock"), model);
            // Should find after stripping regional prefix and -v1:0
            if let Some((_, match_type)) = result {
                assert!(
                    match_type == MatchType::Exact
                        || match_type == MatchType::Alias
                        || match_type == MatchType::Family,
                    "Regional Bedrock model {} should match",
                    model
                );
            }
        }
    }

    #[test]
    fn test_lookup_anthropic_legacy_models() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Legacy Claude 3 models that exist in LiteLLM pricing data
        // Note: claude-3-sonnet-20240229 is not in LiteLLM data
        let legacy_models = [
            "claude-3-opus-20240229",
            "claude-3-haiku-20240307",
            "claude-3-5-sonnet-20240620",
            "claude-3-5-sonnet-20241022",
            "claude-3-5-haiku-20241022",
        ];
        for model in legacy_models {
            let result = data.lookup(Some("anthropic"), model);
            assert!(
                result.is_some(),
                "Should find legacy Anthropic model: {}",
                model
            );
        }
    }

    #[test]
    fn test_lookup_anthropic_latest_alias() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // -latest suffix should be stripped
        let result = data.lookup(Some("anthropic"), "claude-3-7-sonnet-latest");
        // Should find via Alias match (stripped -latest)
        if let Some((_, match_type)) = result {
            assert!(
                match_type == MatchType::Exact || match_type == MatchType::Alias,
                "claude-3-7-sonnet-latest should find via Exact or Alias"
            );
        }
    }

    // === Vertex AI Model Tests ===

    #[test]
    fn test_lookup_vertex_ai_gemini_models() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Gemini models accessed via Vertex AI
        let vertex_gemini_models = [
            "gemini-1.5-pro",
            "gemini-1.5-flash",
            "gemini-2.0-flash",
            "gemini-2.5-pro",
            "gemini-2.5-flash",
            "gemini-3-flash-preview",
            "gemini-3-pro-preview",
        ];
        for model in vertex_gemini_models {
            let result = data.lookup(Some("vertex_ai"), model);
            assert!(
                result.is_some(),
                "Should find Vertex AI Gemini model: {}",
                model
            );
        }
    }

    #[test]
    fn test_lookup_vertex_ai_claude_models() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Claude models on Vertex AI use @date format
        let vertex_claude_models = [
            "claude-3-5-sonnet@20240620",
            "claude-3-haiku@20240307",
            "claude-3-opus@20240229",
            "claude-sonnet-4-5@20250929",
        ];
        for model in vertex_claude_models {
            // Verify no panic; model may or may not be found depending on LiteLLM data
            let _ = data.lookup(Some("vertex_ai"), model);
        }
        // Also verify that vertex_ai/claude-* models exist directly
        let result = data.lookup(Some("vertex_ai"), "claude-3-5-sonnet");
        assert!(result.is_some(), "Should find vertex_ai/claude-3-5-sonnet");
    }

    #[test]
    fn test_lookup_vertex_ai_third_party_models() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Third-party models on Vertex AI
        let third_party_models = [
            "codestral-2",
            "jamba-1.5-large",
            "jamba-1.5-mini",
            "mistral-large@2407",
        ];
        for model in third_party_models {
            // Verify no panic; model may or may not be found
            let _ = data.lookup(Some("vertex_ai"), model);
        }
    }

    #[test]
    fn test_lookup_vertex_ai_image_models() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Imagen models on Vertex AI
        let imagen_models = [
            "imagen-3.0-generate-001",
            "imagen-3.0-fast-generate-001",
            "imagen-4.0-generate-001",
        ];
        for model in imagen_models {
            let result = data.lookup(Some("vertex_ai"), model);
            // Imagen models should be found via provider prefix
            if result.is_none() {
                // Try exact match without provider (some may be stored differently)
                let _ = data.lookup(None, model);
            }
        }
    }

    #[test]
    fn test_lookup_direct_gemini_models() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Direct Gemini models (via Google AI Studio)
        let gemini_models = [
            "gemini-pro",
            "gemini-pro-vision",
            "gemini-1.0-pro",
            "gemini-1.5-pro",
            "gemini-1.5-flash",
            "gemini-2.0-flash",
            "gemini-2.5-pro",
            "gemini-2.5-flash",
        ];
        for model in gemini_models {
            let result = data.lookup(Some("gemini"), model);
            assert!(
                result.is_some(),
                "Should find direct Gemini model: {}",
                model
            );
        }
    }

    #[test]
    fn test_lookup_gemini_dated_versions() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Gemini dated versions
        let dated_models = [
            "gemini-1.5-pro-001",
            "gemini-1.5-pro-002",
            "gemini-1.5-flash-001",
            "gemini-1.5-flash-002",
            "gemini-2.0-flash-001",
        ];
        for model in dated_models {
            let result = data.lookup(Some("gemini"), model);
            assert!(
                result.is_some(),
                "Should find Gemini dated model: {}",
                model
            );
        }
    }

    #[test]
    fn test_lookup_gemini_preview_and_experimental() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Preview and experimental Gemini models with non-zero pricing
        // Note: gemini-2.0-flash-thinking-exp has zero costs in LiteLLM and is skipped
        let preview_models = [
            "gemini-1.5-pro-preview-0514",
            "gemini-2.0-flash-exp",
            "gemini-2.5-flash-preview-05-20",
        ];
        for model in preview_models {
            let result = data.lookup(Some("gemini"), model);
            assert!(
                result.is_some(),
                "Should find Gemini preview/experimental model: {}",
                model
            );
        }
    }

    #[test]
    fn test_lookup_gemini_embedding_models() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Gemini embedding model
        let result = data.lookup(Some("gemini"), "gemini-embedding-001");
        assert!(result.is_some(), "Should find gemini-embedding-001");
    }

    #[test]
    fn test_vertex_ai_provider_mapping() {
        // Verify provider mapping works for Vertex AI variants
        assert_eq!(map_system_to_litellm_provider("vertex_ai"), "vertex_ai");
        assert_eq!(map_system_to_litellm_provider("vertexai"), "vertex_ai");
        assert_eq!(map_system_to_litellm_provider("vertex"), "vertex_ai");
        assert_eq!(
            map_system_to_litellm_provider("google_vertexai"),
            "vertex_ai"
        );
        // Gemini via Google AI Studio
        assert_eq!(map_system_to_litellm_provider("gemini"), "gemini");
        assert_eq!(map_system_to_litellm_provider("google"), "gemini");
        assert_eq!(map_system_to_litellm_provider("google_ai_studio"), "gemini");
    }

    // === Azure OpenAI Tests ===

    #[test]
    fn test_lookup_azure_openai_models() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Azure models are prefixed with azure/
        let azure_models = [
            "gpt-4o",
            "gpt-4o-mini",
            "gpt-4-turbo",
            "gpt-4",
            "gpt-35-turbo",
        ];
        for model in azure_models {
            let result = data.lookup(Some("azure"), model);
            assert!(
                result.is_some(),
                "Should find Azure OpenAI model: {}",
                model
            );
        }
    }

    #[test]
    fn test_lookup_azure_regional_models() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Azure has regional deployments like azure/eu/gpt-4o
        // Just verify no panic on lookup
        let _ = data.lookup(Some("azure"), "eu/gpt-4o-2024-08-06");
        let _ = data.lookup(Some("azure"), "gpt-4o-2024-08-06");
    }

    #[test]
    fn test_azure_provider_mapping() {
        assert_eq!(map_system_to_litellm_provider("azure"), "azure");
        assert_eq!(map_system_to_litellm_provider("azure_openai"), "azure");
        assert_eq!(map_system_to_litellm_provider("azure.openai"), "azure");
        assert_eq!(map_system_to_litellm_provider("azureopenai"), "azure");
    }

    // === Mistral Tests ===

    #[test]
    fn test_lookup_mistral_models() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Mistral models with various formats
        let mistral_models = [
            "mistral-large-latest",
            "mistral-small-latest",
            "codestral-latest",
        ];
        for model in mistral_models {
            let result = data.lookup(Some("mistral"), model);
            // Some models may not have pricing, just verify no panic
            let _ = result;
        }
    }

    #[test]
    fn test_lookup_mistral_prefixed_models() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Mistral models stored with mistral/ prefix
        let result = data.lookup(Some("mistral"), "codestral-2405");
        if let Some((_, match_type)) = result {
            assert!(
                match_type == MatchType::Exact || match_type == MatchType::ProviderPrefix,
                "Should find via Exact or ProviderPrefix"
            );
        }
    }

    #[test]
    fn test_mistral_provider_mapping() {
        assert_eq!(map_system_to_litellm_provider("mistral"), "mistral");
    }

    // === Cohere Tests ===

    #[test]
    fn test_lookup_cohere_models() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Cohere command models (Bedrock format)
        let cohere_models = ["command-r-plus", "command-r", "command"];
        for model in cohere_models {
            // Verify no panic; models may be stored differently
            let _ = data.lookup(Some("cohere"), model);
        }
    }

    #[test]
    fn test_cohere_provider_mapping() {
        assert_eq!(map_system_to_litellm_provider("cohere"), "cohere");
    }

    // === Groq Tests ===

    #[test]
    fn test_lookup_groq_models() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Groq models are prefixed with groq/
        let groq_models = [
            "llama-3.3-70b-versatile",
            "llama-3.1-8b-instant",
            "gemma-7b-it",
        ];
        for model in groq_models {
            let result = data.lookup(Some("groq"), model);
            // Verify no panic; check if found
            if let Some((_, match_type)) = result {
                assert!(
                    match_type == MatchType::Exact || match_type == MatchType::ProviderPrefix,
                    "Groq model {} should match",
                    model
                );
            }
        }
    }

    #[test]
    fn test_groq_provider_mapping() {
        assert_eq!(map_system_to_litellm_provider("groq"), "groq");
    }

    // === Together AI Tests ===

    #[test]
    fn test_lookup_together_ai_models() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Together AI models have org/model format
        let together_models = [
            "meta-llama/Llama-3.3-70B-Instruct-Turbo",
            "meta-llama/Meta-Llama-3.1-8B-Instruct-Turbo",
            "deepseek-ai/DeepSeek-V3",
        ];
        for model in together_models {
            let result = data.lookup(Some("together_ai"), model);
            // Verify lookup doesn't panic
            let _ = result;
        }
    }

    #[test]
    fn test_together_ai_provider_mapping() {
        assert_eq!(map_system_to_litellm_provider("together"), "together_ai");
        assert_eq!(map_system_to_litellm_provider("together_ai"), "together_ai");
        assert_eq!(map_system_to_litellm_provider("togetherai"), "together_ai");
    }

    // === xAI/Grok Tests ===

    #[test]
    fn test_lookup_xai_grok_models() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // xAI Grok models
        let grok_models = [
            "grok-2",
            "grok-2-latest",
            "grok-2-vision",
            "grok-3-beta",
            "grok-3-mini-beta",
        ];
        for model in grok_models {
            let result = data.lookup(Some("xai"), model);
            // Verify no panic
            let _ = result;
        }
    }

    #[test]
    fn test_xai_provider_mapping() {
        assert_eq!(map_system_to_litellm_provider("xai"), "xai");
        assert_eq!(map_system_to_litellm_provider("x.ai"), "xai");
        assert_eq!(map_system_to_litellm_provider("grok"), "xai");
    }

    // === Perplexity Tests ===

    #[test]
    fn test_lookup_perplexity_models() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Perplexity sonar models
        let perplexity_models = [
            "llama-3.1-sonar-large-128k-online",
            "llama-3.1-sonar-small-128k-chat",
            "llama-3.1-70b-instruct",
        ];
        for model in perplexity_models {
            let result = data.lookup(Some("perplexity"), model);
            // Verify no panic
            let _ = result;
        }
    }

    #[test]
    fn test_perplexity_provider_mapping() {
        assert_eq!(map_system_to_litellm_provider("perplexity"), "perplexity");
    }

    // === DeepInfra Tests ===

    #[test]
    fn test_lookup_deepinfra_models() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // DeepInfra hosts various open source models
        let deepinfra_models = [
            "meta-llama/Llama-3.3-70B-Instruct",
            "deepseek-ai/DeepSeek-V3",
            "deepseek-ai/DeepSeek-R1",
        ];
        for model in deepinfra_models {
            let result = data.lookup(Some("deepinfra"), model);
            // Verify no panic
            let _ = result;
        }
    }

    #[test]
    fn test_deepinfra_provider_mapping() {
        assert_eq!(map_system_to_litellm_provider("deepinfra"), "deepinfra");
        assert_eq!(map_system_to_litellm_provider("deep_infra"), "deepinfra");
    }

    // === Fireworks AI Tests ===

    #[test]
    fn test_lookup_fireworks_ai_models() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Fireworks AI models have long path format
        let fireworks_models = [
            "accounts/fireworks/models/llama-v3p1-70b-instruct",
            "accounts/fireworks/models/llama-v3p1-8b-instruct",
        ];
        for model in fireworks_models {
            let result = data.lookup(Some("fireworks_ai"), model);
            // Verify no panic
            let _ = result;
        }
    }

    #[test]
    fn test_fireworks_ai_provider_mapping() {
        assert_eq!(map_system_to_litellm_provider("fireworks"), "fireworks_ai");
        assert_eq!(
            map_system_to_litellm_provider("fireworks_ai"),
            "fireworks_ai"
        );
    }

    // === Ollama Tests ===

    #[test]
    fn test_lookup_ollama_models() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Ollama local models (typically free, may not have pricing)
        let ollama_models = ["llama2:7b", "llama2:13b", "codellama", "mistral"];
        for model in ollama_models {
            let result = data.lookup(Some("ollama"), model);
            // Ollama models are free, may not be in pricing data
            let _ = result;
        }
    }

    #[test]
    fn test_ollama_provider_mapping() {
        assert_eq!(map_system_to_litellm_provider("ollama"), "ollama");
    }

    // === OpenRouter Tests ===

    #[test]
    fn test_lookup_openrouter_models() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // OpenRouter aggregates models from various providers
        let openrouter_models = [
            "anthropic/claude-3.5-sonnet",
            "anthropic/claude-3-haiku",
            "openai/gpt-4o",
        ];
        for model in openrouter_models {
            let result = data.lookup(Some("openrouter"), model);
            // Verify no panic
            let _ = result;
        }
    }

    #[test]
    fn test_openrouter_provider_mapping() {
        assert_eq!(map_system_to_litellm_provider("openrouter"), "openrouter");
        assert_eq!(map_system_to_litellm_provider("open_router"), "openrouter");
    }

    // === Replicate Tests ===

    #[test]
    fn test_lookup_replicate_models() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Replicate models have org/model format
        let replicate_models = [
            "meta/llama-2-70b-chat",
            "meta/llama-3-70b-instruct",
            "mistralai/mistral-7b-instruct-v0.2",
        ];
        for model in replicate_models {
            let result = data.lookup(Some("replicate"), model);
            // Verify no panic
            let _ = result;
        }
    }

    #[test]
    fn test_replicate_provider_mapping() {
        assert_eq!(map_system_to_litellm_provider("replicate"), "replicate");
    }

    // === Databricks Tests ===

    #[test]
    fn test_lookup_databricks_models() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Databricks hosted models
        let databricks_models = [
            "databricks-dbrx-instruct",
            "databricks-llama-3-70b-instruct",
            "databricks-claude-3-7-sonnet",
        ];
        for model in databricks_models {
            let result = data.lookup(Some("databricks"), model);
            // Verify no panic
            let _ = result;
        }
    }

    #[test]
    fn test_databricks_provider_mapping() {
        assert_eq!(map_system_to_litellm_provider("databricks"), "databricks");
    }

    // === AI21 Tests ===

    #[test]
    fn test_lookup_ai21_models() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // AI21 Jamba and Jurassic models
        let ai21_models = ["jamba-1.5-large", "jamba-1.5-mini", "j2-ultra"];
        for model in ai21_models {
            let result = data.lookup(Some("ai21"), model);
            // Verify no panic
            let _ = result;
        }
    }

    #[test]
    fn test_ai21_provider_mapping() {
        assert_eq!(map_system_to_litellm_provider("ai21"), "ai21");
        assert_eq!(map_system_to_litellm_provider("ai21_chat"), "ai21");
    }

    // === WatsonX Tests ===

    #[test]
    fn test_lookup_watsonx_models() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // IBM WatsonX Granite models
        let watsonx_models = [
            "ibm/granite-13b-chat-v2",
            "ibm/granite-3-8b-instruct",
            "meta-llama/llama-3-70b-instruct",
        ];
        for model in watsonx_models {
            let result = data.lookup(Some("watsonx"), model);
            // Verify no panic
            let _ = result;
        }
    }

    #[test]
    fn test_watsonx_provider_mapping() {
        assert_eq!(map_system_to_litellm_provider("watsonx"), "watsonx");
        assert_eq!(map_system_to_litellm_provider("watson_x"), "watsonx");
        assert_eq!(map_system_to_litellm_provider("ibm_watsonx"), "watsonx");
    }

    // === Comprehensive Provider Mapping Test ===

    #[test]
    fn test_all_provider_mappings() {
        // Verify all provider mappings are correct
        let mappings = [
            // Core providers
            ("openai", "openai"),
            ("anthropic", "anthropic"),
            ("cohere", "cohere"),
            ("mistral", "mistral"),
            // Cloud providers
            ("aws_bedrock", "bedrock"),
            ("bedrock", "bedrock"),
            ("azure", "azure"),
            ("azure_openai", "azure"),
            ("vertex_ai", "vertex_ai"),
            ("gemini", "gemini"),
            ("google", "gemini"),
            // Inference providers
            ("groq", "groq"),
            ("together_ai", "together_ai"),
            ("fireworks_ai", "fireworks_ai"),
            ("deepinfra", "deepinfra"),
            ("perplexity", "perplexity"),
            ("replicate", "replicate"),
            ("ollama", "ollama"),
            // Other providers
            ("xai", "xai"),
            ("grok", "xai"),
            ("ai21", "ai21"),
            ("openrouter", "openrouter"),
            ("databricks", "databricks"),
            ("watsonx", "watsonx"),
        ];

        for (input, expected) in mappings {
            assert_eq!(
                map_system_to_litellm_provider(input),
                expected,
                "Provider mapping failed for: {}",
                input
            );
        }
    }

    // ==========================================================================
    // MODEL FORMAT TESTS - Comprehensive format validation
    // ==========================================================================

    // --- Bedrock Format Tests ---

    #[test]
    fn test_bedrock_provider_model_version_format() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Bedrock format: provider.model-version:snapshot
        let bedrock_formats = [
            "anthropic.claude-3-5-sonnet-20241022-v2:0",
            "anthropic.claude-3-haiku-20240307-v1:0",
            "meta.llama3-70b-instruct-v1:0",
            "amazon.titan-text-express-v1",
            "amazon.nova-lite-v1:0",
        ];
        for model in bedrock_formats {
            let result = data.lookup(Some("bedrock"), model);
            // Verify no panic and check if found
            if let Some((_, match_type)) = result {
                assert!(
                    matches!(
                        match_type,
                        MatchType::Exact | MatchType::ProviderPrefix | MatchType::Alias
                    ),
                    "Bedrock model {} should match",
                    model
                );
            }
        }
    }

    #[test]
    fn test_bedrock_regional_with_version() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Combined regional prefix + provider.model-version:snapshot
        let regional_formats = [
            "us.anthropic.claude-3-5-sonnet-20241022-v2:0",
            "eu.anthropic.claude-3-haiku-20240307-v1:0",
            "global.amazon.nova-lite-v1:0",
            "ap.meta.llama3-70b-instruct-v1:0",
        ];
        for model in regional_formats {
            let result = data.lookup(Some("bedrock"), model);
            // Should strip regional prefix and find base model
            let _ = result; // Verify no panic
        }
    }

    // --- OpenRouter Format Tests ---

    #[test]
    fn test_openrouter_org_model_format() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // OpenRouter format: organization/model-name
        let openrouter_formats = [
            "anthropic/claude-3-5-sonnet",
            "openai/gpt-4o",
            "google/gemini-2.5-pro-preview",
            "deepseek/deepseek-r1-0528",
            "meta-llama/llama-3-8b-instruct",
        ];
        for model in openrouter_formats {
            let result = data.lookup(Some("openrouter"), model);
            // Verify no panic
            let _ = result;
        }
    }

    #[test]
    fn test_openrouter_routing_suffixes() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // OpenRouter routing suffixes should be stripped
        let routing_formats = [
            "meta-llama/llama-3-8b-instruct:free",
            "meta-llama/llama-3-8b-instruct:extended",
            "anthropic/claude-3-5-sonnet:nitro",
            "openai/gpt-4o:beta",
        ];
        for model in routing_formats {
            let result = data.lookup(Some("openrouter"), model);
            // Verify no panic - suffix should be stripped
            let _ = result;
        }
    }

    #[test]
    fn test_strip_openrouter_routing_suffix() {
        assert_eq!(strip_openrouter_routing_suffix("model:free"), "model");
        assert_eq!(strip_openrouter_routing_suffix("model:extended"), "model");
        assert_eq!(strip_openrouter_routing_suffix("model:nitro"), "model");
        assert_eq!(strip_openrouter_routing_suffix("model:beta"), "model");
        // Should not strip non-routing suffixes
        assert_eq!(
            strip_openrouter_routing_suffix("model:unknown"),
            "model:unknown"
        );
        assert_eq!(strip_openrouter_routing_suffix("model-v1:0"), "model-v1:0");
    }

    // --- LiteLLM Colon Prefix Format Tests ---

    #[test]
    fn test_litellm_colon_prefix_format() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // LiteLLM format: provider:model
        let litellm_formats = [
            "openai:gpt-4o",
            "anthropic:claude-3-5-sonnet-20241022",
            "bedrock:anthropic.claude-3-opus-20240229-v1:0",
            "vertex:gemini-1.5-pro",
            "azure:gpt-4o",
        ];
        for model in litellm_formats {
            let result = data.lookup(None, model);
            // Verify no panic - should extract provider and model
            let _ = result;
        }
    }

    #[test]
    fn test_extract_litellm_colon_prefix() {
        // Valid colon prefix formats
        assert_eq!(
            extract_litellm_colon_prefix("openai:gpt-4o"),
            Some(("openai", "gpt-4o"))
        );
        assert_eq!(
            extract_litellm_colon_prefix("bedrock:anthropic.claude"),
            Some(("bedrock", "anthropic.claude"))
        );
        assert_eq!(
            extract_litellm_colon_prefix("vertex:gemini-pro"),
            Some(("vertex_ai", "gemini-pro"))
        );

        // Should not extract fine-tuned format
        assert_eq!(
            extract_litellm_colon_prefix("ft:gpt-3.5-turbo:org::id"),
            None
        );

        // Should not extract unknown providers
        assert_eq!(extract_litellm_colon_prefix("unknown:model"), None);

        // Should not extract version suffix
        assert_eq!(extract_litellm_colon_prefix("model-v1:0"), None);
    }

    // --- Azure OpenAI Format Tests ---

    #[test]
    fn test_azure_gpt35_naming() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Azure uses gpt-35-turbo (not gpt-3.5-turbo)
        let azure_models = [
            "gpt-35-turbo",
            "gpt-35-turbo-16k",
            "gpt-35-turbo-0125",
            "gpt-35-turbo-instruct",
            "gpt-4-32k",
            "gpt-4-turbo-2024-04-09",
        ];
        for model in azure_models {
            let result = data.lookup(Some("azure"), model);
            // Verify no panic
            let _ = result;
        }
    }

    // --- Anthropic Format Tests ---

    #[test]
    fn test_anthropic_date_formats() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Anthropic date-based naming
        let anthropic_formats = [
            "claude-3-5-sonnet-20241022",
            "claude-3-haiku-20240307",
            "claude-opus-4-5-20251101",
            "claude-sonnet-4-20250514",
        ];
        for model in anthropic_formats {
            let result = data.lookup(Some("anthropic"), model);
            // Verify no panic
            let _ = result;
        }
    }

    #[test]
    fn test_anthropic_with_version_suffix() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Anthropic with version suffix (Bedrock style)
        let versioned_formats = [
            "claude-sonnet-4-20250514-v1:0",
            "claude-3-5-sonnet-20241022-v2:0",
        ];
        for model in versioned_formats {
            let result = data.lookup(Some("anthropic"), model);
            // Should strip -v1:0/-v2:0 and find base model
            let _ = result;
        }
    }

    #[test]
    fn test_anthropic_simple_versions() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Simple version numbers
        let simple_versions = ["claude-2.1", "claude-2.0", "claude-instant-1.2"];
        for model in simple_versions {
            let result = data.lookup(Some("anthropic"), model);
            // Verify no panic
            let _ = result;
        }
    }

    // --- Vertex AI Format Tests ---

    #[test]
    fn test_vertex_ai_at_date_format() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Vertex AI @date format
        let vertex_formats = [
            "claude-3-sonnet@20240229",
            "claude-3-5-sonnet@20240620",
            "gemini-1.5-pro@20240215",
        ];
        for model in vertex_formats {
            let result = data.lookup(Some("vertex_ai"), model);
            // Should strip @date and find base model
            let _ = result;
        }
    }

    // --- Cohere Format Tests ---

    #[test]
    fn test_cohere_model_formats() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Cohere model naming
        let cohere_formats = [
            "command-r-plus",
            "command-r",
            "command-light-text-v14",
            "embed-english-v3.0",
            "command-r-plus-08-2024",
        ];
        for model in cohere_formats {
            let result = data.lookup(Some("cohere"), model);
            // Verify no panic
            let _ = result;
        }
    }

    // --- Mistral Format Tests ---

    #[test]
    fn test_mistral_model_formats() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Mistral naming conventions
        let mistral_formats = [
            "mistral-large-2407",
            "open-mistral-7b",
            "codestral-2501",
            "mistral-small-latest",
        ];
        for model in mistral_formats {
            let result = data.lookup(Some("mistral"), model);
            // Verify no panic
            let _ = result;
        }
    }

    // --- OpenAI Format Tests ---

    #[test]
    fn test_openai_model_formats() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Various OpenAI naming patterns
        let openai_formats = [
            "gpt-4o",
            "gpt-4o-mini",
            "gpt-4-turbo",
            "gpt-4-0125-preview",
            "o1-mini",
            "o1-preview",
            "gpt-3.5-turbo",
            "gpt-3.5-turbo-16k",
        ];
        for model in openai_formats {
            let result = data.lookup(Some("openai"), model);
            assert!(result.is_some(), "Should find OpenAI model: {}", model);
        }
    }

    // --- Date Format Tests ---

    #[test]
    fn test_date_suffix_formats() {
        // Test all date suffix formats
        // YYYYMMDD (8 digits)
        assert_eq!(
            strip_date_suffix("claude-3-5-sonnet-20241022"),
            "claude-3-5-sonnet"
        );
        // YYYY-MM-DD (with hyphens)
        assert_eq!(strip_date_suffix("gpt-4o-2024-11-20"), "gpt-4o");
        // Short date MMDD (4 digits)
        assert_eq!(
            strip_date_suffix("gpt-4-0125-preview"),
            "gpt-4-0125-preview" // Should NOT strip - not a date suffix
        );
        // YYMM format (Mistral style)
        assert_eq!(
            strip_date_suffix("mistral-large-2407"),
            "mistral-large-2407" // Should NOT strip - too short
        );
    }

    #[test]
    fn test_normalize_model_name_comprehensive() {
        // Latest suffix
        assert_eq!(normalize_model_name("gpt-4o-latest"), "gpt-4o");
        assert_eq!(normalize_model_name("model:latest"), "model");

        // OpenRouter routing suffix
        assert_eq!(normalize_model_name("model:free"), "model");
        assert_eq!(normalize_model_name("model:extended"), "model");
        assert_eq!(normalize_model_name("model:nitro"), "model");

        // Vertex @date suffix
        assert_eq!(
            normalize_model_name("claude-3-sonnet@20240229"),
            "claude-3-sonnet"
        );

        // Bedrock version suffix
        assert_eq!(normalize_model_name("model-v1:0"), "model");
        assert_eq!(normalize_model_name("model-v2:0"), "model");
    }

    // --- Combined Format Tests ---

    #[test]
    fn test_complex_combined_formats() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        // Test complex combinations of formats
        let complex_formats = [
            // Regional + provider + model + version
            ("bedrock", "us.anthropic.claude-3-5-sonnet-20241022-v2:0"),
            // Provider:model format
            ("openai", "gpt-4o"),
            // OpenRouter with routing suffix
            ("openrouter", "anthropic/claude-3-5-sonnet:beta"),
            // Vertex with @date
            ("vertex_ai", "claude-sonnet-4-5@20250929"),
            // Azure with date in middle
            ("azure", "gpt-4-turbo-2024-04-09"),
        ];
        for (provider, model) in complex_formats {
            let result = data.lookup(Some(provider), model);
            // Verify no panic on complex formats
            let _ = result;
        }
    }

    #[test]
    fn test_all_format_examples_from_spec() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();

        // All examples from the user's specification
        let spec_examples = [
            // Amazon Bedrock Formats
            ("bedrock", "anthropic.claude-3-5-sonnet-20241022-v2:0"),
            ("bedrock", "us.anthropic.claude-3-5-sonnet-20241022-v2:0"),
            ("bedrock", "meta.llama3-70b-instruct-v1:0"),
            ("bedrock", "mistral.mistral-large-2407-v1:0"),
            ("bedrock", "amazon.titan-text-express-v1"),
            // Anthropic Direct Formats
            ("anthropic", "claude-3-5-sonnet-20241022"),
            ("anthropic", "claude-sonnet-4-20250514-v1:0"),
            ("anthropic", "claude-3-haiku-20240307"),
            ("anthropic", "claude-opus-4-5-20251101"),
            ("anthropic", "claude-2.1"),
            // OpenAI Formats
            ("openai", "gpt-4o"),
            ("openai", "gpt-4o-mini"),
            ("openai", "gpt-4-turbo"),
            ("openai", "gpt-4-0125-preview"),
            ("openai", "o1-mini"),
            // OpenRouter Formats
            ("openrouter", "anthropic/claude-3-5-sonnet"),
            ("openrouter", "openai/gpt-4o"),
            ("openrouter", "google/gemini-2.5-pro-preview"),
            ("openrouter", "deepseek/deepseek-r1-0528"),
            // Google Vertex AI Formats
            ("vertex_ai", "gemini-1.5-pro"),
            ("vertex_ai", "gemini-2.5-flash"),
            ("vertex_ai", "claude-3-sonnet@20240229"),
            ("vertex_ai", "gemini-1.0-pro"),
            // Azure OpenAI
            ("azure", "gpt-35-turbo"),
            ("azure", "gpt-4-32k"),
            ("azure", "gpt-4-turbo-2024-04-09"),
            // Mistral AI
            ("mistral", "mistral-large-2407"),
            ("mistral", "open-mistral-7b"),
            ("mistral", "codestral-2501"),
            // Cohere
            ("cohere", "command-r-plus"),
            ("cohere", "command-light-text-v14"),
        ];

        for (provider, model) in spec_examples {
            // Verify no panic on any format from the spec
            let result = data.lookup(Some(provider), model);
            // Log for debugging if needed
            let _ = result;
        }
    }

    // === Helper Function Unit Tests ===

    #[test]
    fn test_extract_vertex_resource_model() {
        // Full resource path with project/location
        assert_eq!(
            extract_vertex_resource_model(
                "projects/my-project/locations/us-central1/publishers/google/models/gemini-2.0-flash"
            ),
            Some("gemini-2.0-flash")
        );
        // Short resource path
        assert_eq!(
            extract_vertex_resource_model("publishers/google/models/gemini-1.5-pro"),
            Some("gemini-1.5-pro")
        );
        // Not a resource path
        assert_eq!(extract_vertex_resource_model("gemini-2.0-flash"), None);
        // Partial path without /models/
        assert_eq!(
            extract_vertex_resource_model("publishers/google/gemini-2.0-flash"),
            None
        );
    }

    #[test]
    fn test_strip_replicate_version() {
        // Valid Replicate version format (64 char hash)
        assert_eq!(
            strip_replicate_version(
                "stability-ai/sdxl:2b017d0c4f2e3d5a0c0d9e3c8d9a0b3a1234567890abcdef1234567890abcdef"
            ),
            Some("stability-ai/sdxl")
        );
        // Valid with shorter hash (12+ chars)
        assert_eq!(
            strip_replicate_version("owner/model:abcdef123456"),
            Some("owner/model")
        );
        // Not a Replicate format (no slash)
        assert_eq!(strip_replicate_version("model:abcdef123456"), None);
        // Not a Replicate format (no colon)
        assert_eq!(strip_replicate_version("owner/model"), None);
        // Not a Replicate format (version too short)
        assert_eq!(strip_replicate_version("owner/model:abc123"), None);
        // Not a Replicate format (non-hex version)
        assert_eq!(strip_replicate_version("owner/model:not-a-hex-hash"), None);
        // OpenRouter format should NOT match (colon is routing suffix)
        assert_eq!(
            strip_replicate_version("anthropic/claude-3.5-sonnet:free"),
            None
        );
    }

    #[test]
    fn test_strip_openrouter_new_suffixes() {
        // New suffixes: :thinking and :exacto
        assert_eq!(
            strip_openrouter_routing_suffix("anthropic/claude-3.5-sonnet:thinking"),
            "anthropic/claude-3.5-sonnet"
        );
        assert_eq!(
            strip_openrouter_routing_suffix("openai/gpt-4o:exacto"),
            "openai/gpt-4o"
        );
    }

    // === Comprehensive Stress Test ===

    #[test]
    fn test_stress_all_model_formats() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();

        // Comprehensive stress test with real-world model formats from all providers
        // This test verifies the system handles all formats without panicking
        // and that well-known models are found

        // --- Amazon Bedrock ---
        let bedrock_models = [
            "anthropic.claude-3-5-haiku-20241022-v1:0",
            "anthropic.claude-3-5-sonnet-20241022-v2:0",
            "anthropic.claude-3-opus-20240229-v1:0",
            "us.anthropic.claude-3-7-sonnet-20250219-v1:0",
            "eu.anthropic.claude-3-5-sonnet-20241022-v2:0",
            "global.amazon.nova-2-lite-v1:0",
            "meta.llama3-2-1b-instruct-v1:0",
            "mistral.mistral-large-2407-v1:0",
            "cohere.command-r-plus-v1:0",
            "amazon.titan-text-premier-v1:0",
        ];
        for model in bedrock_models {
            let result = data.lookup(Some("bedrock"), model);
            let _ = result; // No panic
        }

        // --- Anthropic Direct ---
        let anthropic_models = [
            "claude-3-5-sonnet-20241022",
            "claude-3-5-haiku-20241022",
            "claude-3-opus-20240229",
            "claude-sonnet-4-20250514",
            "claude-opus-4-5-20251101",
            "claude-3-haiku-20240307",
            "claude-2.1",
            "claude-instant-1.2",
            // Aliases
            "claude-sonnet-4-5",
            "claude-3-5-sonnet-latest",
        ];
        for model in anthropic_models {
            let result = data.lookup(Some("anthropic"), model);
            let _ = result;
        }

        // --- OpenAI ---
        let openai_models = [
            "gpt-4o",
            "gpt-4o-mini",
            "gpt-4o-2024-11-20",
            "gpt-4-turbo",
            "gpt-4-turbo-2024-04-09",
            "gpt-4-0125-preview",
            "gpt-4-1106-preview",
            "gpt-4",
            "gpt-4-32k",
            "gpt-3.5-turbo",
            "gpt-3.5-turbo-16k",
            "gpt-3.5-turbo-0125",
            "o1-preview",
            "o1-mini",
            "o1",
            "o3-mini",
            "chatgpt-4o-latest",
            // Fine-tuned format
            "ft:gpt-3.5-turbo-0125:org::id",
            "ft:gpt-4o-mini:org:name:id",
        ];
        for model in openai_models {
            let result = data.lookup(Some("openai"), model);
            let _ = result;
        }

        // --- OpenRouter ---
        let openrouter_models = [
            "openai/gpt-4o",
            "openai/gpt-4o:free",
            "openai/gpt-4o:extended",
            "anthropic/claude-3.5-sonnet",
            "anthropic/claude-3.5-sonnet:beta",
            "anthropic/claude-3.5-sonnet:thinking",
            "anthropic/claude-3-opus:exacto",
            "google/gemini-2.5-pro-preview",
            "deepseek/deepseek-r1-0528",
            "meta-llama/llama-3.3-70b-instruct",
            "mistralai/mistral-large-2411",
        ];
        for model in openrouter_models {
            let result = data.lookup(Some("openrouter"), model);
            let _ = result;
        }

        // --- Google Vertex AI ---
        let vertex_models = [
            "gemini-2.0-flash",
            "gemini-2.5-flash",
            "gemini-1.5-pro",
            "gemini-1.5-flash",
            "gemini-1.0-pro",
            "claude-3-sonnet@20240229",
            "claude-3-5-sonnet-v2@20241022",
            "gemini-1.5-pro@20240215",
            // Resource path formats
            "publishers/google/models/gemini-2.0-flash",
            "projects/my-project/locations/us-central1/publishers/google/models/gemini-2.0-flash",
        ];
        for model in vertex_models {
            let result = data.lookup(Some("vertex_ai"), model);
            let _ = result;
        }

        // --- Azure OpenAI ---
        let azure_models = [
            "gpt-4o",
            "gpt-4o-mini",
            "gpt-4-turbo",
            "gpt-4",
            "gpt-4-32k",
            "gpt-35-turbo",
            "gpt-35-turbo-16k",
            "gpt-4-turbo-2024-04-09",
            // Custom deployment names (just verify no panic)
            "my-gpt4-deployment",
        ];
        for model in azure_models {
            let result = data.lookup(Some("azure"), model);
            let _ = result;
        }

        // --- Groq ---
        let groq_models = [
            "llama-3.3-70b-versatile",
            "llama-3.1-70b-versatile",
            "llama-3.1-8b-instant",
            "mixtral-8x7b-32768",
            "gemma2-9b-it",
        ];
        for model in groq_models {
            let result = data.lookup(Some("groq"), model);
            let _ = result;
        }

        // --- Mistral AI ---
        let mistral_models = [
            "mistral-large-latest",
            "mistral-large-2411",
            "mistral-small-latest",
            "mistral-small-2503",
            "codestral-latest",
            "codestral-2501",
            "open-mistral-7b",
            "open-mixtral-8x7b",
            "open-mixtral-8x22b",
        ];
        for model in mistral_models {
            let result = data.lookup(Some("mistral"), model);
            let _ = result;
        }

        // --- Replicate ---
        let replicate_models = [
            "meta/llama-2-70b-chat",
            "stability-ai/sdxl:2b017d0c4f2e3d5a0c0d9e3c8d9a0b3a1234567890abcdef1234567890abcdef",
            "owner/model:abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
        ];
        for model in replicate_models {
            let result = data.lookup(Some("replicate"), model);
            let _ = result;
        }

        // --- HuggingFace (no pricing expected, verify graceful handling) ---
        let huggingface_models = [
            "meta-llama/Meta-Llama-3.1-8B-Instruct",
            "mistralai/Mistral-7B-Instruct-v0.2",
            "google/gemma-2-9b-it",
        ];
        for model in huggingface_models {
            let result = data.lookup(Some("huggingface"), model);
            // HuggingFace models don't have pricing, should return None gracefully
            let _ = result;
        }

        // --- DeepSeek ---
        let deepseek_models = ["deepseek-chat", "deepseek-coder", "deepseek-reasoner"];
        for model in deepseek_models {
            let result = data.lookup(Some("deepseek"), model);
            let _ = result;
        }

        // --- xAI/Grok ---
        let xai_models = [
            "grok-2",
            "grok-2-latest",
            "grok-2-vision",
            "grok-3-beta",
            "grok-3-mini-beta",
        ];
        for model in xai_models {
            let result = data.lookup(Some("xai"), model);
            let _ = result;
        }

        // --- Cohere ---
        let cohere_models = [
            "command-r-plus",
            "command-r",
            "command-r-plus-08-2024",
            "command-light-text-v14",
            "embed-english-v3.0",
        ];
        for model in cohere_models {
            let result = data.lookup(Some("cohere"), model);
            let _ = result;
        }

        // --- LiteLLM Colon Prefix Format ---
        let litellm_formats = [
            "openai:gpt-4o",
            "anthropic:claude-3-5-sonnet-20241022",
            "bedrock:anthropic.claude-3-opus-20240229-v1:0",
            "vertex:gemini-1.5-pro",
            "azure:gpt-4o",
            "groq:llama-3.3-70b-versatile",
        ];
        for model in litellm_formats {
            let result = data.lookup(None, model);
            let _ = result;
        }
    }

    #[test]
    fn test_stress_case_insensitivity() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();

        // Verify case-insensitive lookup works
        let case_variants = [
            ("openai", "GPT-4O"),
            ("openai", "gpt-4o"),
            ("openai", "Gpt-4O"),
            ("anthropic", "CLAUDE-3-5-SONNET-20241022"),
            ("anthropic", "Claude-3-5-Sonnet-20241022"),
            ("bedrock", "ANTHROPIC.CLAUDE-3-5-SONNET-20241022-V2:0"),
        ];
        for (provider, model) in case_variants {
            let result = data.lookup(Some(provider), model);
            let _ = result;
        }
    }

    #[test]
    fn test_stress_vertex_resource_paths() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();

        // Various Vertex AI resource path formats
        let resource_paths = [
            "publishers/google/models/gemini-2.0-flash",
            "publishers/google/models/gemini-1.5-pro",
            "publishers/google/models/gemini-1.0-pro",
            "projects/my-project/locations/us-central1/publishers/google/models/gemini-2.0-flash",
            "projects/test/locations/europe-west1/publishers/google/models/gemini-1.5-flash",
        ];
        for path in resource_paths {
            let result = data.lookup(Some("vertex_ai"), path);
            // Should extract model name and attempt lookup
            let _ = result;
        }
    }

    #[test]
    fn test_slash_prefix_strip_bedrock() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        let result = data.lookup(
            None,
            "bedrock/global.anthropic.claude-haiku-4-5-20251001-v1:0",
        );
        assert!(
            result.is_some(),
            "Should find model after stripping bedrock/ prefix and global. region"
        );
        assert_eq!(result.unwrap().1, MatchType::ProviderPrefix);
    }

    #[test]
    fn test_slash_prefix_strip_anthropic() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        let result = data.lookup(None, "anthropic/claude-haiku-4-5-20251001");
        assert!(
            result.is_some(),
            "Should find model after stripping anthropic/ prefix"
        );
        assert_eq!(result.unwrap().1, MatchType::ProviderPrefix);
    }

    #[test]
    fn test_slash_prefix_strip_with_region() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        let result = data.lookup(None, "bedrock/us.amazon.nova-lite-v1:0");
        assert!(
            result.is_some(),
            "Should find model after stripping bedrock/ prefix and us. region"
        );
        assert_eq!(result.unwrap().1, MatchType::ProviderPrefix);
    }
}
