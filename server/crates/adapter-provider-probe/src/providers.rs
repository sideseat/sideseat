//! The probe for each provider key.

use crate::http::{Call, Plan};
use crate::{Endpoints, ProbeError, ProbeTarget, aws, azure, google, http};

const ANTHROPIC_VERSION: &str = "2023-06-01";
const PERPLEXITY_MODEL: &str = "sonar";

/// Providers whose OpenAI-compatible `/models` listing requires the key.
const OPENAI_COMPATIBLE: &[(&str, &str)] = &[
    ("openai", "https://api.openai.com/v1/models"),
    ("xai", "https://api.x.ai/v1/models"),
    ("mistral", "https://api.mistral.ai/v1/models"),
    ("groq", "https://api.groq.com/openai/v1/models"),
    ("deepseek", "https://api.deepseek.com/models"),
    ("together", "https://api.together.xyz/v1/models"),
    ("fireworks", "https://api.fireworks.ai/inference/v1/models"),
    ("cerebras", "https://api.cerebras.ai/v1/models"),
];

pub(crate) async fn probe(
    target: ProbeTarget<'_>,
    endpoints: &Endpoints,
) -> Result<Option<String>, ProbeError> {
    let key = target.secret();
    if let Some((_, url)) = OPENAI_COMPATIBLE
        .iter()
        .find(|(provider, _)| *provider == target.provider)
    {
        return http::run(Plan::List(Call::get(endpoints.url(url)).bearer(key))).await;
    }

    let plan = match target.provider {
        "anthropic" => Plan::List(
            Call::get(endpoints.url("https://api.anthropic.com/v1/models"))
                .header("x-api-key", key.to_string())
                .header("anthropic-version", ANTHROPIC_VERSION.to_string()),
        ),
        "gemini" => Plan::List(
            Call::get(endpoints.url("https://generativelanguage.googleapis.com/v1beta/models"))
                .header("x-goog-api-key", key.to_string()),
        ),
        "cohere" => {
            Plan::List(Call::get(endpoints.url("https://api.cohere.com/v1/models")).bearer(key))
        }
        // The model listing is public, so only the key endpoint proves the key.
        "openrouter" => {
            Plan::List(Call::get(endpoints.url("https://openrouter.ai/api/v1/key")).bearer(key))
        }
        "perplexity" => Plan::Complete(
            Call::chat_completion(
                endpoints.url("https://api.perplexity.ai/chat/completions"),
                PERPLEXITY_MODEL,
            )
            .bearer(key),
        ),
        "ollama" => {
            let base = target.endpoint.unwrap_or("http://localhost:11434");
            Plan::List(Call::get(format!(
                "{}/v1/models",
                base.trim_end_matches('/')
            )))
        }
        "custom" => {
            let base = target.endpoint.ok_or_else(|| {
                ProbeError::Configuration("endpoint_url is required for a custom provider".into())
            })?;
            Plan::List(Call::get(format!("{}/models", base.trim_end_matches('/'))).bearer(key))
        }
        "azure-ai-foundry" => azure::plan(target).await?,
        "vertex-ai" => google::vertex_plan(target, endpoints).await?,
        "bedrock" => return aws::probe(target, endpoints).await,
        unknown => return Err(ProbeError::UnknownProvider(unknown.to_string())),
    };
    http::run(plan).await
}
