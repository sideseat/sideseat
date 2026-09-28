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
