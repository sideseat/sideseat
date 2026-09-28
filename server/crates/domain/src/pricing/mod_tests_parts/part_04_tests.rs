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

    let from_a = PricingService::load_pricing_data(&a, &TestClock)
        .await
        .unwrap();
    let from_b = PricingService::load_pricing_data(&b, &TestClock)
        .await
        .unwrap();
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

    let loaded = PricingService::load_pricing_data(&path, &TestClock)
        .await
        .unwrap();
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

    // Names the catalogue does not use *yet*, pinned so a rename upstream cannot quietly change the
    // convention. `amazon_bedrock_converse` is the shape a prefix test would have missed.
    for future in [
        "amazon_bedrock_converse",
        "aws_bedrock",
        "bedrock_converse_v2",
    ] {
        assert!(
            cache_counters_are_separate_for_provider(future),
            "{future} is Bedrock, whose cache counters are billed on top"
        );
    }

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
