import { memo, useCallback, useEffect, useRef, useState } from "react";
import { Check, Copy, GitBranch, Layers, Users } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  TimelineRow,
  getBlockPreview,
  getBlockCopyText,
  renderBlockContent,
} from "@/components/thread";
import { JsonContent } from "@/components/thread/content";
import type { Block, SpanSummary } from "@/api/otel/types";

// Breadcrumb path showing session → trace → span navigation
// Only renders links for IDs that exist
const MessageBreadcrumb = memo(function MessageBreadcrumb({
  sessionId,
  traceId,
  spanId,
  onOpenSession,
  onOpenTrace,
  onOpenSpan,
}: {
  sessionId?: string;
  traceId?: string;
  spanId?: string;
  onOpenSession: (sessionId: string) => void;
  onOpenTrace: (traceId: string) => void;
  onOpenSpan: (traceId: string, spanId: string) => void;
}) {
  // Don't render breadcrumb if no IDs available
  if (!sessionId && !traceId && !spanId) return null;

  return (
    <div className="flex items-center gap-1 font-mono text-3xs text-muted-foreground/70 mb-1.5 select-none">
      {sessionId && (
        <>
          <button
            type="button"
            onClick={() => onOpenSession(sessionId)}
            className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded hover:bg-muted hover:text-foreground transition-colors"
            title={`Session ${sessionId}`}
          >
            <Users className="h-3 w-3" />
            <span className="tracking-tight">session:{sessionId.slice(0, 8)}</span>
          </button>
          {(traceId || spanId) && <span className="text-muted-foreground/40">/</span>}
        </>
      )}
      {traceId && (
        <>
          <button
            type="button"
            onClick={() => onOpenTrace(traceId)}
            className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded hover:bg-muted hover:text-foreground transition-colors"
            title={`Trace ${traceId}`}
          >
            <GitBranch className="h-3 w-3" />
            <span className="tracking-tight">trace:{traceId.slice(0, 8)}</span>
          </button>
          {spanId && <span className="text-muted-foreground/40">/</span>}
        </>
      )}
      {spanId && traceId && (
        <button
          type="button"
          onClick={() => onOpenSpan(traceId, spanId)}
          className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded hover:bg-muted hover:text-foreground transition-colors"
          title={`Span ${spanId}`}
        >
          <Layers className="h-3 w-3" />
          <span className="tracking-tight">span:{spanId.slice(0, 8)}</span>
        </button>
      )}
    </div>
  );
});

// Block item component for virtualized list - renders single block directly
export const FeedBlockItem = memo(function FeedBlockItem({
  block,
  startTime,
  markdownEnabled,
  projectId,
  onOpenSession,
  onOpenTrace,
  onOpenSpan,
}: {
  block: Block;
  startTime?: string;
  markdownEnabled: boolean;
  projectId: string;
  onOpenSession: (sessionId: string) => void;
  onOpenTrace: (traceId: string) => void;
  onOpenSpan: (traceId: string, spanId: string) => void;
}) {
  return (
    <div>
      <MessageBreadcrumb
        sessionId={block.session_id}
        traceId={block.trace_id}
        spanId={block.span_id}
        onOpenSession={onOpenSession}
        onOpenTrace={onOpenTrace}
        onOpenSpan={onOpenSpan}
      />
      <TimelineRow
        block={block}
        startTime={startTime}
        preview={getBlockPreview(block)}
        copyText={getBlockCopyText(block)}
      >
        {renderBlockContent(block, markdownEnabled, projectId)}
      </TimelineRow>
    </div>
  );
});

// Raw span item component for virtualized list
export const FeedSpanItem = memo(function FeedSpanItem({ span }: { span: SpanSummary }) {
  const [copied, setCopied] = useState(false);
  const timeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Cleanup timeout on unmount
  useEffect(() => {
    return () => {
      if (timeoutRef.current) {
        clearTimeout(timeoutRef.current);
      }
    };
  }, []);

  const handleCopy = useCallback(async () => {
    try {
      const text = JSON.stringify(span.raw_span ?? span, null, 2);
      await navigator.clipboard.writeText(text);
      setCopied(true);
      if (timeoutRef.current) {
        clearTimeout(timeoutRef.current);
      }
      timeoutRef.current = setTimeout(() => setCopied(false), 2000);
    } catch {
      // Clipboard API failed silently
    }
  }, [span]);

  const displayData = span.raw_span ?? span;

  return (
    <div className="rounded-lg border bg-card">
      <div className="flex items-center justify-between gap-1 border-b px-2 py-1.5 sm:gap-2 sm:px-3 sm:py-2">
        <div className="min-w-0 flex-1">
          <div className="truncate text-xs font-medium sm:text-sm">{span.span_name}</div>
          <code className="block truncate text-xs text-muted-foreground">{span.span_id}</code>
        </div>
        <Button
          variant="ghost"
          size="icon-2xs"
          onClick={handleCopy}
          className="shrink-0 sm:h-7 sm:w-7"
          aria-label="Copy span"
        >
          {copied ? (
            <Check className="h-3 w-3 text-success sm:h-3.5 sm:w-3.5" />
          ) : (
            <Copy className="h-3 w-3 sm:h-3.5 sm:w-3.5" />
          )}
        </Button>
      </div>
      <div className="overflow-x-auto p-2 sm:p-3">
        <JsonContent data={displayData} disableCollapse />
      </div>
    </div>
  );
});
