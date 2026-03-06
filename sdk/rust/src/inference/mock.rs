use std::collections::VecDeque;
use std::sync::Arc;

use parking_lot::Mutex;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::ProviderError;
use crate::provider::{
    AudioProvider, ChatProvider, EmbeddingProvider, ImageProvider, ModerationProvider, Provider,
    ProviderStream, VideoProvider,
};
use crate::types::{
    AudioFormat, ContentBlock, ContentBlockStart, ContentDelta, EmbeddingRequest, EmbeddingResponse,
    GeneratedImage, GeneratedVideo, ImageGenerationRequest, ImageGenerationResponse, Message,
    ModerationCategories, ModerationCategoryScores, ModerationRequest, ModerationResponse,
    ModerationResult, ProviderConfig, Response, SpeechRequest, SpeechResponse, StopReason,
    StreamEvent, ToolUseBlock, TranscriptionRequest, TranscriptionResponse, Usage,
    VideoGenerationRequest, VideoGenerationResponse,
};

type CallRecord = Arc<Mutex<Vec<(Vec<Message>, ProviderConfig)>>>;

/// A canned response to return from `MockProvider`.
#[derive(Debug)]
pub enum MockResponse {
    /// Return a text response with the given content and usage.
    Text(String, Usage),
    /// Return a tool call response.
    ToolCall {
        id: String,
        name: String,
        input: Value,
    },
    /// Return an error.
    Error(ProviderError),
    /// Return an image generation response.
    Image(ImageGenerationResponse),
    /// Return a video generation response.
    Video(VideoGenerationResponse),
    /// Return an embedding response.
    Embedding(EmbeddingResponse),
    /// Return a speech synthesis response.
    Speech(SpeechResponse),
    /// Return a transcription response.
    Transcription(TranscriptionResponse),
    /// Return a moderation response.
    Moderation(ModerationResponse),
}

/// A captured call from any non-chat operation on MockProvider.
#[derive(Debug, Clone)]
pub enum NonChatCall {
    GenerateImage(ImageGenerationRequest),
    GenerateVideo(VideoGenerationRequest),
    Embed(EmbeddingRequest),
    GenerateSpeech(SpeechRequest),
    Transcribe(TranscriptionRequest),
    Moderate(ModerationRequest),
}

/// A deterministic provider for unit testing — no API keys required.
///
/// Queue responses with `with_response()` or `with_text()` before calling.
/// Captured calls are available via `captured_calls()`.
pub struct MockProvider {
    responses: Arc<Mutex<VecDeque<MockResponse>>>,
    calls: CallRecord,
    non_chat_calls: Arc<Mutex<Vec<NonChatCall>>>,
}

impl Default for MockProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl MockProvider {
    pub fn new() -> Self {
        Self {
            responses: Arc::new(Mutex::new(VecDeque::new())),
            calls: Arc::new(Mutex::new(Vec::new())),
            non_chat_calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Queue a response to return on the next call.
    pub fn with_response(self, r: MockResponse) -> Self {
        self.responses.lock().push_back(r);
        self
    }

    /// Shorthand: queue a text response with zero usage.
    pub fn with_text(self, text: impl Into<String>) -> Self {
        self.with_response(MockResponse::Text(text.into(), Usage::default()))
    }

    /// Shorthand: queue an image generation response with a single URL.
    pub fn with_generated_image(self, url: impl Into<String>) -> Self {
        self.with_response(MockResponse::Image(ImageGenerationResponse {
            images: vec![GeneratedImage {
                url: Some(url.into()),
                b64_json: None,
                revised_prompt: None,
            }],
        }))
    }

    /// Shorthand: queue a video generation response with a single URI.
    pub fn with_generated_video(self, uri: impl Into<String>) -> Self {
        self.with_response(MockResponse::Video(VideoGenerationResponse {
            videos: vec![GeneratedVideo {
                uri: Some(uri.into()),
                b64_json: None,
                duration_secs: None,
            }],
        }))
    }

    /// Shorthand: queue an embedding response with a single vector.
    pub fn with_embedding(self, embedding: Vec<f32>) -> Self {
        self.with_response(MockResponse::Embedding(EmbeddingResponse {
            embeddings: vec![embedding],
            usage: Usage::default(),
            model: None,
        }))
    }

    /// Shorthand: queue a transcription response with the given text.
    pub fn with_transcription(self, text: impl Into<String>) -> Self {
        self.with_response(MockResponse::Transcription(TranscriptionResponse {
            text: text.into(),
            language: None,
            duration_secs: None,
            words: vec![],
            segments: vec![],
        }))
    }

    /// Shorthand: queue a moderation response with the given flagged status.
    pub fn with_moderation(self, flagged: bool) -> Self {
        self.with_response(MockResponse::Moderation(ModerationResponse {
            id: String::new(),
            model: String::new(),
            results: vec![ModerationResult {
                flagged,
                categories: ModerationCategories::default(),
                category_scores: ModerationCategoryScores::default(),
            }],
        }))
    }

    /// Number of calls made so far.
    pub fn call_count(&self) -> usize {
        self.calls.lock().len()
    }

    /// All (messages, config) pairs captured from calls.
    pub fn captured_calls(&self) -> Vec<(Vec<Message>, ProviderConfig)> {
        self.calls.lock().clone()
    }

    /// All non-chat calls captured (embed, generate_image, generate_video, etc.).
    pub fn captured_non_chat_calls(&self) -> Vec<NonChatCall> {
        self.non_chat_calls.lock().clone()
    }

    #[track_caller]
    fn pop_response(&self) -> MockResponse {
        self.responses
            .lock()
            .pop_front()
            .unwrap_or_else(|| panic!("MockProvider response queue exhausted — queue more responses with .with_response() or .with_text()"))
    }
}

#[async_trait]
impl Provider for MockProvider {
    fn provider_name(&self) -> &'static str {
        "mock"
    }
}

#[async_trait]
impl ChatProvider for MockProvider {
    fn stream(&self, messages: Vec<Message>, config: ProviderConfig) -> ProviderStream {
        self.calls.lock().push((messages, config.clone()));

        let response = self.pop_response();

        let events: Vec<Result<StreamEvent, ProviderError>> = match response {
            MockResponse::Error(e) => vec![Err(e)],
            MockResponse::Text(text, usage) => vec![
                Ok(StreamEvent::ContentBlockStart {
                    index: 0,
                    block: ContentBlockStart::Text,
                }),
                Ok(StreamEvent::ContentBlockDelta {
                    index: 0,
                    delta: ContentDelta::Text { text },
                }),
                Ok(StreamEvent::ContentBlockStop { index: 0 }),
                Ok(StreamEvent::Metadata {
                    usage,
                    model: Some(config.model.clone()),
                    id: None,
                }),
                Ok(StreamEvent::MessageStop {
                    stop_reason: StopReason::EndTurn,
                }),
            ],
            MockResponse::ToolCall { id, name, input } => {
                let input_json = serde_json::to_string(&input).unwrap_or_else(|e| {
                    tracing::warn!("MockProvider: failed to serialize tool input for '{}': {}", name, e);
                    String::new()
                });
                vec![
                    Ok(StreamEvent::ContentBlockStart {
                        index: 0,
                        block: ContentBlockStart::ToolUse {
                            id: id.clone(),
                            name: name.clone(),
                        },
                    }),
                    Ok(StreamEvent::ContentBlockDelta {
                        index: 0,
                        delta: ContentDelta::ToolInput {
                            partial_json: input_json,
                        },
                    }),
                    Ok(StreamEvent::ContentBlockStop { index: 0 }),
                    Ok(StreamEvent::Metadata {
                        usage: Usage::default(),
                        model: Some(config.model.clone()),
                        id: None,
                    }),
                    Ok(StreamEvent::MessageStop {
                        stop_reason: StopReason::ToolUse,
                    }),
                ]
            }
            MockResponse::Image(_) | MockResponse::Video(_) => {
                panic!(
                    "MockProvider: Image/Video response dequeued in stream() — \
                     use generate_image()/generate_video() to consume image/video responses"
                )
            }
            MockResponse::Embedding(_)
            | MockResponse::Speech(_)
            | MockResponse::Transcription(_)
            | MockResponse::Moderation(_) => {
                panic!(
                    "MockProvider: non-chat response dequeued in stream() — \
                     use the appropriate non-chat method to consume this response"
                )
            }
        };

        Box::pin(futures::stream::iter(events))
    }

    async fn complete(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<Response, ProviderError> {
        self.calls.lock().push((messages, config.clone()));

        match self.pop_response() {
            MockResponse::Error(e) => Err(e),
            MockResponse::Text(text, usage) => Ok(Response {
                content: vec![ContentBlock::text(text)],
                usage: usage.with_totals(),
                stop_reason: StopReason::EndTurn,
                model: Some(config.model),
                id: None,
                container: None,
                logprobs: None,
                grounding_metadata: None,
                warnings: vec![],
                request_body: None,
            }),
            MockResponse::ToolCall { id, name, input } => Ok(Response {
                content: vec![ContentBlock::ToolUse(ToolUseBlock { id, name, input })],
                usage: Usage::default().with_totals(),
                stop_reason: StopReason::ToolUse,
                model: Some(config.model),
                id: None,
                container: None,
                logprobs: None,
                grounding_metadata: None,
                warnings: vec![],
                request_body: None,
            }),
            MockResponse::Image(_) | MockResponse::Video(_) => {
                panic!(
                    "MockProvider: Image/Video response dequeued in complete() — \
                     use generate_image()/generate_video() to consume image/video responses"
                )
            }
            MockResponse::Embedding(_)
            | MockResponse::Speech(_)
            | MockResponse::Transcription(_)
            | MockResponse::Moderation(_) => {
                panic!(
                    "MockProvider: non-chat response dequeued in complete() — \
                     use the appropriate non-chat method to consume this response"
                )
            }
        }
    }

}

#[async_trait]
impl ImageProvider for MockProvider {
    async fn generate_image(
        &self,
        request: ImageGenerationRequest,
    ) -> Result<ImageGenerationResponse, ProviderError> {
        self.non_chat_calls.lock().push(NonChatCall::GenerateImage(request));
        match self.pop_response() {
            MockResponse::Image(r) => Ok(r),
            MockResponse::Error(e) => Err(e),
            _ => Ok(ImageGenerationResponse { images: vec![] }),
        }
    }
}

#[async_trait]
impl VideoProvider for MockProvider {
    async fn generate_video(
        &self,
        request: VideoGenerationRequest,
    ) -> Result<VideoGenerationResponse, ProviderError> {
        self.non_chat_calls.lock().push(NonChatCall::GenerateVideo(request));
        match self.pop_response() {
            MockResponse::Video(r) => Ok(r),
            MockResponse::Error(e) => Err(e),
            _ => Ok(VideoGenerationResponse { videos: vec![] }),
        }
    }
}

#[async_trait]
impl EmbeddingProvider for MockProvider {
    async fn embed(&self, request: EmbeddingRequest) -> Result<EmbeddingResponse, ProviderError> {
        self.non_chat_calls.lock().push(NonChatCall::Embed(request));
        match self.pop_response() {
            MockResponse::Embedding(r) => Ok(r),
            MockResponse::Error(e) => Err(e),
            _ => Ok(EmbeddingResponse { embeddings: vec![], usage: Usage::default(), model: None }),
        }
    }
}

#[async_trait]
impl AudioProvider for MockProvider {
    async fn generate_speech(
        &self,
        request: SpeechRequest,
    ) -> Result<SpeechResponse, ProviderError> {
        self.non_chat_calls.lock().push(NonChatCall::GenerateSpeech(request));
        match self.pop_response() {
            MockResponse::Speech(r) => Ok(r),
            MockResponse::Error(e) => Err(e),
            _ => Ok(SpeechResponse { audio: vec![], format: AudioFormat::Mp3 }),
        }
    }

    async fn transcribe(
        &self,
        request: TranscriptionRequest,
    ) -> Result<TranscriptionResponse, ProviderError> {
        self.non_chat_calls.lock().push(NonChatCall::Transcribe(request.clone()));
        match self.pop_response() {
            MockResponse::Transcription(r) => Ok(r),
            MockResponse::Error(e) => Err(e),
            _ => Ok(TranscriptionResponse {
                text: String::new(),
                language: None,
                duration_secs: None,
                words: vec![],
                segments: vec![],
            }),
        }
    }
}

#[async_trait]
impl ModerationProvider for MockProvider {
    async fn moderate(
        &self,
        request: ModerationRequest,
    ) -> Result<ModerationResponse, ProviderError> {
        self.non_chat_calls.lock().push(NonChatCall::Moderate(request));
        match self.pop_response() {
            MockResponse::Moderation(r) => Ok(r),
            MockResponse::Error(e) => Err(e),
            _ => Ok(ModerationResponse { id: String::new(), model: String::new(), results: vec![] }),
        }
    }
}
