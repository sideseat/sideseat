use std::sync::Arc;

use async_stream::stream;
use async_trait::async_trait;
use aws_sdk_bedrockruntime::Client as BedrockClient;
use serde_json::{Value, json};

use crate::{
    error::ProviderError,
    provider::{ChatProvider, Provider, ProviderStream},
    providers::sse::{check_response, sse_data_stream},
    types::{
        Base64Data, Citation, ContainerInfo, ContentBlock, ContentBlockStart, ContentDelta,
        DocumentContent, ImageContent, MediaSource, Message, ModelInfo, ProviderConfig,
        ResponseFormat, Role, StaticTokenProvider, StopReason, StreamEvent, TextBlock,
        ThinkingBlock, TokenCount, TokenProvider, ToolChoice, ToolUseBlock, Usage,
    },
};

const ANTHROPIC_API_BASE: &str = "https://api.anthropic.com/v1";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const BEDROCK_ANTHROPIC_VERSION: &str = "bedrock-2023-05-31";
const VERTEX_ANTHROPIC_VERSION: &str = "vertex-2023-10-16";

// ---------------------------------------------------------------------------
// Backend enum
// ---------------------------------------------------------------------------

/// Selects which backend the `AnthropicProvider` uses to send requests.
///
/// Note: the `Vertex` variant holds an `Arc<dyn TokenProvider>` — Clone is cheap (ref-count bump).
#[derive(Clone)]
pub(crate) enum AnthropicBackend {
    /// Direct Anthropic API — uses `X-Api-Key` header.
    Direct { api_key: String, base_url: String },
    /// AWS Bedrock — Anthropic Messages API format via `invoke_model_with_response_stream`.
    Bedrock { client: Arc<BedrockClient> },
    /// Google Vertex AI — Anthropic Messages API format via SSE to aiplatform.googleapis.com.
    Vertex {
        project_id: String,
        location: String,
        token_provider: Arc<dyn TokenProvider>,
    },
}

// ---------------------------------------------------------------------------
// Provider struct
// ---------------------------------------------------------------------------

/// Anthropic Claude provider.
///
/// Supports all three deployment targets:
/// - Direct Anthropic API (`AnthropicBackend::Direct`)
/// - AWS Bedrock (`AnthropicBackend::Bedrock`)
/// - Google Vertex AI (`AnthropicBackend::Vertex`)
///
/// All backends accept the same `ProviderConfig` and `Message` types.
///
/// # Beta headers
///
/// Pass `betas` to enable experimental Anthropic features:
/// ```no_run
/// use sideseat::providers::AnthropicProvider;
/// let provider = AnthropicProvider::new("key")
///     .with_betas(vec!["files-api-2025-04-14".into()]);
/// ```
pub struct AnthropicProvider {
    backend: AnthropicBackend,
    client: Arc<reqwest::Client>,
    /// Beta feature names sent as `anthropic-beta: b1,b2,...`
    betas: Vec<String>,
}

impl AnthropicProvider {
    /// Create a provider from the `ANTHROPIC_API_KEY` environment variable.
    pub fn from_env() -> Result<Self, ProviderError> {
        Ok(Self::new(crate::env::require(
            crate::env::keys::ANTHROPIC_API_KEY,
        )?))
    }

    /// Create a direct Anthropic API provider.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            backend: AnthropicBackend::Direct {
                api_key: api_key.into(),
                base_url: ANTHROPIC_API_BASE.to_string(),
            },
            client: Arc::new(reqwest::Client::new()),
            betas: Vec::new(),
        }
    }

    /// Replace the HTTP client. Useful for custom TLS, proxies, or testing.
    pub fn with_client(mut self, client: reqwest::Client) -> Self {
        self.client = Arc::new(client);
        self
    }

    /// Override the API base URL.  Only applies to the `Direct` backend.
    ///
    /// For use with LiteLLM or other Anthropic-compatible proxies:
    /// ```no_run
    /// use sideseat::providers::AnthropicProvider;
    /// // LiteLLM proxy running locally
    /// let p = AnthropicProvider::new("sk-xxx").with_base_url("http://0.0.0.0:4000/v1");
    /// ```
    /// The `/messages` path is appended automatically to the base URL for all requests.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        if let AnthropicBackend::Direct {
            base_url: ref mut u,
            ..
        } = self.backend
        {
            *u = base_url.into();
        }
        self
    }

    /// Replace the full list of beta feature names.  These are sent as `anthropic-beta: b1,b2,…`.
    ///
    /// See [`betas`] for named constants of all known Anthropic beta strings.
    pub fn with_betas(mut self, betas: Vec<String>) -> Self {
        self.betas = betas;
        self
    }

    /// Append a single beta feature name.  See [`betas`] for named constants.
    ///
    /// ```no_run
    /// use sideseat::providers::{AnthropicProvider, anthropic::betas};
    /// let provider = AnthropicProvider::new("key")
    ///     .with_beta(betas::FILES_API)
    ///     .with_beta(betas::INTERLEAVED_THINKING);
    /// ```
    pub fn with_beta(mut self, beta: impl Into<String>) -> Self {
        self.betas.push(beta.into());
        self
    }

    /// Create a provider backed by AWS Bedrock (Anthropic Messages API format).
    ///
    /// The region is determined by the `BedrockClient` configuration — no separate
    /// `region` parameter is needed.
    pub fn from_bedrock(client: Arc<BedrockClient>) -> Self {
        Self {
            backend: AnthropicBackend::Bedrock { client },
            client: Arc::new(reqwest::Client::new()),
            betas: Vec::new(),
        }
    }

    /// Create a provider backed by Google Vertex AI (Anthropic Messages API format).
    pub fn from_vertex(
        project_id: impl Into<String>,
        location: impl Into<String>,
        access_token: impl Into<String>,
    ) -> Self {
        Self::from_vertex_with_token_provider(
            project_id,
            location,
            Arc::new(StaticTokenProvider::new(access_token.into())),
        )
    }

    /// Create a Vertex AI provider with a dynamic token provider (e.g. for rotating credentials).
    pub fn from_vertex_with_token_provider(
        project_id: impl Into<String>,
        location: impl Into<String>,
        token_provider: Arc<dyn TokenProvider>,
    ) -> Self {
        Self {
            backend: AnthropicBackend::Vertex {
                project_id: project_id.into(),
                location: location.into(),
                token_provider,
            },
            client: Arc::new(reqwest::Client::new()),
            betas: Vec::new(),
        }
    }

    // ---- helpers -----------------------------------------------------------

    fn vertex_url(project_id: &str, location: &str, model: &str, stream: bool) -> String {
        let method = if stream {
            "streamRawPredict"
        } else {
            "rawPredict"
        };
        format!(
            "https://{location}-aiplatform.googleapis.com/v1/projects/{project_id}/locations/{location}/publishers/anthropic/models/{model}:{method}"
        )
    }

    fn add_direct_headers(
        req: reqwest::RequestBuilder,
        api_key: &str,
        betas: &[String],
    ) -> reqwest::RequestBuilder {
        let mut r = req
            .header("x-api-key", api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json");
        if !betas.is_empty() {
            r = r.header("anthropic-beta", betas.join(","));
        }
        r
    }

    async fn direct_complete(
        client: &reqwest::Client,
        api_key: &str,
        base_url: &str,
        betas: &[String],
        body: Value,
        timeout_ms: Option<u64>,
    ) -> Result<Value, ProviderError> {
        let mut req =
            Self::add_direct_headers(client.post(format!("{base_url}/messages")), api_key, betas);
        if let Some(ms) = timeout_ms {
            req = req.timeout(std::time::Duration::from_millis(ms));
        }
        let resp = req.json(&body).send().await?;
        let resp = check_response(resp).await?;
        Ok(resp.json().await?)
    }

    async fn vertex_complete(
        client: &reqwest::Client,
        project_id: &str,
        location: &str,
        token: &str,
        model: &str,
        body: Value,
        timeout_ms: Option<u64>,
    ) -> Result<Value, ProviderError> {
        let url = Self::vertex_url(project_id, location, model, false);
        let mut req = client
            .post(url)
            .bearer_auth(token)
            .header("content-type", "application/json")
            .json(&body);
        if let Some(ms) = timeout_ms {
            req = req.timeout(std::time::Duration::from_millis(ms));
        }
        let resp = req.send().await?;
        let resp = check_response(resp).await?;
        Ok(resp.json().await?)
    }
}

// ---------------------------------------------------------------------------
// Provider trait
// ---------------------------------------------------------------------------

#[async_trait]
impl Provider for AnthropicProvider {
    fn provider_name(&self) -> &'static str {
        "anthropic"
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        match &self.backend {
            AnthropicBackend::Direct { api_key, base_url } => {
                list_models_direct(&self.client, api_key, base_url, &self.betas).await
            }
            AnthropicBackend::Bedrock { .. } => Err(ProviderError::Unsupported(
                "list_models not available for Bedrock backend; use BedrockProvider::list_models()"
                    .into(),
            )),
            AnthropicBackend::Vertex { .. } => Err(ProviderError::Unsupported(
                "list_models not available for Vertex backend".into(),
            )),
        }
    }
}

#[async_trait]
impl ChatProvider for AnthropicProvider {
    fn stream(&self, messages: Vec<Message>, config: ProviderConfig) -> ProviderStream {
        let backend = self.backend.clone();
        let client = Arc::clone(&self.client);
        let mut betas = self.betas.clone();
        // Auto-add required betas based on features in use
        for b in compute_auto_betas(&config, &messages) {
            if !betas.contains(&b) {
                betas.push(b);
            }
        }

        Box::pin(stream! {
            match &backend {
                AnthropicBackend::Direct { api_key, base_url } => {
                    let body = match build_messages_request(&messages, &config, true) {
                        Ok(b) => b,
                        Err(e) => { yield Err(e); return; }
                    };

                    let mut req = AnthropicProvider::add_direct_headers(
                        client.post(format!("{base_url}/messages")),
                        api_key,
                        &betas,
                    );
                    if let Some(ms) = config.timeout_ms {
                        req = req.timeout(std::time::Duration::from_millis(ms));
                    }
                    let resp = match req.json(&body).send().await {
                        Ok(r) => r,
                        Err(e) => { yield Err(e.into()); return; }
                    };
                    let resp = match check_response(resp).await {
                        Ok(r) => r,
                        Err(e) => { yield Err(e); return; }
                    };

                    let mut data_stream = Box::pin(sse_data_stream(resp));
                    use futures::StreamExt;

                    while let Some(result) = data_stream.next().await {
                        let data = match result {
                            Ok(d) => d,
                            Err(e) => { yield Err(e); return; }
                        };
                        for event in parse_sse_events(&data) {
                            yield event;
                        }
                    }
                }

                AnthropicBackend::Bedrock { client: bedrock_client, .. } => {
                    // Build request body for Bedrock (no model/stream fields)
                    let body = match build_bedrock_request(&messages, &config) {
                        Ok(b) => b,
                        Err(e) => { yield Err(e); return; }
                    };
                    let body_bytes = match serde_json::to_vec(&body) {
                        Ok(b) => b,
                        Err(e) => { yield Err(ProviderError::Serialization(e.to_string())); return; }
                    };

                    let send_fut = bedrock_client
                        .invoke_model_with_response_stream()
                        .model_id(&config.model)
                        .content_type("application/json")
                        .accept("application/json")
                        .body(aws_sdk_bedrockruntime::primitives::Blob::new(body_bytes))
                        .send();

                    let mut event_stream = if let Some(ms) = config.timeout_ms {
                        match tokio::time::timeout(std::time::Duration::from_millis(ms), send_fut).await {
                            Ok(Ok(r)) => r.body,
                            Ok(Err(e)) => { yield Err(classify_bedrock_sdk_error(format!("{e:?}"))); return; }
                            Err(_) => { yield Err(ProviderError::Timeout { ms: Some(ms) }); return; }
                        }
                    } else {
                        match send_fut.await {
                            Ok(r) => r.body,
                            Err(e) => { yield Err(classify_bedrock_sdk_error(format!("{e:?}"))); return; }
                        }
                    };

                    use aws_sdk_bedrockruntime::types::ResponseStream;
                    loop {
                        match event_stream.recv().await {
                            Ok(Some(ResponseStream::Chunk(chunk))) => {
                                if let Some(blob) = chunk.bytes {
                                    let data = String::from_utf8_lossy(blob.as_ref()).to_string();
                                    let parsed: Value = match serde_json::from_str(&data) {
                                        Ok(v) => v,
                                        Err(_) => continue,
                                    };
                                    for event in parse_sse_events(&serde_json::to_string(&parsed).unwrap_or_default()) {
                                        yield event;
                                    }
                                }
                            }
                            Ok(None) => break,
                            Ok(_) => continue,
                            Err(e) => {
                                yield Err(classify_bedrock_sdk_error(format!("{e:?}")));
                                break;
                            }
                        }
                    }
                }

                AnthropicBackend::Vertex { project_id, location, token_provider } => {
                    let token = match token_provider.get_token().await {
                        Ok(t) => t,
                        Err(e) => { yield Err(e); return; }
                    };
                    let body = match build_vertex_request(&messages, &config, true) {
                        Ok(b) => b,
                        Err(e) => { yield Err(e); return; }
                    };
                    let url = AnthropicProvider::vertex_url(project_id, location, &config.model, true);
                    let mut vertex_req = client
                        .post(url)
                        .bearer_auth(&token)
                        .header("content-type", "application/json")
                        .json(&body);
                    if let Some(ms) = config.timeout_ms {
                        vertex_req = vertex_req.timeout(std::time::Duration::from_millis(ms));
                    }
                    let resp = match vertex_req.send().await {
                        Ok(r) => r,
                        Err(e) => { yield Err(e.into()); return; }
                    };
                    let resp = match check_response(resp).await {
                        Ok(r) => r,
                        Err(e) => { yield Err(e); return; }
                    };

                    let mut data_stream = Box::pin(sse_data_stream(resp));
                    use futures::StreamExt;

                    while let Some(result) = data_stream.next().await {
                        let data = match result {
                            Ok(d) => d,
                            Err(e) => { yield Err(e); return; }
                        };
                        for event in parse_sse_events(&data) {
                            yield event;
                        }
                    }
                }
            }
        })
    }

    async fn complete(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<crate::types::Response, ProviderError> {
        let mut betas = self.betas.clone();
        for b in compute_auto_betas(&config, &messages) {
            if !betas.contains(&b) {
                betas.push(b);
            }
        }
        match &self.backend {
            AnthropicBackend::Direct { api_key, base_url } => {
                let body = build_messages_request(&messages, &config, false)?;
                let json = AnthropicProvider::direct_complete(
                    &self.client,
                    api_key,
                    base_url,
                    &betas,
                    body,
                    config.timeout_ms,
                )
                .await?;
                parse_response(&json)
            }
            AnthropicBackend::Bedrock { client, .. } => {
                let body = build_bedrock_request(&messages, &config)?;
                let body_bytes = serde_json::to_vec(&body)
                    .map_err(|e| ProviderError::Serialization(e.to_string()))?;
                let fut = client
                    .invoke_model()
                    .model_id(&config.model)
                    .content_type("application/json")
                    .accept("application/json")
                    .body(aws_sdk_bedrockruntime::primitives::Blob::new(body_bytes))
                    .send();
                let resp = if let Some(ms) = config.timeout_ms {
                    tokio::time::timeout(std::time::Duration::from_millis(ms), fut)
                        .await
                        .map_err(|_| ProviderError::Timeout { ms: Some(ms) })?
                        .map_err(|e| classify_bedrock_sdk_error(format!("{e:?}")))?
                } else {
                    fut.await
                        .map_err(|e| classify_bedrock_sdk_error(format!("{e:?}")))?
                };
                let json: Value = serde_json::from_slice(resp.body.as_ref())
                    .map_err(|e| ProviderError::Serialization(e.to_string()))?;
                parse_response(&json)
            }
            AnthropicBackend::Vertex {
                project_id,
                location,
                token_provider,
            } => {
                let token = token_provider.get_token().await?;
                let body = build_vertex_request(&messages, &config, false)?;
                let json = AnthropicProvider::vertex_complete(
                    &self.client,
                    project_id,
                    location,
                    &token,
                    &config.model,
                    body,
                    config.timeout_ms,
                )
                .await?;
                parse_response(&json)
            }
        }
    }

    async fn count_tokens(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<TokenCount, ProviderError> {
        match &self.backend {
            AnthropicBackend::Direct { api_key, base_url } => {
                count_tokens_direct(
                    &self.client,
                    api_key,
                    base_url,
                    &self.betas,
                    messages,
                    config,
                )
                .await
            }
            _ => Err(ProviderError::Unsupported(
                "count_tokens is only available for the Direct Anthropic backend".into(),
            )),
        }
    }
}

mod protocol;
use protocol::{
    build_bedrock_request, build_messages_request, build_vertex_request,
    classify_bedrock_sdk_error, compute_auto_betas, count_tokens_direct, list_models_direct,
    parse_response, parse_sse_events,
};
#[cfg(test)]
use protocol::{format_image_block, parse_stop_reason};

// ---------------------------------------------------------------------------
// Beta feature constants
// ---------------------------------------------------------------------------

/// Named constants for Anthropic beta feature strings.
///
/// Pass to [`AnthropicProvider::with_beta`] or [`AnthropicProvider::with_betas`]:
///
/// ```no_run
/// use sideseat::providers::{AnthropicProvider, anthropic::betas};
/// let provider = AnthropicProvider::new("key")
///     .with_beta(betas::FILES_API)
///     .with_beta(betas::INTERLEAVED_THINKING);
/// ```
pub mod betas {
    /// Up to 128 000 output tokens (claude-3-7-sonnet, claude-3-5-sonnet-20241022).
    pub const OUTPUT_128K: &str = "output-128k-2025-02-19";
    /// Extended prompt-cache TTL: 1 hour instead of the default 5 minutes.
    pub const EXTENDED_CACHE_TTL: &str = "extended-cache-ttl-2025-04-11";
    /// Interleaved thinking — `thinking` blocks interspersed with text responses.
    pub const INTERLEAVED_THINKING: &str = "interleaved-thinking-2025-05-14";
    /// Files API — upload files and reference them by ID in messages.
    pub const FILES_API: &str = "files-api-2025-04-14";
    /// MCP client support.
    pub const MCP_CLIENT: &str = "mcp-client-2025-11-20";
    /// Token-efficient tool-use encoding (reduces input tokens for tool calls).
    pub const TOKEN_EFFICIENT_TOOLS: &str = "token-efficient-tools-2025-02-19";
    /// Fast mode — lower latency at the cost of some quality.
    pub const FAST_MODE: &str = "fast-mode-2026-02-01";
    /// Sandboxed code-execution tool.
    pub const CODE_EXECUTION: &str = "code-execution-2025-05-22";
    /// 1 million token context window.
    pub const CONTEXT_1M: &str = "context-1m-2025-08-07";
    /// Context management — per-request token limits and priority controls.
    pub const CONTEXT_MANAGEMENT: &str = "context-management-2025-06-27";
    /// Full thinking output with extended budget tokens (dev accounts only).
    pub const DEV_FULL_THINKING: &str = "dev-full-thinking-2025-05-14";
    /// Prompt caching (GA on most models; required for older API versions).
    pub const PROMPT_CACHING: &str = "prompt-caching-2024-07-31";
    /// Server-side token-counting endpoint.
    pub const TOKEN_COUNTING: &str = "token-counting-2024-11-01";
    /// Computer-use tool (stable 2025 version).
    pub const COMPUTER_USE: &str = "computer-use-2025-01-24";
    /// Native PDF document support.
    pub const PDFS: &str = "pdfs-2024-09-25";
    /// Message Batches API.
    pub const MESSAGE_BATCHES: &str = "message-batches-2024-09-24";
    /// Skills — custom model capability definitions.
    pub const SKILLS: &str = "skills-2025-10-02";
    /// Streaming events when the model context window is exceeded.
    pub const MODEL_CONTEXT_WINDOW_EXCEEDED: &str = "model-context-window-exceeded-2025-08-26";
    /// Web search tool integration.
    pub const WEB_SEARCH: &str = "web-search-2025-03-05";
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "anthropic_tests.rs"]
mod tests;
