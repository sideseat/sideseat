use std::sync::Arc;
use std::time::Duration;

use async_stream::stream;
use async_trait::async_trait;
use aws_sdk_bedrock::Client as BedrockMgmtClient;
use aws_sdk_bedrockruntime::types::error::InvokeModelWithBidirectionalStreamInputError;
use aws_sdk_bedrockruntime::{
    Client,
    primitives::Blob,
    types::{
        AnyToolChoice, AudioBlock, AudioFormat as BAudioFmt, AudioSource as BAudioSource,
        AutoToolChoice, BidirectionalInputPayloadPart, CachePointBlock, CachePointType,
        ContentBlock as BContent, ContentBlockDelta, ContentBlockStart as BContentBlockStart,
        ConversationRole, ConverseOutput, ConverseStreamOutput, ConverseTokensRequest,
        CountTokensInput, DocumentBlock, DocumentFormat, DocumentSource, GuardrailConfiguration,
        GuardrailStreamConfiguration, GuardrailTrace, ImageBlock, ImageFormat, ImageSource,
        InferenceConfiguration, InvokeModelWithBidirectionalStreamInput,
        InvokeModelWithBidirectionalStreamOutput as BidiStreamEvent, Message as BMessage,
        PerformanceConfigLatency, PerformanceConfiguration, PromptVariableValues,
        ReasoningContentBlock, ReasoningTextBlock, S3Location, SpecificToolChoice,
        StopReason as BStopReason, SystemContentBlock, SystemTool, Tool as BTool,
        ToolChoice as BToolChoice, ToolConfiguration, ToolInputSchema,
        ToolResultBlock as BToolResult, ToolResultContentBlock, ToolResultStatus,
        ToolSpecification, ToolUseBlock as BToolUse, VideoBlock, VideoFormat, VideoSource,
    },
};
use aws_smithy_http::event_stream::EventStreamSender;
use aws_smithy_types::Document;
use serde_json::{Value, json};

use crate::{
    error::ProviderError,
    provider::{
        AudioProvider, ChatProvider, EmbeddingProvider, ImageProvider, Provider, ProviderStream,
        VideoProvider,
    },
    types::{
        AudioFormat as CAudioFmt, ContentBlock, ContentBlockStart, ContentDelta,
        DocumentFormat as CDocFmt, EmbeddingRequest, EmbeddingResponse, GeneratedImage,
        GeneratedVideo, ImageFormat as CImgFmt, ImageGenerationRequest, ImageGenerationResponse,
        MediaSource, Message, ModelInfo, ProviderConfig, ReasoningEffort, Role, SpeechRequest,
        SpeechResponse, StopReason, StreamEvent, ThinkingBlock, TokenCount, ToolChoice,
        ToolUseBlock, TranscriptionRequest, TranscriptionResponse, Usage, VideoFormat as CVidFmt,
        VideoGenerationRequest, VideoGenerationResponse,
    },
};

mod protocol;

use protocol::*;

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
    pub async fn with_profile(
        profile_name: impl Into<String>,
        region: impl Into<String>,
    ) -> Result<Self, ProviderError> {
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
        let bytes =
            serde_json::to_vec(body).map_err(|e| ProviderError::Serialization(e.to_string()))?;
        let resp = self
            .client
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
        let resp = self
            .client
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
            let resp = self
                .client
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
            .map(
                |e| -> Result<InvokeModelWithBidirectionalStreamInput, ProviderError> {
                    let bytes = serde_json::to_vec(e)
                        .map_err(|err| ProviderError::Serialization(err.to_string()))?;
                    let chunk = BidirectionalInputPayloadPart::builder()
                        .bytes(Blob::new(bytes))
                        .build();
                    Ok(InvokeModelWithBidirectionalStreamInput::Chunk(chunk))
                },
            )
            .collect::<Result<Vec<_>, _>>()?;

        // Wrap in a futures stream — each item is already Ok, no stream errors.
        let input_stream = futures::stream::iter(
            chunks
                .into_iter()
                .map(Ok::<_, InvokeModelWithBidirectionalStreamInputError>),
        );

        let resp = self
            .client
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
                "list_models requires a management client (unavailable when using from_client())"
                    .into(),
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
    async fn embed(&self, request: EmbeddingRequest) -> Result<EmbeddingResponse, ProviderError> {
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
