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

pub(super) fn map_system_to_litellm_provider(system: &str) -> &'static str {
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
pub(super) fn normalize_model_name(model: &str) -> &str {
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
pub(super) fn strip_openrouter_routing_suffix(model: &str) -> &str {
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
pub(super) fn extract_litellm_colon_prefix(model: &str) -> Option<(&str, &str)> {
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
pub(super) fn strip_bedrock_region_prefix(model: &str) -> Option<&str> {
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
pub(super) fn strip_date_suffix(model: &str) -> String {
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
pub(super) fn extract_finetune_base_model(model: &str) -> Option<&str> {
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
pub(super) fn extract_vertex_resource_model(model: &str) -> Option<&str> {
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
pub(super) fn strip_replicate_version(model: &str) -> Option<&str> {
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
