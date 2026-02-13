import type { ContentBlock } from "@/api/otel/types";
import { TextContent } from "./text-content";
import { JsonContent } from "./json-content";
import { ToolUseContent } from "./tool-use-content";
import { ToolResultContent } from "./tool-result-content";
import { ThinkingContent } from "./thinking-content";
import { ToolDefinitionsContent } from "./tool-definitions-content";
import { MediaContent } from "./media-content";
import { ContextContent } from "./context-content";
import { RefusalContent } from "./refusal-content";
import { RedactedThinkingContent } from "./redacted-thinking-content";
import { UnknownContent } from "./unknown-content";

export interface ContentRendererProps {
  block: ContentBlock;
  markdownEnabled?: boolean;
  /** Project ID for resolving file references */
  projectId?: string;
}

/**
 * Renders any ContentBlock type.
 * Used for inline content within messages (image, audio, context, etc.)
 */
export function ContentRenderer({ block, markdownEnabled, projectId }: ContentRendererProps) {
  switch (block.type) {
    case "text":
      return <TextContent text={block.text} markdownEnabled={markdownEnabled} />;

    case "image":
      return (
        <MediaContent
          type="image"
          mediaType={block.media_type}
          source={block.source}
          data={block.data}
          detail={block.detail}
          projectId={projectId}
        />
      );

    case "audio":
      return (
        <MediaContent
          type="audio"
          mediaType={block.media_type}
          source={block.source}
          data={block.data}
          projectId={projectId}
        />
      );

    case "video":
      return (
        <MediaContent
          type="video"
          mediaType={block.media_type}
          source={block.source}
          data={block.data}
          projectId={projectId}
        />
      );

    case "document":
      return (
        <MediaContent
          type="document"
          mediaType={block.media_type}
          name={block.name}
          source={block.source}
          data={block.data}
          projectId={projectId}
        />
      );

    case "file":
      return (
        <MediaContent
          type="file"
          mediaType={block.media_type}
          name={block.name}
          source={block.source}
          data={block.data}
          projectId={projectId}
        />
      );

    case "tool_use":
      return (
        <ToolUseContent id={block.id} name={block.name} input={block.input} showInlineHeader />
      );

    case "tool_result":
      return (
        <ToolResultContent
          content={block.content}
          isError={block.is_error}
          toolCallId={block.tool_use_id}
          showInlineHeader
          projectId={projectId}
        />
      );

    case "tool_definitions":
      return <ToolDefinitionsContent tools={block.tools} toolChoice={block.tool_choice} />;

    case "context":
      return <ContextContent data={block.data} contextType={block.context_type} />;

    case "refusal":
      return <RefusalContent message={block.message} />;

    case "json":
      return <JsonContent data={block.data} />;

    case "thinking":
      return <ThinkingContent text={block.text} markdownEnabled={markdownEnabled} />;

    case "redacted_thinking":
      return <RedactedThinkingContent />;

    case "unknown":
    default:
      return <UnknownContent data={block} />;
  }
}

// Export all content components
export { TextContent } from "./text-content";
export { JsonContent } from "./json-content";
export { ToolUseContent } from "./tool-use-content";
export { ToolResultContent } from "./tool-result-content";
export { ThinkingContent } from "./thinking-content";
export { ToolDefinitionsContent } from "./tool-definitions-content";
export { MediaContent } from "./media-content";
export { ContextContent } from "./context-content";
export { RefusalContent } from "./refusal-content";
export { RedactedThinkingContent } from "./redacted-thinking-content";
export { UnknownContent } from "./unknown-content";
