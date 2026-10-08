import type { Block } from "@/api/otel/types";
import {
  ContentRenderer,
  TextContent,
  ToolUseContent,
  ToolResultContent,
  ThinkingContent,
  ToolDefinitionsContent,
  JsonContent,
} from "./content";
import { omittedReasoningLabel } from "./content/reasoning-labels";

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
 * The longest text parsed to decide whether a block is a structured answer.
 *
 * A schema answer is a handful of fields. Beyond this a text block is prose, a transcript or a dump,
 * and parsing it on every render would cost more than the JSON tree is worth.
 */
const MAX_STRUCTURED_TEXT = 64_000;

/** Parsed answers, keyed by the content block they came from, so a re-render re-parses nothing. */
const parsed = new WeakMap<object, unknown>();
/** What the cache holds for a text block that is not a structured answer. */
const PROSE = Symbol("prose");

/**
 * The structured answer a block carries, or `undefined` when it carries prose.
 *
 * A model asked for a schema answers with one object, which reaches SideSeat two ways: as a `json`
 * content block where the instrumentation reported the shape, and as the text of a message where it
 * reported only a string. Both are the same answer to a reader, and a JSON tree is how it is read - the
 * string form rendered as Markdown runs the whole object into one paragraph.
 */
export function structuredData(block: Block): unknown | undefined {
  const { content } = block;
  if (content.type === "json") return content.data;
  if (content.type !== "text") return undefined;
  const cached = parsed.get(content);
  if (cached !== undefined) return cached === PROSE ? undefined : cached;
  const answer = parseStructured(content.text);
  parsed.set(content, answer === undefined ? PROSE : answer);
  return answer;
}

function parseStructured(text: string): unknown | undefined {
  const trimmed = text.trim();
  // Only an object or an array: a bare number or a quoted word is prose that happens to parse.
  const first = trimmed[0];
  if ((first !== "{" && first !== "[") || trimmed.length > MAX_STRUCTURED_TEXT) return undefined;
  try {
    const value: unknown = JSON.parse(trimmed);
    return typeof value === "object" && value !== null ? value : undefined;
  } catch {
    return undefined;
  }
}

/** A one-line summary of a structured answer: its keys, or its length. */
export function summariseStructured(data: unknown): string {
  if (Array.isArray(data)) {
    return `[${data.length} item${data.length === 1 ? "" : "s"}]`;
  }
  if (typeof data === "object" && data !== null) {
    const keys = Object.keys(data);
    const shown = keys.slice(0, 6).join(", ");
    return truncate(`{${shown}${keys.length > 6 ? ", …" : ""}}`, 80);
  }
  return truncate(JSON.stringify(data) ?? String(data), 80);
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

  const structured = structuredData(block);
  if (structured !== undefined) return summariseStructured(structured);

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
    const omitted = omittedReasoningLabel(content.text, content.signed);
    if (omitted) return omitted;
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

  // The answer itself, without SideSeat's `{"type":"json"}` envelope around it.
  const structured = structuredData(block);
  if (structured !== undefined) return JSON.stringify(structured, null, 2);

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

  // A structured answer is read as a tree whichever carrier reported it.
  const structured = structuredData(block);
  if (structured !== undefined) return <JsonContent data={structured} />;

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
    return (
      <ThinkingContent
        text={content.text}
        signed={content.signed}
        markdownEnabled={markdownEnabled}
      />
    );
  }

  if (entry_type === "tool_definitions" && content.type === "tool_definitions") {
    return <ToolDefinitionsContent tools={content.tools} toolChoice={content.tool_choice} />;
  }

  // Fallback: use generic content renderer
  return (
    <ContentRenderer block={content} markdownEnabled={markdownEnabled} projectId={projectId} />
  );
}
