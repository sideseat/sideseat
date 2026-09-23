//! Test-connection logic for provider credentials.
//!
//! Uses the sideseat SDK to verify API credentials by attempting a
//! lightweight API call (list_models, then fallback to complete with max_tokens=1).

mod azure;

use std::time::Instant;

use async_trait::async_trait;

use sideseat_domain::providers::{CredentialConnectionTester, ResolvedCredential, TestResult};

use self::azure::{get_managed_identity_token, resolve_base_url};

pub struct SdkCredentialConnectionTester;

#[async_trait]
impl CredentialConnectionTester for SdkCredentialConnectionTester {
    async fn test(&self, resolved: &ResolvedCredential, secret: Option<&str>) -> TestResult {
        test_credential(resolved, secret).await
    }
}

/// Test a resolved credential by attempting to reach its provider API.
async fn test_credential(resolved: &ResolvedCredential, secret: Option<&str>) -> TestResult {
    let start = Instant::now();

    let result = attempt_test(resolved, secret).await;

    let latency_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(model_hint) => TestResult {
            success: true,
            latency_ms,
            error: None,
            model_hint,
        },
        Err(e) => TestResult {
            success: false,
            latency_ms,
            error: Some(e),
            model_hint: None,
        },
    }
}

/// Inner test attempt — returns Ok(model_hint) or Err(error_message).
async fn attempt_test(
    resolved: &ResolvedCredential,
    secret: Option<&str>,
) -> Result<Option<String>, String> {
    use sideseat::provider::ChatProvider;
    use sideseat::test_models;

    let timeout =
        tokio::time::Duration::from_secs(sideseat_core::constants::CRED_TEST_TIMEOUT_SECS);
    let extra = resolved.extra_config.as_ref();
    let api_key = secret.unwrap_or("");
    let endpoint = resolved.endpoint_url.as_deref();

    match resolved.provider_key.as_str() {
        "anthropic" => {
            use sideseat::providers::AnthropicProvider;
            let p = AnthropicProvider::new(api_key);
            try_test_provider(Box::new(p), test_models::ANTHROPIC.to_string(), timeout).await
        }
        "openai" => {
            let variant = extra
                .and_then(|e| e.get("api_variant"))
                .and_then(|v| v.as_str())
                .unwrap_or("chat_completions");

            if variant == "responses_api" {
                use sideseat::providers::OpenAIResponsesProvider;
                let p = OpenAIResponsesProvider::new(api_key);
                try_test_provider(Box::new(p), test_models::OPENAI.to_string(), timeout).await
            } else {
                use sideseat::providers::OpenAIChatProvider;
                let p = OpenAIChatProvider::new(api_key);
                try_test_provider(Box::new(p), test_models::OPENAI.to_string(), timeout).await
            }
        }
        "gemini" => {
            let variant = extra
                .and_then(|e| e.get("api_variant"))
                .and_then(|v| v.as_str())
                .unwrap_or("standard");

            if variant == "interactions" {
                use sideseat::providers::GeminiInteractionsProvider;
                let p = GeminiInteractionsProvider::new(api_key);
                try_test_provider(Box::new(p), test_models::GEMINI.to_string(), timeout).await
            } else {
                use sideseat::providers::{GeminiAuth, GeminiProvider};
                let p = GeminiProvider::new(GeminiAuth::ApiKey(api_key.to_string()));
                try_test_provider(Box::new(p), test_models::GEMINI.to_string(), timeout).await
            }
        }
        "cohere" => {
            use sideseat::providers::CohereProvider;
            let p = CohereProvider::new(api_key);
            try_test_provider(Box::new(p), test_models::COHERE.to_string(), timeout).await
        }
        "groq" => {
            use sideseat::providers::OpenAIChatProvider;
            let p = OpenAIChatProvider::for_groq(api_key);
            try_test_provider(Box::new(p), test_models::GROQ.to_string(), timeout).await
        }
        "deepseek" => {
            use sideseat::providers::OpenAIChatProvider;
            let p = OpenAIChatProvider::for_deepseek(api_key);
            try_test_provider(Box::new(p), test_models::DEEPSEEK.to_string(), timeout).await
        }
        "xai" => {
            use sideseat::providers::XAIProvider;
            let p = XAIProvider::new(api_key);
            try_test_provider(Box::new(p), test_models::XAI.to_string(), timeout).await
        }
        "mistral" => {
            use sideseat::providers::MistralProvider;
            let p = MistralProvider::new(api_key);
            try_test_provider(Box::new(p), test_models::MISTRAL.to_string(), timeout).await
        }
        "together" => {
            use sideseat::providers::OpenAIChatProvider;
            let p = OpenAIChatProvider::for_together(api_key);
            try_test_provider(Box::new(p), test_models::TOGETHER.to_string(), timeout).await
        }
        "fireworks" => {
            use sideseat::providers::OpenAIChatProvider;
            let p = OpenAIChatProvider::for_fireworks(api_key);
            try_test_provider(Box::new(p), test_models::FIREWORKS.to_string(), timeout).await
        }
        "cerebras" => {
            use sideseat::providers::OpenAIChatProvider;
            let p = OpenAIChatProvider::for_cerebras(api_key);
            try_test_provider(Box::new(p), test_models::CEREBRAS.to_string(), timeout).await
        }
        "perplexity" => {
            use sideseat::providers::OpenAIChatProvider;
            let p = OpenAIChatProvider::for_perplexity(api_key);
            try_test_provider(Box::new(p), test_models::PERPLEXITY.to_string(), timeout).await
        }
        "openrouter" => {
            use sideseat::providers::OpenAIChatProvider;
            let p = OpenAIChatProvider::for_openrouter(api_key);
            try_test_provider(Box::new(p), test_models::OPENROUTER.to_string(), timeout).await
        }
        "ollama" => {
            use sideseat::providers::OpenAIChatProvider;
            let ep = endpoint.unwrap_or("http://localhost:11434");
            let p = OpenAIChatProvider::for_ollama(Some(ep));
            try_test_provider(Box::new(p), test_models::OLLAMA.to_string(), timeout).await
        }
        "azure-ai-foundry" => {
            use sideseat::providers::OpenAIChatProvider;
            let raw_ep = endpoint
                .ok_or_else(|| "endpoint_url is required for Azure AI Foundry".to_string())?;
            let deployment = extra
                .and_then(|e| e.get("deployment_name"))
                .and_then(|v| v.as_str());
            let api_variant = extra
                .and_then(|e| e.get("api_variant"))
                .and_then(|v| v.as_str())
                .unwrap_or("standard");
            let auth_mode = extra
                .and_then(|e| e.get("auth_mode"))
                .and_then(|v| v.as_str())
                .unwrap_or("api_key");

            // Normalise whatever endpoint format the user stored into the base
            // URL that OpenAIChatProvider::with_api_base() expects.
            let base = resolve_base_url(raw_ep, deployment, api_variant);
            let model = deployment.unwrap_or(test_models::OPENAI).to_string();

            let p: Box<dyn ChatProvider + Send> = match auth_mode {
                "managed_identity" => {
                    let token = get_managed_identity_token("https://cognitiveservices.azure.com")
                        .await
                        .map_err(|e| format!("Azure Managed Identity failed: {e}"))?;
                    Box::new(OpenAIChatProvider::new(&token).with_api_base(&base))
                }
                _ => Box::new(OpenAIChatProvider::new(api_key).with_api_base(&base)),
            };
            try_test_provider(p, model, timeout).await
        }
        "bedrock" => {
            use sideseat::providers::BedrockProvider;

            let auth_mode = extra
                .and_then(|e| e.get("auth_mode"))
                .and_then(|v| v.as_str())
                .unwrap_or("bearer");

            let region = extra
                .and_then(|e| e.get("region"))
                .and_then(|v| v.as_str())
                .unwrap_or("us-east-1");

            let model = if region.starts_with("eu-") {
                format!("eu.{}", test_models::BEDROCK)
            } else {
                test_models::BEDROCK.to_string()
            };

            // Use try_test_provider for all Bedrock modes: list_models() checks the
            // credential without requiring any specific model to be enabled in the account.
            // Falling back to complete + ModelNotFound-as-success handles accounts where
            // ListFoundationModels isn't permitted but InvokeModel is.
            match auth_mode {
                "access_keys" => {
                    let creds: serde_json::Value = serde_json::from_str(api_key)
                        .map_err(|e| format!("Invalid Bedrock credentials JSON: {}", e))?;
                    let access_key_id = creds
                        .get("access_key_id")
                        .and_then(|v| v.as_str())
                        .ok_or("Missing access_key_id")?
                        .to_string();
                    let secret_key_val = creds
                        .get("secret_access_key")
                        .and_then(|v| v.as_str())
                        .ok_or("Missing secret_access_key")?
                        .to_string();
                    let session_token = creds
                        .get("session_token")
                        .and_then(|v| v.as_str())
                        .map(ToString::to_string);
                    let role_arn = extra
                        .and_then(|e| e.get("role_arn"))
                        .and_then(|v| v.as_str());
                    let external_id = extra
                        .and_then(|e| e.get("external_id"))
                        .and_then(|v| v.as_str())
                        .map(ToString::to_string);

                    let p: Box<dyn ChatProvider + Send> = if let Some(arn) = role_arn {
                        Box::new(
                            BedrockProvider::from_static_assume_role(
                                access_key_id,
                                secret_key_val,
                                session_token,
                                arn,
                                external_id,
                                region,
                            )
                            .await
                            .map_err(|e| format!("AssumeRole failed: {e}"))?,
                        )
                    } else {
                        Box::new(
                            BedrockProvider::with_static_credentials(
                                access_key_id,
                                secret_key_val,
                                session_token,
                                region,
                            )
                            .await
                            .map_err(|e| format!("Failed to initialize Bedrock: {e}"))?,
                        )
                    };
                    try_test_provider(p, model, timeout).await
                }
                "iam_role" | "iam_ambient" => {
                    let role_arn = extra
                        .and_then(|e| e.get("role_arn"))
                        .and_then(|v| v.as_str());
                    let external_id = extra
                        .and_then(|e| e.get("external_id"))
                        .and_then(|v| v.as_str())
                        .map(ToString::to_string);

                    let p: Box<dyn ChatProvider + Send> = if let Some(arn) = role_arn {
                        Box::new(
                            BedrockProvider::from_ambient_assume_role(arn, external_id, region)
                                .await
                                .map_err(|e| format!("Ambient AssumeRole failed: {e}"))?,
                        )
                    } else {
                        Box::new(
                            BedrockProvider::from_env(region)
                                .await
                                .map_err(|e| format!("Ambient IAM failed: {e}"))?,
                        )
                    };
                    try_test_provider(p, model, timeout).await
                }
                _ => {
                    // Default: bearer token
                    let p: Box<dyn ChatProvider + Send> =
                        Box::new(BedrockProvider::with_api_key(api_key, region));
                    try_test_provider(p, model, timeout).await
                }
            }
        }
        "vertex-ai" => {
            use sideseat::providers::GeminiProvider;
            let location = extra
                .and_then(|e| e.get("location"))
                .and_then(|v| v.as_str())
                .unwrap_or("us-central1");
            let auth_mode = extra
                .and_then(|e| e.get("auth_mode"))
                .and_then(|v| v.as_str())
                .unwrap_or("bearer");

            match auth_mode {
                "adc" => {
                    use sideseat::providers::GcpAdcTokenProvider;
                    let project_id = extra
                        .and_then(|e| e.get("project_id"))
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "project_id is required for Vertex AI".to_string())?;
                    let token_provider = GcpAdcTokenProvider::try_new()
                        .await
                        .map_err(|e| format!("GCP ADC failed: {e}"))?;
                    let p = GeminiProvider::from_vertex_with_token_provider(
                        project_id,
                        location,
                        std::sync::Arc::new(token_provider),
                    );
                    try_test_provider(Box::new(p), test_models::GEMINI.to_string(), timeout).await
                }
                "service_account" => {
                    use gcp_auth::TokenProvider as _;
                    let sa_json =
                        secret.ok_or_else(|| "Service account JSON is required".to_string())?;
                    // project_id: extra_config takes precedence, fall back to field in the JSON
                    let project_id = extra
                        .and_then(|e| e.get("project_id"))
                        .and_then(|v| v.as_str())
                        .map(ToString::to_string)
                        .or_else(|| {
                            serde_json::from_str::<serde_json::Value>(sa_json)
                                .ok()
                                .and_then(|v| {
                                    v.get("project_id")
                                        .and_then(|p| p.as_str())
                                        .map(ToString::to_string)
                                })
                        })
                        .ok_or_else(|| {
                            "project_id not found in extra_config or service account JSON"
                                .to_string()
                        })?;
                    let account = gcp_auth::CustomServiceAccount::from_json(sa_json)
                        .map_err(|e| format!("Invalid service account credentials: {e}"))?;
                    let token = account
                        .token(&["https://www.googleapis.com/auth/cloud-platform"])
                        .await
                        .map_err(|e| format!("Failed to obtain GCP token: {e}"))?;
                    let p = GeminiProvider::from_vertex(&project_id, location, token.as_str());
                    try_test_provider(Box::new(p), test_models::GEMINI.to_string(), timeout).await
                }
                _ => {
                    // Default: bearer token (secret is used directly as the OAuth token)
                    let project_id = extra
                        .and_then(|e| e.get("project_id"))
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| "project_id is required for Vertex AI".to_string())?;
                    let p = GeminiProvider::from_vertex(project_id, location, api_key);
                    try_test_provider(Box::new(p), test_models::GEMINI.to_string(), timeout).await
                }
            }
        }
        "custom" => {
            use sideseat::providers::OpenAIChatProvider;
            let ep = endpoint
                .ok_or_else(|| "endpoint_url is required for custom provider".to_string())?;
            let key = if api_key.is_empty() { "none" } else { api_key };
            let p = OpenAIChatProvider::new(key).with_api_base(ep);
            try_test_provider(Box::new(p), test_models::OPENAI.to_string(), timeout).await
        }
        unknown => Err(format!("Unknown provider: {}", unknown)),
    }
}

/// Try list_models first; fall back to complete with max_tokens=1.
/// ModelNotFound from complete = success (auth worked).
async fn try_test_provider(
    provider: Box<dyn sideseat::provider::ChatProvider + Send>,
    fallback_model: String,
    timeout: tokio::time::Duration,
) -> Result<Option<String>, String> {
    use sideseat::error::ProviderError;
    use sideseat::types::{Message, ProviderConfig};

    // 1. Try list_models
    match tokio::time::timeout(timeout, provider.list_models()).await {
        Ok(Ok(models)) => {
            return Ok(models.into_iter().next().map(|m| m.id));
        }
        Ok(Err(ProviderError::Unsupported(_))) => {
            // Not supported, fall through to complete
        }
        Ok(Err(_)) => {
            // Other error — fall through; complete will give a clearer result
        }
        Err(_) => {
            return Err("Connection timed out".to_string());
        }
    }

    // 2. Fall back to complete with max_tokens=1
    let config = ProviderConfig {
        model: fallback_model,
        max_tokens: Some(1),
        ..Default::default()
    };

    match tokio::time::timeout(
        timeout,
        provider.complete(vec![Message::user("Hello")], config),
    )
    .await
    {
        Ok(Ok(resp)) => Ok(resp.model),
        Ok(Err(ProviderError::ModelNotFound { .. })) => Ok(None),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("Connection timed out".to_string()),
    }
}
