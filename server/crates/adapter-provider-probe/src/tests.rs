use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::{Endpoints, ProbeError, ProbeTarget, probe_with};

const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
struct Seen {
    method: Method,
    uri: String,
    headers: HeaderMap,
    body: Value,
}

type Responder = fn(&Seen) -> Response;

/// A local server that records each request and answers with `respond`.
struct Provider {
    origin: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Provider {
    async fn start(respond: Responder) -> Self {
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
        let log = Arc::clone(&seen);
        let app = Router::new().fallback(
            move |method: Method, uri: Uri, headers: HeaderMap, body: Bytes| {
                let log = Arc::clone(&log);
                async move {
                    let request = Seen {
                        method,
                        uri: uri.to_string(),
                        headers,
                        body: serde_json::from_slice(&body).unwrap_or(Value::Null),
                    };
                    let response = respond(&request);
                    log.lock().unwrap().push(request);
                    response
                }
            },
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self { origin, seen }
    }

    fn endpoints(&self) -> Endpoints {
        Endpoints {
            origin: Some(self.origin.clone()),
        }
    }

    fn requests(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

fn target<'a>(provider: &'a str, options: Option<&'a Value>, secret: &'a str) -> ProbeTarget<'a> {
    ProbeTarget {
        provider,
        endpoint: None,
        options,
        secret: Some(secret),
    }
}

fn header<'a>(seen: &'a Seen, name: &str) -> &'a str {
    seen.headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
}

fn ok(body: Value) -> Response {
    (StatusCode::OK, axum::Json(body)).into_response()
}

fn aws_error(status: StatusCode, code: &'static str, message: &str) -> Response {
    (
        status,
        [
            ("x-amzn-errortype", code),
            ("content-type", "application/json"),
        ],
        json!({ "message": message }).to_string(),
    )
        .into_response()
}

#[tokio::test]
async fn anthropic_lists_models_with_its_key_header() {
    let provider = Provider::start(|_| ok(json!({"data": [{"id": "claude-sonnet-5-5"}]}))).await;

    let hint = probe_with(
        target("anthropic", None, "sk-ant"),
        TIMEOUT,
        &provider.endpoints(),
    )
    .await
    .unwrap();

    assert_eq!(hint.as_deref(), Some("claude-sonnet-5-5"));
    let [request] = provider.requests().try_into().unwrap();
    assert_eq!(
        (&request.method, request.uri.as_str()),
        (&Method::GET, "/v1/models")
    );
    assert_eq!(header(&request, "x-api-key"), "sk-ant");
    assert_eq!(header(&request, "anthropic-version"), "2023-06-01");
}

#[tokio::test]
async fn a_refused_key_reports_the_providers_message() {
    let provider = Provider::start(|_| {
        (
            StatusCode::UNAUTHORIZED,
            axum::Json(json!({"error": {"message": "Incorrect API key provided"}})),
        )
            .into_response()
    })
    .await;

    let error = probe_with(
        target("openai", None, "sk-bad"),
        TIMEOUT,
        &provider.endpoints(),
    )
    .await
    .unwrap_err();

    assert!(
        matches!(&error, ProbeError::Rejected { status: 401, message } if message == "Incorrect API key provided"),
        "{error:?}"
    );
    let [request] = provider.requests().try_into().unwrap();
    assert_eq!(header(&request, "authorization"), "Bearer sk-bad");
}

#[tokio::test]
async fn gemini_sends_the_key_in_a_header_not_the_url() {
    let provider =
        Provider::start(|_| ok(json!({"models": [{"name": "models/gemini-2.5-flash"}]}))).await;

    let hint = probe_with(
        target("gemini", None, "AIza"),
        TIMEOUT,
        &provider.endpoints(),
    )
    .await
    .unwrap();

    assert_eq!(hint.as_deref(), Some("gemini-2.5-flash"));
    let [request] = provider.requests().try_into().unwrap();
    assert_eq!(request.uri, "/v1beta/models");
    assert_eq!(header(&request, "x-goog-api-key"), "AIza");
}

#[tokio::test]
async fn openrouter_checks_the_key_endpoint_because_its_listing_is_public() {
    let provider = Provider::start(|_| ok(json!({"data": {"label": "dev"}}))).await;

    probe_with(
        target("openrouter", None, "sk-or"),
        TIMEOUT,
        &provider.endpoints(),
    )
    .await
    .unwrap();

    assert_eq!(provider.requests()[0].uri, "/api/v1/key");
}

#[tokio::test]
async fn a_completion_probe_counts_an_unknown_model_as_an_accepted_key() {
    let provider = Provider::start(|seen| {
        assert_eq!(seen.body["max_tokens"], 1);
        (
            StatusCode::NOT_FOUND,
            axum::Json(json!({"error": {"message": "Model sonar not found"}})),
        )
            .into_response()
    })
    .await;

    let hint = probe_with(
        target("perplexity", None, "pplx"),
        TIMEOUT,
        &provider.endpoints(),
    )
    .await
    .unwrap();

    assert_eq!(hint, None);
    assert_eq!(provider.requests()[0].method, Method::POST);
}

#[tokio::test]
async fn a_completion_probe_reports_a_missing_route() {
    let provider =
        Provider::start(|_| (StatusCode::NOT_FOUND, "no such route").into_response()).await;

    let error = probe_with(
        target("perplexity", None, "pplx"),
        TIMEOUT,
        &provider.endpoints(),
    )
    .await
    .unwrap_err();

    assert!(
        matches!(error, ProbeError::Failed { status: 404, .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn azure_deployments_get_a_versioned_completion_with_the_api_key_header() {
    let provider = Provider::start(|_| ok(json!({"model": "gpt-6.1-sol"}))).await;
    let options = json!({"deployment_name": "chat"});
    let endpoint = provider.origin.clone();

    let hint = probe_with(
        ProbeTarget {
            endpoint: Some(&endpoint),
            ..target("azure-ai-foundry", Some(&options), "azure-key")
        },
        TIMEOUT,
        &Endpoints::default(),
    )
    .await
    .unwrap();

    assert_eq!(hint.as_deref(), Some("gpt-6.1-sol"));
    let [request] = provider.requests().try_into().unwrap();
    assert_eq!(
        request.uri,
        "/openai/deployments/chat/chat/completions?api-version=2024-10-21"
    );
    assert_eq!(header(&request, "api-key"), "azure-key");
    assert_eq!(header(&request, "authorization"), "");
}

#[tokio::test]
async fn azure_v1_endpoints_list_models() {
    let provider = Provider::start(|_| ok(json!({"data": []}))).await;
    let options = json!({"api_variant": "v1"});
    let endpoint = provider.origin.clone();

    probe_with(
        ProbeTarget {
            endpoint: Some(&endpoint),
            ..target("azure-ai-foundry", Some(&options), "azure-key")
        },
        TIMEOUT,
        &Endpoints::default(),
    )
    .await
    .unwrap();

    assert_eq!(provider.requests()[0].uri, "/openai/v1/models");
}

#[tokio::test]
async fn vertex_with_a_bearer_token_asks_its_regional_endpoint_for_one_token() {
    let provider = Provider::start(|seen| {
        assert_eq!(seen.body["generationConfig"]["maxOutputTokens"], 1);
        ok(json!({"modelVersion": "gemini-2.5-flash"}))
    })
    .await;
    let options = json!({"project_id": "acme", "location": "europe-west4"});

    let hint = probe_with(
        target("vertex-ai", Some(&options), "ya29"),
        TIMEOUT,
        &provider.endpoints(),
    )
    .await
    .unwrap();

    assert_eq!(hint.as_deref(), Some("gemini-2.5-flash"));
    let [request] = provider.requests().try_into().unwrap();
    assert_eq!(
        request.uri,
        "/v1/projects/acme/locations/europe-west4/publishers/google/models/gemini-2.5-flash:generateContent"
    );
    assert_eq!(header(&request, "authorization"), "Bearer ya29");
}

#[tokio::test]
async fn vertex_requires_a_project() {
    let error = probe_with(
        target("vertex-ai", None, "ya29"),
        TIMEOUT,
        &Endpoints::default(),
    )
    .await
    .unwrap_err();

    assert!(matches!(error, ProbeError::Configuration(_)), "{error:?}");
}

#[tokio::test]
async fn bedrock_api_keys_list_foundation_models() {
    let provider = Provider::start(|_| {
        ok(json!({"modelSummaries": [{"modelArn": "arn:aws:bedrock:::m", "modelId": "amazon.nova-2-lite-v1:0"}]}))
    })
    .await;

    let hint = probe_with(
        target("bedrock", None, "bedrock-api-key"),
        TIMEOUT,
        &provider.endpoints(),
    )
    .await
    .unwrap();

    assert_eq!(hint.as_deref(), Some("amazon.nova-2-lite-v1:0"));
    let [request] = provider.requests().try_into().unwrap();
    assert_eq!(request.uri, "/foundation-models");
    assert_eq!(header(&request, "authorization"), "Bearer bedrock-api-key");
}

#[tokio::test]
async fn bedrock_falls_back_to_converse_when_listing_is_not_permitted() {
    let provider = Provider::start(|seen| {
        if seen.uri == "/foundation-models" {
            aws_error(
                StatusCode::FORBIDDEN,
                "AccessDeniedException",
                "not allowed to list",
            )
        } else {
            aws_error(
                StatusCode::NOT_FOUND,
                "ResourceNotFoundException",
                "model not enabled",
            )
        }
    })
    .await;

    let hint = probe_with(
        target("bedrock", None, "bedrock-api-key"),
        TIMEOUT,
        &provider.endpoints(),
    )
    .await
    .unwrap();

    assert_eq!(hint, None);
    let requests = provider.requests();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1].uri.ends_with("/converse"),
        "{}",
        requests[1].uri
    );
}

#[tokio::test]
async fn bedrock_reports_a_rejected_key_without_trying_converse() {
    let provider = Provider::start(|_| {
        aws_error(
            StatusCode::FORBIDDEN,
            "UnrecognizedClientException",
            "invalid token",
        )
    })
    .await;

    let error = probe_with(
        target("bedrock", None, "bad"),
        TIMEOUT,
        &provider.endpoints(),
    )
    .await
    .unwrap_err();

    assert!(
        matches!(&error, ProbeError::Rejected { message, .. } if message == "UnrecognizedClientException: invalid token"),
        "{error:?}"
    );
    assert_eq!(provider.requests().len(), 1);
}

#[tokio::test]
async fn bedrock_access_keys_must_be_a_complete_document() {
    let options = json!({"auth_mode": "access_keys"});

    let error = probe_with(
        target("bedrock", Some(&options), r#"{"access_key_id": "AKIA"}"#),
        TIMEOUT,
        &Endpoints::default(),
    )
    .await
    .unwrap_err();

    assert!(
        matches!(&error, ProbeError::Configuration(m) if m.contains("secret_access_key")),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_slow_provider_times_out() {
    let provider = Provider::start(|_| {
        std::thread::sleep(Duration::from_millis(500));
        ok(json!({"data": []}))
    })
    .await;

    let error = probe_with(
        target("groq", None, "gsk"),
        Duration::from_millis(50),
        &provider.endpoints(),
    )
    .await
    .unwrap_err();

    assert!(matches!(error, ProbeError::TimedOut), "{error:?}");
}

#[tokio::test]
async fn unknown_providers_and_missing_endpoints_are_configuration_errors() {
    let unknown = probe_with(target("nope", None, ""), TIMEOUT, &Endpoints::default()).await;
    let custom = probe_with(target("custom", None, ""), TIMEOUT, &Endpoints::default()).await;

    assert!(matches!(unknown, Err(ProbeError::UnknownProvider(_))));
    assert!(matches!(custom, Err(ProbeError::Configuration(_))));
}

#[test]
fn the_override_origin_keeps_path_and_query() {
    let endpoints = Endpoints {
        origin: Some("http://127.0.0.1:9".into()),
    };

    assert_eq!(
        endpoints.url("https://api.x.ai/v1/models?page=2"),
        "http://127.0.0.1:9/v1/models?page=2"
    );
    assert_eq!(endpoints.url("https://api.x.ai"), "http://127.0.0.1:9");
    assert_eq!(
        Endpoints::default().url("https://api.x.ai/v1"),
        "https://api.x.ai/v1"
    );
}
