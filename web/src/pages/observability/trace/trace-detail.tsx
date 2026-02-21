import { lazy, Suspense, useEffect, useCallback, useMemo } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { ThreadView, type ThreadTab } from "@/components/thread";
import type { ViewMode } from "@/components/trace-view/lib/types";
import { useTraceMessages, useTrace } from "@/api/otel/hooks/queries";
import { useSpanStream } from "@/api/otel/hooks/streams";

const TraceView = lazy(() =>
  import("@/components/trace-view").then((m) => ({ default: m.TraceView })),
);
const RawSpansView = lazy(() =>
  import("./raw-spans-view").then((m) => ({ default: m.RawSpansView })),
);

export type TraceTab = "thread" | "trace" | "raw";

interface TraceDetailProps {
  traceId: string;
  projectId: string;
  activeTab: TraceTab;
  traceTab?: ViewMode;
  onTraceTabChange?: (tab: ViewMode) => void;
  threadTab?: ThreadTab;
  onThreadTabChange?: (tab: ThreadTab) => void;
  realtimeEnabled?: boolean;
  onRefreshChange?: (refetch: (() => void) | null, isRefreshing: boolean) => void;
}

export function TraceDetail({
  traceId,
  projectId,
  activeTab,
  traceTab,
  onTraceTabChange,
  threadTab,
  onThreadTabChange,
  realtimeEnabled = true,
  onRefreshChange,
}: TraceDetailProps) {
  const queryClient = useQueryClient();

  const {
    data: messagesData,
    isLoading: messagesLoading,
    isFetching: messagesFetching,
    error: messagesError,
    refetch: refetchMessages,
  } = useTraceMessages(projectId, traceId);

  const {
    data: traceData,
    isLoading: traceLoading,
    isFetching: traceFetching,
    error: traceError,
    refetch: refetchTrace,
  } = useTrace(projectId, traceId, { include_raw_span: true });

  // SSE params filtered by trace_id
  const sseParams = useMemo(() => ({ trace_id: traceId }), [traceId]);

  // Refetch queries for this trace when SSE events arrive
  const handleSseEvent = useCallback(() => {
    // Use predicate to match all queries containing this traceId
    queryClient.refetchQueries({
      predicate: (query) => {
        const key = query.queryKey;
        return Array.isArray(key) && key.includes(traceId);
      },
      type: "active",
    });
  }, [queryClient, traceId]);

  // Subscribe to SSE for this trace
  useSpanStream({
    projectId,
    params: sseParams,
    enabled: realtimeEnabled,
    onSpan: handleSseEvent,
  });

  // Notify parent of refresh function availability
  useEffect(() => {
    if (activeTab === "thread") {
      onRefreshChange?.(refetchMessages, messagesFetching && !messagesLoading);
    } else {
      onRefreshChange?.(refetchTrace, traceFetching && !traceLoading);
    }
  }, [
    activeTab,
    refetchMessages,
    messagesFetching,
    messagesLoading,
    refetchTrace,
    traceFetching,
    traceLoading,
    onRefreshChange,
  ]);

  // Memoize spans with raw data for the Raw tab
  const spansWithRaw = useMemo(() => {
    return traceData?.spans?.filter((s) => s.raw_span) ?? [];
  }, [traceData?.spans]);

  // Memoize breakdown objects to avoid recreation on each render
  const tokenBreakdown = useMemo(() => {
    if (!traceData) return undefined;
    return {
      input_tokens: traceData.input_tokens,
      output_tokens: traceData.output_tokens,
      cache_read_tokens: traceData.cache_read_tokens,
      cache_write_tokens: traceData.cache_write_tokens,
      reasoning_tokens: traceData.reasoning_tokens,
      total_tokens: traceData.total_tokens,
    };
  }, [traceData]);

  const costBreakdown = useMemo(() => {
    if (!traceData) return undefined;
    return {
      input_cost: traceData.input_cost,
      output_cost: traceData.output_cost,
      cache_read_cost: traceData.cache_read_cost,
      cache_write_cost: traceData.cache_write_cost,
      reasoning_cost: traceData.reasoning_cost,
      total_cost: traceData.total_cost,
    };
  }, [traceData]);

  if (activeTab === "thread") {
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
    return (
      <Suspense fallback={<TabFallback />}>
        <RawSpansView
          key={traceId}
          spans={spansWithRaw}
          entityId={traceId}
          downloadPrefix="trace"
          isLoading={traceLoading}
          error={traceError}
          onRetry={refetchTrace}
        />
      </Suspense>
    );
  }

  return (
    <Suspense fallback={<TabFallback />}>
      <TraceView
        projectId={projectId}
        traceId={traceId}
        spans={traceData?.spans}
        durationMs={traceData?.duration_ms}
        tokenBreakdown={tokenBreakdown}
        costBreakdown={costBreakdown}
        isLoading={traceLoading}
        error={traceError ?? undefined}
        onRetry={refetchTrace}
        viewMode={traceTab}
        onViewModeChange={onTraceTabChange}
      />
    </Suspense>
  );
}

function TabFallback() {
  return (
    <div className="flex h-64 w-full items-center justify-center">
      <div className="h-6 w-36 animate-pulse rounded-md bg-muted" />
    </div>
  );
}
