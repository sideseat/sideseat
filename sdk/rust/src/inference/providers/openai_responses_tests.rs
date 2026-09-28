use super::*;
use crate::types::{BuiltinTool, McpToolConfig, WebSearchConfig, WebSearchUserLocation};
use serde_json::json;

#[test]
fn test_builtin_tool_file_search() {
    let config = ProviderConfig::new("gpt-4.1").with_built_in_tool(BuiltinTool::file_search());
    let req = build_request(&[Message::user("Hi")], &config, false, None).unwrap();
    assert_eq!(req["tools"][0]["type"], "file_search");
}

#[test]
fn test_builtin_tool_file_search_with_ids() {
    let config = ProviderConfig::new("gpt-4.1")
        .with_built_in_tool(BuiltinTool::file_search_with_ids(["vs_abc", "vs_def"]));
    let req = build_request(&[Message::user("Hi")], &config, false, None).unwrap();
    assert_eq!(req["tools"][0]["type"], "file_search");
    assert_eq!(req["tools"][0]["vector_store_ids"][0], "vs_abc");
}

#[test]
fn test_builtin_tool_code_interpreter() {
    let config = ProviderConfig::new("gpt-4.1").with_built_in_tool(BuiltinTool::code_interpreter());
    let req = build_request(&[Message::user("Hi")], &config, false, None).unwrap();
    assert_eq!(req["tools"][0]["type"], "code_interpreter");
    assert_eq!(req["tools"][0]["container"]["type"], "auto");
}

#[test]
fn test_builtin_tool_code_interpreter_with_files() {
    let config = ProviderConfig::new("gpt-4.1")
        .with_built_in_tool(BuiltinTool::code_interpreter_with_files(["file-123"]));
    let req = build_request(&[Message::user("Hi")], &config, false, None).unwrap();
    assert_eq!(req["tools"][0]["container"]["file_ids"][0], "file-123");
}

#[test]
fn test_builtin_tool_image_generation() {
    let config = ProviderConfig::new("gpt-4.1").with_built_in_tool(BuiltinTool::image_generation());
    let req = build_request(&[Message::user("Hi")], &config, false, None).unwrap();
    assert_eq!(req["tools"][0]["type"], "image_generation");
}

#[test]
fn test_builtin_tool_computer_use() {
    let config = ProviderConfig::new("computer-use-preview")
        .with_built_in_tool(BuiltinTool::computer_use(1024, 768, "browser"));
    let req = build_request(&[Message::user("Hi")], &config, false, None).unwrap();
    assert_eq!(req["tools"][0]["type"], "computer_use_preview");
    assert_eq!(req["tools"][0]["display_width"], 1024);
    assert_eq!(req["tools"][0]["display_height"], 768);
    assert_eq!(req["tools"][0]["environment"], "browser");
}

#[test]
fn test_builtin_tool_mcp() {
    let config = ProviderConfig::new("gpt-4.1").with_built_in_tool(BuiltinTool::mcp(
        McpToolConfig::new("my-server", "https://mcp.example.com/sse")
            .with_require_approval("never")
            .with_allowed_tools(vec!["search".to_string()]),
    ));
    let req = build_request(&[Message::user("Hi")], &config, false, None).unwrap();
    assert_eq!(req["tools"][0]["type"], "mcp");
    assert_eq!(req["tools"][0]["server_label"], "my-server");
    assert_eq!(req["tools"][0]["server_url"], "https://mcp.example.com/sse");
    assert_eq!(req["tools"][0]["require_approval"], "never");
    assert_eq!(req["tools"][0]["allowed_tools"][0], "search");
}

#[test]
fn test_builtin_tool_local_shell() {
    let config =
        ProviderConfig::new("codex-mini-latest").with_built_in_tool(BuiltinTool::local_shell());
    let req = build_request(&[Message::user("Hi")], &config, false, None).unwrap();
    assert_eq!(req["tools"][0]["type"], "local_shell");
}

#[test]
fn test_builtin_tool_apply_patch() {
    let config = ProviderConfig::new("gpt-5.1").with_built_in_tool(BuiltinTool::apply_patch());
    let req = build_request(&[Message::user("Hi")], &config, false, None).unwrap();
    assert_eq!(req["tools"][0]["type"], "apply_patch");
}

#[test]
fn test_builtin_tools_combined_with_functions() {
    // Function tools and built-in tools should appear together in tools array
    let config = ProviderConfig::new("gpt-4.1")
        .with_tools(vec![crate::types::Tool::new(
            "search",
            "Search",
            json!({"type":"object"}),
        )])
        .with_built_in_tool(BuiltinTool::file_search());
    let req = build_request(&[Message::user("Hi")], &config, false, None).unwrap();
    assert_eq!(req["tools"].as_array().unwrap().len(), 2);
    assert_eq!(req["tools"][0]["type"], "function");
    assert_eq!(req["tools"][1]["type"], "file_search");
}

#[test]
fn test_web_search_with_user_location() {
    let loc = WebSearchUserLocation::new()
        .with_country("GB")
        .with_city("London")
        .with_timezone("Europe/London");
    let config = ProviderConfig::new("gpt-4.1")
        .with_web_search(WebSearchConfig::new().with_user_location(loc));
    let req = build_request(&[Message::user("Hi")], &config, false, None).unwrap();
    let ws = &req["tools"][0];
    assert_eq!(ws["type"], "web_search_preview");
    assert_eq!(ws["user_location"]["country"], "GB");
    assert_eq!(ws["user_location"]["city"], "London");
    assert_eq!(ws["user_location"]["timezone"], "Europe/London");
}

#[test]
fn test_web_search_context_size() {
    let config = ProviderConfig::new("gpt-4.1")
        .with_web_search(WebSearchConfig::new().with_search_context_size("high"));
    let req = build_request(&[Message::user("Hi")], &config, false, None).unwrap();
    assert_eq!(req["tools"][0]["search_context_size"], "high");
}

#[test]
fn test_build_request_basic() {
    let config = ProviderConfig::new("gpt-4.1")
        .with_system("Be helpful")
        .with_max_tokens(512);
    let messages = vec![Message::user("Hello")];
    let req = build_request(&messages, &config, false, None).unwrap();
    assert_eq!(req["model"], "gpt-4.1");
    assert_eq!(req["instructions"], "Be helpful");
    assert_eq!(req["max_output_tokens"], 512);
}

#[test]
fn test_parse_response() {
    let json = json!({
        "id": "resp_123",
        "status": "completed",
        "model": "gpt-4.1",
        "output": [{
            "type": "message",
            "id": "msg_001",
            "status": "completed",
            "role": "assistant",
            "content": [{"type": "output_text", "text": "Hello!", "annotations": []}]
        }],
        "usage": {"input_tokens": 10, "output_tokens": 5, "total_tokens": 15}
    });
    let resp = parse_response(&json).unwrap();
    assert!(matches!(&resp.content[0], ContentBlock::Text(t) if t == "Hello!"));
}

#[test]
fn test_parse_tool_call_response() {
    let json = json!({
        "id": "resp_456",
        "status": "completed",
        "model": "gpt-4.1",
        "output": [{
            "type": "function_call",
            "id": "call_001",
            "call_id": "call_001",
            "name": "get_weather",
            "arguments": "{\"location\":\"NYC\"}",
            "status": "completed"
        }],
        "usage": {"input_tokens": 20, "output_tokens": 10, "total_tokens": 30}
    });
    let resp = parse_response(&json).unwrap();
    assert!(matches!(&resp.content[0], ContentBlock::ToolUse(tu) if tu.name == "get_weather"));
    assert!(matches!(resp.stop_reason, StopReason::ToolUse));
}

#[tokio::test]
async fn test_integration_complete() {
    let api_key = match std::env::var("OPENAI_API_KEY") {
        Ok(k) => k,
        Err(_) => {
            println!("Skipping: OPENAI_API_KEY not set");
            return;
        }
    };
    let provider = OpenAIResponsesProvider::new(api_key);
    let config = ProviderConfig::new("gpt-4o-mini").with_max_tokens(64);
    let messages = vec![Message::user("Say 'hello' in one word.")];
    let resp = provider.complete(messages, config).await.unwrap();
    assert!(!resp.content.is_empty());
}

#[tokio::test]
async fn test_integration_stream() {
    let api_key = match std::env::var("OPENAI_API_KEY") {
        Ok(k) => k,
        Err(_) => {
            println!("Skipping: OPENAI_API_KEY not set");
            return;
        }
    };
    use crate::provider::collect_stream;
    let provider = OpenAIResponsesProvider::new(api_key);
    let config = ProviderConfig::new("gpt-4o-mini").with_max_tokens(64);
    let messages = vec![Message::user("Say 'hi'.")];
    let stream = provider.stream(messages, config);
    let resp = collect_stream(stream).await.unwrap();
    assert!(!resp.content.is_empty());
}
