import { useState, useMemo, useRef, useLayoutEffect } from "react";
import { ChevronDown, ChevronRight, Loader2 } from "lucide-react";
import { ScrollArea } from "@/components/ui/scroll-area";
import { JsonContent } from "@/components/thread/content/json-content";
import { DataInspector } from "@/components/data-inspector";
import { useSpanMessages } from "@/api/otel/hooks/queries";
import { useTraceView } from "../../contexts/use-trace-view";
import { SpanDetailHeader } from "./span-detail-header";
import { formatTimestamp, type RawSpan } from "./utils";
import type { Block } from "@/api/otel/types";
import { formatCost } from "../../lib/span-config";

function transformBlocksToData(blocks: Block[]): Record<string, unknown> {
  const result: Record<string, unknown> = {};
  blocks.forEach((block, i) => {
    const key = `${i + 1}. ${block.role}${block.name ? ` (${block.name})` : ""}`;
    const entry: Record<string, unknown> = { type: block.entry_type, content: block.content };
    if (block.tool_use_id) entry.tool_use_id = block.tool_use_id;
    result[key] = entry;
  });
  return result;
}

interface SpanDetailPanelProps {
  projectId: string;
  traceId: string;
}

export function SpanDetailPanel({ projectId, traceId }: SpanDetailPanelProps) {
  const { selectedNode } = useTraceView();
  const [rawSpanExpanded, setRawSpanExpanded] = useState(false);
  const scrollAreaRef = useRef<HTMLDivElement>(null);
  const selectedSpanId = selectedNode?.id;

  // Scroll to top when selected span changes
  useLayoutEffect(() => {
    if (selectedSpanId && scrollAreaRef.current) {
      const viewport = scrollAreaRef.current.querySelector("[data-slot='scroll-area-viewport']");
      if (viewport) {
        viewport.scrollTop = 0;
      }
    }
  }, [selectedSpanId]);

  const { data: messagesData, isLoading: messagesLoading } = useSpanMessages(
    projectId,
    selectedNode?.span.trace_id ?? traceId,
    selectedNode?.id ?? "",
    undefined,
    { enabled: !!selectedNode },
  );

  // Transform messages for DataInspector (must be before early return)
  const messagesForInspector = useMemo(
    () => (messagesData?.messages ? transformBlocksToData(messagesData.messages) : {}),
    [messagesData?.messages],
  );

  if (!selectedNode) {
    return (
      <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
        Select a span to view details
      </div>
    );
  }

  const { span } = selectedNode;
  const rawSpan = span.raw_span as RawSpan | undefined;
  const attributes = rawSpan?.attributes;
  const resourceAttributes = rawSpan?.resource?.attributes as Record<string, unknown> | undefined;
  const links = rawSpan?.links;

  // Get finish reason from API (single value if only one, otherwise show array)
  const finishReason =
    span.finish_reasons?.length === 1
      ? span.finish_reasons[0]
      : span.finish_reasons?.length
        ? span.finish_reasons
        : undefined;

  // Build overview data for DataInspector (flat structure)
  const overviewData: Record<string, unknown> = {
    ...(span.gen_ai_system && { system: span.gen_ai_system }),
    ...(span.agent_name && { agent: span.agent_name }),
    ...(span.user_id && { user_id: span.user_id }),
    ...(span.session_id && { session_id: span.session_id }),
    trace_id: span.trace_id,
    span_id: span.span_id,
    ...(span.parent_span_id && { parent_span_id: span.parent_span_id }),
    start_time: formatTimestamp(span.timestamp_start),
    ...(span.timestamp_end && { end_time: formatTimestamp(span.timestamp_end) }),
    ...(finishReason && { finish_reason: finishReason }),
  };

  // Build tokens data for DataInspector
  const tokensData: Record<string, number> = {};
  if (span.input_tokens > 0) tokensData.input = span.input_tokens;
  if (span.output_tokens > 0) tokensData.output = span.output_tokens;
  if (span.cache_read_tokens > 0) tokensData.cache_read = span.cache_read_tokens;
  if (span.cache_write_tokens > 0) tokensData.cache_write = span.cache_write_tokens;
  if (span.reasoning_tokens > 0) tokensData.reasoning = span.reasoning_tokens;
  if (span.total_tokens > 0) tokensData.total = span.total_tokens;
  const hasTokensData = Object.keys(tokensData).length > 0;

  // Build cost data for DataInspector (with percentages)
  const costData: Record<string, string> = {};
  const totalCost =
    span.total_cost > 0
      ? span.total_cost
      : span.input_cost +
        span.output_cost +
        span.cache_read_cost +
        span.cache_write_cost +
        span.reasoning_cost;

  const formatCostWithPercent = (value: number) => {
    if (totalCost === 0) return formatCost(value);
    const pct = (value / totalCost) * 100;
    const pctStr = pct > 0 && pct < 1 ? "<1%" : `${Math.round(pct)}%`;
    return `${formatCost(value)} (${pctStr})`;
  };

  if (span.input_cost > 0) costData.input = formatCostWithPercent(span.input_cost);
  if (span.output_cost > 0) costData.output = formatCostWithPercent(span.output_cost);
  if (span.cache_read_cost > 0) costData.cache_read = formatCostWithPercent(span.cache_read_cost);
  if (span.cache_write_cost > 0)
    costData.cache_write = formatCostWithPercent(span.cache_write_cost);
  if (span.reasoning_cost > 0) costData.reasoning = formatCostWithPercent(span.reasoning_cost);
  if (span.total_cost > 0) costData.total = formatCost(span.total_cost);
  const hasCostData = Object.keys(costData).length > 0;

  // Build metadata for DataInspector
  const metadata: Record<string, unknown> = {};
  if (attributes && Object.keys(attributes).length > 0) {
    metadata.attributes = attributes;
  }
  if (resourceAttributes && Object.keys(resourceAttributes).length > 0) {
    metadata.resourceAttributes = resourceAttributes;
  }
  if (links && links.length > 0) {
    metadata.links = links;
  }
  const hasMetadata = Object.keys(metadata).length > 0;

  const hasMessages = Object.keys(messagesForInspector).length > 0;

  return (
    <ScrollArea ref={scrollAreaRef} className="h-full">
      <SpanDetailHeader node={selectedNode} />

      <div key={span.span_id} className="@container space-y-4 p-4">
        <DataInspector data={overviewData} title="Overview" />

        {messagesLoading ? (
          <div className="flex items-center justify-center py-4">
            <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
          </div>
        ) : hasMessages ? (
          <DataInspector data={messagesForInspector} title="Messages" expandLastItem flatten />
        ) : null}

        {hasTokensData && <DataInspector data={tokensData} title="Tokens" />}

        {hasCostData && <DataInspector data={costData} title="Cost" />}

        {hasMetadata && <DataInspector data={metadata} title="Metadata" />}

        {rawSpan && (
          <div>
            <button
              type="button"
              onClick={() => setRawSpanExpanded(!rawSpanExpanded)}
              className="mb-2 flex w-full items-center justify-between text-xs font-medium uppercase tracking-wide text-muted-foreground hover:text-foreground"
            >
              <span>Raw Span</span>
              {rawSpanExpanded ? (
                <ChevronDown className="h-4 w-4" />
              ) : (
                <ChevronRight className="h-4 w-4" />
              )}
            </button>
            <div className="rounded-md border bg-muted/30 p-3">
              <JsonContent data={rawSpan} collapsed={rawSpanExpanded ? undefined : 1} />
            </div>
          </div>
        )}
      </div>
    </ScrollArea>
  );
}
