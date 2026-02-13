//! Span enrichment (Stage 3)
//!
//! Calculates derived data from spans and messages:
//! - Cost calculation from token usage and model pricing
//! - Input/output preview extraction from messages
//!
//! Returns enrichment data separately; persist stage applies it to DB records.

use crate::data::types::MessageCategory;
use crate::domain::pricing::{PricingService, SpanCostInput};
use crate::domain::sideml::{ChatMessage, SideMLMessage};
use crate::domain::traces::SpanData;
use crate::utils::string::{PREVIEW_MAX_LENGTH, truncate_preview};

// ============================================================================
// ENRICHMENT DATA STRUCTURES
// ============================================================================

/// Enrichment data derived from a span (Stage 3 output).
///
/// Contains calculated costs and extracted previews.
/// Applied to DB records during persist stage.
#[derive(Debug, Clone, Default)]
pub(super) struct SpanEnrichment {
    /// Cost for input tokens
    pub input_cost: f64,
    /// Cost for output tokens
    pub output_cost: f64,
    /// Cost for cache read tokens
    pub cache_read_cost: f64,
    /// Cost for cache write tokens
    pub cache_write_cost: f64,
    /// Cost for reasoning tokens
    pub reasoning_cost: f64,
    /// Total cost
    pub total_cost: f64,
    /// Preview of input (from user/system messages)
    pub input_preview: Option<String>,
    /// Preview of output (from assistant/choice messages)
    pub output_preview: Option<String>,
}

// ============================================================================
// BATCH ENRICHMENT
// ============================================================================

/// Calculate enrichments for a batch of spans (Stage 3).
///
/// Returns enrichment data (costs, previews) for each span.
/// The persist stage applies these to DB records.
pub(super) fn enrich_batch(
    spans: &[SpanData],
    messages: &[Vec<SideMLMessage>],
    pricing: &PricingService,
) -> Vec<SpanEnrichment> {
    spans
        .iter()
        .zip(messages.iter())
        .map(|(span, msgs)| enrich_one(span, msgs, pricing))
        .collect()
}

/// Calculate enrichment for a single span.
fn enrich_one(
    span: &SpanData,
    messages: &[SideMLMessage],
    pricing: &PricingService,
) -> SpanEnrichment {
    let costs = calculate_span_cost(span, pricing);
    let (input_preview, output_preview) = extract_io_preview(messages);

    // Fall back to exception_message when no output messages produced a preview
    let output_preview = output_preview.or_else(|| {
        span.exception_message
            .as_ref()
            .map(|msg| truncate_preview(msg, PREVIEW_MAX_LENGTH))
            .filter(|p| !p.is_empty())
    });

    SpanEnrichment {
        input_cost: costs.input_cost,
        output_cost: costs.output_cost,
        cache_read_cost: costs.cache_read_cost,
        cache_write_cost: costs.cache_write_cost,
        reasoning_cost: costs.reasoning_cost,
        total_cost: costs.total_cost,
        input_preview,
        output_preview,
    }
}

// ============================================================================
// COST CALCULATION
// ============================================================================

/// Internal result of cost calculation
#[derive(Debug, Clone, Default)]
struct CostResult {
    input_cost: f64,
    output_cost: f64,
    cache_read_cost: f64,
    cache_write_cost: f64,
    reasoning_cost: f64,
    total_cost: f64,
}

/// Calculate costs for a span using the pricing service.
///
/// Falls back to pre-calculated costs from OpenInference (llm.cost.*) if:
/// - No model is available for pricing lookup, or
/// - Pricing service returns zero costs
fn calculate_span_cost(span: &SpanData, pricing: &PricingService) -> CostResult {
    let model = span
        .gen_ai_response_model
        .as_deref()
        .or(span.gen_ai_request_model.as_deref())
        .unwrap_or("");

    // Try pricing service first if we have a model
    if !model.is_empty() {
        let input = SpanCostInput {
            model: Some(model.to_string()),
            system: span.gen_ai_system.clone(),
            input_tokens: span.gen_ai_usage_input_tokens,
            output_tokens: span.gen_ai_usage_output_tokens,
            total_tokens: span.gen_ai_usage_total_tokens,
            cache_read_tokens: span.gen_ai_usage_cache_read_tokens,
            cache_write_tokens: span.gen_ai_usage_cache_write_tokens,
            reasoning_tokens: span.gen_ai_usage_reasoning_tokens,
        };

        let output = pricing.calculate_cost(&input);

        // If pricing service found the model, use calculated costs
        if output.total_cost > 0.0 {
            return CostResult {
                input_cost: output.input_cost,
                output_cost: output.output_cost,
                cache_read_cost: output.cache_read_cost,
                cache_write_cost: output.cache_write_cost,
                reasoning_cost: output.reasoning_cost,
                total_cost: output.total_cost,
            };
        }
    }

    // Fallback to pre-calculated costs (OpenInference llm.cost.* attributes)
    if let Some(total) = span.extracted_cost_total {
        return CostResult {
            input_cost: span.extracted_cost_input.unwrap_or(0.0),
            output_cost: span.extracted_cost_output.unwrap_or(0.0),
            cache_read_cost: 0.0,
            cache_write_cost: 0.0,
            reasoning_cost: 0.0,
            total_cost: total,
        };
    }

    CostResult::default()
}

// ============================================================================
// PREVIEW EXTRACTION
// ============================================================================

/// Extract input and output preview from SideML messages.
///
/// Returns (input_preview, output_preview) tuple with truncated text.
///
/// **Input priority**: User message > System message (first user wins).
/// **Output**: Last text content wins (skipping tool calls), with tool call fallback.
fn extract_io_preview(messages: &[SideMLMessage]) -> (Option<String>, Option<String>) {
    let mut input_preview: Option<String> = None;
    let mut output_preview: Option<String> = None;
    let mut output_fallback: Option<String> = None;

    for message in messages {
        match message.category {
            // Input: prefer User over System, last User wins
            MessageCategory::GenAIUserMessage => {
                let preview = extract_content_preview(&message.sideml, PREVIEW_MAX_LENGTH, true);
                if !preview.is_empty() {
                    input_preview = Some(preview);
                }
            }
            MessageCategory::GenAISystemMessage => {
                if input_preview.is_none() {
                    let preview =
                        extract_content_preview(&message.sideml, PREVIEW_MAX_LENGTH, true);
                    if !preview.is_empty() {
                        input_preview = Some(preview);
                    }
                }
            }
            // Output: last text wins, tool call as fallback
            MessageCategory::GenAIChoice
            | MessageCategory::GenAIAssistantMessage
            | MessageCategory::Exception => {
                let text_preview =
                    extract_content_preview(&message.sideml, PREVIEW_MAX_LENGTH, false);
                if !text_preview.is_empty() {
                    output_preview = Some(text_preview);
                } else {
                    let any_preview =
                        extract_content_preview(&message.sideml, PREVIEW_MAX_LENGTH, true);
                    if !any_preview.is_empty() {
                        output_fallback = Some(any_preview);
                    }
                }
            }
            _ => {}
        }
    }

    (input_preview, output_preview.or(output_fallback))
}

/// Extract preview text from a ChatMessage.
///
/// Iterates through strongly-typed content blocks and extracts the first text.
/// When `include_tools` is false, ToolUse and ToolResult blocks are skipped.
fn extract_content_preview(msg: &ChatMessage, max_len: usize, include_tools: bool) -> String {
    use crate::domain::sideml::ContentBlock;

    for block in &msg.content {
        match block {
            ContentBlock::Text { text } => {
                return truncate_preview(text, max_len);
            }
            ContentBlock::Thinking { text, .. } => {
                return truncate_preview(text, max_len);
            }
            ContentBlock::Refusal { message } => {
                return truncate_preview(message, max_len);
            }
            ContentBlock::Json { data } => {
                let s = data.to_string();
                return truncate_preview(&s, max_len);
            }
            ContentBlock::ToolUse { name, input, .. } if include_tools => {
                let s = format!("{name}({input})");
                return truncate_preview(&s, max_len);
            }
            ContentBlock::ToolResult { content, .. } if include_tools => {
                let s = content.to_string();
                return truncate_preview(&s, max_len);
            }
            _ => continue,
        }
    }
    String::new()
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::types::MessageSourceType;
    use crate::domain::sideml::ChatRole;
    use crate::domain::traces::MessageSource;
    use chrono::Utc;

    fn make_span() -> SpanData {
        SpanData {
            project_id: Some("test".to_string()),
            trace_id: "trace1".to_string(),
            span_id: "span1".to_string(),
            span_name: "test".to_string(),
            timestamp_start: Utc::now(),
            ..Default::default()
        }
    }

    fn make_message(category: MessageCategory, role: ChatRole, content: &str) -> SideMLMessage {
        use crate::domain::sideml::ContentBlock;
        SideMLMessage {
            source: MessageSource::Attribute {
                key: "test".to_string(),
                time: Utc::now(),
            },
            category,
            source_type: MessageSourceType::Attribute,
            timestamp: Utc::now(),
            sideml: ChatMessage {
                role,
                content: vec![ContentBlock::Text {
                    text: content.to_string(),
                }],
                ..Default::default()
            },
        }
    }

    // === Batch Enrichment Tests ===

    #[test]
    fn test_enrich_batch() {
        let pricing = PricingService::init_for_test().unwrap();
        let spans = vec![make_span(), make_span()];
        let messages = vec![
            vec![
                make_message(MessageCategory::GenAIUserMessage, ChatRole::User, "Hello"),
                make_message(
                    MessageCategory::GenAIAssistantMessage,
                    ChatRole::Assistant,
                    "Hi there!",
                ),
            ],
            vec![],
        ];

        let enrichments = enrich_batch(&spans, &messages, &pricing);

        assert_eq!(enrichments.len(), 2);
        assert_eq!(enrichments[0].input_preview, Some("Hello".to_string()));
        assert_eq!(enrichments[0].output_preview, Some("Hi there!".to_string()));
        assert_eq!(enrichments[1].input_preview, None);
        assert_eq!(enrichments[1].output_preview, None);
    }

    // === Single Span Enrichment Tests ===

    #[test]
    fn test_enrich_one_with_messages() {
        let span = make_span();
        let messages = vec![
            make_message(
                MessageCategory::GenAIUserMessage,
                ChatRole::User,
                "Hello world",
            ),
            make_message(
                MessageCategory::GenAIAssistantMessage,
                ChatRole::Assistant,
                "Hi there!",
            ),
        ];
        let pricing = PricingService::init_for_test().unwrap();

        let enrichment = enrich_one(&span, &messages, &pricing);

        assert_eq!(enrichment.input_preview, Some("Hello world".to_string()));
        assert_eq!(enrichment.output_preview, Some("Hi there!".to_string()));
    }

    #[test]
    fn test_enrich_one_empty_messages() {
        let span = make_span();
        let messages = vec![];
        let pricing = PricingService::init_for_test().unwrap();

        let enrichment = enrich_one(&span, &messages, &pricing);

        assert_eq!(enrichment.input_preview, None);
        assert_eq!(enrichment.output_preview, None);
    }

    // === Cost Calculation Tests ===

    #[test]
    fn test_calculate_span_cost_empty_model() {
        let span = SpanData {
            trace_id: "t1".to_string(),
            span_id: "s1".to_string(),
            span_name: "test".to_string(),
            timestamp_start: Utc::now(),
            ..Default::default()
        };

        let pricing = PricingService::init_for_test().unwrap();
        let cost = calculate_span_cost(&span, &pricing);

        assert_eq!(cost.total_cost, 0.0);
    }

    #[test]
    fn test_span_enrichment_default() {
        let enrichment = SpanEnrichment::default();

        assert_eq!(enrichment.input_cost, 0.0);
        assert_eq!(enrichment.output_cost, 0.0);
        assert_eq!(enrichment.total_cost, 0.0);
        assert_eq!(enrichment.input_preview, None);
        assert_eq!(enrichment.output_preview, None);
    }

    // === Preview Extraction Tests ===

    #[test]
    fn test_extract_io_preview_from_messages() {
        let messages = vec![
            make_message(
                MessageCategory::GenAIUserMessage,
                ChatRole::User,
                "Hello world",
            ),
            make_message(
                MessageCategory::GenAIChoice,
                ChatRole::Assistant,
                "Hi there!",
            ),
        ];

        let (input, output) = extract_io_preview(&messages);
        assert_eq!(input, Some("Hello world".to_string()));
        assert_eq!(output, Some("Hi there!".to_string()));
    }

    #[test]
    fn test_extract_content_preview_string() {
        use crate::domain::sideml::ContentBlock;
        let msg = ChatMessage {
            role: ChatRole::User,
            content: vec![ContentBlock::Text {
                text: "This is a test".to_string(),
            }],
            ..Default::default()
        };
        assert_eq!(
            extract_content_preview(&msg, PREVIEW_MAX_LENGTH, true),
            "This is a test"
        );
    }

    #[test]
    fn test_extract_content_preview_multimodal() {
        use crate::domain::sideml::ContentBlock;
        let msg = ChatMessage {
            role: ChatRole::User,
            content: vec![
                ContentBlock::Text {
                    text: "Look at this image".to_string(),
                },
                ContentBlock::Image {
                    media_type: Some("image/png".to_string()),
                    source: "base64".to_string(),
                    data: "...".to_string(),
                    detail: None,
                },
            ],
            ..Default::default()
        };
        assert_eq!(
            extract_content_preview(&msg, PREVIEW_MAX_LENGTH, true),
            "Look at this image"
        );
    }

    // === Regression: ContentBlock preview extraction for Json/ToolUse/ToolResult ===

    #[test]
    fn test_extract_content_preview_json() {
        use crate::domain::sideml::ContentBlock;
        let msg = ChatMessage {
            role: ChatRole::Assistant,
            content: vec![ContentBlock::Json {
                data: serde_json::json!({"city": "Paris", "population": 2161000}),
            }],
            ..Default::default()
        };
        let preview = extract_content_preview(&msg, PREVIEW_MAX_LENGTH, true);
        assert!(
            preview.contains("Paris"),
            "Json preview should contain data: {preview}"
        );
        assert!(!preview.is_empty(), "Json preview should not be empty");
    }

    #[test]
    fn test_extract_content_preview_tool_use() {
        use crate::domain::sideml::ContentBlock;
        let msg = ChatMessage {
            role: ChatRole::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: Some("call_123".to_string()),
                name: "get_weather".to_string(),
                input: serde_json::json!({"city": "London"}),
            }],
            ..Default::default()
        };
        let preview = extract_content_preview(&msg, PREVIEW_MAX_LENGTH, true);
        assert!(
            preview.starts_with("get_weather("),
            "ToolUse preview should start with name: {preview}"
        );
        assert!(
            preview.contains("London"),
            "ToolUse preview should contain input: {preview}"
        );
    }

    #[test]
    fn test_extract_content_preview_tool_result() {
        use crate::domain::sideml::ContentBlock;
        let msg = ChatMessage {
            role: ChatRole::Tool,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: Some("call_123".to_string()),
                content: serde_json::json!("Sunny, 22°C"),
                is_error: false,
            }],
            ..Default::default()
        };
        let preview = extract_content_preview(&msg, PREVIEW_MAX_LENGTH, true);
        assert!(
            preview.contains("Sunny"),
            "ToolResult preview should contain content: {preview}"
        );
    }

    #[test]
    fn test_extract_content_preview_json_only_no_text() {
        use crate::domain::sideml::ContentBlock;
        // Structured output with no text block
        let msg = ChatMessage {
            role: ChatRole::Assistant,
            content: vec![ContentBlock::Json {
                data: serde_json::json!({"answer": 42}),
            }],
            ..Default::default()
        };
        let preview = extract_content_preview(&msg, PREVIEW_MAX_LENGTH, true);
        assert!(
            !preview.is_empty(),
            "Json-only message should produce a preview"
        );
    }

    #[test]
    fn test_extract_io_preview_structured_output() {
        use crate::domain::sideml::ContentBlock;
        // Strands structured_output pattern: output is Json, not Text
        let messages = vec![
            make_message(
                MessageCategory::GenAIUserMessage,
                ChatRole::User,
                "What is the capital of France?",
            ),
            SideMLMessage {
                source: MessageSource::Attribute {
                    key: "test".to_string(),
                    time: Utc::now(),
                },
                category: MessageCategory::GenAIChoice,
                source_type: MessageSourceType::Attribute,
                timestamp: Utc::now(),
                sideml: ChatMessage {
                    role: ChatRole::Assistant,
                    content: vec![ContentBlock::Json {
                        data: serde_json::json!({"capital": "Paris"}),
                    }],
                    ..Default::default()
                },
            },
        ];

        let (input, output) = extract_io_preview(&messages);
        assert_eq!(
            input,
            Some("What is the capital of France?".to_string()),
            "Input preview from text"
        );
        assert!(
            output.is_some(),
            "Output preview should be populated for Json content"
        );
        assert!(
            output.unwrap().contains("Paris"),
            "Output preview should contain Json data"
        );
    }

    #[test]
    fn test_truncate_preview() {
        let long_text = "a".repeat(300);
        let truncated = truncate_preview(&long_text, PREVIEW_MAX_LENGTH);
        assert!(truncated.ends_with("..."));
        assert!(truncated.len() <= PREVIEW_MAX_LENGTH + 3);
    }

    #[test]
    fn test_truncate_preview_short() {
        let short_text = "hello";
        assert_eq!(truncate_preview(short_text, PREVIEW_MAX_LENGTH), "hello");
    }

    // === Input/Output Priority Tests ===

    fn make_tool_use_message(
        category: MessageCategory,
        name: &str,
        input: serde_json::Value,
    ) -> SideMLMessage {
        use crate::domain::sideml::ContentBlock;
        SideMLMessage {
            source: MessageSource::Attribute {
                key: "test".to_string(),
                time: Utc::now(),
            },
            category,
            source_type: MessageSourceType::Attribute,
            timestamp: Utc::now(),
            sideml: ChatMessage {
                role: ChatRole::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: Some("call_1".to_string()),
                    name: name.to_string(),
                    input,
                }],
                ..Default::default()
            },
        }
    }

    #[test]
    fn test_input_preview_prefers_user_over_system() {
        let messages = vec![
            make_message(
                MessageCategory::GenAISystemMessage,
                ChatRole::System,
                "You are a helpful weather assistant.",
            ),
            make_message(
                MessageCategory::GenAIUserMessage,
                ChatRole::User,
                "What is the weather in NYC?",
            ),
        ];

        let (input, _) = extract_io_preview(&messages);
        assert_eq!(input, Some("What is the weather in NYC?".to_string()));
    }

    #[test]
    fn test_input_preview_takes_last_user_message() {
        let messages = vec![
            make_message(
                MessageCategory::GenAIUserMessage,
                ChatRole::User,
                "First question",
            ),
            make_message(
                MessageCategory::GenAIUserMessage,
                ChatRole::User,
                "Actually, what is the weather in London?",
            ),
        ];

        let (input, _) = extract_io_preview(&messages);
        assert_eq!(
            input,
            Some("Actually, what is the weather in London?".to_string())
        );
    }

    #[test]
    fn test_input_preview_falls_back_to_system() {
        let messages = vec![make_message(
            MessageCategory::GenAISystemMessage,
            ChatRole::System,
            "You are a helpful assistant.",
        )];

        let (input, _) = extract_io_preview(&messages);
        assert_eq!(input, Some("You are a helpful assistant.".to_string()));
    }

    #[test]
    fn test_output_preview_prefers_last_text_over_tool_use() {
        let messages = vec![
            make_message(
                MessageCategory::GenAIUserMessage,
                ChatRole::User,
                "What is the weather?",
            ),
            make_tool_use_message(
                MessageCategory::GenAIChoice,
                "temperature_forecast",
                serde_json::json!({"city": "NYC"}),
            ),
            make_message(
                MessageCategory::GenAIChoice,
                ChatRole::Assistant,
                "The weather in NYC is sunny and 72F.",
            ),
        ];

        let (_, output) = extract_io_preview(&messages);
        assert_eq!(
            output,
            Some("The weather in NYC is sunny and 72F.".to_string())
        );
    }

    #[test]
    fn test_output_preview_falls_back_to_tool_use() {
        let messages = vec![make_tool_use_message(
            MessageCategory::GenAIChoice,
            "get_weather",
            serde_json::json!({"city": "London"}),
        )];

        let (_, output) = extract_io_preview(&messages);
        assert!(output.is_some());
        assert!(output.unwrap().starts_with("get_weather("));
    }

    #[test]
    fn test_output_preview_takes_last_text() {
        let messages = vec![
            make_message(
                MessageCategory::GenAIAssistantMessage,
                ChatRole::Assistant,
                "First response",
            ),
            make_message(
                MessageCategory::GenAIChoice,
                ChatRole::Assistant,
                "Final response",
            ),
        ];

        let (_, output) = extract_io_preview(&messages);
        assert_eq!(output, Some("Final response".to_string()));
    }

    #[test]
    fn test_output_preview_error_fallback() {
        let span = SpanData {
            project_id: Some("test".to_string()),
            trace_id: "trace1".to_string(),
            span_id: "span1".to_string(),
            span_name: "test".to_string(),
            timestamp_start: Utc::now(),
            exception_message: Some("Connection refused".to_string()),
            ..Default::default()
        };
        let messages = vec![make_message(
            MessageCategory::GenAIUserMessage,
            ChatRole::User,
            "Hello",
        )];
        let pricing = PricingService::init_for_test().unwrap();

        let enrichment = enrich_one(&span, &messages, &pricing);
        assert_eq!(
            enrichment.output_preview,
            Some("Connection refused".to_string())
        );
    }

    #[test]
    fn test_output_preview_exception_message() {
        let messages = vec![make_message(
            MessageCategory::Exception,
            ChatRole::Assistant,
            "RuntimeError: something went wrong",
        )];

        let (_, output) = extract_io_preview(&messages);
        assert_eq!(
            output,
            Some("RuntimeError: something went wrong".to_string())
        );
    }

    #[test]
    fn test_extract_content_preview_skip_tools() {
        use crate::domain::sideml::ContentBlock;
        let msg = ChatMessage {
            role: ChatRole::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: Some("call_1".to_string()),
                name: "get_weather".to_string(),
                input: serde_json::json!({"city": "London"}),
            }],
            ..Default::default()
        };
        assert_eq!(extract_content_preview(&msg, PREVIEW_MAX_LENGTH, false), "");
        assert!(!extract_content_preview(&msg, PREVIEW_MAX_LENGTH, true).is_empty());
    }
}
