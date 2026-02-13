import { useEffect, useCallback, useMemo } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { ThreadView, type ThreadTab } from "@/components/thread";
import { useSessionMessages, useSession, useSessionSpans } from "@/api/otel/hooks/queries";
import { useSpanStream } from "@/api/otel/hooks/streams";
import type { ViewMode } from "@/components/trace-view/lib/types";
import { SessionSpansView } from "./session-spans-view";
import { RawSpansView } from "../trace/raw-spans-view";

export type SessionTab = "thread" | "trace" | "raw";

// Stable noop function to avoid creating new references
const noop = () => {};

interface SessionDetailProps {
  sessionId: string;
  projectId: string;
  activeTab: SessionTab;
  threadTab?: ThreadTab;
  onThreadTabChange?: (tab: ThreadTab) => void;
  traceTab?: ViewMode;
  onTraceTabChange?: (tab: ViewMode) => void;
  realtimeEnabled?: boolean;
  onRefreshChange?: (refetch: (() => void) | null, isRefreshing: boolean) => void;
}

export function SessionDetail({
  sessionId,
  projectId,
  activeTab,
  threadTab,
  onThreadTabChange,
  traceTab,
  onTraceTabChange,
  realtimeEnabled = true,
  onRefreshChange,
}: SessionDetailProps) {
  const queryClient = useQueryClient();

  const {
    data: messagesData,
    isLoading: messagesLoading,
    isFetching: messagesFetching,
    error: messagesError,
    refetch: refetchMessages,
  } = useSessionMessages(projectId, sessionId);

  const { data: sessionData } = useSession(projectId, sessionId);

  // Fetch spans for trace/raw tabs
  const {
    data: spansData,
    isLoading: spansLoading,
    isFetching: spansFetching,
    error: spansError,
    refetch: refetchSpans,
  } = useSessionSpans(projectId, sessionId, {
    include_raw_span: activeTab === "raw",
    enabled: activeTab === "trace" || activeTab === "raw",
  });

  const spans = useMemo(() => spansData?.data ?? [], [spansData]);
  const spansWithRaw = useMemo(() => spans.filter((s) => s.raw_span), [spans]);

  // SSE params filtered by session_id
  const sseParams = useMemo(() => ({ session_id: sessionId }), [sessionId]);

  // Refetch queries for this session when SSE events arrive
  const handleSseEvent = useCallback(() => {
    queryClient.refetchQueries({
      predicate: (query) => {
        const key = query.queryKey;
        return Array.isArray(key) && key.includes(sessionId);
      },
      type: "active",
    });
  }, [queryClient, sessionId]);

  // Subscribe to SSE for this session
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
      // trace and raw tabs use spans
      onRefreshChange?.(refetchSpans, spansFetching && !spansLoading);
    }
  }, [
    activeTab,
    refetchMessages,
    messagesFetching,
    messagesLoading,
    refetchSpans,
    spansFetching,
    spansLoading,
    onRefreshChange,
  ]);

  // Memoize breakdown objects
  const tokenBreakdown = useMemo(() => {
    if (!sessionData) return undefined;
    return {
      input_tokens: sessionData.input_tokens,
      output_tokens: sessionData.output_tokens,
      cache_read_tokens: sessionData.cache_read_tokens,
      cache_write_tokens: sessionData.cache_write_tokens,
      reasoning_tokens: sessionData.reasoning_tokens,
      total_tokens: sessionData.total_tokens,
    };
  }, [sessionData]);

  const costBreakdown = useMemo(() => {
    if (!sessionData) return undefined;
    return {
      input_cost: sessionData.input_cost,
      output_cost: sessionData.output_cost,
      cache_read_cost: sessionData.cache_read_cost,
      cache_write_cost: sessionData.cache_write_cost,
      reasoning_cost: sessionData.reasoning_cost,
      total_cost: sessionData.total_cost,
    };
  }, [sessionData]);

  // Compute session duration from start/end times
  const durationMs = useMemo(() => {
    if (!sessionData?.start_time) return undefined;
    const start = new Date(sessionData.start_time).getTime();
    const end = sessionData.end_time ? new Date(sessionData.end_time).getTime() : Date.now();
    return Math.max(0, end - start);
  }, [sessionData]);

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
        showTraceLinks
      />
    );
  }

  if (activeTab === "trace") {
    return (
      <SessionSpansView
        projectId={projectId}
        spans={spans}
        viewMode={traceTab ?? "tree"}
        onViewModeChange={onTraceTabChange ?? noop}
        isLoading={spansLoading}
        error={spansError ?? undefined}
        onRetry={refetchSpans}
        durationMs={durationMs}
        tokenBreakdown={tokenBreakdown}
        costBreakdown={costBreakdown}
      />
    );
  }

  // Raw tab (default fallback)
  return (
    <RawSpansView
      spans={spansWithRaw}
      entityId={sessionId}
      downloadPrefix="session"
      isLoading={spansLoading}
      error={spansError ?? undefined}
      onRetry={refetchSpans}
    />
  );
}
