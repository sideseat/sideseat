//! Provider catalog — static registry of known LLM providers and their env var mappings.
//!
//! This catalog is used for:
//! - Validating `provider_key` on credential creation
//! - Auto-scanning env vars for known API keys

/// A known provider entry
pub struct ProviderCatalogEntry {
    pub key: &'static str,
    pub display_name: &'static str,
    pub supports_endpoint_override: bool,
}

/// An env var → provider mapping for auto-scanning
pub struct EnvMapping {
    pub var_name: &'static str,
    pub provider_key: &'static str,
    pub display_name: &'static str,
}

/// All known provider keys. Used for validating `provider_key` on credential creation.
pub static KNOWN_PROVIDERS: &[ProviderCatalogEntry] = &[
    ProviderCatalogEntry {
        key: "anthropic",
        display_name: "Anthropic",
        supports_endpoint_override: false,
    },
    ProviderCatalogEntry {
        key: "openai",
        display_name: "OpenAI",
        supports_endpoint_override: false,
    },
    ProviderCatalogEntry {
        key: "gemini",
        display_name: "Google Gemini",
        supports_endpoint_override: false,
    },
    ProviderCatalogEntry {
        key: "vertex-ai",
        display_name: "Vertex AI",
        supports_endpoint_override: false,
    },
    ProviderCatalogEntry {
        key: "azure-ai-foundry",
        display_name: "Azure AI Foundry",
        supports_endpoint_override: true,
    },
    ProviderCatalogEntry {
        key: "bedrock",
        display_name: "Amazon Bedrock",
        supports_endpoint_override: false,
    },
    ProviderCatalogEntry {
        key: "cohere",
        display_name: "Cohere",
        supports_endpoint_override: false,
    },
    ProviderCatalogEntry {
        key: "groq",
        display_name: "Groq",
        supports_endpoint_override: false,
    },
    ProviderCatalogEntry {
        key: "deepseek",
        display_name: "DeepSeek",
        supports_endpoint_override: false,
    },
    ProviderCatalogEntry {
        key: "xai",
        display_name: "xAI (Grok)",
        supports_endpoint_override: false,
    },
    ProviderCatalogEntry {
        key: "mistral",
        display_name: "Mistral AI",
        supports_endpoint_override: false,
    },
    ProviderCatalogEntry {
        key: "together",
        display_name: "Together AI",
        supports_endpoint_override: false,
    },
    ProviderCatalogEntry {
        key: "fireworks",
        display_name: "Fireworks AI",
        supports_endpoint_override: false,
    },
    ProviderCatalogEntry {
        key: "cerebras",
        display_name: "Cerebras",
        supports_endpoint_override: false,
    },
    ProviderCatalogEntry {
        key: "perplexity",
        display_name: "Perplexity",
        supports_endpoint_override: false,
    },
    ProviderCatalogEntry {
        key: "openrouter",
        display_name: "OpenRouter",
        supports_endpoint_override: false,
    },
    ProviderCatalogEntry {
        key: "ollama",
        display_name: "Ollama",
        supports_endpoint_override: true,
    },
    ProviderCatalogEntry {
        key: "custom",
        display_name: "Custom (OpenAI-compatible)",
        supports_endpoint_override: true,
    },
];

/// Env var → provider mappings for auto-scanning.
///
/// Notes:
/// - Gemini: GEMINI_API_KEY and GOOGLE_API_KEY are treated as the same key.
///   Both are checked; first found wins. The other is skipped via dedup logic.
/// - Bedrock: bearer token detection via BEDROCK_API_KEY or AWS_BEARER_TOKEN_BEDROCK.
/// - Vertex AI / Azure AI Foundry: NOT scanned (require project_id+location/endpoint — can't auto-detect).
pub static ENV_MAPPINGS: &[EnvMapping] = &[
    EnvMapping {
        var_name: "ANTHROPIC_API_KEY",
        provider_key: "anthropic",
        display_name: "Anthropic (from env)",
    },
    EnvMapping {
        var_name: "OPENAI_API_KEY",
        provider_key: "openai",
        display_name: "OpenAI (from env)",
    },
    EnvMapping {
        var_name: "GEMINI_API_KEY",
        provider_key: "gemini",
        display_name: "Google Gemini (from env)",
    },
    // GOOGLE_API_KEY is treated as Gemini key; skipped if GEMINI_API_KEY already found
    EnvMapping {
        var_name: "GOOGLE_API_KEY",
        provider_key: "gemini",
        display_name: "Google Gemini (from env)",
    },
    EnvMapping {
        var_name: "COHERE_API_KEY",
        provider_key: "cohere",
        display_name: "Cohere (from env)",
    },
    EnvMapping {
        var_name: "GROQ_API_KEY",
        provider_key: "groq",
        display_name: "Groq (from env)",
    },
    EnvMapping {
        var_name: "DEEPSEEK_API_KEY",
        provider_key: "deepseek",
        display_name: "DeepSeek (from env)",
    },
    EnvMapping {
        var_name: "XAI_API_KEY",
        provider_key: "xai",
        display_name: "xAI (from env)",
    },
    EnvMapping {
        var_name: "MISTRAL_API_KEY",
        provider_key: "mistral",
        display_name: "Mistral AI (from env)",
    },
    EnvMapping {
        var_name: "TOGETHER_API_KEY",
        provider_key: "together",
        display_name: "Together AI (from env)",
    },
    EnvMapping {
        var_name: "FIREWORKS_API_KEY",
        provider_key: "fireworks",
        display_name: "Fireworks AI (from env)",
    },
    EnvMapping {
        var_name: "CEREBRAS_API_KEY",
        provider_key: "cerebras",
        display_name: "Cerebras (from env)",
    },
    EnvMapping {
        var_name: "PERPLEXITY_API_KEY",
        provider_key: "perplexity",
        display_name: "Perplexity (from env)",
    },
    EnvMapping {
        var_name: "OPENROUTER_API_KEY",
        provider_key: "openrouter",
        display_name: "OpenRouter (from env)",
    },
    EnvMapping {
        var_name: "BEDROCK_API_KEY",
        provider_key: "bedrock",
        display_name: "Amazon Bedrock (from env)",
    },
    EnvMapping {
        var_name: "AWS_BEARER_TOKEN_BEDROCK",
        provider_key: "bedrock",
        display_name: "Amazon Bedrock (from env)",
    },
];

/// Check if a provider key is valid
pub fn is_known_provider(key: &str) -> bool {
    KNOWN_PROVIDERS.iter().any(|p| p.key == key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_known_providers_not_empty() {
        assert!(!KNOWN_PROVIDERS.is_empty());
    }

    #[test]
    fn test_env_mappings_not_empty() {
        assert!(!ENV_MAPPINGS.is_empty());
    }

    #[test]
    fn test_is_known_provider() {
        assert!(is_known_provider("anthropic"));
        assert!(is_known_provider("openai"));
        assert!(is_known_provider("custom"));
        assert!(!is_known_provider("unknown-provider"));
    }

    #[test]
    fn test_env_mappings_reference_known_providers() {
        for mapping in ENV_MAPPINGS {
            assert!(
                is_known_provider(mapping.provider_key),
                "Env mapping references unknown provider: {}",
                mapping.provider_key
            );
        }
    }
}
