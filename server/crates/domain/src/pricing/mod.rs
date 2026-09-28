//! Pricing service for LLM cost calculations.
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

use sideseat_core::storage::AppStorage;
use sideseat_ports::clock::Clock;
use sideseat_ports::pricing::PricingCatalogueSource;

// ============================================================================
// CONSTANTS
// ============================================================================

/// Embedded pricing data (compile-time)
const EMBEDDED_PRICING_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/pricing/model_prices_and_context_window.json"
));

/// Pricing file name in data directory
const PRICING_FILE_NAME: &str = "model_prices.json";

/// Where the catalogue on disk came from, recorded beside it.
///
/// A model count says nothing about freshness, and it was the only thing distinguishing a local catalogue
/// from the embedded one - so a stale local file that had accumulated retired models won on size. This
/// records the fact directly instead of inferring it from a proxy.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PricingProvenance {
    /// [`PROVENANCE_SYNC`] or [`PROVENANCE_EMBEDDED`].
    source: String,
    /// The digest of the embedded catalogue current when this file was written - for a sync, the build it
    /// was fetched under. This is what lets a later build tell that the file predates its own snapshot,
    /// whichever source produced it.
    #[serde(default)]
    embedded_digest: Option<String>,
    /// Informational; nothing decides on it, because a filesystem clock is not a fact about the catalogue.
    #[serde(default)]
    written_at: String,
}

const PROVENANCE_SYNC: &str = "sync";
const PROVENANCE_EMBEDDED: &str = "embedded";

fn provenance_path(local_path: &Path) -> PathBuf {
    local_path.with_extension("provenance.json")
}

/// A digest of the catalogue compiled into *this* build.
///
/// Computed rather than hand-maintained: a constant somebody has to remember to bump when the embedded file
/// is refreshed is a hole, and it would be silently wrong in exactly the case this exists to detect.
fn embedded_digest() -> &'static str {
    static DIGEST: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    DIGEST.get_or_init(|| {
        blake3::hash(EMBEDDED_PRICING_JSON.as_bytes())
            .to_hex()
            .to_string()
    })
}

/// The smallest catalogue that could plausibly be the real upstream one.
///
/// A structural floor, not a ratio against the current count: a truncated download is the failure this
/// guards, and any genuine LiteLLM catalogue holds thousands of priced models. Unlike a percentage of
/// whatever is loaded, it cannot be dragged upward by a bloated local file until legitimate upstream data
/// is refused.
const MIN_PLAUSIBLE_MODEL_COUNT: usize = 100;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchType {
    /// The model key matched as given (confidence: 100%)
    Exact,
    /// The catalogue holds an entry for exactly this provider *and* model, and the provider was **stated** -
    /// by the telemetry or by the model string itself, e.g. `azure/gpt-4o` (confidence: 100%)
    ///
    /// Strictly more evidence than [`MatchType::Exact`], not less: the generic key is one fact about the
    /// model and this is two about the same call. It reported 0.95 while a generic exact hit reported 1.0,
    /// which inverted the ranking - and since Azure's `gpt-4o-mini` costs ~9% more than OpenAI's, the more
    /// specific answer was the one flagged as less certain.
    ProviderQualified,
    /// A provider prefix was **dropped or guessed** to reach a match, e.g. `bedrock/x` looked up as `x`, or
    /// an unprefixed Vertex model tried as `vertex_ai/x` (confidence: 90%)
    ///
    /// Below [`MatchType::Exact`], because the answer rests on an assumption the telemetry did not make.
    ProviderInferred,
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
            MatchType::Exact | MatchType::ProviderQualified => 1.0,
            MatchType::ProviderInferred => 0.90,
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

        // Strategy 0: the provider-qualified key, *before* the generic one.
        //
        // The generic exact match used to win, so `system=azure, model=gpt-4o-mini` was priced at OpenAI's
        // rates rather than Azure's - understating that call by ~9%, silently, for every Azure deployment.
        // A provider-qualified entry is the more specific fact about the same model: when the catalogue
        // carries `azure/gpt-4o-mini` *and* `gpt-4o-mini`, the caller told us which one it used, and ignoring
        // that in favour of the shorter key discards the only distinguishing information available.
        if let Some(provider) = provider {
            let prefixed = format!("{}/{}", provider, model_lower);
            if let Some(pricing) = self.models.get(&prefixed) {
                return Some((pricing, MatchType::ProviderQualified));
            }
        }

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
                return Some((pricing, MatchType::ProviderInferred));
            }
            if let Some(stripped) = strip_bedrock_region_prefix(model_part)
                && let Some(pricing) = self.models.get(stripped)
            {
                return Some((pricing, MatchType::ProviderInferred));
            }
        }

        // Strategy 1c: LiteLLM colon prefix format (openai:gpt-4o → gpt-4o with openai provider)
        // Only applies if model contains colon and prefix is a known provider
        if let Some((prefix, model_after_colon)) = extract_litellm_colon_prefix(&model_lower) {
            // Try with extracted provider
            let prefixed = format!("{}/{}", prefix, model_after_colon);
            if let Some(pricing) = self.models.get(&prefixed) {
                return Some((pricing, MatchType::ProviderQualified));
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
                return Some((pricing, MatchType::ProviderInferred));
            }
            // Try with gemini prefix (for google models)
            let gemini_prefixed = format!("gemini/{}", extracted);
            if let Some(pricing) = self.models.get(&gemini_prefixed) {
                return Some((pricing, MatchType::ProviderInferred));
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
                return Some((pricing, MatchType::ProviderInferred));
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

        // Strategy 3: Provider index lookup (uses pre-built index).
        //
        // The provider-prefixed key itself is Strategy 0's job now - it has to run before the generic exact
        // match, so a second identical lookup here could never be reached.
        if let Some(provider) = provider {
            let key = (provider.to_string(), model_lower.clone());
            if let Some(canonical_key) = self.provider_models.get(&key)
                && let Some(pricing) = self.models.get(canonical_key)
            {
                return Some((pricing, MatchType::ProviderQualified));
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

mod matching;
pub(crate) use matching::builtin_provider;
#[cfg(test)]
use matching::strip_openrouter_routing_suffix;
pub use matching::{
    cache_counters_are_separate, cache_counters_are_separate_for_provider, reasoning_is_separate,
    reasoning_is_separate_for_provider,
};
use matching::{
    extract_finetune_base_model, extract_litellm_colon_prefix, extract_vertex_resource_model,
    map_system_to_litellm_provider, normalize_model_name, strip_bedrock_region_prefix,
    strip_date_suffix, strip_replicate_version,
};

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

    /// The litellm provider of the entry that priced this call, when one did.
    ///
    /// Reported so the token total is derived from the same answer as the charge. Without it the two were
    /// resolved independently - the charge from the catalogue entry, the total from `gen_ai.system` - and a
    /// Bedrock call whose system attribute the mapper did not recognise was billed under Bedrock's
    /// separate-cache convention while its total assumed the inclusive one.
    pub resolved_provider: Option<String>,
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

mod service;
pub use service::PricingService;

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;

#[cfg(any(test, feature = "test-support"))]
#[derive(Debug)]
struct TestClock;

#[cfg(any(test, feature = "test-support"))]
impl Clock for TestClock {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::UNIX_EPOCH
    }
}

#[cfg(test)]
mod declared_provider_alias_tests {
    use super::map_system_to_litellm_provider;

    /// A framework that names itself in `gen_ai.system` resolves to the provider that serves its models.
    ///
    /// Declared by the asset that owns the framework (`rules/producers/google-adk.json`), not beside the catalogue's own
    /// provider spellings - and consulted before them, so the claim never has to be written as if a provider
    /// had made it. Both spellings, because normalisation replaces a separator rather than removing it.
    #[test]
    fn a_declared_framework_alias_resolves_to_its_provider() {
        for spelling in [
            "google_adk",
            "googleadk",
            "google-adk",
            "Google ADK",
            "GOOGLE_ADK",
        ] {
            assert_eq!(
                map_system_to_litellm_provider(spelling),
                "gemini",
                "`{spelling}` must price from that provider's catalogue entries"
            );
        }
        // And a framework that names *no* provider still answers nothing, so the catalogue is searched by model
        // name instead - which is the whole reason the deleted list said nothing.
        for spelling in ["crewai", "langgraph", "strands-agents", "something_new"] {
            assert_eq!(map_system_to_litellm_provider(spelling), "");
        }
    }
}
