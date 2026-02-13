import { useMemo } from "react";
import { CornerDownRight, AlertCircle } from "lucide-react";
import { JsonContent } from "./json-content";
import { TextContent } from "./text-content";
import { MediaContent } from "./media-content";
import { ContextContent } from "./context-content";
import { RefusalContent } from "./refusal-content";
import type { ContentBlock } from "@/api/otel/types";
import { type EmbeddedMedia, findEmbeddedMedia, inferSource } from "@/lib/media";

interface ToolResultContentProps {
  content: unknown;
  isError?: boolean;
  errorMessage?: string;
  toolName?: string;
  toolCallId?: string;
  /** Show inline header (for ContentRenderer use) */
  showInlineHeader?: boolean;
  /** Project ID for resolving file references */
  projectId?: string;
}

/** Content block types that can be rendered inline */
const RENDERABLE_BLOCK_TYPES = new Set([
  "text",
  "image",
  "audio",
  "video",
  "document",
  "file",
  "json",
  "context",
  "refusal",
]);

/**
 * Check if a value is a renderable content block.
 */
function isContentBlock(value: unknown): value is ContentBlock {
  return (
    typeof value === "object" &&
    value !== null &&
    "type" in value &&
    typeof (value as { type: unknown }).type === "string" &&
    RENDERABLE_BLOCK_TYPES.has((value as { type: string }).type)
  );
}

/**
 * Check if content is an array of renderable content blocks.
 */
function isContentBlockArray(content: unknown): content is ContentBlock[] {
  return Array.isArray(content) && content.length > 0 && content.every(isContentBlock);
}

/**
 * Extract the actual content from tool result wrapper structures.
 * Strips metadata like type, tool_use_id, keeping just the content.
 */
function extractContent(content: unknown): unknown {
  // String content - return as-is
  if (typeof content === "string") {
    return content;
  }

  // Array content
  if (Array.isArray(content)) {
    // Check for tool_result wrapper: [{type: "tool_result", content: ...}]
    if (
      content.length === 1 &&
      content[0]?.type === "tool_result" &&
      content[0]?.content !== undefined
    ) {
      return extractContent(content[0].content);
    }

    // Multiple tool_result wrappers - extract content from each
    if (content.every((item) => item?.type === "tool_result" && item?.content !== undefined)) {
      return content.map((item) => extractContent(item.content));
    }
  }

  // Default: return as-is
  return content;
}

export function ToolResultContent({
  content,
  isError,
  errorMessage,
  toolName,
  toolCallId,
  showInlineHeader = false,
  projectId,
}: ToolResultContentProps) {
  const Icon = isError ? AlertCircle : CornerDownRight;

  const extracted = useMemo(() => extractContent(content), [content]);

  return (
    <div className="space-y-2">
      {showInlineHeader && (
        <div className="flex items-center gap-2">
          <Icon
            className={
              isError
                ? "h-4 w-4 text-red-600 dark:text-red-400"
                : "h-4 w-4 text-teal-600 dark:text-teal-400"
            }
          />
          <span
            className={
              isError
                ? "text-sm font-medium text-red-600 dark:text-red-400"
                : "text-sm font-medium text-teal-600 dark:text-teal-400"
            }
          >
            {isError ? "Error" : "Result"}
          </span>
          {toolName && (
            <span className="font-mono text-sm text-teal-700 dark:text-teal-300">{toolName}</span>
          )}
        </div>
      )}
      {isError && errorMessage && (
        <div className="rounded-md bg-red-100 px-3 py-2 text-sm font-medium text-red-700 dark:bg-red-900/50 dark:text-red-300">
          {errorMessage}
        </div>
      )}
      {toolCallId && (
        <div className="text-xs text-muted-foreground font-mono">tool_call_id: {toolCallId}</div>
      )}
      {renderExtractedContent(extracted, projectId)}
    </div>
  );
}

/**
 * Render embedded media with any extra fields as JSON.
 */
function renderEmbeddedMedia(media: EmbeddedMedia, projectId?: string): React.ReactNode {
  const { data, mediaType, name, type, rest } = media;
  const source = inferSource(data);
  const hasOtherFields = Object.keys(rest).length > 0;

  return (
    <div className="space-y-2">
      <MediaContent
        type={type}
        mediaType={mediaType}
        source={source}
        data={data}
        name={name}
        projectId={projectId}
      />
      {hasOtherFields && <JsonContent data={rest} collapsed />}
    </div>
  );
}

/**
 * Try to find embedded media in a value.
 * Returns null if not an object or no media found.
 */
function tryFindMedia(value: unknown): EmbeddedMedia | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  return findEmbeddedMedia(value as Record<string, unknown>);
}

/**
 * Render extracted content based on its type.
 * Handles: string, ContentBlock[], embedded media objects, or unknown (JSON fallback).
 */
function renderExtractedContent(extracted: unknown, projectId?: string): React.ReactNode {
  // String content
  if (typeof extracted === "string") {
    return <TextContent text={extracted} markdownEnabled={false} />;
  }

  // Array of content blocks - render each block
  if (isContentBlockArray(extracted)) {
    return (
      <div className="space-y-2">
        {extracted.map((block, index) => (
          <ContentBlockRenderer key={index} block={block} projectId={projectId} />
        ))}
      </div>
    );
  }

  // Object with embedded media (e.g., from image_reader tool)
  const media = tryFindMedia(extracted);
  if (media) {
    return renderEmbeddedMedia(media, projectId);
  }

  // Array - check each item for embedded media
  if (Array.isArray(extracted) && extracted.length > 0) {
    // Pre-compute media for all items (single pass, reuse results)
    const mediaResults = extracted.map(tryFindMedia);
    const hasAnyMedia = mediaResults.some(Boolean);

    if (hasAnyMedia) {
      return (
        <div className="space-y-2">
          {extracted.map((item, index) => {
            const media = mediaResults[index];
            return media ? (
              <div key={index}>{renderEmbeddedMedia(media, projectId)}</div>
            ) : (
              <JsonContent key={index} data={item} />
            );
          })}
        </div>
      );
    }
  }

  // Fallback: render as JSON
  return <JsonContent data={extracted} />;
}

/**
 * Render a single content block within tool result.
 * Simplified version that handles the common cases.
 */
function ContentBlockRenderer({
  block,
  projectId,
}: {
  block: ContentBlock;
  projectId?: string;
}): React.ReactNode {
  switch (block.type) {
    case "text":
      return <TextContent text={block.text} markdownEnabled={false} />;

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

    case "json":
      return <JsonContent data={block.data} />;

    case "context":
      return <ContextContent data={block.data} contextType={block.context_type} />;

    case "refusal":
      return <RefusalContent message={block.message} />;

    default:
      return <JsonContent data={block} />;
  }
}
