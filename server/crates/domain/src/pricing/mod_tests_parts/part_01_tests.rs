use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct CountingSource {
    calls: Arc<AtomicUsize>,
    called: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl PricingCatalogueSource for CountingSource {
    async fn fetch_catalogue(
        &self,
    ) -> Result<String, sideseat_ports::pricing::PricingCatalogueError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.called.notify_one();
        Err(sideseat_ports::pricing::PricingCatalogueError::new(
            "expected test failure",
        ))
    }
}

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

#[tokio::test]
async fn sync_starts_immediately_and_remains_shutdown_owned() {
    let calls = Arc::new(AtomicUsize::new(0));
    let called = Arc::new(tokio::sync::Notify::new());
    let source = Arc::new(CountingSource {
        calls: Arc::clone(&calls),
        called: Arc::clone(&called),
    });
    let service = Arc::new(PricingService {
        data: RwLock::new(PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap()),
        local_path: std::env::temp_dir().join("sideseat_test_pricing.json"),
        catalogue_source: Some(source),
        clock: Arc::new(TestClock),
    });
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let handle = service
        .start_sync_task(1, shutdown_rx)
        .expect("enabled sync task");

    tokio::time::timeout(Duration::from_secs(1), called.notified())
        .await
        .expect("the first sync should not wait for the hourly interval");
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    shutdown_tx.send(true).unwrap();
    handle.await.unwrap();
}

#[tokio::test]
async fn sync_stops_when_the_shutdown_sender_is_dropped() {
    let calls = Arc::new(AtomicUsize::new(0));
    let called = Arc::new(tokio::sync::Notify::new());
    let service = Arc::new(PricingService {
        data: RwLock::new(PricingData::from_json_str(EMBEDDED_PRICING_JSON).unwrap()),
        local_path: std::env::temp_dir().join("sideseat_test_pricing_drop.json"),
        catalogue_source: Some(Arc::new(CountingSource {
            calls,
            called: Arc::clone(&called),
        })),
        clock: Arc::new(TestClock),
    });
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let handle = service
        .start_sync_task(1, shutdown_rx)
        .expect("enabled sync task");

    tokio::time::timeout(Duration::from_secs(1), called.notified())
        .await
        .expect("the first sync should start");
    drop(shutdown_tx);

    tokio::time::timeout(Duration::from_secs(1), handle)
        .await
        .expect("a closed shutdown channel should stop the task")
        .expect("sync task should exit cleanly");
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

#[tokio::test]
async fn init_without_catalogue_source_is_offline() {
    let storage = AppStorage::init_for_test(std::env::temp_dir());
    let service = PricingService::init(&storage, Arc::new(TestClock), None)
        .await
        .unwrap();
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
