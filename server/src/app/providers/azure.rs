//! Azure endpoint normalization and managed-identity authentication.

/// Normalize an Azure AI Foundry endpoint for the OpenAI-compatible client.
pub(super) fn resolve_base_url(
    raw_endpoint: &str,
    deployment: Option<&str>,
    api_variant: &str,
) -> String {
    let endpoint = raw_endpoint.trim_end_matches('/');

    if let Some(base) = endpoint.strip_suffix("/chat/completions") {
        return base.to_string();
    }

    if endpoint.contains("/openai/v1") || endpoint.contains("/openai/deployments/") {
        return endpoint.to_string();
    }

    match (api_variant, deployment) {
        ("v1", _) | (_, None) => format!("{endpoint}/openai/v1"),
        (_, Some(name)) => format!("{endpoint}/openai/deployments/{name}"),
    }
}

/// Fetch an Azure OAuth2 token from workload identity or the instance metadata service.
pub(super) async fn get_managed_identity_token(resource: &str) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    if let (Ok(token_file), Ok(tenant_id), Ok(client_id)) = (
        std::env::var("AZURE_FEDERATED_TOKEN_FILE"),
        std::env::var("AZURE_TENANT_ID"),
        std::env::var("AZURE_CLIENT_ID"),
    ) {
        let assertion = tokio::fs::read_to_string(&token_file)
            .await
            .map_err(|e| format!("Cannot read AZURE_FEDERATED_TOKEN_FILE: {e}"))?;
        let authority = std::env::var("AZURE_AUTHORITY_HOST")
            .unwrap_or_else(|_| "https://login.microsoftonline.com".to_string());
        let response: serde_json::Value = client
            .post(format!("{authority}/{tenant_id}/oauth2/v2.0/token"))
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", &client_id),
                (
                    "client_assertion_type",
                    "urn:ietf:params:oauth:client-assertion-type:jwt-bearer",
                ),
                ("client_assertion", &assertion),
                ("scope", &format!("{resource}/.default")),
            ])
            .send()
            .await
            .map_err(|e| format!("Workload identity request failed: {e}"))?
            .json()
            .await
            .map_err(|e| format!("Workload identity response parse error: {e}"))?;
        return response
            .get("access_token")
            .and_then(|value| value.as_str())
            .map(ToString::to_string)
            .ok_or_else(|| format!("Workload identity response missing access_token: {response}"));
    }

    let response: serde_json::Value = client
        .get(format!(
            "http://169.254.169.254/metadata/identity/oauth2/token\
             ?api-version=2018-02-01&resource={resource}"
        ))
        .header("Metadata", "true")
        .send()
        .await
        .map_err(|e| format!("IMDS request failed (not an Azure host?): {e}"))?
        .json()
        .await
        .map_err(|e| format!("IMDS response parse error: {e}"))?;
    response
        .get("access_token")
        .and_then(|value| value.as_str())
        .map(ToString::to_string)
        .ok_or_else(|| format!("IMDS response missing access_token: {response}"))
}

#[cfg(test)]
mod tests {
    use super::resolve_base_url;

    #[test]
    fn normalizes_supported_endpoint_forms() {
        let cases = [
            (
                "https://example.openai.azure.com/openai/deployments/chat/chat/completions",
                Some("ignored"),
                "standard",
                "https://example.openai.azure.com/openai/deployments/chat",
            ),
            (
                "https://example.openai.azure.com/openai/deployments/chat",
                Some("ignored"),
                "standard",
                "https://example.openai.azure.com/openai/deployments/chat",
            ),
            (
                "https://example.services.ai.azure.com/openai/v1/",
                None,
                "v1",
                "https://example.services.ai.azure.com/openai/v1",
            ),
            (
                "https://example.openai.azure.com/",
                Some("chat"),
                "standard",
                "https://example.openai.azure.com/openai/deployments/chat",
            ),
            (
                "https://example.services.ai.azure.com",
                Some("ignored"),
                "v1",
                "https://example.services.ai.azure.com/openai/v1",
            ),
            (
                "https://example.openai.azure.com",
                None,
                "standard",
                "https://example.openai.azure.com/openai/v1",
            ),
        ];

        for (endpoint, deployment, variant, expected) in cases {
            assert_eq!(resolve_base_url(endpoint, deployment, variant), expected);
        }
    }
}
