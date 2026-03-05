use std::collections::VecDeque;
use std::sync::Arc;

use parking_lot::Mutex;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::ProviderError;
use crate::provider::{ChatProvider, ImageProvider, Provider, ProviderStream, VideoProvider};
use crate::types::{
    ContentBlock, ContentBlockStart, ContentDelta, GeneratedImage, GeneratedVideo,
    ImageGenerationRequest, ImageGenerationResponse, Message, ProviderConfig, Response,
    StopReason, StreamEvent, ToolUseBlock, Usage, VideoGenerationRequest, VideoGenerationResponse,
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
}

/// A deterministic provider for unit testing — no API keys required.
///
/// Queue responses with `with_response()` or `with_text()` before calling.
/// Captured calls are available via `captured_calls()`.
pub struct MockProvider {
    responses: Arc<Mutex<VecDeque<MockResponse>>>,
    calls: CallRecord,
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

    /// Number of calls made so far.
    pub fn call_count(&self) -> usize {
        self.calls.lock().len()
    }

    /// All (messages, config) pairs captured from calls.
    pub fn captured_calls(&self) -> Vec<(Vec<Message>, ProviderConfig)> {
        self.calls.lock().clone()
    }

    fn pop_response(&self) -> MockResponse {
        self.responses
            .lock()
            .pop_front()
            .unwrap_or_else(|| MockResponse::Text(String::new(), Usage::default()))
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
                let input_json = serde_json::to_string(&input).unwrap_or_default();
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
            // Image/Video responses are not meaningful over the stream interface
            MockResponse::Image(_) | MockResponse::Video(_) => vec![Ok(StreamEvent::MessageStop {
                stop_reason: StopReason::EndTurn,
            })],
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
                // Image/Video responses are returned via generate_image/generate_video
                // If accidentally dequeued in complete(), return empty text.
                Ok(Response {
                    content: vec![],
                    usage: Usage::default().with_totals(),
                    stop_reason: StopReason::EndTurn,
                    model: Some(config.model),
                    id: None,
                    container: None,
                    logprobs: None,
                    grounding_metadata: None,
                    warnings: vec![],
                    request_body: None,
                })
            }
        }
    }

}

#[async_trait]
impl ImageProvider for MockProvider {
    async fn generate_image(
        &self,
        _request: ImageGenerationRequest,
    ) -> Result<ImageGenerationResponse, ProviderError> {
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
        _request: VideoGenerationRequest,
    ) -> Result<VideoGenerationResponse, ProviderError> {
        match self.pop_response() {
            MockResponse::Video(r) => Ok(r),
            MockResponse::Error(e) => Err(e),
            _ => Ok(VideoGenerationResponse { videos: vec![] }),
        }
    }
}
