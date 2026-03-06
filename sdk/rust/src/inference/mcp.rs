//! Model Context Protocol (MCP) integration.
//!
//! Provides helpers to load tool definitions from an MCP server and
//! create a tool handler that dispatches calls via the MCP client.

use std::{pin::Pin, sync::Arc};

use futures::Future;
use rmcp::{Peer, RoleClient, model::CallToolRequestParams, model::Tool as McpTool};

use crate::{
    error::ProviderError,
    types::{ContentBlock, ImageContent, MediaSource, Base64Data, Tool, ToolUseBlock},
};

/// Convert all tools from an MCP server into sideseat `Tool` definitions.
pub async fn load_mcp_tools(client: &Peer<RoleClient>) -> Result<Vec<Tool>, ProviderError> {
    let mcp_tools = client
        .list_all_tools()
        .await
        .map_err(|e| ProviderError::Network(e.to_string()))?;
    Ok(mcp_tools.into_iter().map(mcp_tool_to_tool).collect())
}

fn mcp_tool_to_tool(t: McpTool) -> Tool {
    Tool {
        name: t.name.to_string(),
        description: t.description.as_deref().unwrap_or("").to_string(),
        input_schema: serde_json::to_value(&*t.input_schema)
            .unwrap_or(serde_json::json!({"type": "object", "properties": {}})),
        strict: false,
        input_examples: vec![],
    }
}

/// Create a tool handler that dispatches all calls through an MCP client.
///
/// Returns rich [`ContentBlock`] results, preserving text, images, and other
/// MCP content types. Compatible with [`run_agent_loop_with_hooks`].
///
/// For the simpler [`run_agent_loop`] (text-only), use [`mcp_text_tool_handler`].
///
/// [`run_agent_loop_with_hooks`]: crate::provider::run_agent_loop_with_hooks
/// [`run_agent_loop`]: crate::provider::run_agent_loop
#[allow(clippy::type_complexity)]
pub fn mcp_tool_handler(
    client: Arc<Peer<RoleClient>>,
) -> impl Fn(Vec<ToolUseBlock>) -> Pin<Box<dyn Future<Output = Vec<(String, Vec<ContentBlock>)>> + Send>> {
    move |tool_uses| {
        let client = client.clone();
        Box::pin(async move {
            let mut results = Vec::new();
            for tu in tool_uses {
                let args = tu.input.as_object().cloned();
                let result = client
                    .call_tool(CallToolRequestParams {
                        meta: None,
                        name: tu.name.clone().into(),
                        arguments: args,
                        task: None,
                    })
                    .await;
                let blocks = match result {
                    Ok(r) => r.content.iter().map(mcp_content_to_block).collect(),
                    Err(e) => vec![ContentBlock::text(format!("MCP tool error: {e}"))],
                };
                results.push((tu.id.clone(), blocks));
            }
            results
        })
    }
}

/// Create a text-only tool handler for use with [`run_agent_loop`].
///
/// Non-text MCP content (images, etc.) is discarded. Use [`mcp_tool_handler`]
/// with [`run_agent_loop_with_hooks`] to preserve all content types.
///
/// [`run_agent_loop`]: crate::provider::run_agent_loop
/// [`run_agent_loop_with_hooks`]: crate::provider::run_agent_loop_with_hooks
#[allow(clippy::type_complexity)]
pub fn mcp_text_tool_handler(
    client: Arc<Peer<RoleClient>>,
) -> impl Fn(Vec<ToolUseBlock>) -> Pin<Box<dyn Future<Output = Vec<(String, String)>> + Send>> {
    move |tool_uses| {
        let client = client.clone();
        Box::pin(async move {
            let mut results = Vec::new();
            for tu in tool_uses {
                let args = tu.input.as_object().cloned();
                let result = client
                    .call_tool(CallToolRequestParams {
                        meta: None,
                        name: tu.name.clone().into(),
                        arguments: args,
                        task: None,
                    })
                    .await;
                let text = match result {
                    Ok(r) => r
                        .content
                        .iter()
                        .filter_map(|c| c.as_text().map(|t| t.text.as_str()))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    Err(e) => format!("MCP tool error: {e}"),
                };
                results.push((tu.id.clone(), text));
            }
            results
        })
    }
}

fn mcp_content_to_block(content: &rmcp::model::Content) -> ContentBlock {
    if let Some(text_content) = content.as_text() {
        return ContentBlock::text(text_content.text.clone());
    }
    if let Some(image_content) = content.as_image() {
        // rmcp RawImageContent: data is raw base64, mime_type is the media type string
        let source = MediaSource::Base64(Base64Data {
            media_type: image_content.mime_type.clone(),
            data: image_content.data.clone(),
        });
        return ContentBlock::Image(ImageContent { source, format: None, detail: None });
    }
    // Unknown content type — fall back to text representation
    ContentBlock::text(format!("{content:?}"))
}
