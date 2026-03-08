use std::sync::Arc;

use async_stream::stream;
use async_trait::async_trait;
use serde_json::{Value, json};

use crate::{
    error::ProviderError,
    provider::{ChatProvider, EmbeddingProvider, ImageProvider, Provider, ProviderStream, VideoProvider},
    providers::sse::{check_response, sse_data_stream},
    types::{
        AudioContent, ContentBlock, ContentBlockStart, ContentDelta, DocumentContent,
        EmbeddingRequest, EmbeddingResponse, EmbeddingTaskType, GeneratedImage, GeneratedVideo,
        GroundingChunk, GroundingMetadata, ImageContent, ImageGenerationRequest,
        ImageGenerationResponse, MediaSource, Message, ModelInfo, ProviderConfig, ResponseFormat,
        Role, StaticTokenProvider, StopReason, StreamEvent, TokenCount, TokenProvider, ToolUseBlock,
        Usage, VideoContent, VideoGenerationRequest,
        VideoGenerationResponse,
    },
};

const GEMINI_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta/models";
const VERTEX_BASE_URL: &str = "https://{location}-aiplatform.googleapis.com/v1/projects/{project}/locations/{location}/publishers/google/models";

/// Authentication and endpoint variant for the Gemini provider.
///
/// Note: the `VertexAI` variant holds an `Arc<dyn TokenProvider>` — Clone is cheap (ref-count bump).
#[derive(Clone)]
pub enum GeminiAuth {
    /// Google AI Studio: API key passed as query param `?key=...`
    ApiKey(String),
    /// Vertex AI: OAuth2 Bearer token with project + location.
    /// Use [`GeminiProvider::from_vertex`] or [`GeminiProvider::from_vertex_with_token_provider`]
    /// to construct; the token is fetched per-request to support rotation.
    VertexAI {
        project_id: String,
        location: String,
        token_provider: Arc<dyn TokenProvider>,
    },
}

/// Google Gemini / Vertex AI provider.
///
/// Supports both Google AI Studio (API key) and Vertex AI (OAuth2),
/// with full streaming, tool calling, multi-modal inputs, and thinking.
///
/// # Examples
///
/// ## Google AI Studio
/// ```no_run
/// use sideseat::{providers::{GeminiProvider, GeminiAuth}, ProviderConfig, Provider, Message};
///
/// let provider = GeminiProvider::new(GeminiAuth::ApiKey("your-api-key".into()));
/// let config = ProviderConfig::new("gemini-2.5-flash").with_max_tokens(1024);
/// ```
///
/// ## Vertex AI
/// ```no_run
/// use sideseat::providers::GeminiProvider;
///
/// let provider = GeminiProvider::from_vertex("my-project", "us-central1", "ya29.xxx");
/// ```
pub struct GeminiProvider {
    auth: GeminiAuth,
    client: Arc<reqwest::Client>,
    /// Optional API base URL override (for proxies / LiteLLM Google AI pass-through).
    api_base: Option<String>,
}

impl GeminiProvider {
    /// Create a provider from the `GEMINI_API_KEY` or `GOOGLE_API_KEY` environment variable.
    pub fn from_env() -> Result<Self, ProviderError> {
        let key = crate::env::require_any(&[
            crate::env::keys::GEMINI_API_KEY,
            crate::env::keys::GOOGLE_API_KEY,
        ])?;
        Ok(Self::new(GeminiAuth::ApiKey(key)))
    }

    pub fn new(auth: GeminiAuth) -> Self {
        Self {
            auth,
            client: Arc::new(reqwest::Client::new()),
            api_base: None,
        }
    }

    /// Create a Vertex AI provider using a static OAuth2 access token.
    ///
    /// The token is wrapped in a [`StaticTokenProvider`]. For automatic token rotation,
    /// use [`from_vertex_with_token_provider`](Self::from_vertex_with_token_provider) instead.
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

    /// Create a Vertex AI provider with a custom [`TokenProvider`] for token rotation.
    pub fn from_vertex_with_token_provider(
        project_id: impl Into<String>,
        location: impl Into<String>,
        token_provider: Arc<dyn TokenProvider>,
    ) -> Self {
        Self::new(GeminiAuth::VertexAI {
            project_id: project_id.into(),
            location: location.into(),
            token_provider,
        })
    }

    /// Replace the HTTP client. Useful for custom TLS, proxies, or testing.
    pub fn with_client(mut self, client: reqwest::Client) -> Self {
        self.client = Arc::new(client);
        self
    }

    /// Override the base URL for use with LiteLLM or other Gemini-compatible proxies.
    ///
    /// The model name and method suffix (`:generateContent`, `:streamGenerateContent`) are
    /// appended automatically, e.g. `{base}/{model}:generateContent`.
    ///
    /// ```no_run
    /// use sideseat::providers::{GeminiProvider, GeminiAuth};
    /// // LiteLLM pass-through
    /// let p = GeminiProvider::new(GeminiAuth::ApiKey("key".into()))
    ///     .with_api_base("http://0.0.0.0:4000");
    /// ```
    pub fn with_api_base(mut self, api_base: impl Into<String>) -> Self {
        self.api_base = Some(api_base.into());
        self
    }

    fn build_url(&self, model: &str, streaming: bool) -> String {
        let method = if streaming {
            "streamGenerateContent"
        } else {
            "generateContent"
        };
        let url = self.build_model_url(model, method);
        if streaming {
            // Gemini streamGenerateContent requires alt=sse to return Server-Sent Events
            // format. Without it, the response is a raw JSON array that eventsource_stream
            // cannot parse.
            if url.contains('?') {
                format!("{}&alt=sse", url)
            } else {
                format!("{}?alt=sse", url)
            }
        } else {
            url
        }
    }

    /// Builds a URL for any model-specific method (countTokens, embedContent, etc.).
    /// Respects the custom `api_base` if set.
    fn build_model_url(&self, model: &str, method: &str) -> String {
        if let Some(base) = &self.api_base {
            return format!("{}/{model}:{method}", base.trim_end_matches('/'));
        }
        match &self.auth {
            GeminiAuth::ApiKey(key) => {
                format!("{}/{}:{}?key={}", GEMINI_BASE_URL, model, method, key)
            }
            GeminiAuth::VertexAI {
                project_id,
                location,
                ..
            } => {
                let base = VERTEX_BASE_URL
                    .replace("{location}", location)
                    .replace("{project}", project_id);
                format!("{}/{}:{}", base, model, method)
            }
        }
    }

    /// Builds the base URL for listing models.
    /// Respects the custom `api_base` if set.
    fn build_list_models_url(&self) -> String {
        if let Some(base) = &self.api_base {
            return format!("{}/models", base.trim_end_matches('/'));
        }
        match &self.auth {
            GeminiAuth::ApiKey(key) => format!("{}?key={}", GEMINI_BASE_URL, key),
            GeminiAuth::VertexAI {
                project_id,
                location,
                ..
            } => VERTEX_BASE_URL
                .replace("{location}", location)
                .replace("{project}", project_id),
        }
    }

    async fn add_auth_header(
        &self,
        builder: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, ProviderError> {
        match &self.auth {
            GeminiAuth::ApiKey(_) => Ok(builder), // API key in URL
            GeminiAuth::VertexAI { token_provider, .. } => {
                let token = token_provider.get_token().await?;
                Ok(builder.bearer_auth(token))
            }
        }
    }

    fn build_request(
        &self,
        messages: &[Message],
        config: &ProviderConfig,
    ) -> Result<Value, ProviderError> {
        let contents = format_contents(messages)?;

        let mut req = json!({ "contents": contents });

        // System instruction
        if let Some(system) = &config.system {
            req["systemInstruction"] = json!({
                "parts": [{"text": system}]
            });
        }

        // Generation config
        let mut gen_config = json!({});
        if let Some(max_tokens) = config.max_tokens {
            gen_config["maxOutputTokens"] = json!(max_tokens);
        }
        if let Some(temp) = config.temperature {
            gen_config["temperature"] = json!(temp);
        }
        if let Some(top_p) = config.top_p {
            gen_config["topP"] = json!(top_p);
        }
        if let Some(top_k) = config.top_k {
            gen_config["topK"] = json!(top_k);
        }
        if !config.stop_sequences.is_empty() {
            gen_config["stopSequences"] = json!(config.stop_sequences);
        }

        // Thinking config
        if let Some(budget) = config.thinking_budget {
            gen_config["thinkingConfig"] = json!({
                "thinkingBudget": budget,
                "includeThoughts": config.include_thinking,
            });
        } else if config.include_thinking {
            gen_config["thinkingConfig"] = json!({"includeThoughts": true});
        }

        // Response format
        match &config.response_format {
            Some(ResponseFormat::Json) => {
                gen_config["responseMimeType"] = json!("application/json");
            }
            Some(ResponseFormat::JsonSchema { schema, .. }) => {
                gen_config["responseMimeType"] = json!("application/json");
                gen_config["responseSchema"] = schema.clone();
            }
            _ => {}
        }

        if gen_config
            .as_object()
            .map(|o| !o.is_empty())
            .unwrap_or(false)
        {
            req["generationConfig"] = gen_config;
        }

        // Generation config: presence/frequency penalty
        if config.presence_penalty.is_some() || config.frequency_penalty.is_some() {
            let gen_cfg = req["generationConfig"].as_object_mut();
            if let Some(gc) = gen_cfg {
                if let Some(pp) = config.presence_penalty {
                    gc.insert("presencePenalty".to_string(), json!(pp));
                }
                if let Some(fp) = config.frequency_penalty {
                    gc.insert("frequencyPenalty".to_string(), json!(fp));
                }
            } else {
                let mut gc = serde_json::Map::new();
                if let Some(pp) = config.presence_penalty {
                    gc.insert("presencePenalty".to_string(), json!(pp));
                }
                if let Some(fp) = config.frequency_penalty {
                    gc.insert("frequencyPenalty".to_string(), json!(fp));
                }
                req["generationConfig"] = Value::Object(gc);
            }
        }

        // Tools (function declarations + optional web search)
        let mut all_tools: Vec<Value> = Vec::new();
        if !config.tools.is_empty() {
            let function_decls: Vec<Value> = config
                .tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema,
                    })
                })
                .collect();
            all_tools.push(json!({"functionDeclarations": function_decls}));
        }
        // Web search → googleSearch tool
        if config.web_search.is_some() {
            all_tools.push(json!({"googleSearch": {}}));
        }
        if !all_tools.is_empty() {
            req["tools"] = json!(all_tools);
        }

        // Safety settings
        if !config.safety_settings.is_empty() {
            let safety: Vec<Value> = config
                .safety_settings
                .iter()
                .map(|s| {
                    json!({
                        "category": s.category.as_str(),
                        "threshold": s.threshold.as_str(),
                    })
                })
                .collect();
            req["safetySettings"] = json!(safety);
        }

        // Tool config
        if let Some(tc) = &config.tool_choice {
            let (mode, allowed) = match tc {
                crate::types::ToolChoice::Auto => ("AUTO", None),
                crate::types::ToolChoice::Any => ("ANY", None),
                crate::types::ToolChoice::None => ("NONE", None),
                crate::types::ToolChoice::Tool { name } => ("ANY", Some(vec![name.as_str()])),
                // AllowedTools maps naturally to Gemini's allowedFunctionNames
                crate::types::ToolChoice::AllowedTools { tools } => {
                    ("ANY", Some(tools.iter().map(|s| s.as_str()).collect::<Vec<_>>()))
                }
            };
            let mut fc_config = json!({"mode": mode});
            if let Some(names) = allowed {
                fc_config["allowedFunctionNames"] = json!(names);
            }
            req["toolConfig"] = json!({"functionCallingConfig": fc_config});
        }

        Ok(req)
    }
}

#[async_trait]
impl Provider for GeminiProvider {
    fn provider_name(&self) -> &'static str {
        "google"
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let url = self.build_list_models_url();

        let mut req_builder = self.client.get(&url);
        req_builder = self.add_auth_header(req_builder).await?;
        let resp = req_builder.send().await?;
        let resp = check_response(resp).await?;
        let json: Value = resp.json().await?;

        let mut models = Vec::new();
        if let Some(arr) = json["models"].as_array() {
            for item in arr {
                let full_name = item["name"].as_str().unwrap_or("");
                let id = full_name
                    .strip_prefix("models/")
                    .unwrap_or(full_name)
                    .to_string();
                models.push(ModelInfo {
                    id,
                    display_name: item["displayName"].as_str().map(|s| s.to_string()),
                    description: item["description"].as_str().map(|s| s.to_string()),
                    created_at: None,
                });
            }
        }
        Ok(models)
    }
}

#[async_trait]
impl ChatProvider for GeminiProvider {
    fn stream(&self, messages: Vec<Message>, config: ProviderConfig) -> ProviderStream {
        let auth = self.auth.clone();
        let client = Arc::clone(&self.client);
        let api_base = self.api_base.clone();

        Box::pin(stream! {
            let provider = GeminiProvider { auth: auth.clone(), client: Arc::clone(&client), api_base };
            let body = match provider.build_request(&messages, &config) {
                Ok(b) => b,
                Err(e) => { yield Err(e); return; }
            };

            let url = provider.build_url(&config.model, true);
            let mut req_builder = client.post(&url).json(&body);
            req_builder = match provider.add_auth_header(req_builder).await {
                Ok(b) => b,
                Err(e) => { yield Err(e); return; }
            };
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

            // Gemini streaming: each SSE data chunk is a full GenerateContentResponse
            // We accumulate text and function calls across chunks
            let mut text_block_started = false;
            let text_index: usize = 0;
            let mut next_index: usize = 1; // 0 reserved for text

            let mut stream = Box::pin(sse_data_stream(resp));
            use futures::StreamExt;

            while let Some(result) = stream.next().await {
                let data = match result {
                    Ok(d) => d,
                    Err(e) => { yield Err(e); return; }
                };

                let chunk: Value = match serde_json::from_str(&data) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                let candidates = match chunk["candidates"].as_array() {
                    Some(c) if !c.is_empty() => c,
                    _ => continue,
                };
                let candidate = &candidates[0];
                let parts = match candidate["content"]["parts"].as_array() {
                    Some(p) => p,
                    None => continue,
                };

                for part in parts {
                    if let Some(thought) = part.get("thought").and_then(|v| v.as_bool())
                        && thought {
                            // Thinking part
                            if let Some(text) = part["text"].as_str() {
                                let idx = next_index;
                                next_index += 1;
                                yield Ok(StreamEvent::ContentBlockStart {
                                    index: idx,
                                    block: ContentBlockStart::Thinking,
                                });
                                yield Ok(StreamEvent::ContentBlockDelta {
                                    index: idx,
                                    delta: ContentDelta::Thinking { text: text.to_string() },
                                });
                                yield Ok(StreamEvent::ContentBlockStop { index: idx });
                            }
                            continue;
                        }

                    if let Some(text) = part["text"].as_str() {
                        if !text_block_started {
                            yield Ok(StreamEvent::ContentBlockStart {
                                index: text_index,
                                block: ContentBlockStart::Text,
                            });
                            text_block_started = true;
                        }
                        yield Ok(StreamEvent::ContentBlockDelta {
                            index: text_index,
                            delta: ContentDelta::Text { text: text.to_string() },
                        });
                    } else if let Some(func_call) = part.get("functionCall") {
                        let name = func_call["name"].as_str().unwrap_or("").to_string();
                        let block_idx = next_index;
                        next_index += 1;
                        let args_str = func_call["args"].to_string();
                        let id = format!("gemini_fc_{}", block_idx);

                        yield Ok(StreamEvent::ContentBlockStart {
                            index: block_idx,
                            block: ContentBlockStart::ToolUse { id, name },
                        });
                        yield Ok(StreamEvent::ContentBlockDelta {
                            index: block_idx,
                            delta: ContentDelta::ToolInput { partial_json: args_str },
                        });
                        yield Ok(StreamEvent::ContentBlockStop { index: block_idx });
                    } else if let Some(inline_data) = part.get("inlineData")
                        && let (Some(media_type), Some(data)) = (
                            inline_data["mimeType"].as_str(),
                            inline_data["data"].as_str(),
                        )
                    {
                        let idx = next_index;
                        next_index += 1;
                        yield Ok(StreamEvent::InlineData {
                            index: idx,
                            media_type: media_type.to_string(),
                            b64_data: data.to_string(),
                        });
                    }
                }

                // Check for finish
                if let Some(finish_reason) = candidate["finishReason"].as_str() {
                    if text_block_started {
                        yield Ok(StreamEvent::ContentBlockStop { index: text_index });
                    }

                    let stop_reason = parse_gemini_finish_reason(finish_reason);
                    let usage = parse_gemini_usage(&chunk["usageMetadata"]);
                    let model = chunk["modelVersion"].as_str().map(|s| s.to_string());
                    yield Ok(StreamEvent::MessageStop { stop_reason });
                    yield Ok(StreamEvent::Metadata { usage, model, id: None });
                    return;
                }
            }

            // Stream ended without finishReason
            if text_block_started {
                yield Ok(StreamEvent::ContentBlockStop { index: text_index });
            }
            yield Ok(StreamEvent::MessageStop { stop_reason: StopReason::EndTurn });
        })
    }

    async fn complete(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<crate::types::Response, ProviderError> {
        let body = self.build_request(&messages, &config)?;
        let url = self.build_url(&config.model, false);
        let mut req_builder = self.client.post(&url).json(&body);
        req_builder = self.add_auth_header(req_builder).await?;
        if let Some(ms) = config.timeout_ms {
            req_builder = req_builder.timeout(std::time::Duration::from_millis(ms));
        }

        let resp = req_builder.send().await?;
        let resp = check_response(resp).await?;
        let json: Value = resp.json().await?;
        parse_gemini_response(&json)
    }

    async fn count_tokens(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<TokenCount, ProviderError> {
        let body = self.build_request(&messages, &config)?;

        // countTokens only accepts "contents" (and optionally "tools"); generationConfig is rejected
        let mut count_body = json!({"contents": body["contents"]});
        if let Some(tools) = body.get("tools") {
            count_body["tools"] = tools.clone();
        }

        let url = self.build_model_url(&config.model, "countTokens");

        let mut req_builder = self.client.post(&url).json(&count_body);
        req_builder = self.add_auth_header(req_builder).await?;
        let resp = req_builder.send().await?;
        let resp = check_response(resp).await?;
        let json: Value = resp.json().await?;

        Ok(TokenCount {
            input_tokens: json["totalTokens"].as_u64().unwrap_or(0),
        })
    }

}

#[async_trait]
impl EmbeddingProvider for GeminiProvider {
    async fn embed(
        &self,
        request: EmbeddingRequest,
    ) -> Result<EmbeddingResponse, ProviderError> {
        let model = &request.model;
        if request.inputs.is_empty() {
            return Ok(EmbeddingResponse {
                embeddings: vec![],
                model: None,
                usage: Usage::default(),
            });
        }

        let task_type_str = request.task_type.as_ref().map(|t| match t {
            EmbeddingTaskType::RetrievalQuery => "RETRIEVAL_QUERY",
            EmbeddingTaskType::RetrievalDocument => "RETRIEVAL_DOCUMENT",
            EmbeddingTaskType::SemanticSimilarity => "SEMANTIC_SIMILARITY",
            EmbeddingTaskType::Classification => "CLASSIFICATION",
            EmbeddingTaskType::Clustering => "CLUSTERING",
            EmbeddingTaskType::QuestionAnswering => "QUESTION_ANSWERING",
            EmbeddingTaskType::FactVerification => "FACT_VERIFICATION",
            EmbeddingTaskType::CodeRetrievalQuery => "CODE_RETRIEVAL_QUERY",
        });

        // Use batchEmbedContents for multiple inputs, embedContent for single
        if request.inputs.len() == 1 {
            let text = &request.inputs[0];
            let mut content = json!({"parts": [{"text": text}]});
            if let Some(tt) = task_type_str {
                content["taskType"] = json!(tt);
            }
            if let Some(dims) = request.dimensions {
                content["outputDimensionality"] = json!(dims);
            }
            let body = json!({"content": content});

            let url = self.build_model_url(model, "embedContent");

            let mut req_builder = self.client.post(&url).json(&body);
            req_builder = self.add_auth_header(req_builder).await?;
            let resp = req_builder.send().await?;
            let resp = check_response(resp).await?;
            let json: Value = resp.json().await?;

            let embedding: Vec<f32> = json["embedding"]["values"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter_map(|v| v.as_f64().map(|f| f as f32))
                .collect();

            Ok(EmbeddingResponse {
                embeddings: vec![embedding],
                model: Some(model.clone()),
                usage: Usage::default(),
            })
        } else {
            // Batch embed
            let requests: Vec<Value> = request
                .inputs
                .iter()
                .map(|text| {
                    let mut req = json!({
                        "model": format!("models/{}", model),
                        "content": {"parts": [{"text": text}]},
                    });
                    if let Some(tt) = task_type_str {
                        req["taskType"] = json!(tt);
                    }
                    if let Some(dims) = request.dimensions {
                        req["outputDimensionality"] = json!(dims);
                    }
                    req
                })
                .collect();

            let body = json!({"requests": requests});

            let url = self.build_model_url(model, "batchEmbedContents");

            let mut req_builder = self.client.post(&url).json(&body);
            req_builder = self.add_auth_header(req_builder).await?;
            let resp = req_builder.send().await?;
            let resp = check_response(resp).await?;
            let json: Value = resp.json().await?;

            let mut embeddings: Vec<Vec<f32>> = Vec::new();
            if let Some(arr) = json["embeddings"].as_array() {
                for emb in arr {
                    let vec: Vec<f32> = emb["values"]
                        .as_array()
                        .unwrap_or(&vec![])
                        .iter()
                        .filter_map(|v| v.as_f64().map(|f| f as f32))
                        .collect();
                    embeddings.push(vec);
                }
            }

            Ok(EmbeddingResponse {
                embeddings,
                model: Some(model.clone()),
                usage: Usage::default(),
            })
        }
    }
}

#[async_trait]
impl ImageProvider for GeminiProvider {
    /// Generate images using the Imagen API (`{model}:predict`).
    ///
    /// Supported models: `imagen-3.0-generate-001`, `imagen-3.0-fast-generate-001`.
    async fn generate_image(
        &self,
        request: ImageGenerationRequest,
    ) -> Result<ImageGenerationResponse, ProviderError> {
        let url = self.build_model_url(&request.model, "predict");

        let aspect_ratio = request
            .size
            .as_ref()
            .map(|s| s.as_aspect_ratio())
            .unwrap_or("1:1");

        let mut params = json!({
            "sampleCount": request.n.unwrap_or(1),
            "aspectRatio": aspect_ratio,
        });
        if let Some(seed) = request.seed {
            params["seed"] = json!(seed);
        }
        let body = json!({
            "instances": [{"prompt": request.prompt}],
            "parameters": params,
        });

        let mut req_builder = self.client.post(&url).json(&body);
        req_builder = self.add_auth_header(req_builder).await?;
        let resp = req_builder.send().await?;
        let resp = check_response(resp).await?;
        let json: Value = resp.json().await?;

        let images = json["predictions"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .map(|pred| GeneratedImage {
                url: None,
                b64_json: pred["bytesBase64Encoded"].as_str().map(|s| s.to_string()),
                revised_prompt: None,
            })
            .collect();

        Ok(ImageGenerationResponse { images })
    }

}

#[async_trait]
impl VideoProvider for GeminiProvider {
    /// Generate videos using the Veo API (`{model}:predictLongRunning`), with polling.
    ///
    /// Supported models: `veo-2.0-generate-001`.
    /// Polls every 5 seconds for up to 5 minutes.
    async fn generate_video(
        &self,
        request: VideoGenerationRequest,
    ) -> Result<VideoGenerationResponse, ProviderError> {
        let url = self.build_model_url(&request.model, "predictLongRunning");

        let mut params = json!({ "sampleCount": request.n.unwrap_or(1) });
        if let Some(ar) = &request.aspect_ratio {
            params["aspectRatio"] = json!(ar.as_str());
        }
        if let Some(dur) = request.duration_secs {
            params["durationSeconds"] = json!(dur);
        }
        if let Some(res) = &request.resolution {
            params["resolution"] = json!(res.as_str());
        }

        let body = json!({
            "instances": [{"prompt": request.prompt}],
            "parameters": params,
        });

        let mut req_builder = self.client.post(&url).json(&body);
        req_builder = self.add_auth_header(req_builder).await?;
        let resp = req_builder.send().await?;
        let resp = check_response(resp).await?;
        let op_json: Value = resp.json().await?;

        let op_name = op_json["name"].as_str().ok_or_else(|| ProviderError::Api {
            status: 200,
            message: "No operation name in predictLongRunning response".into(),
        })?;

        let poll_url = self.build_operation_url(op_name);

        // Poll up to 60 times at 5-second intervals (5 minutes total).
        for _ in 0..60 {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;

            let mut poll_req = self.client.get(&poll_url);
            poll_req = self.add_auth_header(poll_req).await?;
            let resp = poll_req.send().await?;
            let resp = check_response(resp).await?;
            let status: Value = resp.json().await?;

            if status["done"].as_bool().unwrap_or(false) {
                let samples = status["response"]["generateVideoResponse"]["generatedSamples"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();

                let videos = samples
                    .iter()
                    .map(|s| GeneratedVideo {
                        uri: s["video"]["uri"].as_str().map(|u| u.to_string()),
                        b64_json: s["video"]["bytesBase64Encoded"]
                            .as_str()
                            .map(|b| b.to_string()),
                        duration_secs: None,
                    })
                    .collect();

                return Ok(VideoGenerationResponse { videos });
            }
        }

        Err(ProviderError::Timeout { ms: Some(300_000) })
    }
}

impl GeminiProvider {
    fn build_operation_url(&self, op_name: &str) -> String {
        match &self.auth {
            GeminiAuth::ApiKey(key) => {
                format!(
                    "https://generativelanguage.googleapis.com/v1beta/{}?key={}",
                    op_name, key
                )
            }
            GeminiAuth::VertexAI { location, .. } => {
                format!(
                    "https://{}-aiplatform.googleapis.com/v1/{}",
                    location, op_name
                )
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Request formatting helpers
// ---------------------------------------------------------------------------

fn format_contents(messages: &[Message]) -> Result<Value, ProviderError> {
    let mut result: Vec<Value> = Vec::new();

    for msg in messages {
        let role = match &msg.role {
            Role::System => continue, // System handled via systemInstruction
            Role::User | Role::Tool => "user",
            Role::Assistant => "model",
            Role::Other(s) => s.as_str(),
        };

        let parts = format_parts(&msg.content)?;
        result.push(json!({"role": role, "parts": parts}));
    }

    Ok(json!(result))
}

fn format_parts(blocks: &[ContentBlock]) -> Result<Value, ProviderError> {
    let parts: Result<Vec<Value>, _> = blocks.iter().map(format_part).collect();
    Ok(json!(parts?))
}

fn format_part(block: &ContentBlock) -> Result<Value, ProviderError> {
    match block {
        ContentBlock::Text(t) => Ok(json!({"text": t.text})),
        ContentBlock::Image(img) => format_image_part(img),
        ContentBlock::Audio(audio) => format_audio_part(audio),
        ContentBlock::Video(video) => format_video_part(video),
        ContentBlock::Document(doc) => format_document_part(doc),
        ContentBlock::ToolUse(tu) => Ok(json!({
            "functionCall": {
                "name": tu.name,
                "args": tu.input,
            }
        })),
        ContentBlock::ToolResult(tr) => {
            // Gemini tool results: functionResponse part
            let content = tr
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
            Ok(json!({
                "functionResponse": {
                    "name": tr.tool_use_id, // Gemini uses function name, not ID
                    "response": {"result": content},
                }
            }))
        }
        ContentBlock::Thinking(th) => Ok(json!({"text": th.text, "thought": true})),
    }
}

fn format_image_part(img: &ImageContent) -> Result<Value, ProviderError> {
    match &img.source {
        MediaSource::Base64(b64) => Ok(json!({
            "inlineData": {
                "mimeType": b64.media_type,
                "data": b64.data,
            }
        })),
        MediaSource::FileUri { uri, media_type } => Ok(json!({
            "fileData": {
                "mimeType": media_type,
                "fileUri": uri,
            }
        })),
        MediaSource::S3(s3) => Ok(json!({
            "fileData": {
                "fileUri": s3.uri,
            }
        })),
        MediaSource::Url(url) => {
            // Vertex AI supports HTTP URLs for images
            Ok(json!({
                "fileData": {
                    "fileUri": url,
                }
            }))
        }
        _ => Err(ProviderError::Unsupported(
            "Unsupported image source type for Gemini".into(),
        )),
    }
}

fn format_audio_part(audio: &AudioContent) -> Result<Value, ProviderError> {
    match &audio.source {
        MediaSource::Base64(b64) => Ok(json!({
            "inlineData": {
                "mimeType": b64.media_type,
                "data": b64.data,
            }
        })),
        MediaSource::FileUri { uri, media_type } => Ok(json!({
            "fileData": {"mimeType": media_type, "fileUri": uri}
        })),
        MediaSource::Url(url) => Ok(json!({"fileData": {"fileUri": url}})),
        _ => Err(ProviderError::Unsupported(
            "Unsupported audio source type for Gemini".into(),
        )),
    }
}

fn format_video_part(video: &VideoContent) -> Result<Value, ProviderError> {
    match &video.source {
        MediaSource::Base64(b64) => Ok(json!({
            "inlineData": {"mimeType": b64.media_type, "data": b64.data}
        })),
        MediaSource::FileUri { uri, media_type } => Ok(json!({
            "fileData": {"mimeType": media_type, "fileUri": uri}
        })),
        MediaSource::Url(url) => Ok(json!({"fileData": {"fileUri": url}})),
        _ => Err(ProviderError::Unsupported(
            "Unsupported video source type for Gemini".into(),
        )),
    }
}

fn format_document_part(doc: &DocumentContent) -> Result<Value, ProviderError> {
    match &doc.source {
        MediaSource::Base64(b64) => Ok(json!({
            "inlineData": {"mimeType": b64.media_type, "data": b64.data}
        })),
        MediaSource::FileUri { uri, media_type } => Ok(json!({
            "fileData": {"mimeType": media_type, "fileUri": uri}
        })),
        _ => Err(ProviderError::Unsupported(
            "Gemini documents require base64 or Files API URI".into(),
        )),
    }
}

// ---------------------------------------------------------------------------
// Response parsing helpers
// ---------------------------------------------------------------------------

fn parse_gemini_response(json: &Value) -> Result<crate::types::Response, ProviderError> {
    let candidates = json["candidates"].as_array().ok_or_else(|| {
        ProviderError::Serialization("Missing 'candidates' in Gemini response".into())
    })?;

    if candidates.is_empty() {
        return Err(ProviderError::Api {
            status: 0,
            message: "Empty candidates in Gemini response".into(),
        });
    }

    let candidate = &candidates[0];
    // `content.parts` may be absent for SAFETY blocks, MAX_TOKENS hit before any output, or
    // recitation stops. Treat as empty rather than erroring — the caller gets an empty Response
    // with the correct stop_reason (Safety, Length, etc.).
    let empty = vec![];
    let parts = candidate["content"]["parts"].as_array().unwrap_or(&empty);

    let mut content: Vec<ContentBlock> = Vec::new();
    for part in parts {
        if let Some(text) = part["text"].as_str() {
            let is_thought = part["thought"].as_bool().unwrap_or(false);
            if is_thought {
                content.push(ContentBlock::Thinking(crate::types::ThinkingBlock {
                    text: text.to_string(),
                    signature: None,
                }));
            } else {
                content.push(ContentBlock::text(text));
            }
        } else if let Some(func_call) = part.get("functionCall") {
            let name = func_call["name"].as_str().unwrap_or("").to_string();
            let args = func_call["args"].clone();
            let tool_idx = content
                .iter()
                .filter(|b| matches!(b, ContentBlock::ToolUse(_)))
                .count()
                + 1;
            let id = format!("gemini_fc_{}", tool_idx);
            content.push(ContentBlock::ToolUse(ToolUseBlock {
                id,
                name,
                input: args,
            }));
        }
    }

    let finish_reason = candidate["finishReason"].as_str().unwrap_or("STOP");
    let stop_reason = parse_gemini_finish_reason(finish_reason);
    let usage = parse_gemini_usage(&json["usageMetadata"]);
    let model = json["modelVersion"].as_str().map(|s| s.to_string());
    let grounding_metadata = parse_grounding_metadata(&candidate["groundingMetadata"]);

    Ok(crate::types::Response {
        content,
        usage: usage.with_totals(),
        stop_reason,
        model,
        id: None,
        container: None,
        logprobs: None,
        grounding_metadata,
        warnings: vec![],
    })
}

fn parse_grounding_metadata(val: &Value) -> Option<GroundingMetadata> {
    if val.is_null() || !val.is_object() {
        return None;
    }
    let chunks = val["groundingChunks"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|c| GroundingChunk {
                    title: c["web"]["title"].as_str().map(|s| s.to_string()),
                    uri: c["web"]["uri"].as_str().map(|s| s.to_string()),
                })
                .collect()
        })
        .unwrap_or_default();
    let search_queries = val["webSearchQueries"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|q| q.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    Some(GroundingMetadata {
        chunks,
        search_queries,
    })
}

fn parse_gemini_usage(usage: &Value) -> Usage {
    Usage {
        input_tokens: usage["promptTokenCount"].as_u64().unwrap_or(0),
        output_tokens: usage["candidatesTokenCount"].as_u64().unwrap_or(0),
        cache_read_tokens: usage["cachedContentTokenCount"].as_u64().unwrap_or(0),
        reasoning_tokens: usage["thoughtsTokenCount"].as_u64().unwrap_or(0),
        total_tokens: usage["totalTokenCount"].as_u64().unwrap_or(0),
        ..Default::default()
    }
}

fn parse_gemini_finish_reason(reason: &str) -> StopReason {
    match reason {
        "STOP" | "FINISH_REASON_STOP" => StopReason::EndTurn,
        "MAX_TOKENS" | "FINISH_REASON_MAX_TOKENS" => StopReason::MaxTokens,
        "SAFETY" | "FINISH_REASON_SAFETY" => StopReason::ContentFilter,
        "MALFORMED_FUNCTION_CALL" => StopReason::Other("malformed_function_call".into()),
        other => StopReason::Other(other.to_lowercase()),
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Tool, ToolChoice};
    use serde_json::json;

    #[test]
    fn test_build_gemini_url_api_key() {
        let provider = GeminiProvider::new(GeminiAuth::ApiKey("mykey".into()));
        let url = provider.build_url("gemini-2.5-flash", false);
        assert!(url.contains("generativelanguage.googleapis.com"));
        assert!(url.contains("gemini-2.5-flash"));
        assert!(url.contains("generateContent"));
        assert!(url.contains("key=mykey"));
    }

    #[test]
    fn test_build_vertex_url() {
        let provider = GeminiProvider::from_vertex("my-project", "us-central1", "token");
        let url = provider.build_url("gemini-2.5-flash", true);
        assert!(url.contains("aiplatform.googleapis.com"));
        assert!(url.contains("us-central1"));
        assert!(url.contains("my-project"));
        assert!(url.contains("streamGenerateContent"));
    }

    #[test]
    fn test_build_request_with_system() {
        let provider = GeminiProvider::new(GeminiAuth::ApiKey("key".into()));
        let config = ProviderConfig::new("gemini-2.5-flash")
            .with_system("You are helpful")
            .with_max_tokens(512);
        let messages = vec![Message::user("Hello")];
        let req = provider.build_request(&messages, &config).unwrap();
        assert_eq!(
            req["systemInstruction"]["parts"][0]["text"],
            "You are helpful"
        );
        assert_eq!(req["generationConfig"]["maxOutputTokens"], 512);
    }

    #[test]
    fn test_build_request_with_tools() {
        let provider = GeminiProvider::new(GeminiAuth::ApiKey("key".into()));
        let config = ProviderConfig::new("gemini-2.5-flash")
            .with_tools(vec![Tool::new(
                "search",
                "Search",
                json!({"type": "object", "properties": {}}),
            )])
            .with_tool_choice(ToolChoice::Auto);
        let messages = vec![Message::user("Search something")];
        let req = provider.build_request(&messages, &config).unwrap();
        assert_eq!(req["tools"][0]["functionDeclarations"][0]["name"], "search");
        assert_eq!(req["toolConfig"]["functionCallingConfig"]["mode"], "AUTO");
    }

    #[test]
    fn test_build_request_thinking() {
        let provider = GeminiProvider::new(GeminiAuth::ApiKey("key".into()));
        let mut config = ProviderConfig::new("gemini-2.5-pro").with_max_tokens(4096);
        config.thinking_budget = Some(2048);
        config.include_thinking = true;
        let messages = vec![Message::user("Hard problem")];
        let req = provider.build_request(&messages, &config).unwrap();
        assert_eq!(
            req["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            2048
        );
        assert_eq!(
            req["generationConfig"]["thinkingConfig"]["includeThoughts"],
            true
        );
    }

    #[test]
    fn test_parse_response() {
        let json = json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{"text": "Paris is the capital of France."}]
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 8,
                "totalTokenCount": 18
            },
            "modelVersion": "gemini-2.5-flash-001"
        });
        let resp = parse_gemini_response(&json).unwrap();
        assert!(matches!(&resp.content[0], ContentBlock::Text(t) if t.contains("Paris")));
        assert_eq!(resp.usage.input_tokens, 10);
        assert_eq!(resp.usage.output_tokens, 8);
        assert_eq!(resp.model.as_deref(), Some("gemini-2.5-flash-001"));
    }

    #[test]
    fn test_parse_function_call_response() {
        let json = json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": [{
                        "functionCall": {
                            "name": "get_weather",
                            "args": {"location": "Paris"}
                        }
                    }]
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": {"promptTokenCount": 15, "candidatesTokenCount": 5, "totalTokenCount": 20}
        });
        let resp = parse_gemini_response(&json).unwrap();
        assert!(matches!(&resp.content[0], ContentBlock::ToolUse(tu) if tu.name == "get_weather"));
    }

    #[tokio::test]
    async fn test_integration_complete() {
        let api_key = match std::env::var("GEMINI_API_KEY") {
            Ok(k) => k,
            Err(_) => {
                println!("Skipping: GEMINI_API_KEY not set");
                return;
            }
        };
        let provider = GeminiProvider::new(GeminiAuth::ApiKey(api_key));
        let config = ProviderConfig::new("gemini-2.0-flash").with_max_tokens(64);
        let messages = vec![Message::user("Say 'hello' in one word.")];
        let resp = provider.complete(messages, config).await.unwrap();
        assert!(!resp.content.is_empty());
        assert!(resp.usage.output_tokens > 0);
    }

    #[tokio::test]
    async fn test_integration_stream() {
        let api_key = match std::env::var("GEMINI_API_KEY") {
            Ok(k) => k,
            Err(_) => {
                println!("Skipping: GEMINI_API_KEY not set");
                return;
            }
        };
        use crate::provider::collect_stream;
        let provider = GeminiProvider::new(GeminiAuth::ApiKey(api_key));
        let config = ProviderConfig::new("gemini-2.0-flash").with_max_tokens(64);
        let messages = vec![Message::user("Say 'hi'.")];
        let stream = provider.stream(messages, config);
        let resp = collect_stream(stream).await.unwrap();
        assert!(!resp.content.is_empty());
    }
}
