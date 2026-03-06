use std::collections::HashMap;
use std::sync::Arc;

use async_stream::stream;
use async_trait::async_trait;
use aws_sdk_bedrockruntime::Client as BedrockClient;
use aws_sdk_bedrockruntime::primitives::Blob;
use serde_json::{Value, json};

use crate::{
    error::ProviderError,
    provider::{ChatProvider, EmbeddingProvider, Provider, ProviderStream},
    providers::sse::{check_response, sse_data_stream},
    types::{
        ContentBlock, ContentBlockStart, ContentDelta, EmbeddingRequest, EmbeddingResponse,
        EmbeddingTaskType, ImageContent, MediaSource, Message, ModelInfo, ProviderConfig,
        ResponseFormat, Role, StopReason, StreamEvent, TokenCount, TokenLogprob, Tool, ToolChoice,
        ToolUseBlock, Usage,
    },
};

const COHERE_CHAT_URL: &str = "https://api.cohere.com/v2/chat";
const COHERE_API_BASE: &str = "https://api.cohere.com/v2";

// ---------------------------------------------------------------------------
// Backend enum
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub(crate) enum CohereBackend {
    Direct {
        api_key: String,
        base_url: String,
        api_base: String,
    },
    Bedrock {
        client: Arc<BedrockClient>,
    },
}

// ---------------------------------------------------------------------------
// Provider struct
// ---------------------------------------------------------------------------

/// Cohere Chat API v2 provider.
///
/// Supports Command R and Command R+ models via the direct Cohere v2 API
/// or AWS Bedrock (Cohere native format via `invoke_model`).
pub struct CohereProvider {
    backend: CohereBackend,
    client: Arc<reqwest::Client>,
}

impl CohereProvider {
    /// Create a provider from the `COHERE_API_KEY` environment variable.
    pub fn from_env() -> Result<Self, ProviderError> {
        Ok(Self::new(crate::env::require(crate::env::keys::COHERE_API_KEY)?))
    }

    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            backend: CohereBackend::Direct {
                api_key: api_key.into(),
                base_url: COHERE_CHAT_URL.to_string(),
                api_base: COHERE_API_BASE.to_string(),
            },
            client: Arc::new(reqwest::Client::new()),
        }
    }

    /// Replace the HTTP client. Useful for custom TLS, proxies, or testing.
    /// Only applies to the Direct backend (Bedrock uses the AWS SDK).
    pub fn with_client(mut self, client: reqwest::Client) -> Self {
        self.client = Arc::new(client);
        self
    }

    /// Override the API base URL for use with proxies or custom endpoints.
    /// All endpoints are derived from this base:
    /// - Chat:       `{base}/chat`
    /// - Models:     `{base}/models`
    /// - Embeddings: `{base}/embed`
    pub fn with_api_base(mut self, base: impl Into<String>) -> Self {
        let base = base.into();
        if let CohereBackend::Direct { ref mut base_url, ref mut api_base, .. } = self.backend {
            *base_url = format!("{}/chat", base);
            *api_base = base;
        }
        self
    }

    /// Create a provider backed by AWS Bedrock (Cohere native format via invoke_model).
    ///
    /// The region is determined by the `BedrockClient` configuration.
    /// Use `cohere.command-r-v1:0` or `cohere.command-r-plus-v1:0` as model names
    /// (prefix with `eu.` for EU regions, e.g. `eu.cohere.command-r-v1:0`).
    pub fn from_bedrock(client: Arc<BedrockClient>) -> Self {
        Self {
            backend: CohereBackend::Bedrock { client },
            client: Arc::new(reqwest::Client::new()),
        }
    }

    /// Create a Bedrock-backed provider using AWS IAM credentials from the environment.
    ///
    /// Reads standard AWS env vars (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, etc.)
    /// and the provided region.
    pub async fn from_bedrock_from_env(region: impl Into<String>) -> Self {
        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_config::Region::new(region.into()))
            .load()
            .await;
        Self::from_bedrock(Arc::new(BedrockClient::new(&config)))
    }

    /// Create a Bedrock-backed provider using a Bedrock API key (bearer token).
    ///
    /// See: <https://docs.aws.amazon.com/bedrock/latest/userguide/api-keys.html>
    pub fn with_api_key_bedrock(api_key: impl Into<String>, region: impl Into<String>) -> Self {
        use aws_sdk_bedrockruntime::config::{BehaviorVersion, Region, Token};
        let conf = aws_sdk_bedrockruntime::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new(region.into()))
            .bearer_token(Token::new(api_key.into(), None))
            .build();
        Self::from_bedrock(Arc::new(BedrockClient::from_conf(conf)))
    }

    /// Create a Bedrock-backed provider using a Bedrock API key from environment variables.
    ///
    /// Reads `BEDROCK_API_KEY` (or `AWS_BEARER_TOKEN_BEDROCK`) for the key,
    /// and `BEDROCK_REGION` / `AWS_REGION` / `AWS_DEFAULT_REGION` for the region.
    pub fn with_api_key_bedrock_from_env() -> Result<Self, ProviderError> {
        let api_key = crate::env::require(crate::env::keys::BEDROCK_API_KEY)
            .or_else(|_| crate::env::require("AWS_BEARER_TOKEN_BEDROCK"))?;
        let region = crate::env::optional("BEDROCK_REGION")
            .or_else(|| crate::env::optional("AWS_REGION"))
            .or_else(|| crate::env::optional("AWS_DEFAULT_REGION"))
            .unwrap_or_else(|| "us-east-1".to_string());
        Ok(Self::with_api_key_bedrock(api_key, region))
    }
}

// ---------------------------------------------------------------------------
// Provider trait
// ---------------------------------------------------------------------------

#[async_trait]
impl Provider for CohereProvider {
    fn provider_name(&self) -> &'static str {
        "cohere"
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        match &self.backend {
            CohereBackend::Direct { api_key, api_base, .. } => {
                let base_url = format!("{}/models", api_base);
                let mut models = Vec::new();
                let mut page_token: Option<String> = None;

                loop {
                    let url = match &page_token {
                        Some(t) => format!("{}?page_token={}", base_url, t),
                        None => base_url.clone(),
                    };
                    let req = self.client.get(&url).bearer_auth(api_key);
                    let resp = req.send().await?;
                    let resp = check_response(resp).await?;
                    let json: Value = resp.json().await?;

                    if let Some(arr) = json["models"].as_array() {
                        for item in arr {
                            models.push(ModelInfo {
                                id: item["name"].as_str().unwrap_or("").to_string(),
                                display_name: item["display_name"].as_str().map(|s| s.to_string()),
                                description: item["description"].as_str().map(|s| s.to_string()),
                                created_at: None,
                            });
                        }
                    }

                    match json["next_page_token"].as_str() {
                        Some(t) if !t.is_empty() => page_token = Some(t.to_string()),
                        _ => break,
                    }
                }

                Ok(models)
            }
            CohereBackend::Bedrock { .. } => Err(ProviderError::Unsupported(
                "list_models not available for Bedrock backend; use BedrockProvider::list_models()"
                    .into(),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// ChatProvider trait
// ---------------------------------------------------------------------------

#[async_trait]
impl ChatProvider for CohereProvider {
    fn stream(&self, messages: Vec<Message>, config: ProviderConfig) -> ProviderStream {
        let backend = self.backend.clone();
        let client = Arc::clone(&self.client);

        Box::pin(stream! {
            match &backend {
                CohereBackend::Direct { api_key, base_url, .. } => {
                    let body = match build_request(&messages, &config, true) {
                        Ok(b) => b,
                        Err(e) => { yield Err(e); return; }
                    };

                    let mut req_builder = client
                        .post(base_url)
                        .bearer_auth(api_key)
                        .json(&body);
                    if let Some(ms) = config.timeout_ms {
                        req_builder = req_builder.timeout(std::time::Duration::from_millis(ms));
                    }
                    let resp = match req_builder.send().await {
                        Ok(r) => r,
                        Err(e) => { yield Err(e.into()); return; }
                    };
                    let resp = match check_response(resp).await {
                        Ok(r) => r,
                        Err(e) => { yield Err(e); return; }
                    };

                    yield Ok(StreamEvent::MessageStart { role: Role::Assistant });

                    let text_index: usize = 0;
                    let mut text_started = false;
                    let mut tool_calls: HashMap<usize, (String, String, usize)> = HashMap::new();
                    let mut tool_arg_bufs: HashMap<usize, String> = HashMap::new();
                    let mut next_tool_block_idx: usize = 1;
                    let mut response_id: Option<String> = None;

                    let mut data_stream = Box::pin(sse_data_stream(resp));
                    use futures::StreamExt;

                    while let Some(result) = data_stream.next().await {
                        let data = match result {
                            Ok(d) => d,
                            Err(e) => { yield Err(e); return; }
                        };

                        let parsed: Value = match serde_json::from_str(&data) {
                            Ok(v) => v,
                            Err(_) => continue,
                        };

                        let event_type = match parsed["type"].as_str() {
                            Some(t) => t,
                            None => continue,
                        };

                        match event_type {
                            "message-start" => {
                                response_id = parsed["message"]["id"]
                                    .as_str()
                                    .map(|s| s.to_string());
                            }

                            "content-delta" => {
                                if let Some(text) = parsed["delta"]["message"]["content"]["text"].as_str() {
                                    if !text_started {
                                        yield Ok(StreamEvent::ContentBlockStart {
                                            index: text_index,
                                            block: ContentBlockStart::Text,
                                        });
                                        text_started = true;
                                    }
                                    if !text.is_empty() {
                                        yield Ok(StreamEvent::ContentBlockDelta {
                                            index: text_index,
                                            delta: ContentDelta::Text { text: text.to_string() },
                                        });
                                    }
                                }
                            }

                            "tool-call-start" => {
                                let stream_idx = parsed["index"].as_u64().unwrap_or(0) as usize;
                                let id = parsed["delta"]["message"]["tool_calls"]["id"]
                                    .as_str()
                                    .unwrap_or("")
                                    .to_string();
                                let name = parsed["delta"]["message"]["tool_calls"]["function"]["name"]
                                    .as_str()
                                    .unwrap_or("")
                                    .to_string();
                                let block_idx = next_tool_block_idx;
                                next_tool_block_idx += 1;

                                tool_calls.insert(stream_idx, (id.clone(), name.clone(), block_idx));
                                tool_arg_bufs.insert(stream_idx, String::new());

                                yield Ok(StreamEvent::ContentBlockStart {
                                    index: block_idx,
                                    block: ContentBlockStart::ToolUse { id, name },
                                });
                            }

                            "tool-call-delta" => {
                                let stream_idx = parsed["index"].as_u64().unwrap_or(0) as usize;
                                if let Some(args) = parsed["delta"]["message"]["tool_calls"]["function"]["arguments"].as_str() {
                                    if let Some(buf) = tool_arg_bufs.get_mut(&stream_idx) {
                                        buf.push_str(args);
                                    }
                                    if !args.is_empty() {
                                        let block_idx = tool_calls.get(&stream_idx).map(|(_, _, i)| *i)
                                            .unwrap_or(0);
                                        yield Ok(StreamEvent::ContentBlockDelta {
                                            index: block_idx,
                                            delta: ContentDelta::ToolInput { partial_json: args.to_string() },
                                        });
                                    }
                                }
                            }

                            "content-end" => {
                                if text_started {
                                    let idx = parsed["index"].as_u64().unwrap_or(0) as usize;
                                    if idx == 0 {
                                        yield Ok(StreamEvent::ContentBlockStop { index: text_index });
                                        text_started = false;
                                    }
                                }
                            }

                            "message-end" => {
                                let finish_reason = parsed["delta"]["finish_reason"].as_str().unwrap_or("COMPLETE");
                                let usage = parse_cohere_usage(&parsed["delta"]["usage"]);

                                if text_started {
                                    yield Ok(StreamEvent::ContentBlockStop { index: text_index });
                                }
                                for (_, _, block_idx) in tool_calls.values() {
                                    yield Ok(StreamEvent::ContentBlockStop { index: *block_idx });
                                }

                                yield Ok(StreamEvent::MessageStop {
                                    stop_reason: parse_finish_reason(finish_reason),
                                });
                                yield Ok(StreamEvent::Metadata { usage, model: None, id: response_id.clone() });
                                return;
                            }

                            _ => {}
                        }
                    }

                    if text_started {
                        yield Ok(StreamEvent::ContentBlockStop { index: text_index });
                    }
                    for (_, _, block_idx) in tool_calls.values() {
                        yield Ok(StreamEvent::ContentBlockStop { index: *block_idx });
                    }
                    yield Ok(StreamEvent::MessageStop { stop_reason: StopReason::EndTurn });
                }

                CohereBackend::Bedrock { client: bedrock_client } => {
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
                        .body(Blob::new(body_bytes))
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

                    yield Ok(StreamEvent::MessageStart { role: Role::Assistant });

                    let mut text_started = false;
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

                                    match parsed["event_type"].as_str() {
                                        Some("text-generation") => {
                                            let text = parsed["text"].as_str().unwrap_or("");
                                            if !text.is_empty() {
                                                if !text_started {
                                                    yield Ok(StreamEvent::ContentBlockStart {
                                                        index: 0,
                                                        block: ContentBlockStart::Text,
                                                    });
                                                    text_started = true;
                                                }
                                                yield Ok(StreamEvent::ContentBlockDelta {
                                                    index: 0,
                                                    delta: ContentDelta::Text { text: text.to_string() },
                                                });
                                            }
                                        }

                                        Some("stream-end") => {
                                            if text_started {
                                                yield Ok(StreamEvent::ContentBlockStop { index: 0 });
                                            }

                                            let finish_reason = parsed["finish_reason"]
                                                .as_str()
                                                .unwrap_or("COMPLETE");
                                            let response = &parsed["response"];

                                            // Tool calls arrive in full at stream-end (Cohere v1 format)
                                            let mut next_block = if text_started { 1 } else { 0 };
                                            if let Some(tool_calls) = response["tool_calls"].as_array() {
                                                for tc in tool_calls {
                                                    let id = tc["id"].as_str().unwrap_or("").to_string();
                                                    let name = tc["name"].as_str().unwrap_or("").to_string();
                                                    let input = tc["parameters"].clone();
                                                    let input_json = input.to_string();

                                                    yield Ok(StreamEvent::ContentBlockStart {
                                                        index: next_block,
                                                        block: ContentBlockStart::ToolUse {
                                                            id,
                                                            name,
                                                        },
                                                    });
                                                    yield Ok(StreamEvent::ContentBlockDelta {
                                                        index: next_block,
                                                        delta: ContentDelta::ToolInput {
                                                            partial_json: input_json,
                                                        },
                                                    });
                                                    yield Ok(StreamEvent::ContentBlockStop {
                                                        index: next_block,
                                                    });
                                                    next_block += 1;
                                                }
                                            }

                                            // Cohere v1 stream-end puts usage under meta.billed_units
                                            let usage = parse_cohere_v1_usage(response);
                                            yield Ok(StreamEvent::MessageStop {
                                                stop_reason: parse_finish_reason(finish_reason),
                                            });
                                            yield Ok(StreamEvent::Metadata {
                                                usage,
                                                model: None,
                                                id: response["id"].as_str().map(|s| s.to_string()),
                                            });
                                            return;
                                        }

                                        _ => {}
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

                    if text_started {
                        yield Ok(StreamEvent::ContentBlockStop { index: 0 });
                    }
                    yield Ok(StreamEvent::MessageStop { stop_reason: StopReason::EndTurn });
                }
            }
        })
    }

    async fn complete(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<crate::types::Response, ProviderError> {
        match &self.backend {
            CohereBackend::Direct { api_key, base_url, .. } => {
                let body = build_request(&messages, &config, false)?;

                let mut req_builder = self
                    .client
                    .post(base_url)
                    .bearer_auth(api_key)
                    .json(&body);
                if let Some(ms) = config.timeout_ms {
                    req_builder = req_builder.timeout(std::time::Duration::from_millis(ms));
                }
                let resp = req_builder.send().await?;
                let resp = check_response(resp).await?;
                let json: Value = resp.json().await?;
                parse_response(&json)
            }

            CohereBackend::Bedrock { client } => {
                let body = build_bedrock_request(&messages, &config)?;
                let body_bytes = serde_json::to_vec(&body)
                    .map_err(|e| ProviderError::Serialization(e.to_string()))?;

                let fut = client
                    .invoke_model()
                    .model_id(&config.model)
                    .content_type("application/json")
                    .accept("application/json")
                    .body(Blob::new(body_bytes))
                    .send();

                let resp = if let Some(ms) = config.timeout_ms {
                    tokio::time::timeout(std::time::Duration::from_millis(ms), fut)
                        .await
                        .map_err(|_| ProviderError::Timeout { ms: Some(ms) })?
                        .map_err(|e| classify_bedrock_sdk_error(format!("{e:?}")))?
                } else {
                    fut.await.map_err(|e| classify_bedrock_sdk_error(format!("{e:?}")))?
                };

                let json: Value = serde_json::from_slice(resp.body.as_ref())
                    .map_err(|e| ProviderError::Serialization(e.to_string()))?;
                parse_bedrock_response(&json)
            }
        }
    }

    async fn count_tokens(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<TokenCount, ProviderError> {
        match &self.backend {
            CohereBackend::Direct { api_key, api_base, .. } => {
                let url = format!("{}/tokenize", api_base);

                let text: String = messages
                    .iter()
                    .flat_map(|m| &m.content)
                    .filter_map(|b| {
                        if let ContentBlock::Text(t) = b {
                            Some(t.text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ");

                let body = serde_json::json!({
                    "text": text,
                    "model": config.model,
                });

                let resp = self
                    .client
                    .post(&url)
                    .bearer_auth(api_key)
                    .json(&body)
                    .send()
                    .await?;
                let resp = check_response(resp).await?;
                let json: serde_json::Value = resp.json().await?;

                let input_tokens = json["tokens"]
                    .as_array()
                    .map(|a| a.len() as u64)
                    .unwrap_or(0);
                Ok(TokenCount { input_tokens })
            }
            CohereBackend::Bedrock { .. } => Err(ProviderError::Unsupported(
                "count_tokens is only available for the Direct Cohere backend".into(),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// EmbeddingProvider trait
// ---------------------------------------------------------------------------

#[async_trait]
impl EmbeddingProvider for CohereProvider {
    async fn embed(
        &self,
        request: EmbeddingRequest,
    ) -> Result<EmbeddingResponse, ProviderError> {
        let model = request.model.as_str();

        let input_type = request
            .task_type
            .as_ref()
            .map(|t| match t {
                EmbeddingTaskType::RetrievalQuery => "search_query",
                EmbeddingTaskType::RetrievalDocument => "search_document",
                // Cohere v2 does not have semantic_similarity; clustering is the closest
                EmbeddingTaskType::SemanticSimilarity => "clustering",
                EmbeddingTaskType::Classification => "classification",
                EmbeddingTaskType::Clustering => "clustering",
                EmbeddingTaskType::QuestionAnswering => "search_query",
                // Cohere v2 does not have fact_verification; classification is the closest
                EmbeddingTaskType::FactVerification => "classification",
                // Cohere v2 does not have a code type; use search_query
                EmbeddingTaskType::CodeRetrievalQuery => "search_query",
            })
            .unwrap_or("search_document");

        match &self.backend {
            CohereBackend::Direct { api_key, api_base, .. } => {
                let url = format!("{}/embed", api_base);

                let default_types = vec!["float".to_string()];
                let embedding_types = request.embedding_types.as_ref().unwrap_or(&default_types);
                let mut body = json!({
                    "model": model,
                    "texts": request.inputs,
                    "input_type": input_type,
                    "embedding_types": embedding_types,
                    "truncate": request.truncate.as_deref().unwrap_or("END"),
                });
                if let Some(dims) = request.dimensions {
                    body["output_dimension"] = json!(dims);
                }

                let resp = self
                    .client
                    .post(&url)
                    .bearer_auth(api_key)
                    .json(&body)
                    .send()
                    .await?;
                let resp = check_response(resp).await?;
                let json: Value = resp.json().await?;

                parse_cohere_embed_response(&json, model)
            }

            CohereBackend::Bedrock { client } => {
                let default_types = vec!["float".to_string()];
                let embedding_types = request.embedding_types.as_ref().unwrap_or(&default_types);
                let body = json!({
                    "texts": request.inputs,
                    "input_type": input_type,
                    "embedding_types": embedding_types,
                    "truncate": request.truncate.as_deref().unwrap_or("END"),
                });

                let body_bytes = serde_json::to_vec(&body)
                    .map_err(|e| ProviderError::Serialization(e.to_string()))?;

                let resp = client
                    .invoke_model()
                    .model_id(model)
                    .content_type("application/json")
                    .accept("application/json")
                    .body(Blob::new(body_bytes))
                    .send()
                    .await
                    .map_err(|e| classify_bedrock_sdk_error(format!("{e:?}")))?;

                let json: Value = serde_json::from_slice(resp.body.as_ref())
                    .map_err(|e| ProviderError::Serialization(e.to_string()))?;

                parse_cohere_embed_response(&json, model)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Direct v2 request building
// ---------------------------------------------------------------------------

fn build_request(
    messages: &[Message],
    config: &ProviderConfig,
    stream: bool,
) -> Result<Value, ProviderError> {
    let cohere_messages = format_messages(messages, config.system.as_deref())?;

    let mut req = json!({
        "model": config.model,
        "messages": cohere_messages,
        "stream": stream,
    });

    if let Some(max_tokens) = config.max_tokens {
        req["max_tokens"] = json!(max_tokens);
    }
    if let Some(temp) = config.temperature {
        req["temperature"] = json!(temp);
    }
    if let Some(top_p) = config.top_p {
        req["p"] = json!(top_p);
    }
    if let Some(top_k) = config.top_k {
        req["k"] = json!(top_k);
    }
    if let Some(seed) = config.seed {
        req["seed"] = json!(seed);
    }
    if !config.stop_sequences.is_empty() {
        req["stop_sequences"] = json!(config.stop_sequences);
    }
    if let Some(penalty) = config.presence_penalty {
        req["presence_penalty"] = json!(penalty);
    }
    if let Some(penalty) = config.frequency_penalty {
        req["frequency_penalty"] = json!(penalty);
    }

    if !config.tools.is_empty() {
        req["tools"] = format_tools(&config.tools);
    }
    if let Some(tc) = &config.tool_choice {
        req["tool_choice"] = format_tool_choice(tc);
    }

    match &config.response_format {
        Some(ResponseFormat::Json) => {
            req["response_format"] = json!({"type": "json_object"});
        }
        Some(ResponseFormat::JsonSchema { name, schema, .. }) => {
            req["response_format"] = json!({
                "type": "json_schema",
                "json_schema": { "name": name, "schema": schema }
            });
        }
        Some(ResponseFormat::Text) | None => {}
    }

    if let Some(budget) = config.thinking_budget {
        req["thinking"] = json!({"type": "enabled", "token_budget": budget});
    } else if config.include_thinking {
        req["thinking"] = json!({"type": "enabled"});
    }

    if config.logprobs == Some(true) || config.top_logprobs.is_some() {
        req["logprobs"] = json!(true);
    }

    for (k, v) in &config.extra {
        req[k] = v.clone();
    }

    Ok(req)
}

// ---------------------------------------------------------------------------
// Bedrock (Cohere v1 native) request building
// ---------------------------------------------------------------------------

fn build_bedrock_request(
    messages: &[Message],
    config: &ProviderConfig,
) -> Result<Value, ProviderError> {
    let (current_message, chat_history, tool_results) = format_bedrock_messages(messages);

    let mut req = json!({ "message": current_message });

    if !chat_history.is_empty() {
        req["chat_history"] = json!(chat_history);
    }

    // Collect preamble: config.system + any Role::System messages
    let mut preamble_parts = Vec::new();
    if let Some(s) = &config.system {
        preamble_parts.push(s.as_str());
    }
    for msg in messages {
        if msg.role == Role::System {
            for block in &msg.content {
                if let ContentBlock::Text(t) = block {
                    preamble_parts.push(t.text.as_str());
                }
            }
        }
    }
    if !preamble_parts.is_empty() {
        req["preamble"] = json!(preamble_parts.join("\n\n"));
    }

    if let Some(max_tokens) = config.max_tokens {
        req["max_tokens"] = json!(max_tokens);
    }
    if let Some(temp) = config.temperature {
        req["temperature"] = json!(temp);
    }
    if let Some(top_p) = config.top_p {
        req["p"] = json!(top_p);
    }
    if let Some(top_k) = config.top_k {
        req["k"] = json!(top_k);
    }
    if let Some(seed) = config.seed {
        req["seed"] = json!(seed);
    }
    if !config.stop_sequences.is_empty() {
        req["stop_sequences"] = json!(config.stop_sequences);
    }

    if !config.tools.is_empty() {
        req["tools"] = format_bedrock_tools(&config.tools);
    }

    if !tool_results.is_empty() {
        req["tool_results"] = json!(tool_results);
    }

    Ok(req)
}

/// Convert `Vec<Message>` to Cohere v1 Bedrock format.
///
/// Returns `(current_message, chat_history, tool_results)`.
fn format_bedrock_messages(messages: &[Message]) -> (String, Vec<Value>, Vec<Value>) {
    // Build a lookup from tool_use_id → (name, parameters) for populating tool_results.call
    let mut tool_use_map: HashMap<String, (String, Value)> = HashMap::new();
    for msg in messages {
        if msg.role == Role::Assistant {
            for block in &msg.content {
                if let ContentBlock::ToolUse(tu) = block {
                    tool_use_map.insert(tu.id.clone(), (tu.name.clone(), tu.input.clone()));
                }
            }
        }
    }

    let mut chat_history: Vec<Value> = Vec::new();
    let mut current_message = String::new();
    let mut tool_results: Vec<Value> = Vec::new();

    for (i, msg) in messages.iter().enumerate() {
        let is_last = i == messages.len() - 1;

        match &msg.role {
            Role::System => {
                // Handled separately as preamble; skip in chat_history
            }
            Role::User | Role::Tool | Role::Other(_) => {
                let has_tool_results = msg
                    .content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::ToolResult(_)));

                if has_tool_results {
                    let results: Vec<Value> = msg
                        .content
                        .iter()
                        .filter_map(|b| {
                            if let ContentBlock::ToolResult(tr) = b {
                                let output_text: String = tr
                                    .content
                                    .iter()
                                    .filter_map(|b| {
                                        if let ContentBlock::Text(t) = b {
                                            Some(t.text.as_str())
                                        } else {
                                            None
                                        }
                                    })
                                    .collect::<Vec<_>>()
                                    .join("\n");
                                let (name, params) = tool_use_map
                                    .get(&tr.tool_use_id)
                                    .map(|(n, p)| (n.as_str(), p.clone()))
                                    .unwrap_or(("", Value::Object(Default::default())));
                                Some(json!({
                                    "call": { "name": name, "parameters": params },
                                    "outputs": [{ "text": output_text }],
                                }))
                            } else {
                                None
                            }
                        })
                        .collect();

                    if is_last {
                        tool_results = results;
                        // Empty message signals a tool-continuation turn
                        current_message = String::new();
                    }
                    // Mid-conversation tool results are implicit via the preceding CHATBOT entry
                } else {
                    let text: String = msg
                        .content
                        .iter()
                        .filter_map(|b| {
                            if let ContentBlock::Text(t) = b {
                                Some(t.text.as_str())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("");

                    if is_last {
                        current_message = text;
                    } else {
                        chat_history.push(json!({ "role": "USER", "message": text }));
                    }
                }
            }
            Role::Assistant => {
                let tool_uses: Vec<_> = msg
                    .content
                    .iter()
                    .filter_map(|b| {
                        if let ContentBlock::ToolUse(tu) = b {
                            Some(tu)
                        } else {
                            None
                        }
                    })
                    .collect();
                let text: String = msg
                    .content
                    .iter()
                    .filter_map(|b| {
                        if let ContentBlock::Text(t) = b {
                            Some(t.text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("");

                let mut entry = json!({ "role": "CHATBOT", "message": text });
                if !tool_uses.is_empty() {
                    let tc: Vec<Value> = tool_uses
                        .iter()
                        .map(|tu| {
                            json!({
                                "id": tu.id,
                                "name": tu.name,
                                "parameters": tu.input,
                            })
                        })
                        .collect();
                    entry["tool_calls"] = json!(tc);
                }
                chat_history.push(entry);
            }
        }
    }

    (current_message, chat_history, tool_results)
}

/// Format tools for Cohere v1 (Bedrock): `parameter_definitions` instead of JSON Schema.
fn format_bedrock_tools(tools: &[Tool]) -> Value {
    json!(tools
        .iter()
        .map(|t| {
            let schema = &t.input_schema;
            let required: Vec<&str> = schema["required"]
                .as_array()
                .map(|r| r.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();

            let param_defs: serde_json::Map<String, Value> = schema["properties"]
                .as_object()
                .map(|props| {
                    props
                        .iter()
                        .map(|(name, prop)| {
                            let cohere_type = match prop["type"].as_str().unwrap_or("str") {
                                "string" => "str",
                                "integer" => "int",
                                "number" => "float",
                                "boolean" => "bool",
                                "array" => "List[str]",
                                "object" => "Dict",
                                other => other,
                            };
                            let desc =
                                prop["description"].as_str().unwrap_or("").to_string();
                            let is_required = required.contains(&name.as_str());
                            (
                                name.clone(),
                                json!({
                                    "description": desc,
                                    "type": cohere_type,
                                    "required": is_required,
                                }),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();

            json!({
                "name": t.name,
                "description": t.description,
                "parameter_definitions": param_defs,
            })
        })
        .collect::<Vec<_>>())
}

// ---------------------------------------------------------------------------
// Message formatting (Direct v2)
// ---------------------------------------------------------------------------

fn format_messages(messages: &[Message], system: Option<&str>) -> Result<Value, ProviderError> {
    let mut result = Vec::new();

    if let Some(sys) = system {
        result.push(json!({"role": "system", "content": sys}));
    }

    for msg in messages {
        let role = match &msg.role {
            Role::System => {
                result.push(
                    json!({"role": "system", "content": format_content_blocks(&msg.content)}),
                );
                continue;
            }
            Role::User | Role::Tool => "user",
            Role::Assistant => "assistant",
            Role::Other(s) => s.as_str(),
        };

        let has_tool_results = msg
            .content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolResult(_)));
        if role == "user" && has_tool_results {
            for block in &msg.content {
                if let ContentBlock::ToolResult(tr) = block {
                    let content_str: String = tr
                        .content
                        .iter()
                        .filter_map(|b| {
                            if let ContentBlock::Text(t) = b {
                                Some(t.text.as_str())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    result.push(json!({
                        "role": "tool",
                        "tool_call_id": tr.tool_use_id,
                        "content": content_str,
                    }));
                }
            }
            let other: Vec<&ContentBlock> = msg
                .content
                .iter()
                .filter(|b| !matches!(b, ContentBlock::ToolResult(_)))
                .collect();
            if !other.is_empty() {
                let text = other
                    .iter()
                    .filter_map(|b| {
                        if let ContentBlock::Text(t) = b {
                            Some(t.text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("");
                if !text.is_empty() {
                    result.push(json!({"role": "user", "content": text}));
                }
            }
            continue;
        }

        if role == "assistant" {
            let tool_uses: Vec<_> = msg
                .content
                .iter()
                .filter_map(|b| {
                    if let ContentBlock::ToolUse(tu) = b {
                        Some(tu)
                    } else {
                        None
                    }
                })
                .collect();
            if !tool_uses.is_empty() {
                let tool_calls: Vec<Value> = tool_uses
                    .iter()
                    .map(|tu| {
                        json!({
                            "id": tu.id,
                            "type": "function",
                            "function": {
                                "name": tu.name,
                                "arguments": tu.input.to_string(),
                            }
                        })
                    })
                    .collect();
                let text: String = msg
                    .content
                    .iter()
                    .filter_map(|b| {
                        if let ContentBlock::Text(t) = b {
                            Some(t.text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("");
                let mut am = json!({"role": "assistant", "tool_calls": tool_calls});
                if !text.is_empty() {
                    am["content"] = json!(text);
                }
                result.push(am);
                continue;
            }
        }

        let content = format_content_blocks(&msg.content);
        result.push(json!({"role": role, "content": content}));
    }

    Ok(json!(result))
}

/// Format content blocks for Cohere v2 chat.
/// Returns a plain string when text-only, or an array when images are present.
fn format_content_blocks(blocks: &[ContentBlock]) -> serde_json::Value {
    let has_images = blocks.iter().any(|b| matches!(b, ContentBlock::Image(_)));
    if !has_images {
        let text: String = blocks
            .iter()
            .filter_map(|b| {
                if let ContentBlock::Text(t) = b {
                    Some(t.text.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("");
        return json!(text);
    }
    let parts: Vec<serde_json::Value> = blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text(t) if !t.text.is_empty() => {
                Some(json!({"type": "text", "text": t.text}))
            }
            ContentBlock::Image(img) => format_cohere_image(img),
            _ => None,
        })
        .collect();
    json!(parts)
}

fn format_cohere_image(img: &ImageContent) -> Option<serde_json::Value> {
    match &img.source {
        MediaSource::Url(url) => Some(json!({
            "type": "image_url",
            "image_url": {"url": url}
        })),
        MediaSource::Base64(b64) => {
            let data_url = format!("data:{};base64,{}", b64.media_type, b64.data);
            Some(json!({
                "type": "image_url",
                "image_url": {"url": data_url}
            }))
        }
        _ => None,
    }
}

fn format_tools(tools: &[Tool]) -> Value {
    json!(
        tools
            .iter()
            .map(|t| json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.input_schema,
                }
            }))
            .collect::<Vec<_>>()
    )
}

fn format_tool_choice(tc: &ToolChoice) -> Value {
    match tc {
        ToolChoice::Auto => json!("auto"),
        ToolChoice::Any => json!("required"),
        ToolChoice::None => json!("none"),
        ToolChoice::Tool { .. } | ToolChoice::AllowedTools { .. } => json!("required"),
    }
}

// ---------------------------------------------------------------------------
// Response parsing
// ---------------------------------------------------------------------------

fn parse_response(json: &Value) -> Result<crate::types::Response, ProviderError> {
    let message = &json["message"];
    let finish_reason = json["finish_reason"].as_str().unwrap_or("COMPLETE");

    let mut content: Vec<ContentBlock> = Vec::new();

    if let Some(content_arr) = message["content"].as_array() {
        for block in content_arr {
            if block["type"].as_str() == Some("text")
                && let Some(text) = block["text"].as_str()
                && !text.is_empty()
            {
                content.push(ContentBlock::text(text));
            }
        }
    }

    if let Some(tool_calls) = message["tool_calls"].as_array() {
        for tc in tool_calls {
            let id = tc["id"].as_str().unwrap_or("").to_string();
            let name = tc["function"]["name"].as_str().unwrap_or("").to_string();
            let args_str = tc["function"]["arguments"].as_str().unwrap_or("{}");
            let input = serde_json::from_str(args_str).unwrap_or(Value::Null);
            content.push(ContentBlock::ToolUse(ToolUseBlock { id, name, input }));
        }
    }

    let usage = parse_cohere_usage(&json["usage"]);
    let stop_reason = parse_finish_reason(finish_reason);
    let model = json["model"].as_str().map(|s| s.to_string());
    let id = json["id"].as_str().map(|s| s.to_string());
    let logprobs = parse_logprobs(&json["logprobs"]);

    Ok(crate::types::Response {
        content,
        usage: usage.with_totals(),
        stop_reason,
        model,
        id,
        container: None,
        logprobs,
        grounding_metadata: None,
        warnings: vec![],
        request_body: None,
    })
}

/// Parse a Cohere v1 (Bedrock) response.
///
/// Differences from v2: `text` (not `message.content`), `parameters` (not `function.arguments`),
/// usage under `meta.billed_units` (not `usage.billed_units`).
fn parse_bedrock_response(json: &Value) -> Result<crate::types::Response, ProviderError> {
    let finish_reason = json["finish_reason"].as_str().unwrap_or("COMPLETE");
    let mut content: Vec<ContentBlock> = Vec::new();

    if let Some(text) = json["text"].as_str()
        && !text.is_empty()
    {
        content.push(ContentBlock::text(text));
    }

    if let Some(tool_calls) = json["tool_calls"].as_array() {
        for tc in tool_calls {
            let id = tc["id"].as_str().unwrap_or("").to_string();
            let name = tc["name"].as_str().unwrap_or("").to_string();
            // v1: parameters is a plain object, not a JSON string
            let input = tc["parameters"].clone();
            content.push(ContentBlock::ToolUse(ToolUseBlock { id, name, input }));
        }
    }

    // Cohere v1 puts usage under meta.billed_units (v2 uses usage.billed_units)
    let usage = parse_cohere_v1_usage(json);
    let stop_reason = parse_finish_reason(finish_reason);

    Ok(crate::types::Response {
        content,
        usage: usage.with_totals(),
        stop_reason,
        model: None,
        id: json["id"].as_str().map(|s| s.to_string()),
        container: None,
        logprobs: None,
        grounding_metadata: None,
        warnings: vec![],
        request_body: None,
    })
}

fn parse_cohere_embed_response(
    json: &Value,
    model: &str,
) -> Result<EmbeddingResponse, ProviderError> {
    let mut embeddings: Vec<Vec<f32>> = Vec::new();
    if let Some(float_arr) = json["embeddings"]["float"].as_array() {
        for vec_val in float_arr {
            if let Some(vec_arr) = vec_val.as_array() {
                let vec: Vec<f32> = vec_arr
                    .iter()
                    .filter_map(|v| v.as_f64().map(|f| f as f32))
                    .collect();
                embeddings.push(vec);
            }
        }
    }

    let input_tokens = json["meta"]["billed_units"]["input_tokens"]
        .as_u64()
        .or_else(|| json["usage"]["billed_units"]["input_tokens"].as_u64())
        .unwrap_or(0);
    let usage = Usage {
        input_tokens,
        ..Default::default()
    };

    Ok(EmbeddingResponse {
        embeddings,
        model: Some(model.to_string()),
        usage,
    })
}

/// Parse usage from a Cohere Bedrock response. Checks all known field locations:
/// - `meta.billed_units` (v1 streaming stream-end response)
/// - `usage.billed_units` / `usage.tokens` (v2-style invoke_model response)
/// - `token_count` (v1 non-streaming fallback)
fn parse_cohere_v1_usage(json: &Value) -> Usage {
    let input = json["meta"]["billed_units"]["input_tokens"]
        .as_u64()
        .or_else(|| json["usage"]["billed_units"]["input_tokens"].as_u64())
        .or_else(|| json["usage"]["tokens"]["input_tokens"].as_u64())
        .or_else(|| json["token_count"]["prompt_tokens"].as_u64())
        .unwrap_or(0);
    let output = json["meta"]["billed_units"]["output_tokens"]
        .as_u64()
        .or_else(|| json["usage"]["billed_units"]["output_tokens"].as_u64())
        .or_else(|| json["usage"]["tokens"]["output_tokens"].as_u64())
        .or_else(|| json["token_count"]["response_tokens"].as_u64())
        .unwrap_or(0);
    Usage { input_tokens: input, output_tokens: output, ..Default::default() }
}

fn parse_cohere_usage(usage: &Value) -> Usage {
    let tokens = &usage["tokens"];
    let billed = &usage["billed_units"];

    let input = tokens["input_tokens"]
        .as_u64()
        .or_else(|| billed["input_tokens"].as_u64())
        .unwrap_or(0);
    let output = tokens["output_tokens"]
        .as_u64()
        .or_else(|| billed["output_tokens"].as_u64())
        .unwrap_or(0);

    Usage {
        input_tokens: input,
        output_tokens: output,
        ..Default::default()
    }
}

fn parse_finish_reason(reason: &str) -> StopReason {
    match reason {
        "COMPLETE" => StopReason::EndTurn,
        "MAX_TOKENS" => StopReason::MaxTokens,
        "TOOL_CALL" => StopReason::ToolUse,
        "STOP_SEQUENCE" => StopReason::StopSequence(String::new()),
        "ERROR" => StopReason::Other("error".to_string()),
        "TIMEOUT" => StopReason::Other("timeout".to_string()),
        other => {
            tracing::debug!("Cohere: unknown finish_reason {:?}, mapping to Other", other);
            StopReason::Other(other.to_string())
        }
    }
}

/// Parse Cohere v2 logprobs into our `TokenLogprob` format.
///
/// Cohere returns `[{text, token_ids, logprobs}, ...]` — each item is a text chunk
/// with one logprob per token ID. We expand them into one `TokenLogprob` per token.
fn parse_logprobs(val: &Value) -> Option<Vec<TokenLogprob>> {
    let items = val.as_array()?;
    if items.is_empty() {
        return None;
    }
    let mut result = Vec::new();
    for item in items {
        let text = item["text"].as_str().unwrap_or("");
        let lps = item["logprobs"].as_array().map(|a| a.as_slice()).unwrap_or(&[]);
        let ids = item["token_ids"].as_array().map(|a| a.len()).unwrap_or(1).max(1);
        for (i, lp) in lps.iter().enumerate().take(ids) {
            result.push(TokenLogprob {
                token: if i == 0 { text.to_string() } else { String::new() },
                logprob: lp.as_f64().unwrap_or(0.0),
                bytes: None,
                top_logprobs: vec![],
            });
        }
        if lps.is_empty() {
            result.push(TokenLogprob {
                token: text.to_string(),
                logprob: 0.0,
                bytes: None,
                top_logprobs: vec![],
            });
        }
    }
    if result.is_empty() { None } else { Some(result) }
}

// ---------------------------------------------------------------------------
// Bedrock error classification
// ---------------------------------------------------------------------------

fn classify_bedrock_sdk_error(msg: String) -> ProviderError {
    if msg.contains("ThrottlingException") || msg.to_lowercase().contains("throttl") {
        ProviderError::TooManyRequests { message: msg, retry_after_secs: None }
    } else if msg.contains("ModelTimeoutException") {
        ProviderError::Timeout { ms: None }
    } else {
        ProviderError::Api { status: 0, message: msg }
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_build_request_basic() {
        let config = ProviderConfig::new("command-r-plus").with_max_tokens(512);
        let messages = vec![Message::user("Hello")];
        let req = build_request(&messages, &config, false).unwrap();
        assert_eq!(req["model"], "command-r-plus");
        assert_eq!(req["max_tokens"], 512);
        assert_eq!(req["stream"], false);
        assert_eq!(req["messages"][0]["role"], "user");
        assert_eq!(req["messages"][0]["content"], "Hello");
    }

    #[test]
    fn test_system_injection() {
        let config = ProviderConfig::new("command-r-plus")
            .with_system("Be helpful")
            .with_max_tokens(256);
        let messages = vec![Message::user("Hi")];
        let req = build_request(&messages, &config, false).unwrap();
        assert_eq!(req["messages"][0]["role"], "system");
        assert_eq!(req["messages"][0]["content"], "Be helpful");
        assert_eq!(req["messages"][1]["role"], "user");
    }

    #[test]
    fn test_tool_choice_required() {
        use crate::types::Tool;
        let config = ProviderConfig::new("command-r-plus")
            .with_tools(vec![Tool::new(
                "search",
                "Search the web",
                json!({"type": "object"}),
            )])
            .with_tool_choice(ToolChoice::Any);
        let messages = vec![Message::user("Search something")];
        let req = build_request(&messages, &config, false).unwrap();
        assert_eq!(req["tool_choice"], "required");
        assert_eq!(req["tools"][0]["function"]["name"], "search");
    }

    #[test]
    fn test_parse_response_text() {
        let json = json!({
            "id": "chat-123",
            "finish_reason": "COMPLETE",
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": "Hello there!"}]
            },
            "usage": {
                "billed_units": {"input_tokens": 10, "output_tokens": 5},
                "tokens": {"input_tokens": 10, "output_tokens": 5}
            }
        });
        let resp = parse_response(&json).unwrap();
        assert!(matches!(&resp.content[0], ContentBlock::Text(t) if t == "Hello there!"));
        assert_eq!(resp.usage.input_tokens, 10);
        assert_eq!(resp.usage.output_tokens, 5);
        assert!(matches!(resp.stop_reason, StopReason::EndTurn));
    }

    #[test]
    fn test_parse_response_tool_call() {
        let json = json!({
            "id": "chat-456",
            "finish_reason": "TOOL_CALL",
            "message": {
                "role": "assistant",
                "content": [],
                "tool_calls": [{
                    "id": "tc_abc",
                    "type": "function",
                    "function": {
                        "name": "get_weather",
                        "arguments": "{\"location\":\"Paris\"}"
                    }
                }]
            },
            "usage": {
                "billed_units": {"input_tokens": 20, "output_tokens": 10},
                "tokens": {"input_tokens": 20, "output_tokens": 10}
            }
        });
        let resp = parse_response(&json).unwrap();
        assert!(matches!(&resp.content[0], ContentBlock::ToolUse(tu) if tu.name == "get_weather"));
        assert!(matches!(resp.stop_reason, StopReason::ToolUse));
    }

    #[test]
    fn test_parse_finish_reasons() {
        assert!(matches!(parse_finish_reason("COMPLETE"), StopReason::EndTurn));
        assert!(matches!(parse_finish_reason("MAX_TOKENS"), StopReason::MaxTokens));
        assert!(matches!(parse_finish_reason("TOOL_CALL"), StopReason::ToolUse));
        assert!(matches!(parse_finish_reason("STOP_SEQUENCE"), StopReason::StopSequence(_)));
        assert!(matches!(parse_finish_reason("ERROR"), StopReason::Other(s) if s == "error"));
    }

    #[test]
    fn test_with_api_base() {
        let provider = CohereProvider::new("key").with_api_base("https://proxy.example.com/v2");
        if let CohereBackend::Direct { base_url, api_base, .. } = &provider.backend {
            assert_eq!(base_url, "https://proxy.example.com/v2/chat");
            assert_eq!(api_base, "https://proxy.example.com/v2");
        } else {
            panic!("expected Direct backend");
        }
    }

    #[test]
    fn test_tool_result_formatting() {
        use crate::types::{ContentBlock, ToolResultBlock};
        let messages = vec![
            Message::user("Call the tool"),
            Message::with_content(
                Role::Assistant,
                vec![ContentBlock::ToolUse(ToolUseBlock {
                    id: "tc_1".into(),
                    name: "search".into(),
                    input: json!({"query": "rust"}),
                })],
            ),
            Message::with_content(
                Role::User,
                vec![ContentBlock::ToolResult(ToolResultBlock {
                    tool_use_id: "tc_1".into(),
                    content: vec![ContentBlock::text("Results here")],
                    is_error: false,
                })],
            ),
        ];
        let formatted = format_messages(&messages, None).unwrap();
        let arr = formatted.as_array().unwrap();
        let tool_msg = arr.iter().find(|m| m["role"] == "tool").unwrap();
        assert_eq!(tool_msg["tool_call_id"], "tc_1");
        assert_eq!(tool_msg["content"], "Results here");
    }

    #[test]
    fn test_build_bedrock_request_basic() {
        let config = ProviderConfig::new("cohere.command-r-v1:0").with_max_tokens(256);
        let messages = vec![Message::user("Hello")];
        let req = build_bedrock_request(&messages, &config).unwrap();
        assert_eq!(req["message"], "Hello");
        assert_eq!(req["max_tokens"], 256);
        assert!(!req.as_object().unwrap().contains_key("model"));
        assert!(!req.as_object().unwrap().contains_key("stream"));
    }

    #[test]
    fn test_build_bedrock_request_system() {
        let config = ProviderConfig::new("cohere.command-r-v1:0")
            .with_system("You are helpful")
            .with_max_tokens(256);
        let messages = vec![Message::user("Hi")];
        let req = build_bedrock_request(&messages, &config).unwrap();
        assert_eq!(req["preamble"], "You are helpful");
        assert_eq!(req["message"], "Hi");
    }

    #[test]
    fn test_build_bedrock_request_chat_history() {
        let config = ProviderConfig::new("cohere.command-r-v1:0").with_max_tokens(256);
        let messages = vec![
            Message::user("What is 2+2?"),
            Message::assistant("4"),
            Message::user("And 3+3?"),
        ];
        let req = build_bedrock_request(&messages, &config).unwrap();
        assert_eq!(req["message"], "And 3+3?");
        let history = req["chat_history"].as_array().unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0]["role"], "USER");
        assert_eq!(history[0]["message"], "What is 2+2?");
        assert_eq!(history[1]["role"], "CHATBOT");
        assert_eq!(history[1]["message"], "4");
    }

    #[test]
    fn test_format_bedrock_tools() {
        let tools = vec![Tool::new(
            "get_weather",
            "Get the weather",
            json!({
                "type": "object",
                "properties": {
                    "location": {
                        "type": "string",
                        "description": "The city"
                    }
                },
                "required": ["location"]
            }),
        )];
        let formatted = format_bedrock_tools(&tools);
        let tool = &formatted[0];
        assert_eq!(tool["name"], "get_weather");
        let param = &tool["parameter_definitions"]["location"];
        assert_eq!(param["type"], "str");
        assert_eq!(param["required"], true);
        assert_eq!(param["description"], "The city");
    }

    #[test]
    fn test_parse_bedrock_response_text() {
        let json = json!({
            "id": "b1",
            "text": "Hello!",
            "finish_reason": "COMPLETE",
            "usage": {
                "billed_units": { "input_tokens": 5, "output_tokens": 3 }
            }
        });
        let resp = parse_bedrock_response(&json).unwrap();
        assert!(matches!(&resp.content[0], ContentBlock::Text(t) if t == "Hello!"));
        assert_eq!(resp.usage.input_tokens, 5);
        assert_eq!(resp.usage.output_tokens, 3);
    }

    #[test]
    fn test_parse_bedrock_response_tool_call() {
        let json = json!({
            "id": "b2",
            "text": "",
            "finish_reason": "TOOL_CALL",
            "tool_calls": [{
                "id": "tc_1",
                "name": "get_weather",
                "parameters": { "location": "Paris" }
            }],
            "usage": {
                "billed_units": { "input_tokens": 15, "output_tokens": 8 }
            }
        });
        let resp = parse_bedrock_response(&json).unwrap();
        assert!(matches!(&resp.content[0], ContentBlock::ToolUse(tu) if tu.name == "get_weather" && tu.input["location"] == "Paris"));
        assert!(matches!(resp.stop_reason, StopReason::ToolUse));
    }

    #[tokio::test]
    async fn test_integration_complete() {
        let api_key = match std::env::var("COHERE_API_KEY") {
            Ok(k) => k,
            Err(_) => {
                println!("Skipping: COHERE_API_KEY not set");
                return;
            }
        };
        let provider = CohereProvider::new(api_key);
        let config = ProviderConfig::new("command-r-08-2024").with_max_tokens(64);
        let messages = vec![Message::user("Say 'hello' in one word.")];
        let resp = provider.complete(messages, config).await.unwrap();
        assert!(!resp.content.is_empty());
    }

    #[tokio::test]
    async fn test_integration_stream() {
        let api_key = match std::env::var("COHERE_API_KEY") {
            Ok(k) => k,
            Err(_) => {
                println!("Skipping: COHERE_API_KEY not set");
                return;
            }
        };
        use crate::provider::collect_stream;
        let provider = CohereProvider::new(api_key);
        let config = ProviderConfig::new("command-r-08-2024").with_max_tokens(64);
        let messages = vec![Message::user("Say 'hi'.")];
        let stream = provider.stream(messages, config);
        let resp = collect_stream(stream).await.unwrap();
        assert!(!resp.content.is_empty());
    }
}
