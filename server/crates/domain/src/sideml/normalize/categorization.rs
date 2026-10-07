use super::*;

// ============================================================================
// MESSAGE CATEGORIZATION
// ============================================================================

/// Determine message category based on source and content.
///
/// Categorization rules (in priority order):
/// 1. Event with LLM output name (gen_ai.choice, gen_ai.content.completion)
///    → Use event name (semantic output categorization)
/// 2. Event with special role (tool_call, tool, tools, data, context)
///    → Use role-based categorization
/// 3. Other events
///    → Use event name
/// 4. Attribute with role
///    → Use role-based categorization
/// 5. Attribute without role
///    → Default to user message
pub(super) fn determine_category(source: &MessageSource, content: &JsonValue) -> MessageCategory {
    let role = content.get("role").and_then(|r| r.as_str());

    match source {
        MessageSource::Event { name, .. } => categorize_event_message(name, role, content),
        MessageSource::Attribute { .. } => categorize_attribute_message(role),
    }
}

/// Check if event represents LLM output (always uses event-based categorization).
pub(super) fn is_llm_output_event(event_name: &str) -> bool {
    matches!(event_name, "gen_ai.choice" | "gen_ai.content.completion")
}

/// Check if role should use role-based categorization instead of event-based.
///
/// This is different from SPECIAL_ROLES (which controls role derivation):
/// - SPECIAL_ROLES: Roles that MUST NOT be overridden during role derivation
/// - is_special_role: Roles that MUST use role-based categorization
///
/// The `tool` role is included here but not in SPECIAL_ROLES because:
/// - `tool` CAN be derived from event names (so not in SPECIAL_ROLES)
/// - `tool` MUST use role-based categorization (so included here)
///
/// Special roles for categorization:
/// - tool_call: tool invocation (input to tool) → GenAIToolInput
/// - tool: tool result (output from tool) → GenAIToolMessage
/// - tools: tool definitions → GenAIToolDefinitions
/// - data/context: conversation history → GenAIContext
/// - documents: retrieved documents → Retrieval
pub(super) fn is_special_role(role: &str) -> bool {
    matches!(
        role.to_lowercase().as_str(),
        "tool_call" | "tool" | "tools" | "data" | "context" | "documents"
    )
}

/// Categorize event-sourced message.
fn categorize_event_message(
    event_name: &str,
    role: Option<&str>,
    content: &JsonValue,
) -> MessageCategory {
    // LLM output events always use event-based categorization
    // This ensures tool messages in choice events get GenAIChoice (output)
    // rather than GenAIToolMessage (input)
    if is_llm_output_event(event_name) {
        return category_from_event_name(event_name, content);
    }

    // Special roles override event name, but "tool" needs content inspection
    // to distinguish between INPUT (toolUse) and OUTPUT (toolResult)
    if let Some(role_str) = role
        && is_special_role(role_str)
    {
        return category_from_role_with_content(role_str, content);
    }

    // Default: use event name
    category_from_event_name(event_name, content)
}

/// Categorize attribute-sourced message.
pub(super) fn categorize_attribute_message(role: Option<&str>) -> MessageCategory {
    role.map(category_from_role)
        .unwrap_or(MessageCategory::GenAIUserMessage)
}

/// Map event name to MessageCategory.
fn category_from_event_name(event_name: &str, raw_message: &JsonValue) -> MessageCategory {
    match event_name {
        "gen_ai.system.message" => MessageCategory::GenAISystemMessage,
        "gen_ai.user.message" => MessageCategory::GenAIUserMessage,
        "gen_ai.assistant.message" => MessageCategory::GenAIAssistantMessage,
        "gen_ai.tool.message" => categorize_tool_message(raw_message),
        "gen_ai.choice" | "gen_ai.content.completion" => MessageCategory::GenAIChoice,
        "gen_ai.content.prompt" => MessageCategory::GenAIUserMessage,
        "exception" => MessageCategory::Exception,
        "log" => MessageCategory::Log,
        n if n.contains("retrieval") || n.contains("search") => MessageCategory::Retrieval,
        n if n.contains("score") || n.contains("observation") => MessageCategory::Observation,
        _ => MessageCategory::Other,
    }
}

/// Categorize tool message as input (tool invocation) or output (tool result).
///
/// Which members say so is declared (`holds_tool_calls` on a message, `means_tool_call` and
/// `means_tool_result` on a block); SideML's own block types say it too.
pub(in crate::sideml) fn categorize_tool_message(raw_message: &JsonValue) -> MessageCategory {
    let members = &crate::rules::ruleset().message_members;
    // A message carrying its tool calls is the calling side.
    if raw_message
        .as_object()
        .is_some_and(|object| members.any_holds_tool_calls(object.keys()))
    {
        return MessageCategory::GenAIToolInput;
    }

    // The first block that is a call or a result decides.
    if let Some(content) = raw_message.get("content")
        && let Some(arr) = content.as_array()
    {
        for block in arr {
            let kind = block.get("type").and_then(|t| t.as_str());
            let object = block.as_object();
            if object.is_some_and(|object| members.any_means_tool_call(object.keys()))
                || kind == Some("tool_use")
            {
                return MessageCategory::GenAIToolInput;
            }
            if object.is_some_and(|object| members.any_means_tool_result(object.keys()))
                || kind == Some("tool_result")
            {
                return MessageCategory::GenAIToolMessage;
            }
        }
    }

    // Default: tool result/output (role="tool" with content)
    MessageCategory::GenAIToolMessage
}

/// Map a role to MessageCategory, with content inspection for ambiguous roles.
///
/// The "tool" role is ambiguous - it can mean:
/// - Tool INPUT (assistant calling a tool): contains toolUse/tool_calls
/// - Tool OUTPUT (tool result): contains toolResult or plain content
///
/// This function inspects content to distinguish between these cases.
fn category_from_role_with_content(role: &str, content: &JsonValue) -> MessageCategory {
    let role_lower = role.to_lowercase();
    match role_lower.as_str() {
        // Tool role needs content inspection to distinguish INPUT vs OUTPUT
        "tool" => categorize_tool_message(content),
        // Other roles delegate to simple role-based categorization
        _ => category_from_role(role),
    }
}

/// Map a role to the appropriate MessageCategory (without content inspection).
///
/// Use `category_from_role_with_content` when content is available and the role
/// might be "tool" (which needs content inspection for INPUT vs OUTPUT).
fn category_from_role(role: &str) -> MessageCategory {
    let role_lower = role.to_lowercase();
    match role_lower.as_str() {
        // Tool definitions message
        "tools" => MessageCategory::GenAIToolDefinitions,
        // Tool invocation (assistant calling a tool)
        "tool_call" => MessageCategory::GenAIToolInput,
        // Context/data roles (conversation history, chat context)
        "data" | "context" => MessageCategory::GenAIContext,
        // Retrieved documents (RAG results)
        "documents" => MessageCategory::Retrieval,
        // Standard roles (including "tool" which defaults to OUTPUT)
        _ => match ChatRole::from_str_normalized(role) {
            ChatRole::System => MessageCategory::GenAISystemMessage,
            ChatRole::Assistant => MessageCategory::GenAIAssistantMessage,
            ChatRole::Tool => MessageCategory::GenAIToolMessage,
            ChatRole::User => MessageCategory::GenAIUserMessage,
        },
    }
}

/// The event-name role table this file used to hold, kept to hold `event_roles` to account.
///
/// A golden can be regenerated and bless a regression; an oracle cannot. Compared by
/// `the_declared_event_roles_reproduce_the_table_they_replaced`.
#[cfg(test)]
pub(crate) fn role_from_event_name_with_context_legacy(
    event_name: &str,
    is_tool_span: bool,
) -> Option<ChatRole> {
    match event_name {
        "gen_ai.system.message" => Some(ChatRole::System),
        "gen_ai.user.message" | "gen_ai.content.prompt" => Some(ChatRole::User),
        "gen_ai.tool.message" => {
            if is_tool_span {
                Some(ChatRole::Assistant)
            } else {
                Some(ChatRole::Tool)
            }
        }
        "gen_ai.tool.result" => Some(ChatRole::Tool),
        "tool.output" => Some(ChatRole::Tool),
        "gen_ai.assistant.message" => Some(ChatRole::Assistant),
        "gen_ai.choice" | "gen_ai.content.completion" => {
            if is_tool_span {
                Some(ChatRole::Tool)
            } else {
                Some(ChatRole::Assistant)
            }
        }
        _ => None,
    }
}
