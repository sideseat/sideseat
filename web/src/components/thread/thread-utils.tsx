import type { Block } from "@/api/otel/types";
import {
  ContentRenderer,
  TextContent,
  ToolUseContent,
  ToolResultContent,
  ThinkingContent,
  ToolDefinitionsContent,
} from "./content";

/**
 * Generate a unique key for a block.
 *
 * Span ids are client-provided and only unique within their trace, and a session thread spans many
 * traces, so the trace id is part of the key.
 */
export function getBlockKey(block: Block): string {
  return `${block.trace_id}-${block.span_id}-${block.message_index}-${block.entry_index}`;
}

export interface IncompleteReason {
  label: string;
  description: string;
}

/**
 * Why a message stopped before the model finished it, when it did.
 *
 * A response cut off at the token limit or by a content filter reads exactly like a complete one, so
 * these finish reasons are surfaced on the message instead of only in the span's attributes.
 */
export function getIncompleteReason(finishReason: string | undefined): IncompleteReason | null {
  switch (finishReason) {
    case "length":
      return {
        label: "Truncated",
        description: "The model stopped at its output token limit, so this message is incomplete.",
      };
    case "content_filter":
      return {
        label: "Filtered",
        description: "A content filter stopped this response before the model finished it.",
      };
    case "error":
      return {
        label: "Failed",
        description: "Generation failed before the model finished this message.",
      };
    default:
      return null;
  }
}

function truncate(text: string, max: number): string {
  return text.length > max ? text.slice(0, max) + "..." : text;
}

/**
 * The tool a result answers, by the most direct identification available.
 *
 * The result's own name comes first: for Gemini and ADK it is the only identification the source
 * gave, and the block-level fields are derived rather than reported.
 */
export function getToolResultName(block: Block): string | undefined {
  const own = block.content.type === "tool_result" ? block.content.name : undefined;
  return own || block.tool_name || block.name;
}

/**
 * Get a preview string for a block.
 *
 * Tool calls and results lead with the tool's name: with several tools in one turn, arguments or
 * output alone do not say which call a collapsed row is.
 */
export function getBlockPreview(block: Block): string {
  const { entry_type, content } = block;

  if (entry_type === "tool_use" && content.type === "tool_use") {
    const inputStr = JSON.stringify(content.input) ?? "";
    return `${content.name}(${truncate(inputStr, 60)})`;
  }

  if (entry_type === "tool_result" && content.type === "tool_result") {
    const resultStr =
      typeof content.content === "string"
        ? content.content
        : (JSON.stringify(content.content) ?? "");
    const firstLine = truncate(resultStr.split("\n")[0], 60);
    const toolName = getToolResultName(block);
    return toolName ? `${toolName} → ${firstLine}` : firstLine;
  }

  if (entry_type === "thinking" && content.type === "thinking") {
    return `"${truncate(content.text, 60)}" (${content.text.length} chars)`;
  }

  if (entry_type === "tool_definitions" && content.type === "tool_definitions") {
    return `${content.tools.length} tool${content.tools.length !== 1 ? "s" : ""} defined`;
  }

  if (entry_type === "text" && content.type === "text") {
    return truncate(content.text.split("\n")[0], 80);
  }

  return `[${entry_type}]`;
}

/**
 * Get copyable text for a block.
 */
export function getBlockCopyText(block: Block): string {
  const { entry_type, content } = block;

  if (entry_type === "tool_use" && content.type === "tool_use") {
    return JSON.stringify(content.input, null, 2);
  }

  if (entry_type === "tool_result" && content.type === "tool_result") {
    return typeof content.content === "string"
      ? content.content
      : JSON.stringify(content.content, null, 2);
  }

  if (entry_type === "thinking" && content.type === "thinking") {
    return content.text;
  }

  if (entry_type === "tool_definitions" && content.type === "tool_definitions") {
    return JSON.stringify(content.tools, null, 2);
  }

  if (entry_type === "text" && content.type === "text") {
    return content.text;
  }

  return JSON.stringify(content, null, 2);
}

/**
 * Render content for a block.
 */
export function renderBlockContent(
  block: Block,
  markdownEnabled: boolean,
  projectId?: string,
): React.ReactNode {
  const { entry_type, content } = block;

  if (entry_type === "text" && content.type === "text") {
    return <TextContent text={content.text} markdownEnabled={markdownEnabled} />;
  }

  if (entry_type === "tool_use" && content.type === "tool_use") {
    return <ToolUseContent id={content.id} name={content.name} input={content.input} />;
  }

  if (entry_type === "tool_result" && content.type === "tool_result") {
    return (
      <ToolResultContent
        content={content.content}
        isError={content.is_error || block.is_error}
        toolCallId={content.tool_use_id || block.tool_use_id}
        toolCallIdInferred={block.tool_use_id_correlated}
        toolName={getToolResultName(block)}
        projectId={projectId}
      />
    );
  }

  if (entry_type === "thinking" && content.type === "thinking") {
    return <ThinkingContent text={content.text} markdownEnabled={markdownEnabled} />;
  }

  if (entry_type === "tool_definitions" && content.type === "tool_definitions") {
    return <ToolDefinitionsContent tools={content.tools} toolChoice={content.tool_choice} />;
  }

  // Fallback: use generic content renderer
  return (
    <ContentRenderer block={content} markdownEnabled={markdownEnabled} projectId={projectId} />
  );
}
