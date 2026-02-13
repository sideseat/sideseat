import { useCallback, useMemo } from "react";
import { useEventStream } from "@/hooks/use-event-stream";
import { useOtelClient } from "@/lib/app-context";
import { otelKeys } from "../keys";
import type { SseSpanEvent, SseParams } from "../types";

interface UseSpanStreamOptions {
  projectId: string;
  params?: SseParams;
  enabled?: boolean;
  onSpan?: (span: SseSpanEvent) => void;
  onError?: (error: Error) => void;
}

interface UseSpanStreamResult {
  /** Current connection status */
  status: "disconnected" | "connecting" | "connected" | "error";
  /** Manually trigger reconnection */
  reconnect: () => void;
  /** Number of retry attempts */
  retryCount: number;
}

export function useSpanStream({
  projectId,
  params,
  enabled = true,
  onSpan,
  onError,
}: UseSpanStreamOptions): UseSpanStreamResult {
  const otelClient = useOtelClient();

  // Serialize params to prevent stale closure issues
  const paramsJson = useMemo(() => JSON.stringify(params ?? {}), [params]);

  // Subscribe function matches useEventStream's expected signature
  const subscribe = useCallback(
    (
      onEvent: (event: SseSpanEvent) => void,
      onStreamError: (error: Error) => void,
      onOpen?: () => void,
    ) => {
      const parsedParams = JSON.parse(paramsJson) as SseParams;
      return otelClient.subscribeToSpans(projectId, parsedParams, {
        onSpan: onEvent,
        onError: onStreamError,
        onOpen,
      });
    },
    [otelClient, projectId, paramsJson],
  );

  // Memoize invalidateKeys to prevent infinite loop
  // Includes stats for dashboard widgets (TokenTrend, TraceLatency, QuickStats, etc.)
  const invalidateKeys = useMemo(
    () =>
      [
        [...otelKeys.traces.lists(projectId)],
        [...otelKeys.spans.lists(projectId)],
        [...otelKeys.sessions.lists(projectId)],
        [...otelKeys.stats.all(projectId)],
      ] as unknown[][],
    [projectId],
  );

  return useEventStream({
    subscribe,
    subscribeKey: `${projectId}:${paramsJson}`,
    invalidateKeys,
    enabled: enabled && !!projectId,
    onEvent: onSpan,
    onError,
  });
}
