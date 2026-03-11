use std::sync::Arc;
use std::time::Duration;

use async_stream::stream;
use async_trait::async_trait;
use aws_sdk_bedrock::Client as BedrockMgmtClient;
use aws_sdk_bedrockruntime::{
    Client,
    primitives::Blob,
    types::{
        AnyToolChoice, AudioBlock, AudioFormat as BAudioFmt, AudioSource as BAudioSource,
        AutoToolChoice,
        BidirectionalInputPayloadPart, CachePointBlock, CachePointType,
        ContentBlock as BContent, ContentBlockDelta,
        ContentBlockStart as BContentBlockStart, ConversationRole, ConverseOutput,
        ConverseStreamOutput, ConverseTokensRequest, CountTokensInput, DocumentBlock,
        DocumentFormat, DocumentSource, GuardrailConfiguration, GuardrailStreamConfiguration,
        GuardrailTrace, ImageBlock, ImageFormat, ImageSource, InferenceConfiguration,
        InvokeModelWithBidirectionalStreamInput,
        InvokeModelWithBidirectionalStreamOutput as BidiStreamEvent, Message as BMessage,
        PerformanceConfigLatency, PerformanceConfiguration, PromptVariableValues,
        ReasoningContentBlock, ReasoningTextBlock, S3Location, SpecificToolChoice,
        StopReason as BStopReason, SystemContentBlock, SystemTool, Tool as BTool,
        ToolChoice as BToolChoice, ToolConfiguration, ToolInputSchema,
        ToolResultBlock as BToolResult, ToolResultContentBlock, ToolResultStatus,
        ToolSpecification, ToolUseBlock as BToolUse, VideoBlock, VideoFormat, VideoSource,
    },
};
use aws_sdk_bedrockruntime::types::error::InvokeModelWithBidirectionalStreamInputError;
use aws_smithy_http::event_stream::EventStreamSender;
use aws_smithy_types::Document;
use serde_json::{Value, json};

use crate::{
    error::ProviderError,
    provider::{AudioProvider, ChatProvider, EmbeddingProvider, ImageProvider, Provider, ProviderStream, VideoProvider},
    types::{
        AudioFormat as CAudioFmt, ContentBlock, ContentBlockStart, ContentDelta,
        DocumentFormat as CDocFmt,
        EmbeddingRequest, EmbeddingResponse, GeneratedImage, GeneratedVideo,
        ImageFormat as CImgFmt, ImageGenerationRequest, ImageGenerationResponse, MediaSource,
        Message, ModelInfo, ProviderConfig, ReasoningEffort, Role, SpeechRequest, SpeechResponse,
        StopReason, StreamEvent, ThinkingBlock, TokenCount, ToolChoice, ToolUseBlock,
        TranscriptionRequest, TranscriptionResponse, Usage, VideoFormat as CVidFmt,
        VideoGenerationRequest, VideoGenerationResponse,
    },
};

// ---------------------------------------------------------------------------
// Provider struct
// ---------------------------------------------------------------------------

/// AWS Bedrock Converse / ConverseStream API provider.
///
/// Supports all Bedrock foundation models via the unified Converse API.
///
/// # Authentication options
/// - `from_env()` — default AWS credential chain (env vars, instance profile, etc.)
/// - `with_static_credentials()` — explicit access key ID + secret access key
/// - `with_profile()` — named AWS profile from `~/.aws/credentials`
/// - `from_client()` — wrap an existing `aws_sdk_bedrockruntime::Client`
/// - `with_api_key()` — Bedrock API key via the SDK's native bearer-token auth
/// - `from_static_assume_role()` — static base credentials + explicit STS AssumeRole
/// - `from_ambient_assume_role()` — ambient credentials (EC2/ECS/IRSA) + STS AssumeRole
pub struct BedrockProvider {
    client: Arc<Client>,
    /// Bedrock management client (for listing models). None only when created via [`from_client()`](Self::from_client).
    mgmt_client: Option<Arc<BedrockMgmtClient>>,
}

impl BedrockProvider {
    /// Create using the default AWS credential chain.
    pub async fn from_env(region: impl Into<String>) -> Result<Self, ProviderError> {
        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_config::Region::new(region.into()))
            .load()
            .await;
        Ok(Self {
            client: Arc::new(Client::new(&config)),
            mgmt_client: Some(Arc::new(BedrockMgmtClient::new(&config))),
        })
    }

    /// Create with explicit static credentials.
    pub async fn with_static_credentials(
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
        session_token: Option<String>,
        region: impl Into<String>,
    ) -> Result<Self, ProviderError> {
        use aws_credential_types::Credentials;
        let creds = Credentials::new(
            access_key_id,
            secret_access_key,
            session_token,
            None,
            "sideseat-static",
        );
        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_config::Region::new(region.into()))
            .credentials_provider(creds)
            .load()
            .await;
        Ok(Self {
            client: Arc::new(Client::new(&config)),
            mgmt_client: Some(Arc::new(BedrockMgmtClient::new(&config))),
        })
    }

    /// Create using a named AWS profile.
    pub async fn with_profile(profile_name: impl Into<String>, region: impl Into<String>) -> Result<Self, ProviderError> {
        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_config::Region::new(region.into()))
            .profile_name(profile_name)
            .load()
            .await;
        Ok(Self {
            client: Arc::new(Client::new(&config)),
            mgmt_client: Some(Arc::new(BedrockMgmtClient::new(&config))),
        })
    }

    /// Wrap an existing `aws_sdk_bedrockruntime::Client`.
    /// Note: `list_models()` is unavailable when using this constructor.
    pub fn from_client(client: Client) -> Self {
        Self {
            client: Arc::new(client),
            mgmt_client: None,
        }
    }

    /// Create from an existing runtime client and management client (for testing).
    pub fn from_clients(client: Client, mgmt_client: BedrockMgmtClient) -> Self {
        Self {
            client: Arc::new(client),
            mgmt_client: Some(Arc::new(mgmt_client)),
        }
    }

    /// Create using a Bedrock API key.
    ///
    /// Configures the AWS SDK client with bearer-token authentication
    /// (`Authorization: Bearer <api_key>`). No custom HTTP client is needed —
    /// the SDK handles everything natively.
    ///
    /// See: <https://docs.aws.amazon.com/bedrock/latest/userguide/api-keys.html>
    pub fn with_api_key(api_key: impl Into<String>, region: impl Into<String>) -> Self {
        use aws_sdk_bedrockruntime::config::{BehaviorVersion, Region, Token};
        let api_key = api_key.into();
        let region = region.into();
        let conf = aws_sdk_bedrockruntime::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new(region.clone()))
            .bearer_token(Token::new(api_key.clone(), None))
            .build();
        let mgmt_conf = aws_sdk_bedrock::config::Builder::new()
            .behavior_version(aws_sdk_bedrock::config::BehaviorVersion::latest())
            .region(aws_sdk_bedrock::config::Region::new(region))
            .bearer_token(aws_sdk_bedrock::config::Token::new(api_key, None))
            .build();
        Self {
            client: Arc::new(Client::from_conf(conf)),
            mgmt_client: Some(Arc::new(BedrockMgmtClient::from_conf(mgmt_conf))),
        }
    }

    /// Create using static credentials + STS AssumeRole.
    ///
    /// Constructs a static base credential then configures STS to assume `role_arn`.
    /// The actual STS call is lazy — it happens on the first credential use.
    pub async fn from_static_assume_role(
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
        session_token: Option<String>,
        role_arn: impl Into<String>,
        external_id: Option<String>,
        region: impl Into<String>,
    ) -> Result<Self, ProviderError> {
        use aws_config::sts::AssumeRoleProvider;
        use aws_credential_types::Credentials;
        let region_val = aws_config::Region::new(region.into());
        let base = Credentials::new(
            access_key_id,
            secret_access_key,
            session_token,
            None,
            "sideseat",
        );
        let mut builder = AssumeRoleProvider::builder(role_arn.into())
            .region(region_val.clone())
            .session_name("sideseat");
        if let Some(ext) = external_id {
            builder = builder.external_id(ext);
        }
        // build_from_provider returns a Future resolved to AssumeRoleProvider
        let assume_provider = builder.build_from_provider(base).await;
        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(region_val)
            .credentials_provider(assume_provider)
            .load()
            .await;
        Ok(Self {
            client: Arc::new(Client::new(&config)),
            mgmt_client: Some(Arc::new(BedrockMgmtClient::new(&config))),
        })
    }

    /// Create using ambient credentials (EC2/ECS/IRSA) + STS AssumeRole.
    ///
    /// Loads the default AWS credential chain as the base identity, then
    /// wraps it in an `AssumeRoleProvider` for cross-account access.
    /// The base credential chain is resolved during construction (`build().await`).
    pub async fn from_ambient_assume_role(
        role_arn: impl Into<String>,
        external_id: Option<String>,
        region: impl Into<String>,
    ) -> Result<Self, ProviderError> {
        use aws_config::sts::AssumeRoleProvider;
        let region_val = aws_config::Region::new(region.into());
        let mut builder = AssumeRoleProvider::builder(role_arn.into())
            .region(region_val.clone())
            .session_name("sideseat");
        if let Some(ext) = external_id {
            builder = builder.external_id(ext);
        }
        // build() is async — loads default credential chain as base for AssumeRole
        let assume_provider = builder.build().await;
        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(region_val)
            .credentials_provider(assume_provider)
            .load()
            .await;
        Ok(Self {
            client: Arc::new(Client::new(&config)),
            mgmt_client: Some(Arc::new(BedrockMgmtClient::new(&config))),
        })
    }

    /// Call `invoke_model` with a JSON body and return the parsed JSON response.
    async fn invoke_model_json(&self, model: &str, body: &Value) -> Result<Value, ProviderError> {
        let bytes = serde_json::to_vec(body)
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;
        let resp = self.client
            .invoke_model()
            .model_id(model)
            .content_type("application/json")
            .accept("application/json")
            .body(Blob::new(bytes))
            .send()
            .await
            .map_err(|e| map_bedrock_error(e.into()))?;
        serde_json::from_slice(resp.body().as_ref())
            .map_err(|e| ProviderError::Serialization(e.to_string()))
    }

    /// Start an async invoke job (Nova Reel) and return the invocation ARN.
    async fn start_async_invoke_json(
        &self,
        model: &str,
        model_input: &Value,
        s3_uri: &str,
    ) -> Result<String, ProviderError> {
        use aws_sdk_bedrockruntime::types::{
            AsyncInvokeOutputDataConfig, AsyncInvokeS3OutputDataConfig,
        };
        let s3_config = AsyncInvokeS3OutputDataConfig::builder()
            .s3_uri(s3_uri)
            .build()
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;
        let output_config = AsyncInvokeOutputDataConfig::S3OutputDataConfig(s3_config);
        let resp = self.client
            .start_async_invoke()
            .model_id(model)
            .model_input(json_to_document(model_input))
            .output_data_config(output_config)
            .send()
            .await
            .map_err(|e| map_bedrock_error(e.into()))?;
        Ok(resp.invocation_arn().to_string())
    }

    /// Poll async invoke status until Completed or Failed (up to ~10 minutes).
    async fn poll_async_invoke_until_done(
        &self,
        arn: &str,
        s3_output_uri: &str,
    ) -> Result<Vec<GeneratedVideo>, ProviderError> {
        use aws_sdk_bedrockruntime::types::AsyncInvokeStatus;
        let max_polls = 120; // ~10 minutes at 5s intervals
        for _ in 0..max_polls {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            let resp = self.client
                .get_async_invoke()
                .invocation_arn(arn)
                .send()
                .await
                .map_err(|e| map_bedrock_error(e.into()))?;
            match resp.status() {
                AsyncInvokeStatus::Completed => {
                    return Ok(vec![GeneratedVideo {
                        uri: Some(format!(
                            "{}/output.mp4",
                            s3_output_uri.trim_end_matches('/')
                        )),
                        b64_json: None,
                        duration_secs: None,
                    }]);
                }
                AsyncInvokeStatus::Failed => {
                    let reason = resp
                        .failure_message()
                        .unwrap_or("unknown failure")
                        .to_string();
                    return Err(ProviderError::Api {
                        status: 0,
                        message: format!("Bedrock async invoke failed (arn={arn}): {reason}"),
                    });
                }
                _ => {} // InProgress — keep polling
            }
        }
        Err(ProviderError::Timeout { ms: Some(600_000) })
    }

    /// Run a Nova Sonic bidirectional stream session.
    ///
    /// `events` is a list of JSON protocol events (each wrapped in `{"event": {...}}`).
    /// Returns the list of JSON events received from the model.
    async fn nova_sonic_session(
        &self,
        model: &str,
        events: Vec<Value>,
    ) -> Result<Vec<Value>, ProviderError> {
        // Build all input chunks eagerly (no errors at this stage).
        let chunks: Vec<InvokeModelWithBidirectionalStreamInput> = events
            .iter()
            .map(|e| -> Result<InvokeModelWithBidirectionalStreamInput, ProviderError> {
                let bytes = serde_json::to_vec(e)
                    .map_err(|err| ProviderError::Serialization(err.to_string()))?;
                let chunk = BidirectionalInputPayloadPart::builder()
                    .bytes(Blob::new(bytes))
                    .build();
                Ok(InvokeModelWithBidirectionalStreamInput::Chunk(chunk))
            })
            .collect::<Result<Vec<_>, _>>()?;

        // Wrap in a futures stream — each item is already Ok, no stream errors.
        let input_stream = futures::stream::iter(
            chunks
                .into_iter()
                .map(Ok::<_, InvokeModelWithBidirectionalStreamInputError>),
        );

        let resp = self.client
            .invoke_model_with_bidirectional_stream()
            .model_id(model)
            .body(EventStreamSender::from(input_stream))
            .send()
            .await
            .map_err(|e| ProviderError::Api {
                status: 0,
                message: e.to_string(),
            })?;

        // Drain output events.
        let mut output_events: Vec<Value> = Vec::new();
        let mut body = resp.body;
        loop {
            match body.recv().await {
                Ok(Some(BidiStreamEvent::Chunk(part))) => {
                    if let Some(bytes) = part.bytes()
                        && let Ok(json) = serde_json::from_slice::<Value>(bytes.as_ref())
                    {
                        output_events.push(json);
                    }
                }
                Ok(None) => break,
                Err(e) => return Err(ProviderError::Stream(e.to_string())),
                _ => {} // Unknown variant
            }
        }
        Ok(output_events)
    }
}

// ---------------------------------------------------------------------------
// Provider trait implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl Provider for BedrockProvider {
    fn provider_name(&self) -> &'static str {
        "aws_bedrock"
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let Some(mgmt) = &self.mgmt_client else {
            return Err(ProviderError::Unsupported(
                "list_models requires a management client (unavailable when using from_client())".into(),
            ));
        };
        let resp = mgmt
            .list_foundation_models()
            .send()
            .await
            .map_err(|e| map_bedrock_mgmt_error(e.into()))?;

        let models = resp
            .model_summaries()
            .iter()
            .map(|m| ModelInfo {
                id: m.model_id().to_string(),
                display_name: m.model_name().map(|s| s.to_string()),
                description: None,
                created_at: None,
            })
            .collect();
        Ok(models)
    }
}

#[async_trait]
impl ChatProvider for BedrockProvider {
    fn stream(&self, messages: Vec<Message>, config: ProviderConfig) -> ProviderStream {
        let client = Arc::clone(&self.client);
        Box::pin(stream! {
            let (bedrock_msgs, sys_blocks) = match build_messages_and_system(&messages, &config) {
                Ok(v) => v,
                Err(e) => { yield Err(e); return; }
            };

            let inf_config = build_inference_config(&config);
            let tool_config = match build_tool_config(&config) {
                Ok(v) => v,
                Err(e) => { yield Err(e); return; }
            };

            let mut req = client.converse_stream().model_id(&config.model);
            for msg in bedrock_msgs { req = req.messages(msg); }
            for sys in sys_blocks { req = req.system(sys); }
            req = req.inference_config(inf_config);
            if let Some(tc) = tool_config { req = req.tool_config(tc); }
            let amrf = build_additional_model_request_fields(&config);
            if !amrf.is_null() {
                req = req.additional_model_request_fields(json_to_document(&amrf));
            }
            if let Some(gc) = build_guardrail_stream_config(&config) {
                req = req.guardrail_config(gc);
            }
            if let Some(pc) = build_performance_config(&config) {
                req = req.performance_config(pc);
            }
            if let Some(meta) = build_request_metadata_map(&config) {
                for (k, v) in meta { req = req.request_metadata(k, v); }
            }
            if let Some(pv) = build_prompt_variables_map(&config) {
                for (k, v) in pv { req = req.prompt_variables(k, v); }
            }
            for path in build_amr_paths(&config) {
                req = req.additional_model_response_field_paths(path);
            }

            let send_result = if let Some(ms) = config.timeout_ms {
                tokio::time::timeout(Duration::from_millis(ms), req.send())
                    .await
                    .map_err(|_| ProviderError::Timeout { ms: Some(ms) })
                    .and_then(|r| r.map_err(|e| map_bedrock_error(e.into())))
            } else {
                req.send().await.map_err(|e| map_bedrock_error(e.into()))
            };
            let resp = match send_result {
                Ok(r) => r,
                Err(e) => { yield Err(e); return; }
            };

            let mut event_stream = resp.stream;
            loop {
                match event_stream.recv().await {
                    Ok(Some(event)) => {
                        match handle_stream_event(event, &config.model) {
                            Some(Ok(ev)) => yield Ok(ev),
                            Some(Err(e)) => { yield Err(e); return; }
                            None => {}
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        yield Err(ProviderError::Stream(e.to_string()));
                        return;
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
        let (bedrock_msgs, sys_blocks) = build_messages_and_system(&messages, &config)?;
        let inf_config = build_inference_config(&config);
        let tool_config = build_tool_config(&config)?;

        let mut req = self.client.converse().model_id(&config.model);
        for msg in bedrock_msgs {
            req = req.messages(msg);
        }
        for sys in sys_blocks {
            req = req.system(sys);
        }
        req = req.inference_config(inf_config);
        if let Some(tc) = tool_config {
            req = req.tool_config(tc);
        }
        let amrf = build_additional_model_request_fields(&config);
        if !amrf.is_null() {
            req = req.additional_model_request_fields(json_to_document(&amrf));
        }
        if let Some(gc) = build_guardrail_config(&config) {
            req = req.guardrail_config(gc);
        }
        if let Some(pc) = build_performance_config(&config) {
            req = req.performance_config(pc);
        }
        if let Some(meta) = build_request_metadata_map(&config) {
            for (k, v) in meta {
                req = req.request_metadata(k, v);
            }
        }
        if let Some(pv) = build_prompt_variables_map(&config) {
            for (k, v) in pv {
                req = req.prompt_variables(k, v);
            }
        }
        for path in build_amr_paths(&config) {
            req = req.additional_model_response_field_paths(path);
        }

        let resp = if let Some(ms) = config.timeout_ms {
            tokio::time::timeout(Duration::from_millis(ms), req.send())
                .await
                .map_err(|_| ProviderError::Timeout { ms: Some(ms) })?
                .map_err(|e| map_bedrock_error(e.into()))?
        } else {
            req.send().await.map_err(|e| map_bedrock_error(e.into()))?
        };

        let msg = match resp.output() {
            Some(ConverseOutput::Message(m)) => m,
            _ => {
                return Err(ProviderError::Serialization(
                    "No message in Bedrock response".into(),
                ));
            }
        };

        let content: Vec<ContentBlock> = msg
            .content()
            .iter()
            .filter_map(bedrock_content_to_block)
            .collect();

        let usage = resp
            .usage()
            .map(|u| Usage {
                input_tokens: u.input_tokens() as u64,
                output_tokens: u.output_tokens() as u64,
                cache_read_tokens: u.cache_read_input_tokens().unwrap_or(0) as u64,
                cache_write_tokens: u.cache_write_input_tokens().unwrap_or(0) as u64,
                ..Default::default()
            })
            .unwrap_or_default();

        let stop_reason = parse_stop_reason(Some(resp.stop_reason()));

        Ok(crate::types::Response {
            content,
            usage: usage.with_totals(),
            stop_reason,
            model: Some(config.model.clone()),
            id: None,
            container: None,
            logprobs: None,
            grounding_metadata: None,
            warnings: vec![],
        })
    }

    async fn count_tokens(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<TokenCount, ProviderError> {
        let (bedrock_msgs, sys_blocks) = build_messages_and_system(&messages, &config)?;
        let tool_config = build_tool_config(&config)?;

        let converse_req = ConverseTokensRequest::builder()
            .set_messages(if bedrock_msgs.is_empty() {
                None
            } else {
                Some(bedrock_msgs)
            })
            .set_system(if sys_blocks.is_empty() {
                None
            } else {
                Some(sys_blocks)
            })
            .set_tool_config(tool_config)
            .build();

        let fut = self
            .client
            .count_tokens()
            .model_id(&config.model)
            .input(CountTokensInput::Converse(converse_req))
            .send();
        let resp = if let Some(ms) = config.timeout_ms {
            tokio::time::timeout(Duration::from_millis(ms), fut)
                .await
                .map_err(|_| ProviderError::Timeout { ms: Some(ms) })?
                .map_err(|e| map_bedrock_error(e.into()))?
        } else {
            fut.await.map_err(|e| map_bedrock_error(e.into()))?
        };

        Ok(TokenCount {
            input_tokens: resp.input_tokens() as u64,
        })
    }
}

#[async_trait]
impl EmbeddingProvider for BedrockProvider {
    async fn embed(
        &self,
        request: EmbeddingRequest,
    ) -> Result<EmbeddingResponse, ProviderError> {
        let model = request.model.as_str();
        // Build the invoke_model request body based on the model family
        let body = if model.contains("cohere.embed") {
            // Cohere Embed V3 on Bedrock — fixed 1024 dims, no dimension control
            json!({
                "texts": request.inputs,
                "input_type": "search_document",
                "embedding_types": ["float"],
            })
        } else if model.contains("titan-embed-image") {
            // Titan Multimodal Embeddings G1 — uses embeddingConfig.outputEmbeddingLength
            let mut b = json!({
                "inputText": request.inputs.first().cloned().unwrap_or_default(),
            });
            if let Some(dims) = request.dimensions {
                b["embeddingConfig"] = json!({ "outputEmbeddingLength": dims });
            }
            b
        } else if model.contains("titan-embed-text-v1") {
            // Titan Embed Text V1 — fixed 1536 dims, no extra parameters accepted
            json!({
                "inputText": request.inputs.first().cloned().unwrap_or_default(),
            })
        } else {
            // Titan Embed Text V2+ — supports dimensions (256/512/1024) + normalize
            let mut b = json!({
                "inputText": request.inputs.first().cloned().unwrap_or_default(),
                "normalize": true,
            });
            if let Some(dims) = request.dimensions {
                b["dimensions"] = json!(dims);
            }
            b
        };

        let resp_json = self.invoke_model_json(model, &body).await?;

        // Parse response based on model family
        let (embeddings, input_tokens) = if model.contains("cohere.embed") {
            let vecs: Vec<Vec<f32>> = resp_json["embeddings"]["float"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .map(|v| {
                    v.as_array()
                        .unwrap_or(&vec![])
                        .iter()
                        .filter_map(|f| f.as_f64().map(|n| n as f32))
                        .collect()
                })
                .collect();
            (vecs, 0u64)
        } else {
            // Titan Embed
            let vec: Vec<f32> = resp_json["embedding"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter_map(|f| f.as_f64().map(|n| n as f32))
                .collect();
            let tokens = resp_json["inputTextTokenCount"].as_u64().unwrap_or(0);
            (vec![vec], tokens)
        };

        Ok(EmbeddingResponse {
            embeddings,
            model: Some(model.to_string()),
            usage: Usage {
                input_tokens,
                ..Default::default()
            },
        })
    }
}

#[async_trait]
impl ImageProvider for BedrockProvider {
    async fn generate_image(
        &self,
        request: ImageGenerationRequest,
    ) -> Result<ImageGenerationResponse, ProviderError> {
        let n = request.n.unwrap_or(1);
        let (width, height) = request
            .size
            .as_ref()
            .map(|s| {
                let parts: Vec<&str> = s.as_str().split('x').collect();
                let w = parts
                    .first()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1024u32);
                let h = parts.get(1).and_then(|v| v.parse().ok()).unwrap_or(1024u32);
                (w, h)
            })
            .unwrap_or((1024, 1024));

        let quality = request
            .quality
            .as_ref()
            .map(|q| q.as_str())
            .unwrap_or("standard");

        // Nova Canvas and Titan Image Generator share the same request/response format
        let mut img_config = json!({
            "numberOfImages": n,
            "width": width,
            "height": height,
            "quality": quality,
        });
        if let Some(seed) = request.seed {
            img_config["seed"] = json!(seed);
        }

        let body = json!({
            "taskType": "TEXT_IMAGE",
            "textToImageParams": {
                "text": request.prompt,
            },
            "imageGenerationConfig": img_config,
        });

        let resp_json = self.invoke_model_json(&request.model, &body).await?;

        let images: Vec<GeneratedImage> = resp_json["images"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .map(|img_val| GeneratedImage {
                url: None,
                b64_json: img_val.as_str().map(|s| s.to_string()),
                revised_prompt: None,
            })
            .collect();

        Ok(ImageGenerationResponse { images })
    }
}

#[async_trait]
impl VideoProvider for BedrockProvider {
    async fn generate_video(
        &self,
        request: VideoGenerationRequest,
    ) -> Result<VideoGenerationResponse, ProviderError> {
        let s3_uri = request.output_storage_uri.as_deref().ok_or_else(|| {
            ProviderError::InvalidRequest(
                "Bedrock Nova Reel requires output_storage_uri (s3://bucket/prefix)".into(),
            )
        })?;

        let duration = request.duration_secs.unwrap_or(6);
        let dimension = match request.resolution.as_ref().map(|r| r.as_str()) {
            Some("1080p") => "1920x1080",
            _ => "1280x720",
        };

        let mut video_config = json!({
            "durationSeconds": duration,
            "fps": 24,
            "dimension": dimension,
        });
        if let Some(seed) = request.seed {
            video_config["seed"] = json!(seed);
        }

        let model_input = json!({
            "taskType": "TEXT_VIDEO",
            "textToVideoParams": {
                "text": request.prompt,
            },
            "videoGenerationConfig": video_config,
        });

        let arn = self
            .start_async_invoke_json(&request.model, &model_input, s3_uri)
            .await?;

        // Poll until complete (up to ~10 minutes)
        let videos = self.poll_async_invoke_until_done(&arn, s3_uri).await?;
        Ok(VideoGenerationResponse { videos })
    }
}

#[async_trait]
impl AudioProvider for BedrockProvider {
    /// Generate speech audio from text via Amazon Nova Sonic.
    ///
    /// Supported model: `amazon.nova-sonic-v1:0`.
    /// Returns LPCM audio at 24 kHz (16-bit signed, mono) by default.
    /// Request `AudioFormat::Mp3` to get compressed audio output.
    async fn generate_speech(
        &self,
        request: SpeechRequest,
    ) -> Result<SpeechResponse, ProviderError> {
        // Choose output media type based on requested format.
        let (media_type, returned_format) = match &request.response_format {
            Some(CAudioFmt::Mp3) => ("audio/mpeg", CAudioFmt::Mp3),
            Some(CAudioFmt::Opus) => ("audio/opus", CAudioFmt::Opus),
            // Default: LPCM 24 kHz. Callers should treat this as raw signed-16 PCM.
            _ => (
                "audio/lpcm;rate=24000;encoding=signed-int;bits=16;channels=1;big-endian=false",
                CAudioFmt::Wav,
            ),
        };

        let prompt_name = "prompt_1";
        let mut prompt_start = json!({
            "promptName": prompt_name,
            "audioOutputConfiguration": { "mediaType": media_type },
        });
        // Map the voice field to Nova Sonic's voiceConfiguration.
        if !request.voice.is_empty() {
            prompt_start["voiceConfiguration"] = json!({ "voiceId": request.voice });
        }

        let events = vec![
            json!({"event": {"sessionStart": {"inferenceConfiguration": {"maxTokens": 4096}}}}),
            json!({"event": {"promptStart": prompt_start}}),
            json!({"event": {"contentBlockStart": {"promptName": prompt_name, "content": {"text": {}}}}}),
            json!({"event": {"contentBlockDelta": {"promptName": prompt_name, "content": {"text": {"value": request.input}}}}}),
            json!({"event": {"contentBlockStop": {"promptName": prompt_name}}}),
            json!({"event": {"promptEnd": {"promptName": prompt_name}}}),
            json!({"event": {"sessionEnd": {}}}),
        ];

        let output_events = self.nova_sonic_session(&request.model, events).await?;

        // Collect raw audio bytes from contentBlockDelta audio events.
        let mut audio_bytes: Vec<u8> = Vec::new();
        for ev in &output_events {
            if let Some(b64) =
                ev["event"]["contentBlockDelta"]["content"]["audio"]["bytes"].as_str()
            {
                use base64::Engine;
                if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) {
                    audio_bytes.extend_from_slice(&bytes);
                }
            }
        }

        Ok(SpeechResponse {
            audio: audio_bytes,
            format: returned_format,
        })
    }

    /// Transcribe audio to text via Amazon Nova Sonic.
    ///
    /// Supported model: `amazon.nova-sonic-v1:0`.
    /// The audio in `request.audio` must match `request.format`:
    /// - `Mp3` → `audio/mpeg`
    /// - `Opus` → `audio/opus`
    /// - `Wav` / other → LPCM at 16 kHz (signed-16, mono)
    async fn transcribe(
        &self,
        request: TranscriptionRequest,
    ) -> Result<TranscriptionResponse, ProviderError> {
        use base64::Engine;
        let audio_b64 = base64::engine::general_purpose::STANDARD.encode(&request.audio);

        let media_type = match &request.format {
            CAudioFmt::Mp3 => "audio/mpeg",
            CAudioFmt::Opus => "audio/opus",
            _ => "audio/lpcm;rate=16000;encoding=signed-int;bits=16;channels=1;big-endian=false",
        };

        let prompt_name = "prompt_1";
        let events = vec![
            json!({"event": {"sessionStart": {"inferenceConfiguration": {"maxTokens": 4096}}}}),
            json!({"event": {"promptStart": {"promptName": prompt_name, "audioInputConfiguration": {"mediaType": media_type}}}}),
            json!({"event": {"contentBlockStart": {"promptName": prompt_name, "content": {"audio": {}}}}}),
            json!({"event": {"contentBlockDelta": {"promptName": prompt_name, "content": {"audio": {"bytes": audio_b64}}}}}),
            json!({"event": {"contentBlockStop": {"promptName": prompt_name}}}),
            json!({"event": {"promptEnd": {"promptName": prompt_name}}}),
            json!({"event": {"sessionEnd": {}}}),
        ];

        let output_events = self.nova_sonic_session(&request.model, events).await?;

        // Collect text deltas from contentBlockDelta text events.
        let mut transcript = String::new();
        for ev in &output_events {
            if let Some(text) =
                ev["event"]["contentBlockDelta"]["content"]["text"]["value"].as_str()
            {
                transcript.push_str(text);
            }
        }

        Ok(TranscriptionResponse {
            text: transcript,
            language: None,
            duration_secs: None,
            words: vec![],
            segments: vec![],
        })
    }
}

// ---------------------------------------------------------------------------
// SDK stream event handler
// ---------------------------------------------------------------------------

fn handle_stream_event(event: ConverseStreamOutput, req_model: &str) -> Option<Result<StreamEvent, ProviderError>> {
    match event {
        ConverseStreamOutput::MessageStart(e) => {
            let role = match e.role() {
                ConversationRole::User => Role::User,
                _ => Role::Assistant,
            };
            Some(Ok(StreamEvent::MessageStart { role }))
        }
        ConverseStreamOutput::ContentBlockStart(e) => {
            let index = e.content_block_index() as usize;
            let block = match e.start() {
                Some(BContentBlockStart::ToolUse(tu)) => ContentBlockStart::ToolUse {
                    id: tu.tool_use_id().to_string(),
                    name: tu.name().to_string(),
                },
                _ => ContentBlockStart::Text,
            };
            Some(Ok(StreamEvent::ContentBlockStart { index, block }))
        }
        ConverseStreamOutput::ContentBlockDelta(e) => {
            let index = e.content_block_index() as usize;
            let cd = match e.delta() {
                Some(ContentBlockDelta::Text(text)) => ContentDelta::Text { text: text.clone() },
                Some(ContentBlockDelta::ToolUse(tu)) => ContentDelta::ToolInput {
                    partial_json: tu.input().to_string(),
                },
                Some(ContentBlockDelta::ReasoningContent(rc)) => {
                    use aws_sdk_bedrockruntime::types::ReasoningContentBlockDelta;
                    match rc {
                        ReasoningContentBlockDelta::Text(t) => ContentDelta::Thinking {
                            text: t.clone(),
                        },
                        ReasoningContentBlockDelta::Signature(s) => ContentDelta::Signature {
                            signature: s.clone(),
                        },
                        _ => return None,
                    }
                }
                _ => return None,
            };
            Some(Ok(StreamEvent::ContentBlockDelta { index, delta: cd }))
        }
        ConverseStreamOutput::ContentBlockStop(e) => {
            let index = e.content_block_index() as usize;
            Some(Ok(StreamEvent::ContentBlockStop { index }))
        }
        ConverseStreamOutput::MessageStop(e) => {
            let stop_reason = parse_stop_reason(Some(e.stop_reason()));
            Some(Ok(StreamEvent::MessageStop { stop_reason }))
        }
        ConverseStreamOutput::Metadata(e) => {
            let usage = e
                .usage()
                .map(|u| Usage {
                    input_tokens: u.input_tokens() as u64,
                    output_tokens: u.output_tokens() as u64,
                    cache_read_tokens: u.cache_read_input_tokens().unwrap_or(0) as u64,
                    cache_write_tokens: u.cache_write_input_tokens().unwrap_or(0) as u64,
                    ..Default::default()
                })
                .unwrap_or_default();
            Some(Ok(StreamEvent::Metadata {
                usage: usage.with_totals(),
                model: Some(req_model.to_string()),
                id: None,
            }))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// SDK request building helpers
// ---------------------------------------------------------------------------

fn build_messages_and_system(
    messages: &[Message],
    config: &ProviderConfig,
) -> Result<(Vec<BMessage>, Vec<SystemContentBlock>), ProviderError> {
    let mut bmsgs: Vec<BMessage> = Vec::new();
    let mut sys: Vec<SystemContentBlock> = Vec::new();

    if let Some(s) = &config.system {
        sys.push(SystemContentBlock::Text(s.clone()));
    }

    for msg in messages {
        match &msg.role {
            Role::System => {
                for block in &msg.content {
                    if let ContentBlock::Text(t) = block {
                        sys.push(SystemContentBlock::Text(t.text.clone()));
                    }
                }
                if msg.cache_control.is_some() {
                    let cpb = CachePointBlock::builder()
                        .r#type(CachePointType::Default)
                        .build()
                        .map_err(|e| ProviderError::Serialization(e.to_string()))?;
                    sys.push(SystemContentBlock::CachePoint(cpb));
                }
            }
            Role::User | Role::Tool | Role::Assistant | Role::Other(_) => {
                let role = if msg.role == Role::User || msg.role == Role::Tool {
                    ConversationRole::User
                } else {
                    ConversationRole::Assistant
                };
                let mut content: Vec<BContent> =
                    msg.content.iter().map(block_to_bedrock).collect::<Result<_, _>>()?;
                if msg.cache_control.is_some() {
                    let cpb = CachePointBlock::builder()
                        .r#type(CachePointType::Default)
                        .build()
                        .map_err(|e| ProviderError::Serialization(e.to_string()))?;
                    content.push(BContent::CachePoint(cpb));
                }
                let bmsg = BMessage::builder()
                    .role(role)
                    .set_content(Some(content))
                    .build()
                    .map_err(|e| ProviderError::Serialization(e.to_string()))?;
                bmsgs.push(bmsg);
            }
        }
    }
    Ok((bmsgs, sys))
}

/// Build the `additionalModelRequestFields` JSON value for Bedrock Converse.
///
/// Merges:
/// - `reasoning_effort` → `thinking.budget_tokens` (for Claude models)
/// - `extra["additional_model_request_fields"]` generic pass-through
fn build_additional_model_request_fields(config: &ProviderConfig) -> Value {
    let mut fields = serde_json::Map::new();

    // thinking_budget (explicit) takes priority over reasoning_effort
    if let Some(budget) = config.thinking_budget {
        fields.insert(
            "thinking".to_string(),
            json!({"type": "enabled", "budget_tokens": budget}),
        );
    } else if let Some(effort) = &config.reasoning_effort {
        let budget_tokens = match effort {
            ReasoningEffort::Low => 1000,
            ReasoningEffort::Medium => 5000,
            ReasoningEffort::High => 16000,
            ReasoningEffort::Max => 32000,
        };
        fields.insert(
            "thinking".to_string(),
            json!({"type": "enabled", "budget_tokens": budget_tokens}),
        );
    }

    // Generic pass-through from extra
    if let Some(extra_fields) = config.extra.get("additional_model_request_fields")
        && let Some(obj) = extra_fields.as_object()
    {
        for (k, v) in obj {
            fields.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }

    if fields.is_empty() {
        Value::Null
    } else {
        Value::Object(fields)
    }
}

fn build_inference_config(config: &ProviderConfig) -> InferenceConfiguration {
    let mut b = InferenceConfiguration::builder();
    if let Some(m) = config.max_tokens {
        b = b.max_tokens(m as i32);
    }
    if let Some(t) = config.temperature {
        b = b.temperature(t as f32);
    }
    if let Some(p) = config.top_p {
        b = b.top_p(p as f32);
    }
    for s in &config.stop_sequences {
        b = b.stop_sequences(s);
    }
    b.build()
}

/// Extract the three shared guardrail params from `config.extra`, or return `None` if no guardrail
/// is configured (`guardrail_id` is the required key).
fn guardrail_params(config: &ProviderConfig) -> Option<(String, String, GuardrailTrace)> {
    let id = config.extra.get("guardrail_id")?.as_str()?.to_string();
    let version = config
        .extra
        .get("guardrail_version")
        .and_then(|v| v.as_str())
        .unwrap_or("DRAFT")
        .to_string();
    let trace = GuardrailTrace::from(
        config
            .extra
            .get("guardrail_trace")
            .and_then(|v| v.as_str())
            .unwrap_or("disabled"),
    );
    Some((id, version, trace))
}

fn build_guardrail_config(config: &ProviderConfig) -> Option<GuardrailConfiguration> {
    let (id, version, trace) = guardrail_params(config)?;
    Some(
        GuardrailConfiguration::builder()
            .guardrail_identifier(id)
            .guardrail_version(version)
            .trace(trace)
            .build(),
    )
}

fn build_guardrail_stream_config(config: &ProviderConfig) -> Option<GuardrailStreamConfiguration> {
    let (id, version, trace) = guardrail_params(config)?;
    Some(
        GuardrailStreamConfiguration::builder()
            .guardrail_identifier(id)
            .guardrail_version(version)
            .trace(trace)
            .build(),
    )
}

fn build_performance_config(config: &ProviderConfig) -> Option<PerformanceConfiguration> {
    let latency_str = config
        .extra
        .get("performance_config_latency")
        .and_then(|v| v.as_str())?;
    Some(
        PerformanceConfiguration::builder()
            .latency(PerformanceConfigLatency::from(latency_str))
            .build(),
    )
}

fn build_request_metadata_map(
    config: &ProviderConfig,
) -> Option<std::collections::HashMap<String, String>> {
    let obj = config.extra.get("request_metadata")?.as_object()?;
    let map: std::collections::HashMap<String, String> = obj
        .iter()
        .map(|(k, v)| {
            let s = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            (k.clone(), s)
        })
        .collect();
    if map.is_empty() { None } else { Some(map) }
}

fn build_prompt_variables_map(
    config: &ProviderConfig,
) -> Option<std::collections::HashMap<String, PromptVariableValues>> {
    let obj = config.extra.get("prompt_variables")?.as_object()?;
    let map: std::collections::HashMap<String, PromptVariableValues> = obj
        .iter()
        .map(|(k, v)| {
            let text = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            (k.clone(), PromptVariableValues::Text(text))
        })
        .collect();
    if map.is_empty() { None } else { Some(map) }
}

fn build_amr_paths(config: &ProviderConfig) -> Vec<String> {
    config
        .extra
        .get("additional_model_response_field_paths")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

fn build_tool_config(config: &ProviderConfig) -> Result<Option<ToolConfiguration>, ProviderError> {
    // ToolChoice::None means "send no tool config at all"
    if matches!(config.tool_choice, Some(ToolChoice::None)) {
        return Ok(None);
    }

    let sys_tool_names: Vec<&str> = config
        .extra
        .get("system_tools")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    if config.tools.is_empty() && sys_tool_names.is_empty() {
        return Ok(None);
    }

    let mut tools: Vec<BTool> = config
        .tools
        .iter()
        .map(|t| {
            let spec = ToolSpecification::builder()
                .name(&t.name)
                .description(&t.description)
                .input_schema(ToolInputSchema::Json(json_to_document(&t.input_schema)))
                .build()
                .map_err(|e| ProviderError::Serialization(e.to_string()))?;
            Ok(BTool::ToolSpec(spec))
        })
        .collect::<Result<Vec<_>, ProviderError>>()?;

    for name in sys_tool_names {
        let sys_tool = SystemTool::builder()
            .name(name)
            .build()
            .map_err(|e| ProviderError::Serialization(e.to_string()))?;
        tools.push(BTool::SystemTool(sys_tool));
    }

    let mut builder = ToolConfiguration::builder().set_tools(Some(tools));

    if let Some(choice) = &config.tool_choice {
        let bc = match choice {
            ToolChoice::Auto => BToolChoice::Auto(AutoToolChoice::builder().build()),
            ToolChoice::Any => BToolChoice::Any(AnyToolChoice::builder().build()),
            ToolChoice::None => unreachable!("ToolChoice::None handled at top of build_tool_config"),
            ToolChoice::Tool { name } => BToolChoice::Tool(
                SpecificToolChoice::builder()
                    .name(name)
                    .build()
                    .map_err(|e| ProviderError::Serialization(e.to_string()))?,
            ),
            // Bedrock has no subset restriction; fall back to auto
            ToolChoice::AllowedTools { .. } => BToolChoice::Auto(AutoToolChoice::builder().build()),
        };
        builder = builder.tool_choice(bc);
    }

    Ok(Some(
        builder
            .build()
            .map_err(|e| ProviderError::Serialization(e.to_string()))?,
    ))
}

// ---------------------------------------------------------------------------
// SDK content block conversions: our types → Bedrock SDK types
// ---------------------------------------------------------------------------

fn block_to_bedrock(block: &ContentBlock) -> Result<BContent, ProviderError> {
    match block {
        ContentBlock::Text(t) => Ok(BContent::Text(t.text.clone())),

        ContentBlock::Image(img) => {
            let (fmt, source) = match &img.source {
                MediaSource::Base64(b64) => {
                    use base64::Engine;
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(&b64.data)
                        .map_err(|e| ProviderError::Serialization(e.to_string()))?;
                    (
                        cimg_to_bedrock(img.format.as_ref()),
                        ImageSource::Bytes(Blob::new(bytes)),
                    )
                }
                MediaSource::S3(s3) => {
                    let s3_loc = S3Location::builder()
                        .uri(&s3.uri)
                        .set_bucket_owner(s3.bucket_owner.clone())
                        .build()
                        .map_err(|e| ProviderError::Serialization(e.to_string()))?;
                    (
                        cimg_to_bedrock(img.format.as_ref()),
                        ImageSource::S3Location(s3_loc),
                    )
                }
                _ => {
                    return Err(ProviderError::Unsupported(
                        "Bedrock images require base64 bytes or S3 source".into(),
                    ));
                }
            };
            let ib = ImageBlock::builder()
                .format(fmt)
                .source(source)
                .build()
                .map_err(|e| ProviderError::Serialization(e.to_string()))?;
            Ok(BContent::Image(ib))
        }

        ContentBlock::Document(doc) => {
            let source = match &doc.source {
                MediaSource::Base64(b64) => {
                    use base64::Engine;
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(&b64.data)
                        .map_err(|e| ProviderError::Serialization(e.to_string()))?;
                    DocumentSource::Bytes(Blob::new(bytes))
                }
                MediaSource::S3(s3) => {
                    let s3_loc = S3Location::builder()
                        .uri(&s3.uri)
                        .set_bucket_owner(s3.bucket_owner.clone())
                        .build()
                        .map_err(|e| ProviderError::Serialization(e.to_string()))?;
                    DocumentSource::S3Location(s3_loc)
                }
                MediaSource::Text(text) => DocumentSource::Text(text.clone()),
                _ => {
                    return Err(ProviderError::Unsupported(
                        "Bedrock documents require base64 bytes, S3, or text source".into(),
                    ));
                }
            };
            let db = DocumentBlock::builder()
                .format(cdoc_to_bedrock(&doc.format))
                .name(doc.name.as_deref().unwrap_or("document"))
                .source(source)
                .build()
                .map_err(|e| ProviderError::Serialization(e.to_string()))?;
            Ok(BContent::Document(db))
        }

        ContentBlock::Video(video) => {
            let source = match &video.source {
                MediaSource::Base64(b64) => {
                    use base64::Engine;
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(&b64.data)
                        .map_err(|e| ProviderError::Serialization(e.to_string()))?;
                    VideoSource::Bytes(Blob::new(bytes))
                }
                MediaSource::S3(s3) => {
                    let s3_loc = S3Location::builder()
                        .uri(&s3.uri)
                        .set_bucket_owner(s3.bucket_owner.clone())
                        .build()
                        .map_err(|e| ProviderError::Serialization(e.to_string()))?;
                    VideoSource::S3Location(s3_loc)
                }
                _ => {
                    return Err(ProviderError::Unsupported(
                        "Bedrock video requires base64 or S3 source".into(),
                    ));
                }
            };
            let vb = VideoBlock::builder()
                .format(cvid_to_bedrock(&video.format))
                .source(source)
                .build()
                .map_err(|e| ProviderError::Serialization(e.to_string()))?;
            Ok(BContent::Video(vb))
        }

        ContentBlock::ToolUse(tu) => {
            let btu = BToolUse::builder()
                .tool_use_id(&tu.id)
                .name(&tu.name)
                .input(json_to_document(&tu.input))
                .build()
                .map_err(|e| ProviderError::Serialization(e.to_string()))?;
            Ok(BContent::ToolUse(btu))
        }

        ContentBlock::ToolResult(tr) => {
            let result_content: Vec<ToolResultContentBlock> = tr
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text(t) => Some(ToolResultContentBlock::Text(t.text.clone())),
                    ContentBlock::Image(img) => block_to_bedrock(&ContentBlock::Image(img.clone()))
                        .ok()
                        .and_then(|bc| {
                            if let BContent::Image(ib) = bc {
                                Some(ToolResultContentBlock::Image(ib))
                            } else {
                                None
                            }
                        }),
                    ContentBlock::Document(doc) => {
                        block_to_bedrock(&ContentBlock::Document(doc.clone()))
                            .ok()
                            .and_then(|bc| {
                                if let BContent::Document(db) = bc {
                                    Some(ToolResultContentBlock::Document(db))
                                } else {
                                    None
                                }
                            })
                    }
                    ContentBlock::Video(video) => {
                        block_to_bedrock(&ContentBlock::Video(video.clone()))
                            .ok()
                            .and_then(|bc| {
                                if let BContent::Video(vb) = bc {
                                    Some(ToolResultContentBlock::Video(vb))
                                } else {
                                    None
                                }
                            })
                    }
                    _ => None,
                })
                .collect();
            let status = if tr.is_error {
                ToolResultStatus::Error
            } else {
                ToolResultStatus::Success
            };
            let btr = BToolResult::builder()
                .tool_use_id(&tr.tool_use_id)
                .set_content(Some(result_content))
                .status(status)
                .build()
                .map_err(|e| ProviderError::Serialization(e.to_string()))?;
            Ok(BContent::ToolResult(btr))
        }

        ContentBlock::Thinking(th) => {
            let rt = ReasoningTextBlock::builder()
                .text(&th.text)
                .set_signature(th.signature.clone())
                .build()
                .map_err(|e| ProviderError::Serialization(e.to_string()))?;
            Ok(BContent::ReasoningContent(
                ReasoningContentBlock::ReasoningText(rt),
            ))
        }

        ContentBlock::Audio(audio) => {
            let (fmt, source) = match &audio.source {
                MediaSource::Base64(b64) => {
                    use base64::Engine;
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(&b64.data)
                        .map_err(|e| ProviderError::Serialization(e.to_string()))?;
                    (caudio_to_bedrock(&audio.format), BAudioSource::Bytes(Blob::new(bytes)))
                }
                MediaSource::S3(s3) => {
                    let s3_loc = S3Location::builder()
                        .uri(&s3.uri)
                        .set_bucket_owner(s3.bucket_owner.clone())
                        .build()
                        .map_err(|e| ProviderError::Serialization(e.to_string()))?;
                    (caudio_to_bedrock(&audio.format), BAudioSource::S3Location(s3_loc))
                }
                _ => {
                    return Err(ProviderError::Unsupported(
                        "Bedrock audio requires base64 bytes or S3 source".into(),
                    ));
                }
            };
            let ab = AudioBlock::builder()
                .format(fmt)
                .source(source)
                .build()
                .map_err(|e| ProviderError::Serialization(e.to_string()))?;
            Ok(BContent::Audio(ab))
        }
    }
}

fn bedrock_content_to_block(block: &BContent) -> Option<ContentBlock> {
    match block {
        BContent::Text(t) => Some(ContentBlock::text(t.as_str())),
        BContent::ToolUse(tu) => Some(ContentBlock::ToolUse(ToolUseBlock {
            id: tu.tool_use_id().to_string(),
            name: tu.name().to_string(),
            input: document_to_json(tu.input()),
        })),
        BContent::ReasoningContent(ReasoningContentBlock::ReasoningText(rt)) => {
            Some(ContentBlock::Thinking(ThinkingBlock {
                text: rt.text().to_string(),
                signature: rt.signature().map(|s| s.to_string()),
            }))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Format conversions
// ---------------------------------------------------------------------------

fn caudio_to_bedrock(fmt: &CAudioFmt) -> BAudioFmt {
    match fmt {
        CAudioFmt::Mp3 => BAudioFmt::Mp3,
        CAudioFmt::Wav => BAudioFmt::Wav,
        CAudioFmt::Aac => BAudioFmt::Aac,
        CAudioFmt::Flac => BAudioFmt::Flac,
        CAudioFmt::Ogg => BAudioFmt::Ogg,
        CAudioFmt::Webm => BAudioFmt::Webm,
        CAudioFmt::M4a => BAudioFmt::M4A,
        CAudioFmt::Opus => BAudioFmt::Opus,
        CAudioFmt::Aiff => BAudioFmt::from("aiff"),
        CAudioFmt::Pcm16 => BAudioFmt::from("pcm16"),
    }
}

fn cimg_to_bedrock(fmt: Option<&CImgFmt>) -> ImageFormat {
    match fmt {
        Some(CImgFmt::Png) => ImageFormat::Png,
        Some(CImgFmt::Gif) => ImageFormat::Gif,
        Some(CImgFmt::Webp) => ImageFormat::Webp,
        Some(CImgFmt::Jpeg) | Some(CImgFmt::Heic) | Some(CImgFmt::Heif) | None => ImageFormat::Jpeg,
    }
}

fn cdoc_to_bedrock(fmt: &CDocFmt) -> DocumentFormat {
    match fmt {
        CDocFmt::Pdf => DocumentFormat::Pdf,
        CDocFmt::Csv => DocumentFormat::Csv,
        CDocFmt::Doc => DocumentFormat::Doc,
        CDocFmt::Docx => DocumentFormat::Docx,
        CDocFmt::Xls => DocumentFormat::Xls,
        CDocFmt::Xlsx => DocumentFormat::Xlsx,
        CDocFmt::Html => DocumentFormat::Html,
        CDocFmt::Txt => DocumentFormat::Txt,
        CDocFmt::Md => DocumentFormat::Md,
    }
}

fn cvid_to_bedrock(fmt: &CVidFmt) -> VideoFormat {
    match fmt {
        CVidFmt::Mp4 => VideoFormat::Mp4,
        CVidFmt::Mov => VideoFormat::Mov,
        CVidFmt::Mkv => VideoFormat::Mkv,
        CVidFmt::Webm => VideoFormat::Webm,
        CVidFmt::Avi => VideoFormat::from("avi"),
        CVidFmt::Flv => VideoFormat::Flv,
        CVidFmt::Mpeg => VideoFormat::Mpeg,
        CVidFmt::Wmv => VideoFormat::Wmv,
        CVidFmt::ThreeGp => VideoFormat::ThreeGp,
    }
}

fn parse_stop_reason(r: Option<&BStopReason>) -> StopReason {
    match r {
        Some(BStopReason::EndTurn) => StopReason::EndTurn,
        Some(BStopReason::ToolUse) => StopReason::ToolUse,
        Some(BStopReason::MaxTokens) => StopReason::MaxTokens,
        Some(BStopReason::StopSequence) => StopReason::StopSequence(String::new()),
        Some(BStopReason::ContentFiltered) => StopReason::ContentFilter,
        Some(BStopReason::GuardrailIntervened) => StopReason::ContentFilter,
        Some(other) => StopReason::Other(other.as_str().to_string()),
        None => StopReason::EndTurn,
    }
}

// ---------------------------------------------------------------------------
// Error mapping helpers
// ---------------------------------------------------------------------------

/// Classify an AWS SDK Bedrock runtime error into a [`ProviderError`].
fn map_bedrock_error(e: aws_sdk_bedrockruntime::Error) -> ProviderError {
    use aws_sdk_bedrockruntime::Error as BE;
    match e {
        BE::ThrottlingException(inner) => ProviderError::TooManyRequests {
            message: inner.to_string(),
            retry_after_secs: None,
        },
        BE::ModelNotReadyException(inner) => ProviderError::TooManyRequests {
            message: inner.to_string(),
            retry_after_secs: None,
        },
        BE::ServiceQuotaExceededException(inner) => ProviderError::TooManyRequests {
            message: inner.to_string(),
            retry_after_secs: None,
        },
        BE::AccessDeniedException(inner) => ProviderError::Auth(inner.to_string()),
        BE::ResourceNotFoundException(inner) => ProviderError::ModelNotFound {
            model: inner.to_string(),
        },
        BE::ModelTimeoutException(_) => ProviderError::Timeout { ms: None },
        BE::InternalServerException(inner) => ProviderError::Api {
            status: 500,
            message: inner.to_string(),
        },
        BE::ServiceUnavailableException(inner) => ProviderError::Api {
            status: 503,
            message: inner.to_string(),
        },
        BE::ModelErrorException(inner) => ProviderError::Api {
            status: 424,
            message: inner.to_string(),
        },
        BE::ValidationException(inner) => {
            let msg = inner.to_string();
            let lower = msg.to_lowercase();
            if lower.contains("context window")
                || lower.contains("input is too long")
                || lower.contains("too many tokens")
            {
                ProviderError::ContextWindowExceeded(msg)
            } else if lower.contains("doesn't support")
                || lower.contains("does not support")
                || lower.contains("not supported")
                || lower.contains("must set one of the following keys")
            {
                ProviderError::Unsupported(msg)
            } else {
                ProviderError::Api { status: 400, message: msg }
            }
        }
        e => {
            let msg = e.to_string();
            if msg.to_lowercase().contains("timeout") {
                ProviderError::Timeout { ms: None }
            } else {
                ProviderError::Api { status: 0, message: msg }
            }
        }
    }
}

/// Classify an AWS SDK Bedrock management-plane error into a [`ProviderError`].
fn map_bedrock_mgmt_error(e: aws_sdk_bedrock::Error) -> ProviderError {
    use aws_sdk_bedrock::Error as BE;
    match e {
        BE::ThrottlingException(inner) => ProviderError::TooManyRequests {
            message: inner.to_string(),
            retry_after_secs: None,
        },
        BE::AccessDeniedException(inner) => ProviderError::Auth(inner.to_string()),
        BE::ResourceNotFoundException(inner) => ProviderError::ModelNotFound {
            model: inner.to_string(),
        },
        e => ProviderError::Api { status: 0, message: e.to_string() },
    }
}

// ---------------------------------------------------------------------------
// JSON ↔ smithy Document
// ---------------------------------------------------------------------------

pub(crate) fn json_to_document(value: &Value) -> Document {
    match value {
        Value::Null => Document::Null,
        Value::Bool(b) => Document::Bool(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_u64() {
                Document::Number(aws_smithy_types::Number::PosInt(i))
            } else if let Some(i) = n.as_i64() {
                Document::Number(aws_smithy_types::Number::NegInt(i))
            } else {
                Document::Number(aws_smithy_types::Number::Float(n.as_f64().unwrap_or(0.0)))
            }
        }
        Value::String(s) => Document::String(s.clone()),
        Value::Array(a) => Document::Array(a.iter().map(json_to_document).collect()),
        Value::Object(o) => Document::Object(
            o.iter()
                .map(|(k, v)| (k.clone(), json_to_document(v)))
                .collect(),
        ),
    }
}

pub(crate) fn document_to_json(doc: &Document) -> Value {
    match doc {
        Document::Null => Value::Null,
        Document::Bool(b) => Value::Bool(*b),
        Document::Number(n) => match n {
            aws_smithy_types::Number::PosInt(i) => Value::Number((*i).into()),
            aws_smithy_types::Number::NegInt(i) => Value::Number((*i).into()),
            aws_smithy_types::Number::Float(f) => serde_json::Number::from_f64(*f)
                .map(Value::Number)
                .unwrap_or(Value::Null),
        },
        Document::String(s) => Value::String(s.clone()),
        Document::Array(a) => Value::Array(a.iter().map(document_to_json).collect()),
        Document::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| (k.clone(), document_to_json(v)))
                .collect(),
        ),
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
    fn json_to_document_round_trip() {
        let original = json!({
            "string": "hello",
            "number": 42,
            "float": std::f64::consts::PI,
            "bool": true,
            "null": null,
            "array": [1, 2, 3],
            "nested": {"key": "value"}
        });
        let doc = json_to_document(&original);
        let back = document_to_json(&doc);
        assert_eq!(original, back);
    }

    #[test]
    fn json_to_document_negative_int() {
        let val = json!(-5);
        let doc = json_to_document(&val);
        let back = document_to_json(&doc);
        assert_eq!(val, back);
    }
}
