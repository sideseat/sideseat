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
        Ok(Self::new(crate::env::require(
            crate::env::keys::COHERE_API_KEY,
        )?))
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
        if let CohereBackend::Direct {
            ref mut base_url,
            ref mut api_base,
            ..
        } = self.backend
        {
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
            CohereBackend::Direct {
                api_key, api_base, ..
            } => {
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

                            "content-end" if text_started => {
                                let idx = parsed["index"].as_u64().unwrap_or(0) as usize;
                                if idx == 0 {
                                    yield Ok(StreamEvent::ContentBlockStop { index: text_index });
                                    text_started = false;
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
            CohereBackend::Direct {
                api_key, base_url, ..
            } => {
                let body = build_request(&messages, &config, false)?;

                let mut req_builder = self.client.post(base_url).bearer_auth(api_key).json(&body);
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
                    fut.await
                        .map_err(|e| classify_bedrock_sdk_error(format!("{e:?}")))?
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
            CohereBackend::Direct {
                api_key, api_base, ..
            } => {
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
    async fn embed(&self, request: EmbeddingRequest) -> Result<EmbeddingResponse, ProviderError> {
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
            CohereBackend::Direct {
                api_key, api_base, ..
            } => {
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

mod protocol;
use protocol::{
    build_bedrock_request, build_request, classify_bedrock_sdk_error, parse_bedrock_response,
    parse_cohere_embed_response, parse_cohere_usage, parse_cohere_v1_usage, parse_finish_reason,
    parse_response,
};
