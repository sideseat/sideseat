import { useEffect, useCallback, useMemo, useState } from "react";
import { useNavigate } from "react-router";
import { useQueryClient } from "@tanstack/react-query";
import {
  Loader2,
  AlertCircle,
  RefreshCw,
  ChevronDown,
  ChevronRight,
  Clock,
  Cpu,
  Coins,
  Layers,
  ArrowUpRight,
  Users,
  Info,
  MessageSquare,
  Braces,
} from "lucide-react";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Tooltip, TooltipContent, TooltipTrigger, TooltipProvider } from "@/components/ui/tooltip";
import { ThreadView, type ThreadTab } from "@/components/thread";
import { DataInspector } from "@/components/data-inspector";
import { JsonContent } from "@/components/thread/content/json-content";
import { useSpan, useSpanMessages } from "@/api/otel/hooks/queries";
import { useSpanStream } from "@/api/otel/hooks/streams";
import {
  SPAN_TYPE_CONFIG,
  formatDuration,
  formatCost,
} from "@/components/trace-view/lib/span-config";
import { RawSpanView } from "./raw-span-view";
import { cn } from "@/lib/utils";
import type { ReactNode } from "react";
import type { SpanDetail as SpanDetailType, Block } from "@/api/otel/types";
import type { SpanType } from "@/components/trace-view/lib/types";

export type SpanTab = "overview" | "messages" | "raw";

// eslint-disable-next-line react-refresh/only-export-components
export const SPAN_TABS: { value: SpanTab; label: string; icon: ReactNode }[] = [
  { value: "overview", label: "Overview", icon: <Info className="h-4 w-4" /> },
  { value: "messages", label: "Messages", icon: <MessageSquare className="h-4 w-4" /> },
  { value: "raw", label: "Raw", icon: <Braces className="h-4 w-4" /> },
];

function formatTimestamp(ts: string | null | undefined): string {
  if (!ts) return "-";
  try {
    const d = new Date(ts);
    return d.toLocaleString(undefined, {
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
      fractionalSecondDigits: 3,
    });
  } catch {
    return ts;
  }
}

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

/** Map span_category to SpanType for icon/label lookup */
function getSpanType(span: SpanDetailType): SpanType {
  const category = span.span_category?.toLowerCase();
  if (category === "llm") return "llm";
  if (category === "tool") return "tool";
  if (category === "agent") return "agent";
  if (category === "embedding") return "embedding";
  if (category === "retriever") return "retriever";
  if (category === "http") return "http";
  if (category === "db" || category === "database") return "db";
  return "span";
}

interface TagBadgeProps {
  children: React.ReactNode;
  tooltip?: string;
  variant?: "secondary" | "destructive";
  className?: string;
}

function TagBadge({ children, tooltip, variant = "secondary", className }: TagBadgeProps) {
  const badge = (
    <Badge variant={variant} className={cn("gap-1 text-xs font-normal", className)}>
      {children}
    </Badge>
  );

  if (!tooltip) {
    return badge;
  }

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span className="inline-flex">{badge}</span>
      </TooltipTrigger>
      <TooltipContent side="bottom" className="max-w-sm break-all font-mono text-xs">
        {tooltip}
      </TooltipContent>
    </Tooltip>
  );
}

interface SpanHeaderProps {
  span: SpanDetailType;
  onViewInTrace?: () => void;
  onViewInSession?: () => void;
}

function SpanHeader({ span, onViewInTrace, onViewInSession }: SpanHeaderProps) {
  const spanType = getSpanType(span);
  const config = SPAN_TYPE_CONFIG[spanType];
  const Icon = config.icon;

  const hasError = span.status_code === "ERROR";
  const hasTokens = span.total_tokens > 0;
  const hasCost = span.total_cost > 0;
  const hasSession = !!span.session_id;

  const tokenDisplay = hasTokens
    ? span.input_tokens > 0 && span.output_tokens > 0
      ? `${span.input_tokens.toLocaleString()} → ${span.output_tokens.toLocaleString()} (Σ ${span.total_tokens.toLocaleString()})`
      : `${span.total_tokens.toLocaleString()} tokens`
    : null;

  return (
    <div className="@container space-y-2 border-b bg-background px-3 py-2.5 @[400px]:px-4 @[400px]:py-3">
      {/* Title row */}
      <div className="flex min-w-0 items-center gap-2">
        <Icon className={cn("h-4 w-4 shrink-0 @[400px]:h-5 @[400px]:w-5", config.accent)} />
        <h3 className="min-w-0 flex-1 truncate text-sm font-semibold @[400px]:text-base">
          {span.span_name}
        </h3>

        {/* Navigation buttons */}
        <TooltipProvider delayDuration={300}>
          <div className="flex shrink-0 items-center gap-1">
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-7 gap-1.5 px-2 text-xs"
                  onClick={onViewInTrace}
                >
                  <ArrowUpRight className="h-3.5 w-3.5" />
                  <span className="hidden @[500px]:inline">Trace</span>
                </Button>
              </TooltipTrigger>
              <TooltipContent side="bottom">View in Trace</TooltipContent>
            </Tooltip>

            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-7 gap-1.5 px-2 text-xs"
                  onClick={onViewInSession}
                  disabled={!hasSession}
                >
                  <Users className="h-3.5 w-3.5" />
                  <span className="hidden @[500px]:inline">Session</span>
                </Button>
              </TooltipTrigger>
              <TooltipContent side="bottom">
                {hasSession ? "View in Session" : "No session available"}
              </TooltipContent>
            </Tooltip>
          </div>
        </TooltipProvider>
      </div>

      {/* Tags cloud */}
      <div className="flex flex-wrap items-center gap-1.5">
        <TagBadge>{config.label}</TagBadge>

        {span.framework && (
          <TagBadge>
            <Layers className="h-3 w-3" />
            {span.framework}
          </TagBadge>
        )}

        {span.duration_ms != null && (
          <TagBadge className="font-mono">
            <Clock className="h-3 w-3" />
            {formatDuration(span.duration_ms)}
          </TagBadge>
        )}

        {tokenDisplay && (
          <TagBadge className="font-mono">
            <Cpu className="h-3 w-3" />
            {tokenDisplay}
          </TagBadge>
        )}

        {hasCost && (
          <TagBadge className="font-mono">
            <Coins className="h-3 w-3" />
            {formatCost(span.total_cost)}
          </TagBadge>
        )}

        {hasError && (
          <TagBadge variant="destructive">
            <AlertCircle className="h-3 w-3" />
            {span.status_code}
          </TagBadge>
        )}

        {span.model && (
          <TagBadge className="max-w-48 font-mono @[500px]:max-w-64" tooltip={span.model}>
            <span className="truncate">{span.model}</span>
          </TagBadge>
        )}

        {span.finish_reasons && span.finish_reasons.length > 0 && (
          <TagBadge>{span.finish_reasons.join(", ")}</TagBadge>
        )}

        {span.environment && <TagBadge>{span.environment}</TagBadge>}
      </div>
    </div>
  );
}

interface RawSpan {
  attributes?: Record<string, unknown>;
  resource?: { attributes?: Record<string, unknown> };
  links?: Array<{ trace_id: string; span_id: string; attributes: Record<string, unknown> }>;
}

interface SpanDetailProps {
  traceId: string;
  spanId: string;
  projectId: string;
  activeTab: SpanTab;
  threadTab?: ThreadTab;
  onThreadTabChange?: (tab: ThreadTab) => void;
  realtimeEnabled?: boolean;
  onRefreshChange?: (refetch: (() => void) | null, isRefreshing: boolean) => void;
}

export function SpanDetail({
  traceId,
  spanId,
  projectId,
  activeTab,
  threadTab,
  onThreadTabChange,
  realtimeEnabled = true,
  onRefreshChange,
}: SpanDetailProps) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [rawSpanExpanded, setRawSpanExpanded] = useState(false);

  // Reset expanded state when span changes
  useEffect(() => {
    setRawSpanExpanded(false);
  }, [spanId]);

  const {
    data: spanData,
    isLoading: spanLoading,
    isFetching: spanFetching,
    error: spanError,
    refetch: refetchSpan,
  } = useSpan(projectId, traceId, spanId, { include_raw_span: true });

  const {
    data: messagesData,
    isLoading: messagesLoading,
    isFetching: messagesFetching,
    error: messagesError,
    refetch: refetchMessages,
  } = useSpanMessages(projectId, traceId, spanId);

  // SSE params filtered by span_id
  const sseParams = useMemo(() => ({ span_id: spanId }), [spanId]);

  // Refetch queries when SSE events arrive
  const handleSseEvent = useCallback(() => {
    queryClient.refetchQueries({
      predicate: (query) => {
        const key = query.queryKey;
        return Array.isArray(key) && key.includes(spanId);
      },
      type: "active",
    });
  }, [queryClient, spanId]);

  // Subscribe to SSE
  useSpanStream({
    projectId,
    params: sseParams,
    enabled: realtimeEnabled,
    onSpan: handleSseEvent,
  });

  // Notify parent of refresh function availability
  useEffect(() => {
    if (activeTab === "messages") {
      onRefreshChange?.(refetchMessages, messagesFetching && !messagesLoading);
    } else {
      onRefreshChange?.(refetchSpan, spanFetching && !spanLoading);
    }
  }, [
    activeTab,
    refetchMessages,
    messagesFetching,
    messagesLoading,
    refetchSpan,
    spanFetching,
    spanLoading,
    onRefreshChange,
  ]);

  // Memoize breakdown objects
  const tokenBreakdown = useMemo(() => {
    if (!spanData) return undefined;
    return {
      input_tokens: spanData.input_tokens,
      output_tokens: spanData.output_tokens,
      cache_read_tokens: spanData.cache_read_tokens,
      cache_write_tokens: spanData.cache_write_tokens,
      reasoning_tokens: spanData.reasoning_tokens,
      total_tokens: spanData.total_tokens,
    };
  }, [spanData]);

  const costBreakdown = useMemo(() => {
    if (!spanData) return undefined;
    return {
      input_cost: spanData.input_cost,
      output_cost: spanData.output_cost,
      cache_read_cost: spanData.cache_read_cost,
      cache_write_cost: spanData.cache_write_cost,
      reasoning_cost: spanData.reasoning_cost,
      total_cost: spanData.total_cost,
    };
  }, [spanData]);

  // Transform messages for DataInspector (for overview tab)
  const messagesForInspector = useMemo(
    () => (messagesData?.messages ? transformBlocksToData(messagesData.messages) : {}),
    [messagesData?.messages],
  );

  // Memoize overview data to prevent unnecessary re-renders
  const { overviewData, tokensData, costData, metadataData, rawSpan } = useMemo(() => {
    if (!spanData) {
      return {
        overviewData: null,
        tokensData: null,
        costData: null,
        metadataData: null,
        rawSpan: null,
      };
    }

    // Get finish reason
    const finishReason =
      spanData.finish_reasons?.length === 1
        ? spanData.finish_reasons[0]
        : spanData.finish_reasons?.length
          ? spanData.finish_reasons
          : undefined;

    // Build overview data (matching span-detail-panel structure)
    const overview: Record<string, unknown> = {
      ...(spanData.gen_ai_system && { system: spanData.gen_ai_system }),
      ...(spanData.agent_name && { agent: spanData.agent_name }),
      ...(spanData.user_id && { user_id: spanData.user_id }),
      ...(spanData.session_id && { session_id: spanData.session_id }),
      trace_id: spanData.trace_id,
      span_id: spanData.span_id,
      ...(spanData.parent_span_id && { parent_span_id: spanData.parent_span_id }),
      start_time: formatTimestamp(spanData.timestamp_start),
      ...(spanData.timestamp_end && { end_time: formatTimestamp(spanData.timestamp_end) }),
      ...(finishReason && { finish_reason: finishReason }),
    };

    // Build tokens data
    const tokens: Record<string, number> = {};
    if (spanData.input_tokens > 0) tokens.input = spanData.input_tokens;
    if (spanData.output_tokens > 0) tokens.output = spanData.output_tokens;
    if (spanData.cache_read_tokens > 0) tokens.cache_read = spanData.cache_read_tokens;
    if (spanData.cache_write_tokens > 0) tokens.cache_write = spanData.cache_write_tokens;
    if (spanData.reasoning_tokens > 0) tokens.reasoning = spanData.reasoning_tokens;
    if (spanData.total_tokens > 0) tokens.total = spanData.total_tokens;

    // Build cost data with percentages
    const cost: Record<string, string> = {};
    const totalCost =
      spanData.total_cost > 0
        ? spanData.total_cost
        : spanData.input_cost +
          spanData.output_cost +
          spanData.cache_read_cost +
          spanData.cache_write_cost +
          spanData.reasoning_cost;

    const formatCostWithPercent = (value: number) => {
      if (totalCost === 0) return formatCost(value);
      const pct = (value / totalCost) * 100;
      const pctStr = pct > 0 && pct < 1 ? "<1%" : `${Math.round(pct)}%`;
      return `${formatCost(value)} (${pctStr})`;
    };

    if (spanData.input_cost > 0) cost.input = formatCostWithPercent(spanData.input_cost);
    if (spanData.output_cost > 0) cost.output = formatCostWithPercent(spanData.output_cost);
    if (spanData.cache_read_cost > 0)
      cost.cache_read = formatCostWithPercent(spanData.cache_read_cost);
    if (spanData.cache_write_cost > 0)
      cost.cache_write = formatCostWithPercent(spanData.cache_write_cost);
    if (spanData.reasoning_cost > 0)
      cost.reasoning = formatCostWithPercent(spanData.reasoning_cost);
    if (spanData.total_cost > 0) cost.total = formatCost(spanData.total_cost);

    // Build metadata from raw_span
    const raw = spanData.raw_span as RawSpan | undefined;
    const metadata: Record<string, unknown> = {};
    if (raw?.attributes && Object.keys(raw.attributes).length > 0) {
      metadata.attributes = raw.attributes;
    }
    if (raw?.resource?.attributes && Object.keys(raw.resource.attributes).length > 0) {
      metadata.resourceAttributes = raw.resource.attributes;
    }
    if (raw?.links && raw.links.length > 0) {
      metadata.links = raw.links;
    }

    return {
      overviewData: overview,
      tokensData: Object.keys(tokens).length > 0 ? tokens : null,
      costData: Object.keys(cost).length > 0 ? cost : null,
      metadataData: Object.keys(metadata).length > 0 ? metadata : null,
      rawSpan: raw,
    };
  }, [spanData]);

  const hasMessages = Object.keys(messagesForInspector).length > 0;

  // Navigation handlers
  const handleViewInTrace = useCallback(() => {
    navigate(`/projects/${projectId}/observability/traces/${traceId}`);
  }, [navigate, projectId, traceId]);

  const handleViewInSession = useCallback(() => {
    if (spanData?.session_id) {
      navigate(`/projects/${projectId}/observability/sessions/${spanData.session_id}`);
    }
  }, [navigate, projectId, spanData?.session_id]);

  if (activeTab === "messages") {
    return (
      <ThreadView
        blocks={messagesData?.messages ?? []}
        metadata={messagesData?.metadata}
        toolDefinitions={messagesData?.tool_definitions}
        tokenBreakdown={tokenBreakdown}
        costBreakdown={costBreakdown}
        isLoading={messagesLoading}
        error={messagesError ?? undefined}
        onRetry={refetchMessages}
        className="h-full"
        activeTab={threadTab}
        onTabChange={onThreadTabChange}
        projectId={projectId}
      />
    );
  }

  if (activeTab === "raw") {
    if (spanLoading) {
      return (
        <div className="flex h-full items-center justify-center">
          <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
        </div>
      );
    }

    if (spanError) {
      return (
        <div className="flex h-full flex-col items-center justify-center gap-4 p-8">
          <AlertCircle className="h-12 w-12 text-destructive" />
          <div className="text-center">
            <h3 className="font-medium">Failed to load span</h3>
            <p className="text-sm text-muted-foreground">{spanError.message}</p>
          </div>
          <Button variant="outline" size="sm" onClick={() => refetchSpan()}>
            <RefreshCw className="mr-2 h-4 w-4" />
            Retry
          </Button>
        </div>
      );
    }

    const rawSpanData = spanData?.raw_span;
    if (!rawSpanData) {
      return (
        <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
          No raw data available
        </div>
      );
    }

    return (
      <RawSpanView spanId={spanId} spanName={spanData?.span_name ?? ""} rawSpan={rawSpanData} />
    );
  }

  // Overview tab (default)
  if (spanLoading) {
    return (
      <div className="flex h-full items-center justify-center">
        <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
      </div>
    );
  }

  if (spanError) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-4 p-8">
        <AlertCircle className="h-12 w-12 text-destructive" />
        <div className="text-center">
          <h3 className="font-medium">Failed to load span</h3>
          <p className="text-sm text-muted-foreground">{spanError.message}</p>
        </div>
        <Button variant="outline" size="sm" onClick={() => refetchSpan()}>
          <RefreshCw className="mr-2 h-4 w-4" />
          Retry
        </Button>
      </div>
    );
  }

  if (!spanData || !overviewData) {
    return (
      <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
        Span not found
      </div>
    );
  }

  return (
    <ScrollArea className="h-full">
      <SpanHeader
        span={spanData}
        onViewInTrace={handleViewInTrace}
        onViewInSession={handleViewInSession}
      />

      <div key={spanData.span_id} className="@container space-y-4 p-4">
        <DataInspector data={overviewData} title="Overview" />

        {messagesLoading ? (
          <div className="flex items-center justify-center py-4">
            <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
          </div>
        ) : hasMessages ? (
          <DataInspector data={messagesForInspector} title="Messages" expandLastItem flatten />
        ) : null}

        {tokensData && <DataInspector data={tokensData} title="Tokens" />}

        {costData && <DataInspector data={costData} title="Cost" />}

        {metadataData && <DataInspector data={metadataData} title="Metadata" />}

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
