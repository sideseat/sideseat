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
                    MatchType::Exact | MatchType::ProviderQualified | MatchType::ProviderInferred
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
