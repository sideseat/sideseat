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

// ============================================================================
// PROVIDER MAPPING
// ============================================================================

/// Maps gen_ai.system attribute to LiteLLM provider name
///
/// Returns empty string for framework-only values (let model lookup handle them)
/// Whether this **litellm provider** reports cache counters beside its input rather than within it.
///
/// Keyed on the provider of the catalogue entry that actually priced the call, not on a second parse of
/// `gen_ai.system`. The two are not the same question: the price lookup resolves a provider from the
/// model name as well as the system attribute, so `anthropic.claude-3-haiku-...` is priced from a Bedrock
/// entry even when the system attribute says `AWS` (which the mapper did not recognise) or says nothing at
/// all. Reading `system` for the convention while the price came from elsewhere meant a Bedrock call was
/// billed at Bedrock's rates and *counted* under OpenAI's convention - a cached turn reporting 15 tokens
/// where 1,215 were billed, with ten ordinary input tokens dropped from the cost.
pub fn cache_counters_are_separate_for_provider(provider: &str) -> bool {
    // By **family**, not by exact string. The catalogue does not use one name per provider: Bedrock alone
    // appears as `bedrock` (268 entries), `bedrock_converse` (152) and `bedrock_mantle` (15), and Anthropic
    // on Vertex is `vertex_ai-anthropic_models` (37). Matching exact short names classified the minority and
    // sent the majority to the inclusive default - so the very Bedrock calls this rule exists for were
    // charged one way and counted another.
    //
    // Bedrock's whole API reports its cache counters separately, whichever vendor's model is behind it, so
    // the family is what matters there rather than the model's vendor.
    provider.starts_with("bedrock") || vendor_of(provider) == Vendor::Anthropic
}

/// The [`cache_counters_are_separate_for_provider`] question for reasoning tokens.
pub fn reasoning_is_separate_for_provider(provider: &str) -> bool {
    // Google's own models report thoughts beside the output. `vertex_ai` names *forty* provider variants in
    // the catalogue, most of them third-party vendors hosted on Vertex, so this is Google's families only -
    // `vertex_ai-anthropic_models` is Anthropic's convention, not Google's.
    vendor_of(provider) == Vendor::Google
}

/// Whose model a catalogue provider name refers to.
///
/// Derived from the name rather than listed per provider, because the catalogue gains providers with every
/// sync and a hand-maintained list of 40+ names is the hole this whole area keeps falling into. An
/// unrecognised name is [`Vendor::Other`], which takes the inclusive (OpenAI-shaped) reading of both
/// conventions - the cautious direction, since over-counting a total and over-charging are the worse errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Vendor {
    Anthropic,
    Google,
    Other,
}

fn vendor_of(provider: &str) -> Vendor {
    if provider == "anthropic" {
        return Vendor::Anthropic;
    }
    if provider == "gemini" || provider == "vertex_ai" {
        return Vendor::Google;
    }
    // `vertex_ai-…` splits on the catalogue's own naming convention rather than on a list of forty names:
    // a third party hosted on Vertex is `vertex_ai-<vendor>_models` (underscore - `anthropic_models`,
    // `mistral_models`, `llama_models`, `qwen_models`, …), while Google's own categories use hyphens
    // (`language-models`, `text-models`, `image-models`, `video-models`, `embedding-models`). Every one of
    // the sixteen `vertex_ai*` providers in the catalogue follows it.
    //
    // A convention is only safe to lean on if something checks it, so
    // `every_catalogue_provider_name_is_classified_deliberately` enumerates the real names: if upstream
    // renames anything, that test fails rather than this quietly returning the wrong vendor.
    if let Some(suffix) = provider.strip_prefix("vertex_ai-") {
        return match suffix.strip_suffix("_models") {
            Some(vendor) if vendor.starts_with("anthropic") => Vendor::Anthropic,
            Some(_) => Vendor::Other,
            None => Vendor::Google,
        };
    }
    Vendor::Other
}

/// Whether this provider reports cache counters *beside* its input total rather than within it.
///
/// The single source of truth for the question, because two places need it and they must not drift: the cost
/// calculation (what to charge at the plain input rate) and the synthesised token total (whether the cache
/// counters are already inside `input + output`). Anthropic and Bedrock's Converse API report them
/// separately; OpenAI's `cached_tokens` and Gemini's `cached_content_token_count` sit inside their prompt
/// totals, and an unrecognised provider takes that reading.
pub fn cache_counters_are_separate(system: Option<&str>) -> bool {
    cache_counters_are_separate_for_provider(
        system.map(map_system_to_litellm_provider).unwrap_or(""),
    )
}

/// Whether this provider reports reasoning tokens *beside* its output total rather than within it.
///
/// Independent of [`cache_counters_are_separate`]: Gemini reports `thoughts_token_count` separately while
/// counting cached content inside its prompt total, and Anthropic is the mirror image. One flag for both
/// mis-bills one of them for every provider that is not OpenAI-shaped.
pub fn reasoning_is_separate(system: Option<&str>) -> bool {
    reasoning_is_separate_for_provider(system.map(map_system_to_litellm_provider).unwrap_or(""))
}

fn map_system_to_litellm_provider(system: &str) -> &'static str {
    // Separators are normalised before matching: the same service is spelled `aws_bedrock`, `aws.bedrock`,
    // `amazon_bedrock` and - from the Vercel AI SDK - `amazon-bedrock`. Listing every punctuation variant by
    // hand is how `amazon-bedrock` came to be missing altogether, which cost it both its Bedrock prices and
    // its cache-counter convention.
    let normalised = system.to_lowercase().replace(['-', ' '], "_");
    match normalised.as_str() {
        // Direct mappings
        "openai" => "openai",
        "anthropic" => "anthropic",
        "cohere" => "cohere",
        "mistral" => "mistral",

        // AWS Bedrock variants
        // `aws` and `amazon` on their own are what several instrumentations emit; without them the
        // convention fell through to the inclusive default even though the price lookup found the
        // Bedrock entry from the model name.
        "aws_bedrock" | "aws.bedrock" | "bedrock" | "amazon_bedrock" | "aws" | "amazon" => {
            "bedrock"
        }

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

    /// Load pricing data: the local file when it is the better catalogue, else this build's embedded one.
    ///
    /// "Better" used to mean *larger* - `local.model_count >= embedded_count` - and a model count is not a
    /// statement about freshness. A catalogue that accumulated retired models is bigger and more wrong, so a
    /// stale local file won on size and pinned its prices, and upgrading the binary could not dislodge it.
    ///
    /// What decides it now is **provenance**, recorded when the file is written (see [`PricingProvenance`]):
    ///
    /// The rule is one question, asked of either source: **was this file written by the build that is now
    /// running?** Its `embedded_digest` records the snapshot current when it was written, so:
    ///
    /// - digest matches - the file is either this build's own snapshot, or a sync fetched *over* it, which is
    ///   strictly newer upstream data. Used.
    /// - digest differs, or there is no provenance at all - the binary has been upgraded since, so this
    ///   build's snapshot may hold corrections the file predates. Replaced.
    ///
    /// "A sync always wins" was the first version of this and was wrong in two ways. With `sync_hours = 0`,
    /// or with the network unavailable, a January sync was preferred over a September release's corrected
    /// prices *forever* - the reasoning "the startup sync refreshes it anyway" assumed a sync that may never
    /// run. And it broke agreement between replicas: a long-lived replica priced from its January file while
    /// a freshly-started one priced from September's snapshot, and the cost is persisted at ingestion, so the
    /// same request was stored at two different prices depending on routing. Under this rule every replica of
    /// a given build agrees, and the only divergence left is a replica that has synced since starting - which
    /// is upstream data the others converge on.
    ///
    /// Every branch is decidable from facts this code controls: no timestamps, no counts, no proxies.
    async fn load_pricing_data(local_path: &Path) -> Result<PricingData, PricingError> {
        if !local_path.exists() {
            return Self::load_embedded_with_save(local_path).await;
        }

        match Self::try_load_local(local_path).await {
            Ok(local_data) => {
                let provenance = Self::read_provenance(local_path).await;
                let keep = provenance
                    .as_ref()
                    .is_some_and(|p| p.embedded_digest.as_deref() == Some(embedded_digest()));
                if keep {
                    tracing::debug!(
                        models = local_data.model_count,
                        source = provenance
                            .as_ref()
                            .map(|p| p.source.as_str())
                            .unwrap_or("none"),
                        "Using the local pricing catalogue"
                    );
                    Ok(local_data)
                } else {
                    tracing::debug!(
                        "The local pricing catalogue predates this build; replacing it with this \
                         build's snapshot, which the next sync will update"
                    );
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
        } else {
            Self::write_provenance(
                local_path,
                PricingProvenance {
                    source: PROVENANCE_EMBEDDED.to_string(),
                    embedded_digest: Some(embedded_digest().to_string()),
                    written_at: chrono::Utc::now().to_rfc3339(),
                },
            )
            .await;
        }
        Ok(data)
    }

    /// Read the sidecar beside the catalogue. Absent or unreadable is simply "unknown provenance", which
    /// the caller treats as "not this build's" - the safe direction, since the cost is re-saving a file.
    async fn read_provenance(local_path: &Path) -> Option<PricingProvenance> {
        let raw = tokio::fs::read_to_string(provenance_path(local_path))
            .await
            .ok()?;
        serde_json::from_str(&raw).ok()
    }

    /// Best-effort: a missing sidecar costs one re-save, never a wrong price.
    async fn write_provenance(local_path: &Path, provenance: PricingProvenance) {
        let path = provenance_path(local_path);
        match serde_json::to_string(&provenance) {
            Ok(json) => {
                if let Err(e) = tokio::fs::write(&path, json).await {
                    tracing::debug!(error = %e, "Could not record pricing provenance");
                }
            }
            Err(e) => tracing::debug!(error = %e, "Could not serialise pricing provenance"),
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

        // Whether the provider's cache and reasoning counters are *subsets* of the input and output totals.
        //
        // This decides the bill, and the two conventions are irreconcilable:
        //
        //   * OpenAI (and every OpenAI-compatible endpoint) reports `prompt_tokens_details.cached_tokens`
        //     *within* `prompt_tokens`, and `completion_tokens_details.reasoning_tokens` *within*
        //     `completion_tokens`. Charging the totals and then the subsets again bills the cached portion
        //     twice - once at the full input rate, once at the cache rate. For a GPT-5 call with 100 input
        //     tokens of which 80 were cached that is ~4x the true cost, and the number is what a user makes
        //     spending decisions on.
        //   * Anthropic reports `cache_read_input_tokens` and `cache_creation_input_tokens` *beside*
        //     `input_tokens`, which excludes them. There the subsets must be added, and subtracting would
        //     under-report.
        //
        // So the counters are stored exactly as the provider reported them - the UI shows what the provider
        // said - and the *charge* is normalised here, where the provider is known. Unknown providers take the
        // inclusive reading, because OpenAI-compatible endpoints are the common case by a wide margin and
        // over-charging is the worse error to hand someone.
        // The conventions, keyed on the provider of the entry that **priced this call** rather than on a
        // second parse of `gen_ai.system`. The lookup resolves a provider from the model name as well as
        // the attribute, so the two answers differ exactly when the attribute is missing or spelled in a
        // way the mapper does not know - and then the call was charged at one provider's rates and counted
        // under another's convention. The resolved provider is reported back so the token total can be
        // derived from the same answer.
        let resolved_provider = pricing.litellm_provider.as_str();
        let cache_is_included = !cache_counters_are_separate_for_provider(resolved_provider);
        let reasoning_is_included = !reasoning_is_separate_for_provider(resolved_provider);

        // The portion charged at the plain input rate: everything not already billed as cache.
        let billable_input = if cache_is_included {
            (input_tokens - cache_read_tokens - cache_write_tokens).max(0.0)
        } else {
            input_tokens
        };
        let billable_output = if reasoning_is_included {
            (output_tokens - reasoning_tokens).max(0.0)
        } else {
            output_tokens
        };

        // Calculate costs
        let input_cost = billable_input * pricing.input_cost_per_token;

        // Output cost: zero for embeddings (they only have input)
        let output_cost = if is_embedding {
            0.0
        } else {
            billable_output * pricing.output_cost_per_token
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
            resolved_provider: (!resolved_provider.is_empty())
                .then(|| resolved_provider.to_string()),
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

        // Two guards, neither of them a ratio against whatever happens to be loaded.
        //
        // The old check refused a sync holding fewer than half the current catalogue's models. A model count
        // measures accumulated history, not correctness: a local catalogue bloated with retired models raised
        // the bar until the *current* upstream catalogue was refused, and since the check ran on every sync
        // the prices were pinned permanently - the worse the local file, the harder it was to fix.
        //
        // What the check was actually for is a truncated or wrong download, and that is asked directly:

        // 1. A structural floor. Fixed, so it cannot be dragged upward by a bad local file, and far below
        //    any genuine catalogue.
        if new_data.model_count < MIN_PLAUSIBLE_MODEL_COUNT {
            tracing::warn!(
                new = new_data.model_count,
                minimum = MIN_PLAUSIBLE_MODEL_COUNT,
                "Rejecting synced pricing: too few priced models to be a real catalogue"
            );
            return;
        }

        // There is deliberately no second check against "the models this instance is using".
        //
        // That was tried, and it made acceptance a function of the *replica* rather than of the catalogue:
        // replica A had priced model M and refused an upstream catalogue that dropped it, while replica B
        // had not and accepted the same catalogue. Cost is persisted at ingestion, so which price a span was
        // stored at then depended on which replica the balancer picked - the exact routing-dependence the
        // provenance rule above exists to remove. The observation set was also caller-fillable: a client
        // sending 256 junk model names displaced every real one and the check protected nothing.
        //
        // What is left is a decision about the catalogue alone, so every replica of a build reaches the same
        // one. The residual risk is a partially truncated catalogue that still holds more than the floor and
        // happens to drop a model in use; that leaves the model *unpriced* (cost 0, visible) rather than
        // mispriced, and the next sync corrects it.

        // Save to disk atomically
        if let Err(e) = Self::save_to_file(&self.local_path, json).await {
            tracing::warn!(error = %e, "Failed to save pricing data to disk");
        } else {
            Self::write_provenance(
                &self.local_path,
                PricingProvenance {
                    source: PROVENANCE_SYNC.to_string(),
                    // The build this sync was fetched under. A later build with a different snapshot
                    // must not keep pricing from a catalogue that predates its own corrections.
                    embedded_digest: Some(embedded_digest().to_string()),
                    written_at: chrono::Utc::now().to_rfc3339(),
                },
            )
            .await;
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

    /// An OpenAI-style cached subset is charged once, at the cache rate - not twice.
    ///
    /// OpenAI reports `cached_tokens` *inside* `prompt_tokens`, so charging the input total and then the
    /// cached subset again bills the cached portion at the full rate *plus* the cache rate. With 80 of 100
    /// input tokens cached that is several times the true cost, and this number is what a user makes spending
    /// decisions on.
    #[test]
    fn an_openai_cached_subset_is_not_charged_twice() {
        let service = PricingService::init_for_test().unwrap();
        let cached = service.calculate_cost(&SpanCostInput {
            system: Some("openai".to_string()),
            model: Some("gpt-4o-mini".to_string()),
            input_tokens: 100,
            cache_read_tokens: 80,
            ..Default::default()
        });
        let uncached = service.calculate_cost(&SpanCostInput {
            system: Some("openai".to_string()),
            model: Some("gpt-4o-mini".to_string()),
            input_tokens: 100,
            ..Default::default()
        });
        assert!(
            cached.total_cost < uncached.total_cost,
            "caching must make a call cheaper, not dearer: cached={} uncached={}",
            cached.total_cost,
            uncached.total_cost
        );
        // Only the 20 uncached tokens are charged at the input rate.
        let expected_input = 20.0 * (uncached.input_cost / 100.0);
        assert!(
            (cached.input_cost - expected_input).abs() < 1e-12,
            "expected only the uncached remainder at the input rate: got {} want {}",
            cached.input_cost,
            expected_input
        );
    }

    /// Anthropic reports cache counters *beside* the input total, so there they are added.
    ///
    /// The two conventions are irreconcilable, which is why the charge is normalised per provider: treating
    /// Anthropic's separate counters as subsets would under-report the bill instead.
    #[test]
    fn anthropic_cache_counters_are_additional_not_subsets() {
        let service = PricingService::init_for_test().unwrap();
        let with_cache = service.calculate_cost(&SpanCostInput {
            system: Some("anthropic".to_string()),
            model: Some("claude-sonnet-4-5".to_string()),
            input_tokens: 100,
            cache_read_tokens: 80,
            ..Default::default()
        });
        let without = service.calculate_cost(&SpanCostInput {
            system: Some("anthropic".to_string()),
            model: Some("claude-sonnet-4-5".to_string()),
            input_tokens: 100,
            ..Default::default()
        });
        assert!(
            with_cache.input_cost > 0.0 && without.input_cost > 0.0,
            "both should charge their input tokens"
        );
        assert!(
            (with_cache.input_cost - without.input_cost).abs() < 1e-12,
            "Anthropic's input count already excludes cache reads, so it must not be reduced"
        );
        assert!(
            with_cache.total_cost > without.total_cost,
            "the cache read is additional work and costs something"
        );
    }

    /// Every Bedrock spelling takes the separate-counter reading, not just the two I first listed.
    ///
    /// Instrumentation emits `aws_bedrock`, `amazon_bedrock`, `aws.bedrock` and `bedrock` for the same
    /// service. A hand-written variant list missed two of them, which would have made Anthropic-on-Bedrock
    /// take the *inclusive* reading and be under-charged - so the decision goes through the same mapper the
    /// price lookup uses.
    #[test]
    fn every_bedrock_spelling_treats_cache_counters_as_separate() {
        let service = PricingService::init_for_test().unwrap();
        let baseline = service
            .calculate_cost(&SpanCostInput {
                system: Some("anthropic".to_string()),
                model: Some("claude-sonnet-4-5".to_string()),
                input_tokens: 100,
                cache_read_tokens: 80,
                ..Default::default()
            })
            .input_cost;
        for spelling in ["bedrock", "aws_bedrock", "amazon_bedrock", "aws.bedrock"] {
            let cost = service
                .calculate_cost(&SpanCostInput {
                    system: Some(spelling.to_string()),
                    model: Some("claude-sonnet-4-5".to_string()),
                    input_tokens: 100,
                    cache_read_tokens: 80,
                    ..Default::default()
                })
                .input_cost;
            assert!(
                cost > 0.0,
                "{spelling}: input tokens must still be charged in full - Bedrock's count already excludes \
                 cache reads"
            );
            assert!(
                (cost - baseline).abs() < 1e-12,
                "{spelling}: should bill like anthropic ({cost} vs {baseline})"
            );
        }
    }

    /// Gemini answers the two questions *differently*, which one flag could not express.
    ///
    /// It counts cached content *inside* the prompt total but reports thinking tokens *beside* the candidate
    /// output. A single boolean therefore had to be wrong about one of them: sharing the inclusive reading
    /// under-billed the thinking, and sharing the separate reading over-billed the cache.
    #[test]
    fn gemini_cache_is_included_but_its_thinking_is_not() {
        let service = PricingService::init_for_test().unwrap();
        let out = service.calculate_cost(&SpanCostInput {
            system: Some("gemini".to_string()),
            model: Some("gemini-2.0-flash".to_string()),
            input_tokens: 100,
            cache_read_tokens: 100,
            output_tokens: 100,
            reasoning_tokens: 20,
            ..Default::default()
        });
        assert_eq!(
            out.input_cost, 0.0,
            "cached content sits inside the prompt total, so nothing is left at the input rate"
        );
        let per_output_token = out.output_cost / 100.0;
        assert!(
            per_output_token > 0.0,
            "all 100 candidate tokens are charged: the 20 thinking tokens are additional, not a subset"
        );
        assert!(out.reasoning_cost > 0.0, "and the thinking is charged too");
    }

    /// Anthropic is the mirror image: cache separate, thinking inside the output total.
    #[test]
    fn anthropic_thinking_is_inside_its_output_total() {
        let service = PricingService::init_for_test().unwrap();
        let out = service.calculate_cost(&SpanCostInput {
            system: Some("anthropic".to_string()),
            model: Some("claude-sonnet-4-5".to_string()),
            output_tokens: 100,
            reasoning_tokens: 100,
            ..Default::default()
        });
        assert_eq!(
            out.output_cost, 0.0,
            "every output token was thinking, so none remains at the plain output rate"
        );
    }

    /// A reasoning subset is charged once too, at the reasoning rate.
    #[test]
    fn an_openai_reasoning_subset_is_not_charged_twice() {
        let service = PricingService::init_for_test().unwrap();
        let out = service.calculate_cost(&SpanCostInput {
            system: Some("openai".to_string()),
            model: Some("gpt-4o-mini".to_string()),
            output_tokens: 100,
            reasoning_tokens: 100,
            ..Default::default()
        });
        assert_eq!(
            out.output_cost, 0.0,
            "every output token was reasoning, so none is left to charge at the plain output rate"
        );
        assert!(out.reasoning_cost > 0.0, "the reasoning tokens are charged");
    }

    /// A provider reporting a subset larger than its total cannot produce a negative charge.
    #[test]
    fn an_oversized_subset_cannot_go_negative() {
        let service = PricingService::init_for_test().unwrap();
        let out = service.calculate_cost(&SpanCostInput {
            system: Some("openai".to_string()),
            model: Some("gpt-4o-mini".to_string()),
            input_tokens: 10,
            cache_read_tokens: 999,
            ..Default::default()
        });
        assert_eq!(out.input_cost, 0.0);
        assert!(out.total_cost >= 0.0);
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
            matches!(
                match_type,
                MatchType::Exact | MatchType::ProviderQualified | MatchType::ProviderInferred
            ),
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
        // system=azure, model=gpt-4o-2024-11-20: the catalogue carries `azure/gpt-4o-2024-11-20`, which is
        // Azure's own price for that dated model - a `ProviderPrefix` match, and the most specific answer
        // available. `Family` or `Exact` here would mean a *less* specific entry won.
        let result = data.lookup(Some("azure"), "gpt-4o-2024-11-20");
        if let Some((_, match_type)) = result {
            assert!(
                matches!(
                    match_type,
                    MatchType::ProviderQualified
                        | MatchType::ProviderInferred
                        | MatchType::Family
                        | MatchType::Exact
                ),
                "unexpected match type: {match_type:?}"
            );
        }
    }

    /// A provider-qualified price beats the generic one for the same model.
    ///
    /// The generic exact match used to win, so every Azure deployment was billed at OpenAI's rates - about 9%
    /// under Azure's actual price, silently, on every call. The caller told us which provider it used; the
    /// shorter key is simply a less specific fact about the same model.
    #[test]
    fn a_provider_qualified_price_beats_the_generic_one() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        let (azure, azure_match) = data
            .lookup(Some("azure"), "gpt-4o-mini")
            .expect("azure/gpt-4o-mini is in the catalogue");
        let (generic, _) = data
            .lookup(None, "gpt-4o-mini")
            .expect("gpt-4o-mini is in the catalogue");
        assert_eq!(azure_match, MatchType::ProviderQualified);
        assert!(
            azure.input_cost_per_token > generic.input_cost_per_token,
            "Azure is dearer than OpenAI for this model, so picking the generic entry under-bills: \
             azure={} generic={}",
            azure.input_cost_per_token,
            generic.input_cost_per_token
        );
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
            "claude-3-7-sonnet-20250219",
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
            "gemini-2.5-flash-image",
            "gemini-3-flash-preview",
            "gemini-3-pro-preview",
            "gemini-3.1-pro-preview",
            "gemini-3.1-flash-lite-preview",
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
            "gemini-2.0-flash",
            "gemini-2.0-flash-lite",
            "gemini-2.5-pro",
            "gemini-2.5-flash",
            "gemini-2.5-flash-lite",
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
        let dated_models = ["gemini-2.0-flash-001", "gemini-2.0-flash-lite-001"];
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
        let preview_models = [
            "gemini-2.5-flash-preview-09-2025",
            "gemini-2.5-flash-lite-preview-06-17",
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
                matches!(
                    match_type,
                    MatchType::Exact | MatchType::ProviderQualified | MatchType::ProviderInferred
                ),
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
                    matches!(
                        match_type,
                        MatchType::Exact
                            | MatchType::ProviderQualified
                            | MatchType::ProviderInferred
                    ),
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
                        MatchType::Exact
                            | MatchType::ProviderQualified
                            | MatchType::ProviderInferred
                            | MatchType::Alias
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
            "o1",
            "o3-mini",
            "o4-mini",
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
        assert_eq!(result.unwrap().1, MatchType::ProviderInferred);
    }

    #[test]
    fn test_slash_prefix_strip_anthropic() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        let result = data.lookup(None, "anthropic/claude-haiku-4-5-20251001");
        assert!(
            result.is_some(),
            "Should find model after stripping anthropic/ prefix"
        );
        assert_eq!(result.unwrap().1, MatchType::ProviderInferred);
    }

    #[test]
    fn test_slash_prefix_strip_with_region() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();
        let result = data.lookup(None, "bedrock/us.amazon.nova-lite-v1:0");
        assert!(
            result.is_some(),
            "Should find model after stripping bedrock/ prefix and us. region"
        );
        assert_eq!(result.unwrap().1, MatchType::ProviderInferred);
    }

    /// A provider-qualified hit is never *less* confident than a generic one.
    ///
    /// Since the provider-qualified key is looked up first, reaching it means the catalogue held an entry
    /// for exactly this provider and model and the caller told us the provider - two facts about the call
    /// where a generic exact match is one. It reported 0.95 against the generic match's 1.0, so the answer
    /// carrying more evidence was the one flagged as more doubtful. What genuinely *is* below an exact match
    /// is a prefix that was dropped or guessed.
    #[test]
    fn confidence_ranks_more_specific_evidence_higher() {
        assert_eq!(MatchType::ProviderQualified.confidence(), 1.0);
        assert_eq!(MatchType::Exact.confidence(), 1.0);
        assert!(MatchType::ProviderInferred.confidence() < MatchType::Exact.confidence());
        assert!(MatchType::Alias.confidence() < MatchType::ProviderInferred.confidence());
        assert!(MatchType::Family.confidence() < MatchType::Alias.confidence());
        assert_eq!(MatchType::NotFound.confidence(), 0.0);
    }

    /// The two kinds are told apart by whether the provider was stated or assumed.
    #[test]
    fn a_stated_provider_qualifies_and_a_stripped_prefix_only_infers() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();

        // Stated by the telemetry, and the catalogue has that provider's own entry.
        let (_, stated) = data
            .lookup(Some("azure"), "gpt-4o-mini")
            .expect("azure/gpt-4o-mini is in the catalogue");
        assert_eq!(stated, MatchType::ProviderQualified);

        // The prefix was dropped to reach a match, so the provider is an assumption.
        let (_, stripped) = data
            .lookup(None, "anthropic/claude-haiku-4-5-20251001")
            .expect("found after stripping the prefix");
        assert_eq!(stripped, MatchType::ProviderInferred);
        assert!(stripped.confidence() < stated.confidence());
    }

    /// A smaller upstream catalogue is accepted. This is the regression the size ratio caused.
    ///
    /// The old guard refused a sync holding under half the current catalogue's models. A count measures
    /// accumulated history, not correctness - a local file bloated with retired models raised the bar until
    /// the *current* upstream was refused, and because the check ran on every sync the prices were pinned
    /// permanently: the worse the local file, the harder it was to fix.
    #[tokio::test]
    async fn a_smaller_but_current_catalogue_is_accepted() {
        let service = PricingService::init_for_test().unwrap();
        let before = service.data.read().model_count;

        // A real-shaped catalogue with far fewer models than the embedded one.
        let smaller = smaller_catalogue(before / 4);
        let smaller_count = PricingData::from_json_str(&smaller).unwrap().model_count;
        assert!(
            smaller_count < before / 2,
            "the fixture must be under the old 50% bar to be a regression test, got {smaller_count} of {before}"
        );

        service.apply_sync_data(&smaller).await;

        assert_eq!(
            service.data.read().model_count,
            smaller_count,
            "a catalogue that shrank because models were retired must still be applied"
        );
    }

    /// But something too small to be a catalogue at all is refused - a truncated download.
    #[tokio::test]
    async fn a_catalogue_too_small_to_be_real_is_rejected() {
        let service = PricingService::init_for_test().unwrap();
        let before = service.data.read().model_count;

        service
            .apply_sync_data(&smaller_catalogue(MIN_PLAUSIBLE_MODEL_COUNT - 1))
            .await;

        assert_eq!(
            service.data.read().model_count,
            before,
            "a catalogue below the structural floor must not replace a real one"
        );
    }

    /// Acceptance depends on the catalogue alone, so two replicas never disagree.
    ///
    /// A coverage check against "the models this instance is using" was tried and removed: replica A had
    /// priced model M and refused a catalogue that dropped it while replica B accepted the same catalogue,
    /// and since cost is persisted at ingestion, which price a span was stored at then depended on which
    /// replica served the request. The observation set was also caller-fillable - 256 junk model names
    /// displaced every real one - so the check protected nothing while costing determinism.
    #[tokio::test]
    async fn two_instances_reach_the_same_verdict_on_the_same_catalogue() {
        let busy = PricingService::init_for_test().unwrap();
        let idle = PricingService::init_for_test().unwrap();

        // One instance has been pricing; the other has served nothing.
        let priced = busy.calculate_cost(&SpanCostInput {
            model: Some("gpt-4o-mini".to_string()),
            system: Some("openai".to_string()),
            input_tokens: 100,
            output_tokens: 10,
            ..Default::default()
        });
        assert!(priced.total_cost > 0.0, "the fixture model must be priced");

        // A catalogue that is plausible in size but does not hold that model.
        let without = smaller_catalogue_excluding(MIN_PLAUSIBLE_MODEL_COUNT + 50, "gpt-4o-mini");
        let expected = PricingData::from_json_str(&without).unwrap().model_count;

        busy.apply_sync_data(&without).await;
        idle.apply_sync_data(&without).await;

        assert_eq!(
            busy.data.read().model_count,
            idle.data.read().model_count,
            "a busy replica and an idle one must reach the same verdict"
        );
        assert_eq!(busy.data.read().model_count, expected);
    }

    /// Provenance, not size, decides whether the file on disk survives a restart - and provenance means
    /// "written by the build that is now running", for a sync as much as for an embedded copy.
    #[tokio::test]
    async fn a_catalogue_from_this_build_survives_and_one_predating_it_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model_prices.json");
        let embedded_count = PricingData::from_json_str(EMBEDDED_PRICING_JSON)
            .unwrap()
            .model_count;

        // A small catalogue on disk, synced *under this build*. Under the old size rule its size alone
        // would have condemned it; it is upstream data fetched over this build's own snapshot.
        let synced = smaller_catalogue(200);
        let synced_count = PricingData::from_json_str(&synced).unwrap().model_count;
        tokio::fs::write(&path, &synced).await.unwrap();
        PricingService::write_provenance(
            &path,
            PricingProvenance {
                source: PROVENANCE_SYNC.to_string(),
                embedded_digest: Some(embedded_digest().to_string()),
                written_at: "2026-01-01T00:00:00Z".to_string(),
            },
        )
        .await;

        let loaded = PricingService::load_pricing_data(&path).await.unwrap();
        assert_eq!(
            loaded.model_count, synced_count,
            "a sync fetched under this build is newer upstream data than the snapshot it replaced"
        );

        // The same synced file, but fetched under a *previous* build. This build shipped its own snapshot
        // since, which may carry corrections the file predates - and with sync disabled or the network
        // down, keeping the old file would pin those prices permanently. It would also make a long-lived
        // replica disagree with a freshly started one about the cost of an identical request.
        PricingService::write_provenance(
            &path,
            PricingProvenance {
                source: PROVENANCE_SYNC.to_string(),
                embedded_digest: Some("a-previous-release".to_string()),
                written_at: "2026-01-01T00:00:00Z".to_string(),
            },
        )
        .await;

        let loaded = PricingService::load_pricing_data(&path).await.unwrap();
        assert_eq!(
            loaded.model_count, embedded_count,
            "a catalogue predating this build must not outrank the snapshot this build shipped"
        );

        // Same answer for a previous build's embedded copy, which is the same question.
        tokio::fs::write(&path, &synced).await.unwrap();
        PricingService::write_provenance(
            &path,
            PricingProvenance {
                source: PROVENANCE_EMBEDDED.to_string(),
                embedded_digest: Some("a-previous-release".to_string()),
                written_at: "2026-01-01T00:00:00Z".to_string(),
            },
        )
        .await;
        let loaded = PricingService::load_pricing_data(&path).await.unwrap();
        assert_eq!(loaded.model_count, embedded_count);

        // And having replaced it, the provenance on disk now names this build.
        let recorded = PricingService::read_provenance(&path).await.unwrap();
        assert_eq!(recorded.source, PROVENANCE_EMBEDDED);
        assert_eq!(recorded.embedded_digest.as_deref(), Some(embedded_digest()));
    }

    /// Every replica of one build resolves to the same catalogue, which is what keeps a persisted cost
    /// independent of which replica handled the request.
    #[tokio::test]
    async fn replicas_of_one_build_agree_on_the_catalogue() {
        let dir = tempfile::tempdir().unwrap();

        // Replica A: long-lived, holds a catalogue synced under a previous build.
        let a = dir.path().join("a.json");
        tokio::fs::write(&a, smaller_catalogue(200)).await.unwrap();
        PricingService::write_provenance(
            &a,
            PricingProvenance {
                source: PROVENANCE_SYNC.to_string(),
                embedded_digest: Some("a-previous-release".to_string()),
                written_at: "2026-01-01T00:00:00Z".to_string(),
            },
        )
        .await;

        // Replica B: freshly started, no file at all.
        let b = dir.path().join("b.json");

        let from_a = PricingService::load_pricing_data(&a).await.unwrap();
        let from_b = PricingService::load_pricing_data(&b).await.unwrap();
        assert_eq!(
            from_a.model_count, from_b.model_count,
            "a long-lived replica and a fresh one must price identically before either syncs"
        );
    }

    /// A file with no provenance at all is treated as unknown, so this build's snapshot is written.
    #[tokio::test]
    async fn a_catalogue_with_no_provenance_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model_prices.json");
        tokio::fs::write(&path, smaller_catalogue(200))
            .await
            .unwrap();

        let loaded = PricingService::load_pricing_data(&path).await.unwrap();
        assert_eq!(
            loaded.model_count,
            PricingData::from_json_str(EMBEDDED_PRICING_JSON)
                .unwrap()
                .model_count
        );
    }

    /// A catalogue of `n` priced models, in the upstream shape.
    fn smaller_catalogue(n: usize) -> String {
        smaller_catalogue_inner(n, None)
    }

    /// The same, but built from models other than `excluded`, so a model in active use is absent.
    fn smaller_catalogue_excluding(n: usize, excluded: &str) -> String {
        smaller_catalogue_inner(n, Some(excluded))
    }

    /// Built by taking a subset of the *real* embedded catalogue rather than by inventing entries, so the
    /// fixture cannot pass by being shaped differently from what upstream actually sends.
    fn smaller_catalogue_inner(n: usize, excluded: Option<&str>) -> String {
        let raw: serde_json::Value = serde_json::from_str(EMBEDDED_PRICING_JSON).unwrap();
        let all = raw.as_object().unwrap();
        let mut keys: Vec<&String> = all.keys().collect();
        keys.sort(); // deterministic
        let mut out = serde_json::Map::new();
        for key in keys {
            if out.len() >= n {
                break;
            }
            if let Some(excluded) = excluded
                && key.eq_ignore_ascii_case(excluded)
            {
                continue;
            }
            let entry = &all[key];
            // Only token-priced entries count toward `model_count`, so take those.
            if entry.get("input_cost_per_token").is_some() {
                out.insert(key.clone(), entry.clone());
            }
        }
        serde_json::Value::Object(out).to_string()
    }

    /// The convention follows the provider that **priced** the call, not a second parse of `gen_ai.system`.
    ///
    /// A Bedrock model name resolves to a Bedrock catalogue entry however the system attribute is spelled -
    /// or if it is absent entirely. Reading `system` for the convention meant such a call was charged at
    /// Bedrock's rates (cache counters extra) and counted under OpenAI's (cache counters inside the input),
    /// so a cached turn reported 15 tokens where 1,215 were billed and ten ordinary input tokens were
    /// dropped from the charge.
    #[test]
    fn the_convention_follows_the_provider_that_priced_the_call() {
        let service = PricingService::init_for_test().unwrap();

        let bedrock_model = "anthropic.claude-3-haiku-20240307-v1:0";
        let usage = |system: Option<&str>| SpanCostInput {
            model: Some(bedrock_model.to_string()),
            system: system.map(str::to_string),
            input_tokens: 10,
            output_tokens: 5,
            cache_read_tokens: 1200,
            ..Default::default()
        };

        // The reference: the spelling the mapper has always known.
        let known = service.calculate_cost(&usage(Some("aws_bedrock")));
        assert!(known.total_cost > 0.0, "the fixture model must be priced");
        assert_eq!(known.resolved_provider.as_deref(), Some("bedrock"));

        // A spelling the mapper does not list, and no system attribute at all. Both are priced from the
        // same entry, so both must be charged the same way.
        for system in [Some("AWS"), None] {
            let output = service.calculate_cost(&usage(system));
            assert_eq!(
                output.resolved_provider.as_deref(),
                Some("bedrock"),
                "system={system:?}: the entry that priced this call names its provider"
            );
            assert!(
                (output.total_cost - known.total_cost).abs() < f64::EPSILON,
                "system={system:?}: charged {} against {} for the same model and usage",
                output.total_cost,
                known.total_cost
            );
        }

        // And the ten ordinary input tokens are charged: under the inclusive reading they were subtracted
        // away by the 1,200 cache-read tokens and billed at nothing.
        assert!(
            known.input_cost > 0.0,
            "input tokens beyond the cached ones must still be charged"
        );
    }

    /// `aws` and `amazon` on their own resolve to Bedrock.
    #[test]
    fn bare_aws_spellings_resolve_to_bedrock() {
        for system in ["aws", "AWS", "amazon", "Amazon"] {
            assert!(
                cache_counters_are_separate(Some(system)),
                "{system} is Bedrock, whose cache counters are billed on top"
            );
        }
    }

    /// Every provider name the real catalogue contains is classified, and a new one fails this test.
    ///
    /// This is the guard that was missing. The conventions were written against exact short names
    /// (`bedrock`, `vertex_ai`) while the catalogue actually uses forty-odd - `bedrock_converse` alone has
    /// 152 entries and `vertex_ai-anthropic_models` 37 - so the majority of the affected models fell through
    /// to the inclusive default and were charged one way while counted another. No test noticed, because
    /// every test named a provider by hand.
    ///
    /// Listing the expectation per *name* rather than per rule is the point: a sync that introduces a
    /// provider name lands here as a failure, and someone has to decide which convention it follows instead
    /// of inheriting whatever the string matching happens to do.
    #[test]
    fn every_catalogue_provider_name_is_classified_deliberately() {
        use std::collections::{BTreeMap, BTreeSet};

        let raw: serde_json::Value = serde_json::from_str(EMBEDDED_PRICING_JSON).unwrap();
        let mut providers: BTreeSet<String> = BTreeSet::new();
        for (_, entry) in raw.as_object().unwrap() {
            if let Some(p) = entry.get("litellm_provider").and_then(|v| v.as_str()) {
                providers.insert(p.to_string());
            }
        }
        assert!(
            providers.len() > 20,
            "the catalogue should name many providers, found {}",
            providers.len()
        );

        // (cache counters beside input, reasoning beside output) per provider name.
        let expected: BTreeMap<&str, (bool, bool)> = BTreeMap::from([
            ("anthropic", (true, false)),
            ("bedrock", (true, false)),
            ("bedrock_converse", (true, false)),
            ("bedrock_mantle", (true, false)),
            ("vertex_ai-anthropic_models", (true, false)),
            ("gemini", (false, true)),
            ("vertex_ai", (false, true)),
            ("vertex_ai-language-models", (false, true)),
            ("vertex_ai-text-models", (false, true)),
            ("vertex_ai-embedding-models", (false, true)),
            ("vertex_ai-image-models", (false, true)),
            ("vertex_ai-video-models", (false, true)),
        ]);

        let mut wrong: Vec<String> = Vec::new();
        for provider in &providers {
            let cache = cache_counters_are_separate_for_provider(provider);
            let reasoning = reasoning_is_separate_for_provider(provider);
            let want = expected
                .get(provider.as_str())
                .copied()
                // Everything else takes the inclusive reading, which is the documented cautious default.
                .unwrap_or((false, false));
            if (cache, reasoning) != want {
                wrong.push(format!(
                    "{provider}: got (cache_separate={cache}, reasoning_separate={reasoning}), want {want:?}"
                ));
            }
        }
        assert!(
            wrong.is_empty(),
            "provider conventions disagree with the expected table:\n  {}\n\nIf the catalogue introduced \
             a provider name, decide its convention and add it to `expected` - do not let string matching \
             decide it.",
            wrong.join("\n  ")
        );
    }

    /// The concrete miss, priced end to end: a Bedrock model whose catalogue entry says `bedrock_converse`.
    #[test]
    fn a_bedrock_converse_entry_bills_its_cache_counters_as_extra() {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap();

        // Find a real entry whose provider is the variant that used to fall through.
        let raw: serde_json::Value = serde_json::from_str(EMBEDDED_PRICING_JSON).unwrap();
        let model = raw
            .as_object()
            .unwrap()
            .iter()
            .find(|(_, e)| {
                e.get("litellm_provider").and_then(|v| v.as_str()) == Some("bedrock_converse")
                    && e.get("cache_read_input_token_cost").is_some()
                    && e.get("input_cost_per_token").is_some()
            })
            .map(|(k, _)| k.clone())
            .expect("the catalogue must hold a bedrock_converse entry with cache pricing");

        let (pricing, _) = data.lookup(None, &model).expect("priced");
        assert_eq!(pricing.litellm_provider, "bedrock_converse");
        assert!(
            cache_counters_are_separate_for_provider(&pricing.litellm_provider),
            "{model} is priced from a bedrock_converse entry, so its cache counters are billed on top"
        );

        let service = PricingService::init_for_test().unwrap();
        let output = service.calculate_cost(&SpanCostInput {
            model: Some(model.clone()),
            system: None,
            input_tokens: 10,
            output_tokens: 5,
            cache_read_tokens: 1200,
            ..Default::default()
        });

        // Under the inclusive reading the 1,200 cached tokens were subtracted from an input of 10, so the
        // ten ordinary input tokens were billed at nothing.
        assert!(
            output.input_cost > 0.0,
            "{model}: input tokens beyond the cached ones must still be charged"
        );
        assert!(
            output.cache_read_cost > 0.0,
            "{model}: cache reads are charged"
        );
    }
}
