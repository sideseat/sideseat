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
    // `contains`, not `starts_with`: the catalogue already spells it `bedrock`, `bedrock_converse` and
    // `bedrock_mantle`, and a future `amazon_bedrock_converse` is the same API with the same convention. A
    // prefix test would send it to the inclusive default and undercharge every cached call on it.
    provider.contains("bedrock") || vendor_of(provider) == Vendor::Anthropic
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
    match builtin_provider(&normalised) {
        "" => {
            // A framework naming *itself* here is declared by the asset that owns that framework - a statement
            // about a producer rather than about the catalogue's vocabulary. Consulted **after** the table
            // above, not before: an alias then cannot shadow a provider's own spelling, whatever an asset
            // declares. Compilation refuses such a declaration as well, but the ordering is what makes it
            // impossible rather than merely caught.
            crate::rules::ruleset()
                .provider_aliases
                .get(&normalised)
                .map(String::as_str)
                .unwrap_or("")
        }
        found => found,
    }
}

/// The catalogue's own provider vocabulary: spellings of a provider, mapped to the name the catalogue uses.
///
/// Separate from the entry point because it must be callable **while the ruleset is compiling** - that is where
/// a declared alias is checked against it, and re-entering `ruleset()` there would deadlock on its own lock.
pub(crate) fn builtin_provider(normalised: &str) -> &'static str {
    match normalised {
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

        // Anything else, framework names included: no provider, so the catalogue is searched by *model* name
        // instead. There used to be a list of framework slugs here answering the same `""`, which made this
        // function the last place in the server that named one - and it said nothing the catch-all does not.
        // A framework is not a provider because it is not in the provider table, not because it is listed as
        // an exception to it.
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

    catalogue_source: Option<Arc<dyn PricingCatalogueSource>>,
    clock: Arc<dyn Clock>,
}

impl PricingService {
    /// Initialize pricing service
    ///
    /// Loading priority:
    /// 1. Try local file from data directory
    /// 2. If local valid and has >= models than embedded, use it
    /// 3. Otherwise, use embedded data and save to disk
    ///
    /// An optional catalogue source is retained for a runtime-owned background sync task.
    pub async fn init(
        storage: &AppStorage,
        clock: Arc<dyn Clock>,
        catalogue_source: Option<Arc<dyn PricingCatalogueSource>>,
    ) -> Result<Arc<Self>, PricingError> {
        let local_path = storage.data_dir().join(PRICING_FILE_NAME);

        let data = Self::load_pricing_data(&local_path, clock.as_ref()).await?;

        Ok(Arc::new(Self {
            data: RwLock::new(data),
            local_path,
            catalogue_source,
            clock,
        }))
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
    async fn load_pricing_data(
        local_path: &Path,
        clock: &dyn Clock,
    ) -> Result<PricingData, PricingError> {
        if !local_path.exists() {
            return Self::load_embedded_with_save(local_path, clock).await;
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
                    Self::load_embedded_with_save(local_path, clock).await
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to load local pricing, using embedded");
                Self::load_embedded_with_save(local_path, clock).await
            }
        }
    }

    /// Load embedded pricing data and save to disk (best-effort)
    async fn load_embedded_with_save(
        local_path: &Path,
        clock: &dyn Clock,
    ) -> Result<PricingData, PricingError> {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON)?;
        if let Err(e) = Self::save_to_file(local_path, EMBEDDED_PRICING_JSON).await {
            tracing::warn!(error = %e, "Failed to save pricing to disk (continuing with embedded)");
        } else {
            Self::write_provenance(
                local_path,
                PricingProvenance {
                    source: PROVENANCE_EMBEDDED.to_string(),
                    embedded_digest: Some(embedded_digest().to_string()),
                    written_at: clock.now().to_rfc3339(),
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
    #[cfg(any(test, feature = "test-support"))]
    pub fn init_for_test() -> Result<Self, PricingError> {
        let data = PricingData::from_json_str(EMBEDDED_PRICING_JSON)?;
        Ok(Self {
            data: RwLock::new(data),
            local_path: std::env::temp_dir().join("sideseat_test_pricing.json"),
            catalogue_source: None,
            clock: Arc::new(TestClock),
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
        let Some(source) = &self.catalogue_source else {
            return;
        };

        match source.fetch_catalogue().await {
            Ok(text) => self.apply_sync_data(&text).await,
            Err(error) => tracing::warn!(%error, "Pricing catalogue sync failed"),
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
                    written_at: self.clock.now().to_rfc3339(),
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

    /// Start the background catalogue sync task.
    ///
    /// `sync_hours = 0` or an absent catalogue source disables the task. Enabled intervals are clamped to
    /// [`MIN_SYNC_HOURS`], and the first sync starts immediately.
    pub fn start_sync_task(
        self: &Arc<Self>,
        sync_hours: u64,
        mut shutdown_rx: watch::Receiver<bool>,
    ) -> Option<JoinHandle<()>> {
        if sync_hours == 0 || self.catalogue_source.is_none() {
            return None;
        }

        let sync_hours = sync_hours.max(MIN_SYNC_HOURS);
        let interval = Duration::from_secs(sync_hours.saturating_mul(3600));
        let service = Arc::clone(self);

        Some(tokio::spawn(async move {
            let mut timer = tokio::time::interval(interval);
            // A fresh catalogue fetch supersedes missed intervals; catch-up bursts only duplicate traffic.
            timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                tokio::select! {
                    biased;
                    changed = shutdown_rx.changed() => {
                        if changed.is_err() || *shutdown_rx.borrow() {
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
