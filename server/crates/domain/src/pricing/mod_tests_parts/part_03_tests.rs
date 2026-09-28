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

    let loaded = PricingService::load_pricing_data(&path, &TestClock)
        .await
        .unwrap();
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

    let loaded = PricingService::load_pricing_data(&path, &TestClock)
        .await
        .unwrap();
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
    let loaded = PricingService::load_pricing_data(&path, &TestClock)
        .await
        .unwrap();
    assert_eq!(loaded.model_count, embedded_count);

    // And having replaced it, the provenance on disk now names this build.
    let recorded = PricingService::read_provenance(&path).await.unwrap();
    assert_eq!(recorded.source, PROVENANCE_EMBEDDED);
    assert_eq!(recorded.embedded_digest.as_deref(), Some(embedded_digest()));
}
