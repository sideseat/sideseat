use std::collections::HashMap;
use std::pin::Pin;
use std::task::{Context, Poll};

use async_trait::async_trait;
use futures::{Stream, StreamExt, pin_mut};

use crate::error::ProviderError;
use crate::types::{
    ContentBlock, ContentBlockStart, ContentDelta, EmbeddingRequest, EmbeddingResponse, Message,
    ModelInfo, ProviderConfig, Response, Role, StopReason, StreamEvent, ThinkingBlock, TokenCount,
    ToolResultBlock, ToolUseBlock, Usage,
};

/// Boxed stream of provider events.
pub type ProviderStream = Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send>>;

/// Common interface for all LLM providers.
///
/// Implement `stream()` — defaults for `complete()`, `list_models()`, `count_tokens()`, and
/// `embed()` are provided (the latter three return `Unsupported` unless overridden).
#[async_trait]
pub trait Provider: Send + Sync {
    /// Start a streaming conversation. All providers must implement this.
    fn stream(&self, messages: Vec<Message>, config: ProviderConfig) -> ProviderStream;

    /// Run a non-streaming request. Default implementation collects the stream.
    async fn complete(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<Response, ProviderError> {
        let stream = self.stream(messages, config);
        collect_stream(stream).await
    }

    /// List available models for this provider.
    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        Err(ProviderError::Unsupported(
            "list_models not supported by this provider".into(),
        ))
    }

    /// Count tokens for a request without generating a response.
    async fn count_tokens(
        &self,
        _messages: Vec<Message>,
        _config: ProviderConfig,
    ) -> Result<TokenCount, ProviderError> {
        Err(ProviderError::Unsupported(
            "count_tokens not supported by this provider".into(),
        ))
    }

    /// Generate embeddings for one or more texts.
    async fn embed(
        &self,
        _request: EmbeddingRequest,
        _model: &str,
    ) -> Result<EmbeddingResponse, ProviderError> {
        Err(ProviderError::Unsupported(
            "embed not supported by this provider".into(),
        ))
    }
}

/// Collect a `ProviderStream` into a `Response` by assembling content blocks.
pub async fn collect_stream(stream: ProviderStream) -> Result<Response, ProviderError> {
    pin_mut!(stream);

    let mut usage = Usage::default();
    let mut stop_reason = StopReason::EndTurn;
    let mut model: Option<String> = None;
    let mut response_id: Option<String> = None;

    // Accumulate per-block state
    let mut text_blocks: HashMap<usize, String> = HashMap::new();
    let mut tool_blocks: HashMap<usize, (String, String, String)> = HashMap::new(); // (id, name, partial_json)
    let mut thinking_blocks: HashMap<usize, (String, Option<String>)> = HashMap::new(); // (thinking, signature)

    // Ordered block indices so we preserve output order
    let mut block_order: Vec<usize> = Vec::new();

    while let Some(result) = stream.next().await {
        match result? {
            StreamEvent::ContentBlockStart { index, block } => {
                block_order.push(index);
                match block {
                    ContentBlockStart::Text => {
                        text_blocks.insert(index, String::new());
                    }
                    ContentBlockStart::ToolUse { id, name } => {
                        tool_blocks.insert(index, (id, name, String::new()));
                    }
                    ContentBlockStart::Thinking => {
                        thinking_blocks.insert(index, (String::new(), None));
                    }
                }
            }
            StreamEvent::ContentBlockDelta { index, delta } => match delta {
                ContentDelta::Text { text } => {
                    if let Some(t) = text_blocks.get_mut(&index) {
                        t.push_str(&text);
                    }
                }
                ContentDelta::ToolInput { partial_json } => {
                    if let Some((_, _, buf)) = tool_blocks.get_mut(&index) {
                        buf.push_str(&partial_json);
                    }
                }
                ContentDelta::Thinking { thinking } => {
                    if let Some((t, _)) = thinking_blocks.get_mut(&index) {
                        t.push_str(&thinking);
                    }
                }
                ContentDelta::Signature { signature } => {
                    if let Some((_, sig)) = thinking_blocks.get_mut(&index) {
                        *sig = Some(signature);
                    }
                }
            },
            StreamEvent::ContentBlockStop { .. } => {
                // Nothing to do — we finalize on MessageStop below
            }
            StreamEvent::MessageStop {
                stop_reason: sr, ..
            } => {
                stop_reason = sr;
            }
            StreamEvent::Metadata {
                usage: u,
                model: m,
                id,
            } => {
                usage = u;
                model = m;
                response_id = id;
            }
            StreamEvent::MessageStart { .. } => {}
        }
    }

    // Assemble content blocks in original order (dedup by index)
    let mut seen = std::collections::HashSet::new();
    let mut content: Vec<ContentBlock> = Vec::new();
    for index in block_order {
        if !seen.insert(index) {
            continue;
        }
        if let Some(text) = text_blocks.remove(&index) {
            if !text.is_empty() {
                content.push(ContentBlock::Text(text));
            }
        } else if let Some((id, name, json_buf)) = tool_blocks.remove(&index) {
            let input = serde_json::from_str(&json_buf).unwrap_or(serde_json::Value::Null);
            content.push(ContentBlock::ToolUse(ToolUseBlock { id, name, input }));
        } else if let Some((thinking, signature)) = thinking_blocks.remove(&index) {
            content.push(ContentBlock::Thinking(ThinkingBlock {
                thinking,
                signature,
            }));
        }
    }

    // Drain any remaining text/tool/thinking that had no explicit start event
    for text in text_blocks.into_values() {
        if !text.is_empty() {
            content.push(ContentBlock::Text(text));
        }
    }
    for (id, name, json_buf) in tool_blocks.into_values() {
        let input = serde_json::from_str(&json_buf).unwrap_or(serde_json::Value::Null);
        content.push(ContentBlock::ToolUse(ToolUseBlock { id, name, input }));
    }
    for (thinking, signature) in thinking_blocks.into_values() {
        content.push(ContentBlock::Thinking(ThinkingBlock {
            thinking,
            signature,
        }));
    }

    // Remove the ToolResultBlock variant from the public type since it should
    // not appear in responses.
    let _ = ToolResultBlock {
        tool_use_id: String::new(),
        content: vec![],
        is_error: false,
    };

    Ok(Response {
        content,
        usage: usage.with_totals(),
        stop_reason,
        model,
        id: response_id,
    })
}

/// Collect a stream into both a `Response` and all raw events.
pub async fn collect_stream_with_events(
    stream: ProviderStream,
) -> Result<(Response, Vec<StreamEvent>), ProviderError> {
    pin_mut!(stream);
    let mut all_events: Vec<StreamEvent> = Vec::new();
    while let Some(result) = stream.next().await {
        all_events.push(result?);
    }
    let replay_events = all_events.clone();
    let replay: ProviderStream = Box::pin(futures::stream::iter(
        replay_events
            .into_iter()
            .map(Ok::<StreamEvent, ProviderError>),
    ));
    let response = collect_stream(replay).await?;
    Ok((response, all_events))
}

// ---------------------------------------------------------------------------
// TextStream — yields only text delta strings
// ---------------------------------------------------------------------------

/// A stream adapter that yields only text delta strings from a provider stream.
pub struct TextStream(ProviderStream);

impl TextStream {
    pub fn new(stream: ProviderStream) -> Self {
        Self(stream)
    }
}

impl Stream for TextStream {
    type Item = Result<String, ProviderError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            match self.0.poll_next_unpin(cx) {
                Poll::Ready(Some(Ok(StreamEvent::ContentBlockDelta {
                    delta: crate::types::ContentDelta::Text { text },
                    ..
                }))) => return Poll::Ready(Some(Ok(text))),
                Poll::Ready(Some(Ok(_))) => {}
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Some(Err(e))),
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ProviderExt — ergonomic single-message helpers
// ---------------------------------------------------------------------------

/// Extension trait that adds convenience methods to any `Provider`.
#[async_trait]
pub trait ProviderExt: Provider {
    /// Stream a single user message.
    fn ask_stream(&self, content: String, config: ProviderConfig) -> ProviderStream {
        self.stream(vec![Message::user(content)], config)
    }

    /// Complete a single user message.
    async fn ask(
        &self,
        content: String,
        config: ProviderConfig,
    ) -> Result<Response, ProviderError> {
        self.complete(vec![Message::user(content)], config).await
    }

    /// Complete a single user message and return the first text block.
    async fn ask_text(
        &self,
        content: String,
        config: ProviderConfig,
    ) -> Result<String, ProviderError> {
        let response = self.ask(content, config).await?;
        response
            .first_text()
            .map(|s| s.to_string())
            .ok_or_else(|| ProviderError::Stream("No text content in response".into()))
    }
}

impl<T: Provider + ?Sized> ProviderExt for T {}

// ---------------------------------------------------------------------------
// RetryProvider
// ---------------------------------------------------------------------------

/// Wraps a provider with automatic retry on transient errors with exponential backoff.
pub struct RetryProvider<P> {
    inner: P,
    max_retries: u32,
    base_delay_ms: u64,
}

impl<P> RetryProvider<P> {
    pub fn new(inner: P, max_retries: u32) -> Self {
        Self {
            inner,
            max_retries,
            base_delay_ms: 1000,
        }
    }

    pub fn with_base_delay_ms(mut self, ms: u64) -> Self {
        self.base_delay_ms = ms;
        self
    }
}

#[async_trait]
impl<P: Provider + Send + Sync> Provider for RetryProvider<P> {
    fn stream(&self, messages: Vec<Message>, config: ProviderConfig) -> ProviderStream {
        self.inner.stream(messages, config)
    }

    async fn complete(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<Response, ProviderError> {
        let mut last_err: Option<ProviderError> = None;
        for attempt in 0..=self.max_retries {
            if attempt > 0 {
                let delay = self.base_delay_ms * (1u64 << (attempt - 1).min(5));
                tokio::time::sleep(tokio::time::Duration::from_millis(delay)).await;
            }
            match self.inner.complete(messages.clone(), config.clone()).await {
                Ok(response) => return Ok(response),
                Err(e) if e.is_retryable() => last_err = Some(e),
                Err(e) => return Err(e),
            }
        }
        Err(last_err.unwrap())
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        self.inner.list_models().await
    }

    async fn count_tokens(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<TokenCount, ProviderError> {
        self.inner.count_tokens(messages, config).await
    }

    async fn embed(
        &self,
        request: EmbeddingRequest,
        model: &str,
    ) -> Result<EmbeddingResponse, ProviderError> {
        self.inner.embed(request, model).await
    }
}

// ---------------------------------------------------------------------------
// FallbackProvider
// ---------------------------------------------------------------------------

/// Tries each provider in order, returning the first successful response.
pub struct FallbackProvider {
    providers: Vec<Box<dyn Provider + Send + Sync>>,
}

impl FallbackProvider {
    pub fn new(providers: Vec<Box<dyn Provider + Send + Sync>>) -> Self {
        Self { providers }
    }
}

#[async_trait]
impl Provider for FallbackProvider {
    fn stream(&self, messages: Vec<Message>, config: ProviderConfig) -> ProviderStream {
        if let Some(p) = self.providers.first() {
            p.stream(messages, config)
        } else {
            Box::pin(futures::stream::once(async {
                Err(ProviderError::Config("No providers configured".into()))
            }))
        }
    }

    async fn complete(
        &self,
        messages: Vec<Message>,
        config: ProviderConfig,
    ) -> Result<Response, ProviderError> {
        let mut last_err: Option<ProviderError> = None;
        for provider in &self.providers {
            match provider.complete(messages.clone(), config.clone()).await {
                Ok(response) => return Ok(response),
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or_else(|| ProviderError::Config("No providers configured".into())))
    }
}

// ---------------------------------------------------------------------------
// Agent loop
// ---------------------------------------------------------------------------

/// Run an agentic tool-call loop until the model stops requesting tools.
///
/// Calls `complete`, appends the assistant turn, invokes `tool_handler` with
/// all tool-use blocks, appends the tool results, and repeats until
/// `stop_reason != ToolUse` or no tool-use blocks are present.
pub async fn run_agent_loop<P, F, Fut>(
    provider: &P,
    mut messages: Vec<Message>,
    config: ProviderConfig,
    tool_handler: F,
) -> Result<Response, ProviderError>
where
    P: Provider,
    F: Fn(Vec<ToolUseBlock>) -> Fut,
    Fut: std::future::Future<Output = Vec<(String, String)>>,
{
    loop {
        let response = provider.complete(messages.clone(), config.clone()).await?;

        if response.stop_reason != StopReason::ToolUse || !response.has_tool_use() {
            return Ok(response);
        }

        let tool_uses: Vec<ToolUseBlock> = response.tool_uses().into_iter().cloned().collect();

        messages.push(Message::with_content(Role::Assistant, response.content));

        let results = tool_handler(tool_uses).await;
        messages.push(Message::with_tool_results(results));
    }
}

// ---------------------------------------------------------------------------
// Batch completion
// ---------------------------------------------------------------------------

/// Run multiple completion requests concurrently and collect all results.
pub async fn batch_complete<P: Provider>(
    provider: &P,
    requests: Vec<(Vec<Message>, ProviderConfig)>,
) -> Vec<Result<Response, ProviderError>> {
    futures::future::join_all(
        requests
            .into_iter()
            .map(|(msgs, cfg)| provider.complete(msgs, cfg)),
    )
    .await
}
