//! Vertex AI: OAuth tokens and a one-token Gemini request.
//!
//! Vertex has no per-project model listing that proves access, so the probe generates one token.

use gcp_auth::TokenProvider;
use serde_json::{Value, json};

use crate::http::{Call, Plan};
use crate::{Endpoints, ProbeError, ProbeTarget};

const MODEL: &str = "gemini-2.5-flash";
const SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

pub(crate) async fn vertex_plan(
    target: ProbeTarget<'_>,
    endpoints: &Endpoints,
) -> Result<Plan, ProbeError> {
    let location = target.option("location").unwrap_or("us-central1");
    let (token, project) = match target.option("auth_mode") {
        Some("adc") => {
            let provider = gcp_auth::provider()
                .await
                .map_err(|e| ProbeError::Configuration(format!("GCP ADC: {e}")))?;
            (token(provider.as_ref()).await?, required_project(target)?)
        }
        Some("service_account") => {
            let json = target.secret.ok_or_else(|| {
                ProbeError::Configuration("service account JSON is required".into())
            })?;
            let account = gcp_auth::CustomServiceAccount::from_json(json).map_err(|e| {
                ProbeError::Configuration(format!("invalid service account credentials: {e}"))
            })?;
            let project = target
                .option("project_id")
                .map(ToString::to_string)
                .or_else(|| account.project_id().map(ToString::to_string))
                .ok_or_else(|| {
                    ProbeError::Configuration(
                        "project_id not found in the options or the service account JSON".into(),
                    )
                })?;
            (token(&account).await?, project)
        }
        _ => (target.secret().to_string(), required_project(target)?),
    };

    let host = if location == "global" {
        "https://aiplatform.googleapis.com".to_string()
    } else {
        format!("https://{location}-aiplatform.googleapis.com")
    };
    let url = format!(
        "{host}/v1/projects/{project}/locations/{location}/publishers/google/models/{MODEL}:generateContent"
    );
    let call = Call {
        url: endpoints.url(&url),
        headers: Vec::new(),
        body: Some(one_token_request()),
    };
    Ok(Plan::Complete(call.bearer(&token)))
}

fn one_token_request() -> Value {
    json!({
        "contents": [{"role": "user", "parts": [{"text": "Hello"}]}],
        "generationConfig": {"maxOutputTokens": 1},
    })
}

fn required_project(target: ProbeTarget<'_>) -> Result<String, ProbeError> {
    target
        .option("project_id")
        .map(ToString::to_string)
        .ok_or_else(|| ProbeError::Configuration("project_id is required for Vertex AI".into()))
}

async fn token(provider: &dyn TokenProvider) -> Result<String, ProbeError> {
    provider
        .token(&[SCOPE])
        .await
        .map(|token| token.as_str().to_string())
        .map_err(|e| ProbeError::Configuration(format!("could not obtain a GCP token: {e}")))
}
