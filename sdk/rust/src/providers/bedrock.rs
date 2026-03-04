use std::sync::Arc;

use async_stream::stream;
use async_trait::async_trait;
use aws_sdk_bedrock::Client as BedrockMgmtClient;
use aws_sdk_bedrockruntime::{
    Client,
    primitives::Blob,
    types::{
        AnyToolChoice, AutoToolChoice, ContentBlock as BContent, ContentBlockDelta,
        ContentBlockStart as BContentBlockStart, ConversationRole, ConverseOutput,
        ConverseStreamOutput, DocumentBlock, DocumentFormat, DocumentSource, ImageBlock,
        ImageFormat, ImageSource, InferenceConfiguration, Message as BMessage,
        ReasoningContentBlock, ReasoningTextBlock, S3Location, SpecificToolChoice,
        StopReason as BStopReason, SystemContentBlock, Tool as BTool, ToolChoice as BToolChoice,
        ToolConfiguration, ToolInputSchema, ToolResultBlock as BToolResult, ToolResultContentBlock,
        ToolResultStatus, ToolSpecification, ToolUseBlock as BToolUse, VideoBlock, VideoFormat,
        VideoSource,
    },
};
use aws_smithy_types::Document;
use reqwest::Client as HttpClient;
use serde_json::{Value, json};

use crate::{
    error::ProviderError,
    provider::{Provider, ProviderStream},
    types::{
        ContentBlock, ContentBlockStart, ContentDelta, DocumentFormat as CDocFmt,
        ImageFormat as CImgFmt, MediaSource, Message, ModelInfo, ProviderConfig, Role, StopReason,
        StreamEvent, ThinkingBlock, ToolChoice, ToolUseBlock, Usage, VideoFormat as CVidFmt,
    },
};

// ---------------------------------------------------------------------------
// Backend enum
// ---------------------------------------------------------------------------

enum BedrockBackend {
    /// Native AWS SDK client (IAM credentials / instance profile / static keys / profile).
    Sdk(Arc<Client>),
    /// Bedrock API key — uses plain HTTP with `Authorization: Bearer <key>`.
    /// Streaming uses the `/converse` endpoint and emits events from the response.
    ApiKey {
        api_key: String,
        region: String,
        /// Custom endpoint URL (e.g. for local emulators or enterprise deployments).
        /// Defaults to `https://bedrock-runtime.<region>.amazonaws.com`.
        endpoint_url: Option<String>,
        http_client: Arc<HttpClient>,
    },
}

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
/// - `with_api_key()` — Bedrock API key (`Authorization: Bearer`) via plain HTTP
pub struct BedrockProvider {
    backend: BedrockBackend,
    /// Bedrock management client (for listing models). None when created via `from_client()`.
    mgmt_client: Option<Arc<BedrockMgmtClient>>,
}

impl BedrockProvider {
    /// Create using the default AWS credential chain.
    pub async fn from_env(region: impl Into<String>) -> Self {
        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_config::Region::new(region.into()))
            .load()
            .await;
        Self {
            backend: BedrockBackend::Sdk(Arc::new(Client::new(&config))),
            mgmt_client: Some(Arc::new(BedrockMgmtClient::new(&config))),
        }
    }

    /// Create with explicit static credentials.
    pub async fn with_static_credentials(
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
        session_token: Option<String>,
        region: impl Into<String>,
    ) -> Self {
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
        Self {
            backend: BedrockBackend::Sdk(Arc::new(Client::new(&config))),
            mgmt_client: Some(Arc::new(BedrockMgmtClient::new(&config))),
        }
    }

    /// Create using a named AWS profile.
    pub async fn with_profile(profile_name: impl Into<String>, region: impl Into<String>) -> Self {
        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_config::Region::new(region.into()))
            .profile_name(profile_name)
            .load()
            .await;
        Self {
            backend: BedrockBackend::Sdk(Arc::new(Client::new(&config))),
            mgmt_client: Some(Arc::new(BedrockMgmtClient::new(&config))),
        }
    }

    /// Wrap an existing `aws_sdk_bedrockruntime::Client`.
    /// Note: `list_models()` is unavailable when using this constructor.
    pub fn from_client(client: Client) -> Self {
        Self {
            backend: BedrockBackend::Sdk(Arc::new(client)),
            mgmt_client: None,
        }
    }

    /// Create using a Bedrock API key (short-term or long-term).
    ///
    /// Uses plain HTTP with `Authorization: Bearer <api_key>` against the
    /// Bedrock Runtime REST API. The `/converse` endpoint is used for both
    /// streaming and non-streaming calls.
    ///
    /// See: <https://docs.aws.amazon.com/bedrock/latest/userguide/api-keys.html>
    pub fn with_api_key(api_key: impl Into<String>, region: impl Into<String>) -> Self {
        Self {
            backend: BedrockBackend::ApiKey {
                api_key: api_key.into(),
                region: region.into(),
                endpoint_url: None,
                http_client: Arc::new(HttpClient::new()),
            },
            mgmt_client: None,
        }
    }

    /// Override the Bedrock runtime endpoint URL.
    ///
    /// Only applies to the `with_api_key()` backend. Useful for local emulators,
    /// custom Bedrock deployments, or proxies that expose the Bedrock Converse API.
    /// The model path (`/model/{model}/converse`) is appended automatically.
    ///
    /// # Example
    /// ```no_run
    /// use sideseat::providers::BedrockProvider;
    /// let provider = BedrockProvider::with_api_key("key", "us-east-1")
    ///     .with_endpoint_url("http://localhost:8080");
    /// ```
    pub fn with_endpoint_url(mut self, url: impl Into<String>) -> Self {
        if let BedrockBackend::ApiKey {
            ref mut endpoint_url,
            ..
        } = self.backend
        {
            *endpoint_url = Some(url.into());
        }
        self
    }
}

// ---------------------------------------------------------------------------
// Provider trait implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl Provider for BedrockProvider {
    fn stream(&self, messages: Vec<Message>, config: ProviderConfig) -> ProviderStream {
        match &self.backend {
            BedrockBackend::Sdk(client) => {
                let client = Arc::clone(client);
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
                    if let Some(budget) = config.thinking_budget {
                        req = req.additional_model_request_fields(
                            json_to_document(&json!({
                                "thinking": {"type": "enabled", "budget_tokens": budget}
                            }))
                        );
                    }

                    let resp = match req.send().await {
                        Ok(r) => r,
                        Err(e) => {
                            let msg = e.to_string();
                            let err = if msg.to_lowercase().contains("context window")
                                || msg.to_lowercase().contains("input is too long")
                            {
                                ProviderError::ContextWindowExceeded(msg)
                            } else {
                                ProviderError::Api { status: 0, message: msg }
                            };
                            yield Err(err);
                            return;
                        }
                    };

                    let mut event_stream = resp.stream;
                    loop {
                        match event_stream.recv().await {
                            Ok(Some(event)) => {
                                match handle_stream_event(event) {
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

            BedrockBackend::ApiKey {
                api_key,
                region,
                endpoint_url,
                http_client,
            } => {
                let api_key = api_key.clone();
                let region = region.clone();
                let endpoint_url = endpoint_url.clone();
                let http_client = Arc::clone(http_client);
                Box::pin(stream! {
                    let response = match api_key_complete(&http_client, &api_key, &region, endpoint_url.as_deref(), &messages, &config).await {
                        Ok(r) => r,
                        Err(e) => { yield Err(e); return; }
                    };

                    yield Ok(StreamEvent::MessageStart { role: Role::Assistant });

                    for (idx, block) in response.content.iter().enumerate() {
                        match block {
                            ContentBlock::Text(t) => {
                                yield Ok(StreamEvent::ContentBlockStart { index: idx, block: ContentBlockStart::Text });
                                yield Ok(StreamEvent::ContentBlockDelta {
                                    index: idx,
                                    delta: ContentDelta::Text { text: t.clone() },
                                });
                                yield Ok(StreamEvent::ContentBlockStop { index: idx });
                            }
                            ContentBlock::ToolUse(tu) => {
                                yield Ok(StreamEvent::ContentBlockStart {
                                    index: idx,
                                    block: ContentBlockStart::ToolUse { id: tu.id.clone(), name: tu.name.clone() },
                                });
                                yield Ok(StreamEvent::ContentBlockDelta {
                                    index: idx,
                                    delta: ContentDelta::ToolInput { partial_json: tu.input.to_string() },
                                });
                                yield Ok(StreamEvent::ContentBlockStop { index: idx });
                            }
                            ContentBlock::Thinking(th) => {
                                yield Ok(StreamEvent::ContentBlockStart { index: idx, block: ContentBlockStart::Thinking });
                                yield Ok(StreamEvent::ContentBlockDelta {
                                    index: idx,
                                    delta: ContentDelta::Thinking { thinking: th.thinking.clone() },
                                });
                                if let Some(sig) = &th.signature {
                                    yield Ok(StreamEvent::ContentBlockDelta {
                                        index: idx,
                                        delta: ContentDelta::Signature { signature: sig.clone() },
                                    });
                                }
                                yield Ok(StreamEvent::ContentBlockStop { index: idx });
                            }
                            _ => {}
                        }
                    }

                    yield Ok(StreamEvent::MessageStop { stop_reason: response.stop_reason.clone() });
                    yield Ok(StreamEvent::Metadata { usage: response.usage.clone(), model: None, id: None });
                })
            }
        }
    }

    async fn complete(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<crate::types::Response, ProviderError> {
        match &self.backend {
            BedrockBackend::Sdk(client) => {
                let (bedrock_msgs, sys_blocks) = build_messages_and_system(&messages, &config)?;
                let inf_config = build_inference_config(&config);
                let tool_config = build_tool_config(&config)?;

                let mut req = client.converse().model_id(&config.model);
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
                if let Some(budget) = config.thinking_budget {
                    req = req.additional_model_request_fields(json_to_document(&json!({
                        "thinking": {"type": "enabled", "budget_tokens": budget}
                    })));
                }

                let resp = req.send().await.map_err(|e| {
                    let msg = e.to_string();
                    if msg.to_lowercase().contains("context window")
                        || msg.to_lowercase().contains("input is too long")
                    {
                        ProviderError::ContextWindowExceeded(msg)
                    } else {
                        ProviderError::Api {
                            status: 0,
                            message: msg,
                        }
                    }
                })?;

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
                    model: None,
                    id: None,
                })
            }

            BedrockBackend::ApiKey {
                api_key,
                region,
                endpoint_url,
                http_client,
            } => {
                api_key_complete(
                    http_client,
                    api_key,
                    region,
                    endpoint_url.as_deref(),
                    &messages,
                    &config,
                )
                .await
            }
        }
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        if let Some(mgmt) = &self.mgmt_client {
            let resp =
                mgmt.list_foundation_models()
                    .send()
                    .await
                    .map_err(|e| ProviderError::Api {
                        status: 0,
                        message: e.to_string(),
                    })?;

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
        } else {
            Err(ProviderError::Unsupported(
                "list_models requires SDK credentials; not available for from_client() or with_api_key()".into(),
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// SDK stream event handler
// ---------------------------------------------------------------------------

fn handle_stream_event(event: ConverseStreamOutput) -> Option<Result<StreamEvent, ProviderError>> {
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
                            thinking: t.clone(),
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
                model: None,
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
        match msg.role {
            Role::System => {
                for block in &msg.content {
                    if let ContentBlock::Text(t) = block {
                        sys.push(SystemContentBlock::Text(t.clone()));
                    }
                }
            }
            Role::User | Role::Assistant => {
                let role = if msg.role == Role::User {
                    ConversationRole::User
                } else {
                    ConversationRole::Assistant
                };
                let content: Result<Vec<BContent>, _> =
                    msg.content.iter().map(block_to_bedrock).collect();
                let bmsg = BMessage::builder()
                    .role(role)
                    .set_content(Some(content?))
                    .build()
                    .map_err(|e| ProviderError::Config(e.to_string()))?;
                bmsgs.push(bmsg);
            }
        }
    }
    Ok((bmsgs, sys))
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

fn build_tool_config(config: &ProviderConfig) -> Result<Option<ToolConfiguration>, ProviderError> {
    if config.tools.is_empty() {
        return Ok(None);
    }
    let tools: Result<Vec<BTool>, ProviderError> = config
        .tools
        .iter()
        .map(|t| {
            let spec = ToolSpecification::builder()
                .name(&t.name)
                .description(&t.description)
                .input_schema(ToolInputSchema::Json(json_to_document(&t.input_schema)))
                .build()
                .map_err(|e| ProviderError::Config(e.to_string()))?;
            Ok(BTool::ToolSpec(spec))
        })
        .collect();

    let mut builder = ToolConfiguration::builder().set_tools(Some(tools?));

    if let Some(choice) = &config.tool_choice {
        let bc = match choice {
            ToolChoice::Auto => BToolChoice::Auto(AutoToolChoice::builder().build()),
            ToolChoice::Any => BToolChoice::Any(AnyToolChoice::builder().build()),
            ToolChoice::None => return Ok(None),
            ToolChoice::Tool { name } => BToolChoice::Tool(
                SpecificToolChoice::builder()
                    .name(name)
                    .build()
                    .map_err(|e| ProviderError::Config(e.to_string()))?,
            ),
        };
        builder = builder.tool_choice(bc);
    }

    Ok(Some(
        builder
            .build()
            .map_err(|e| ProviderError::Config(e.to_string()))?,
    ))
}

// ---------------------------------------------------------------------------
// SDK content block conversions: our types → Bedrock SDK types
// ---------------------------------------------------------------------------

fn block_to_bedrock(block: &ContentBlock) -> Result<BContent, ProviderError> {
    match block {
        ContentBlock::Text(t) => Ok(BContent::Text(t.clone())),

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
                        .map_err(|e| ProviderError::Config(e.to_string()))?;
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
                .map_err(|e| ProviderError::Config(e.to_string()))?;
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
                _ => {
                    return Err(ProviderError::Unsupported(
                        "Bedrock documents require base64 source".into(),
                    ));
                }
            };
            let db = DocumentBlock::builder()
                .format(cdoc_to_bedrock(&doc.format))
                .name(doc.name.as_deref().unwrap_or("document"))
                .source(source)
                .build()
                .map_err(|e| ProviderError::Config(e.to_string()))?;
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
                        .map_err(|e| ProviderError::Config(e.to_string()))?;
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
                .map_err(|e| ProviderError::Config(e.to_string()))?;
            Ok(BContent::Video(vb))
        }

        ContentBlock::ToolUse(tu) => {
            let btu = BToolUse::builder()
                .tool_use_id(&tu.id)
                .name(&tu.name)
                .input(json_to_document(&tu.input))
                .build()
                .map_err(|e| ProviderError::Config(e.to_string()))?;
            Ok(BContent::ToolUse(btu))
        }

        ContentBlock::ToolResult(tr) => {
            let result_content: Vec<ToolResultContentBlock> = tr
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text(t) => Some(ToolResultContentBlock::Text(t.clone())),
                    ContentBlock::Image(img) => block_to_bedrock(&ContentBlock::Image(img.clone()))
                        .ok()
                        .and_then(|bc| {
                            if let BContent::Image(ib) = bc {
                                Some(ToolResultContentBlock::Image(ib))
                            } else {
                                None
                            }
                        }),
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
                .map_err(|e| ProviderError::Config(e.to_string()))?;
            Ok(BContent::ToolResult(btr))
        }

        ContentBlock::Thinking(th) => {
            let rt = ReasoningTextBlock::builder()
                .text(&th.thinking)
                .set_signature(th.signature.clone())
                .build()
                .map_err(|e| ProviderError::Config(e.to_string()))?;
            Ok(BContent::ReasoningContent(
                ReasoningContentBlock::ReasoningText(rt),
            ))
        }

        ContentBlock::Audio(_) => Err(ProviderError::Unsupported(
            "Audio via Converse API not supported; use InvokeModel directly".into(),
        )),
    }
}

fn bedrock_content_to_block(block: &BContent) -> Option<ContentBlock> {
    match block {
        BContent::Text(t) => Some(ContentBlock::Text(t.clone())),
        BContent::ToolUse(tu) => Some(ContentBlock::ToolUse(ToolUseBlock {
            id: tu.tool_use_id().to_string(),
            name: tu.name().to_string(),
            input: document_to_json(tu.input()),
        })),
        BContent::ReasoningContent(ReasoningContentBlock::ReasoningText(rt)) => {
            Some(ContentBlock::Thinking(ThinkingBlock {
                thinking: rt.text().to_string(),
                signature: rt.signature().map(|s| s.to_string()),
            }))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// API key HTTP backend
// ---------------------------------------------------------------------------

async fn api_key_complete(
    http_client: &HttpClient,
    api_key: &str,
    region: &str,
    endpoint_url: Option<&str>,
    messages: &[Message],
    config: &ProviderConfig,
) -> Result<crate::types::Response, ProviderError> {
    let url = if let Some(base) = endpoint_url {
        format!(
            "{}/model/{}/converse",
            base.trim_end_matches('/'),
            config.model
        )
    } else {
        format!(
            "https://bedrock-runtime.{}.amazonaws.com/model/{}/converse",
            region, config.model
        )
    };

    let body = build_converse_json(messages, config)?;

    let resp = http_client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&body)
        .send()
        .await
        .map_err(|e| ProviderError::Network(e.to_string()))?;

    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();

    if status == 429 {
        return Err(ProviderError::RateLimited(text));
    }
    if status != 200 {
        let lower = text.to_lowercase();
        if lower.contains("context_length_exceeded")
            || lower.contains("context window")
            || lower.contains("input is too long")
        {
            return Err(ProviderError::ContextWindowExceeded(text));
        }
        return Err(ProviderError::Api {
            status,
            message: text,
        });
    }

    let json: Value =
        serde_json::from_str(&text).map_err(|e| ProviderError::Serialization(e.to_string()))?;
    parse_converse_response(&json)
}

fn build_converse_json(
    messages: &[Message],
    config: &ProviderConfig,
) -> Result<Value, ProviderError> {
    let mut json_messages: Vec<Value> = Vec::new();
    let mut system_blocks: Vec<Value> = Vec::new();

    if let Some(s) = &config.system {
        system_blocks.push(json!({"text": s}));
    }

    for msg in messages {
        match msg.role {
            Role::System => {
                for block in &msg.content {
                    if let ContentBlock::Text(t) = block {
                        system_blocks.push(json!({"text": t}));
                    }
                }
            }
            Role::User | Role::Assistant => {
                let role_str = if msg.role == Role::User {
                    "user"
                } else {
                    "assistant"
                };
                let content: Result<Vec<Value>, ProviderError> =
                    msg.content.iter().map(block_to_converse_json).collect();
                json_messages.push(json!({ "role": role_str, "content": content? }));
            }
        }
    }

    let mut body = json!({ "messages": json_messages });

    if !system_blocks.is_empty() {
        body["system"] = json!(system_blocks);
    }

    // inferenceConfig
    let mut inf = serde_json::Map::new();
    if let Some(m) = config.max_tokens {
        inf.insert("maxTokens".into(), json!(m));
    }
    if let Some(t) = config.temperature {
        inf.insert("temperature".into(), json!(t));
    }
    if let Some(p) = config.top_p {
        inf.insert("topP".into(), json!(p));
    }
    if !config.stop_sequences.is_empty() {
        inf.insert("stopSequences".into(), json!(config.stop_sequences));
    }
    if !inf.is_empty() {
        body["inferenceConfig"] = Value::Object(inf);
    }

    // toolConfig
    if !config.tools.is_empty() {
        let tools: Vec<Value> = config
            .tools
            .iter()
            .map(|t| {
                json!({
                    "toolSpec": {
                        "name": t.name,
                        "description": t.description,
                        "inputSchema": {"json": t.input_schema}
                    }
                })
            })
            .collect();
        let mut tool_config = json!({ "tools": tools });
        if let Some(choice) = &config.tool_choice {
            tool_config["toolChoice"] = match choice {
                ToolChoice::Auto => json!({"auto": {}}),
                ToolChoice::Any => json!({"any": {}}),
                ToolChoice::Tool { name } => json!({"tool": {"name": name}}),
                ToolChoice::None => json!({"auto": {}}),
            };
        }
        body["toolConfig"] = tool_config;
    }

    // thinking
    if let Some(budget) = config.thinking_budget {
        body["additionalModelRequestFields"] = json!({
            "thinking": {"type": "enabled", "budget_tokens": budget}
        });
    }

    Ok(body)
}

fn block_to_converse_json(block: &ContentBlock) -> Result<Value, ProviderError> {
    match block {
        ContentBlock::Text(t) => Ok(json!({"text": t})),

        ContentBlock::Image(img) => {
            let fmt = img.format.as_ref().map(img_fmt_str).unwrap_or("jpeg");
            match &img.source {
                MediaSource::Base64(b64) => Ok(json!({
                    "image": { "format": fmt, "source": {"bytes": b64.data} }
                })),
                MediaSource::S3(s3) => Ok(json!({
                    "image": { "format": fmt, "source": {"s3Location": {"uri": s3.uri}} }
                })),
                _ => Err(ProviderError::Unsupported(
                    "Bedrock images require base64 or S3".into(),
                )),
            }
        }

        ContentBlock::Document(doc) => match &doc.source {
            MediaSource::Base64(b64) => Ok(json!({
                "document": {
                    "format": doc_fmt_str(&doc.format),
                    "name": doc.name.as_deref().unwrap_or("document"),
                    "source": {"bytes": b64.data}
                }
            })),
            _ => Err(ProviderError::Unsupported(
                "Bedrock documents require base64".into(),
            )),
        },

        ContentBlock::ToolUse(tu) => Ok(json!({
            "toolUse": { "toolUseId": tu.id, "name": tu.name, "input": tu.input }
        })),

        ContentBlock::ToolResult(tr) => {
            let content: Vec<Value> = tr
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text(t) => Some(json!({"text": t})),
                    _ => None,
                })
                .collect();
            Ok(json!({
                "toolResult": {
                    "toolUseId": tr.tool_use_id,
                    "content": content,
                    "status": if tr.is_error { "error" } else { "success" }
                }
            }))
        }

        ContentBlock::Thinking(th) => Ok(json!({
            "reasoningContent": {
                "reasoningText": { "text": th.thinking, "signature": th.signature }
            }
        })),

        ContentBlock::Video(_) | ContentBlock::Audio(_) => Err(ProviderError::Unsupported(
            "Video/Audio not supported via Bedrock API key".into(),
        )),
    }
}

fn parse_converse_response(body: &Value) -> Result<crate::types::Response, ProviderError> {
    let content_arr = body["output"]["message"]["content"]
        .as_array()
        .ok_or_else(|| ProviderError::Serialization("Missing content array in response".into()))?;

    let content: Vec<ContentBlock> = content_arr
        .iter()
        .filter_map(parse_converse_content_block)
        .collect();

    let usage = Usage {
        input_tokens: body["usage"]["inputTokens"].as_u64().unwrap_or(0),
        output_tokens: body["usage"]["outputTokens"].as_u64().unwrap_or(0),
        ..Default::default()
    };

    let stop_reason = match body["stopReason"].as_str() {
        Some("end_turn") | None => StopReason::EndTurn,
        Some("tool_use") => StopReason::ToolUse,
        Some("max_tokens") => StopReason::MaxTokens,
        Some("stop_sequence") => StopReason::StopSequence(String::new()),
        Some("content_filtered") | Some("guardrail_intervened") => StopReason::ContentFilter,
        Some(other) => StopReason::Other(other.to_string()),
    };

    Ok(crate::types::Response {
        content,
        usage: usage.with_totals(),
        stop_reason,
        model: None,
        id: None,
    })
}

fn parse_converse_content_block(c: &Value) -> Option<ContentBlock> {
    if let Some(text) = c["text"].as_str() {
        return Some(ContentBlock::Text(text.to_string()));
    }
    if let Some(tu) = c.get("toolUse") {
        return Some(ContentBlock::ToolUse(ToolUseBlock {
            id: tu["toolUseId"].as_str().unwrap_or("").to_string(),
            name: tu["name"].as_str().unwrap_or("").to_string(),
            input: tu["input"].clone(),
        }));
    }
    if let Some(rc) = c.get("reasoningContent")
        && let Some(rt) = rc.get("reasoningText")
    {
        return Some(ContentBlock::Thinking(ThinkingBlock {
            thinking: rt["text"].as_str().unwrap_or("").to_string(),
            signature: rt["signature"].as_str().map(|s| s.to_string()),
        }));
    }
    None
}

// ---------------------------------------------------------------------------
// Format conversions
// ---------------------------------------------------------------------------

fn cimg_to_bedrock(fmt: Option<&CImgFmt>) -> ImageFormat {
    match fmt {
        Some(CImgFmt::Png) => ImageFormat::Png,
        Some(CImgFmt::Gif) => ImageFormat::Gif,
        Some(CImgFmt::Webp) => ImageFormat::Webp,
        Some(CImgFmt::Jpeg) | Some(CImgFmt::Heic) | Some(CImgFmt::Heif) | None => ImageFormat::Jpeg,
    }
}

fn img_fmt_str(fmt: &CImgFmt) -> &'static str {
    match fmt {
        CImgFmt::Jpeg => "jpeg",
        CImgFmt::Png => "png",
        CImgFmt::Gif => "gif",
        CImgFmt::Webp => "webp",
        CImgFmt::Heic => "jpeg", // Bedrock doesn't support HEIC, fall back
        CImgFmt::Heif => "jpeg",
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

fn doc_fmt_str(fmt: &CDocFmt) -> &'static str {
    match fmt {
        CDocFmt::Pdf => "pdf",
        CDocFmt::Csv => "csv",
        CDocFmt::Doc => "doc",
        CDocFmt::Docx => "docx",
        CDocFmt::Xls => "xls",
        CDocFmt::Xlsx => "xlsx",
        CDocFmt::Html => "html",
        CDocFmt::Txt => "txt",
        CDocFmt::Md => "md",
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
            "float": 3.14,
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

    #[test]
    fn parse_converse_response_text() {
        let body = json!({
            "output": {
                "message": {
                    "role": "assistant",
                    "content": [{"text": "Hello world"}]
                }
            },
            "stopReason": "end_turn",
            "usage": {"inputTokens": 10, "outputTokens": 5}
        });
        let resp = parse_converse_response(&body).unwrap();
        assert_eq!(resp.content.len(), 1);
        assert!(matches!(&resp.content[0], ContentBlock::Text(t) if t == "Hello world"));
        assert_eq!(resp.usage.input_tokens, 10);
        assert_eq!(resp.usage.output_tokens, 5);
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
    }

    #[test]
    fn parse_converse_response_tool_use() {
        let body = json!({
            "output": {
                "message": {
                    "role": "assistant",
                    "content": [{
                        "toolUse": {
                            "toolUseId": "call_123",
                            "name": "my_tool",
                            "input": {"key": "value"}
                        }
                    }]
                }
            },
            "stopReason": "tool_use",
            "usage": {"inputTokens": 20, "outputTokens": 10}
        });
        let resp = parse_converse_response(&body).unwrap();
        assert_eq!(resp.stop_reason, StopReason::ToolUse);
        let ContentBlock::ToolUse(tu) = &resp.content[0] else {
            panic!("expected ToolUse")
        };
        assert_eq!(tu.id, "call_123");
        assert_eq!(tu.name, "my_tool");
        assert_eq!(tu.input["key"], "value");
    }

    #[test]
    fn parse_converse_response_thinking() {
        let body = json!({
            "output": {
                "message": {
                    "role": "assistant",
                    "content": [{
                        "reasoningContent": {
                            "reasoningText": {
                                "text": "Let me think...",
                                "signature": "sig123"
                            }
                        }
                    }]
                }
            },
            "stopReason": "end_turn",
            "usage": {"inputTokens": 5, "outputTokens": 15}
        });
        let resp = parse_converse_response(&body).unwrap();
        let ContentBlock::Thinking(th) = &resp.content[0] else {
            panic!("expected Thinking")
        };
        assert_eq!(th.thinking, "Let me think...");
        assert_eq!(th.signature.as_deref(), Some("sig123"));
    }

    #[test]
    fn build_converse_json_basic() {
        use crate::types::ProviderConfig;
        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text("Hello".to_string())],
            cache_control: None,
        }];
        let config = ProviderConfig::new("us.amazon.nova-lite-v1:0").with_max_tokens(100);
        let body = build_converse_json(&messages, &config).unwrap();
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"][0]["text"], "Hello");
        assert_eq!(body["inferenceConfig"]["maxTokens"], 100);
    }

    #[test]
    fn build_converse_json_system() {
        use crate::types::ProviderConfig;
        let messages = vec![];
        let mut config = ProviderConfig::new("test-model");
        config.system = Some("You are a helpful assistant".to_string());
        let body = build_converse_json(&messages, &config).unwrap();
        assert_eq!(body["system"][0]["text"], "You are a helpful assistant");
    }

    #[test]
    fn build_converse_json_tools() {
        use crate::types::{ProviderConfig, Tool};
        let messages = vec![];
        let mut config = ProviderConfig::new("test-model");
        config.tools = vec![Tool::new(
            "search",
            "Search the web",
            json!({"type": "object", "properties": {"query": {"type": "string"}}}),
        )];
        let body = build_converse_json(&messages, &config).unwrap();
        assert_eq!(body["toolConfig"]["tools"][0]["toolSpec"]["name"], "search");
    }
}
