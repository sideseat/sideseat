use super::*;

// ---------------------------------------------------------------------------
// SDK stream event handler
// ---------------------------------------------------------------------------

pub(super) fn handle_stream_event(
    event: ConverseStreamOutput,
    req_model: &str,
) -> Option<Result<StreamEvent, ProviderError>> {
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
                        ReasoningContentBlockDelta::Text(t) => {
                            ContentDelta::Thinking { text: t.clone() }
                        }
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

pub(super) fn build_messages_and_system(
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
                let mut content: Vec<BContent> = msg
                    .content
                    .iter()
                    .map(block_to_bedrock)
                    .collect::<Result<_, _>>()?;
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
pub(super) fn build_additional_model_request_fields(config: &ProviderConfig) -> Value {
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

pub(super) fn build_inference_config(config: &ProviderConfig) -> InferenceConfiguration {
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
pub(super) fn guardrail_params(
    config: &ProviderConfig,
) -> Option<(String, String, GuardrailTrace)> {
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

pub(super) fn build_guardrail_config(config: &ProviderConfig) -> Option<GuardrailConfiguration> {
    let (id, version, trace) = guardrail_params(config)?;
    Some(
        GuardrailConfiguration::builder()
            .guardrail_identifier(id)
            .guardrail_version(version)
            .trace(trace)
            .build(),
    )
}

pub(super) fn build_guardrail_stream_config(
    config: &ProviderConfig,
) -> Option<GuardrailStreamConfiguration> {
    let (id, version, trace) = guardrail_params(config)?;
    Some(
        GuardrailStreamConfiguration::builder()
            .guardrail_identifier(id)
            .guardrail_version(version)
            .trace(trace)
            .build(),
    )
}

pub(super) fn build_performance_config(
    config: &ProviderConfig,
) -> Option<PerformanceConfiguration> {
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

pub(super) fn build_request_metadata_map(
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

pub(super) fn build_prompt_variables_map(
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

pub(super) fn build_amr_paths(config: &ProviderConfig) -> Vec<String> {
    config
        .extra
        .get("additional_model_response_field_paths")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn build_tool_config(
    config: &ProviderConfig,
) -> Result<Option<ToolConfiguration>, ProviderError> {
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
            ToolChoice::None => {
                unreachable!("ToolChoice::None handled at top of build_tool_config")
            }
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

    Ok(Some(builder.build().map_err(|e| {
        ProviderError::Serialization(e.to_string())
    })?))
}

// ---------------------------------------------------------------------------
// SDK content block conversions: our types → Bedrock SDK types
// ---------------------------------------------------------------------------

pub(super) fn block_to_bedrock(block: &ContentBlock) -> Result<BContent, ProviderError> {
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
                    (
                        caudio_to_bedrock(&audio.format),
                        BAudioSource::Bytes(Blob::new(bytes)),
                    )
                }
                MediaSource::S3(s3) => {
                    let s3_loc = S3Location::builder()
                        .uri(&s3.uri)
                        .set_bucket_owner(s3.bucket_owner.clone())
                        .build()
                        .map_err(|e| ProviderError::Serialization(e.to_string()))?;
                    (
                        caudio_to_bedrock(&audio.format),
                        BAudioSource::S3Location(s3_loc),
                    )
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

pub(super) fn bedrock_content_to_block(block: &BContent) -> Option<ContentBlock> {
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

pub(super) fn caudio_to_bedrock(fmt: &CAudioFmt) -> BAudioFmt {
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

pub(super) fn cimg_to_bedrock(fmt: Option<&CImgFmt>) -> ImageFormat {
    match fmt {
        Some(CImgFmt::Png) => ImageFormat::Png,
        Some(CImgFmt::Gif) => ImageFormat::Gif,
        Some(CImgFmt::Webp) => ImageFormat::Webp,
        Some(CImgFmt::Jpeg) | Some(CImgFmt::Heic) | Some(CImgFmt::Heif) | None => ImageFormat::Jpeg,
    }
}

pub(super) fn cdoc_to_bedrock(fmt: &CDocFmt) -> DocumentFormat {
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

pub(super) fn cvid_to_bedrock(fmt: &CVidFmt) -> VideoFormat {
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

pub(super) fn parse_stop_reason(r: Option<&BStopReason>) -> StopReason {
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
pub(super) fn map_bedrock_error(e: aws_sdk_bedrockruntime::Error) -> ProviderError {
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
                ProviderError::Api {
                    status: 400,
                    message: msg,
                }
            }
        }
        e => {
            let msg = e.to_string();
            if msg.to_lowercase().contains("timeout") {
                ProviderError::Timeout { ms: None }
            } else {
                ProviderError::Api {
                    status: 0,
                    message: msg,
                }
            }
        }
    }
}

/// Classify an AWS SDK Bedrock management-plane error into a [`ProviderError`].
pub(super) fn map_bedrock_mgmt_error(e: aws_sdk_bedrock::Error) -> ProviderError {
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
        e => ProviderError::Api {
            status: 0,
            message: e.to_string(),
        },
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

#[cfg(test)]
#[path = "../bedrock_tests.rs"]
mod tests;
