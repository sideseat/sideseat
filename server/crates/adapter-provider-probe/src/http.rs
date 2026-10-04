//! Probes for providers reached over plain HTTPS.

use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::ProbeError;

/// One request: a GET when it has no body, a POST with a JSON body otherwise.
pub(crate) struct Call {
    pub url: String,
    pub headers: Vec<(&'static str, String)>,
    pub body: Option<Value>,
}

impl Call {
    pub fn get(url: String) -> Self {
        Self {
            url,
            headers: Vec::new(),
            body: None,
        }
    }

    /// An OpenAI-shaped chat completion of at most one token.
    pub fn chat_completion(url: String, model: &str) -> Self {
        Self {
            url,
            headers: Vec::new(),
            body: Some(json!({
                "model": model,
                "max_tokens": 1,
                "messages": [{"role": "user", "content": "Hello"}],
            })),
        }
    }

    pub fn bearer(self, token: &str) -> Self {
        if token.is_empty() {
            return self;
        }
        self.header("authorization", format!("Bearer {token}"))
    }

    pub fn header(mut self, name: &'static str, value: String) -> Self {
        self.headers.push((name, value));
        self
    }
}

/// How a provider's credential is checked: by listing models, or by asking for one token where the
/// provider has no listing that requires the credential.
pub(crate) enum Plan {
    List(Call),
    Complete(Call),
}

pub(crate) async fn run(plan: Plan) -> Result<Option<String>, ProbeError> {
    let client = reqwest::Client::new();
    match plan {
        Plan::List(list) => list_models(&client, list).await,
        Plan::Complete(complete) => completion(&client, complete).await,
    }
}

async fn list_models(client: &reqwest::Client, call: Call) -> Result<Option<String>, ProbeError> {
    let body = send(client, call).await?;
    Ok(first_model(&body))
}

async fn completion(client: &reqwest::Client, call: Call) -> Result<Option<String>, ProbeError> {
    match send(client, call).await {
        Ok(body) => Ok(body
            .get("model")
            .or_else(|| body.get("modelVersion"))
            .and_then(Value::as_str)
            .map(ToString::to_string)),
        Err(ProbeError::Failed {
            status: 404,
            message,
        }) if mentions_model(&message) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn send(client: &reqwest::Client, call: Call) -> Result<Value, ProbeError> {
    let mut request = match &call.body {
        Some(body) => client.post(&call.url).json(body),
        None => client.get(&call.url),
    };
    for (name, value) in call.headers {
        request = request.header(name, value);
    }
    let response = request
        .send()
        .await
        .map_err(|error| ProbeError::Unreachable(error.without_url().to_string()))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    let body: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if status.is_success() {
        return Ok(body);
    }
    let message = error_message(&body).unwrap_or_else(|| truncate(&text));
    Err(classify(status, message))
}

pub(crate) fn classify(status: StatusCode, message: String) -> ProbeError {
    let status = status.as_u16();
    if status == 401 || status == 403 {
        ProbeError::Rejected { status, message }
    } else {
        ProbeError::Failed { status, message }
    }
}

pub(crate) fn mentions_model(message: &str) -> bool {
    message.to_ascii_lowercase().contains("model")
}

/// The first model id in the listing shapes providers use: `data[]` (OpenAI-compatible,
/// Anthropic) and `models[]` (Gemini, Cohere).
fn first_model(body: &Value) -> Option<String> {
    let first = body
        .get("data")
        .or_else(|| body.get("models"))?
        .as_array()?
        .first()?;
    first
        .get("id")
        .or_else(|| first.get("name"))
        .and_then(Value::as_str)
        .map(|id| id.trim_start_matches("models/").to_string())
}

fn error_message(body: &Value) -> Option<String> {
    let error = body.get("error").unwrap_or(body);
    error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| error.as_str())
        .map(ToString::to_string)
}

fn truncate(text: &str) -> String {
    const LIMIT: usize = 300;
    match text.char_indices().nth(LIMIT) {
        Some((end, _)) => format!("{}...", &text[..end]),
        None => text.to_string(),
    }
}
