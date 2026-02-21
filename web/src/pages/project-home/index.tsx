import { useState, useEffect, useMemo, useCallback, useRef } from "react";
import { useParams } from "react-router";
import { RefreshCw } from "lucide-react";
import { useProjectStats, useTraces } from "@/api/otel/hooks/queries";
import { useSpanStream } from "@/api/otel/hooks/streams";
import { Button } from "@/components/ui/button";
import { ButtonGroup } from "@/components/ui/button-group";
import { settings, HOME_REALTIME_KEY } from "@/lib/settings";
import { cn } from "@/lib/utils";
import { type TimeRange, getTimeRangeParams, loadTimeRange, saveTimeRange } from "@/lib/time-range";
import {
  TimeRangeSelector,
  QuickStats,
  FuelGauge,
  FrameworkChart,
  ModelMix,
  ActivityFeed,
  TokenTrend,
  TraceLatency,
  InsightsPanel,
  StatsError,
  WidgetErrorBoundary,
} from "./components";

// Debounce delay for refetch on SSE events (ms)
const SSE_REFETCH_DEBOUNCE_MS = 2000;

export default function ProjectHomePage() {
  const { projectId = "default" } = useParams<{ projectId: string }>();
  const refetchTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Time range state (persisted in localStorage, reset on project change)
  const [timeRange, setTimeRange] = useState<TimeRange>(() => loadTimeRange(projectId));
  useEffect(() => setTimeRange(loadTimeRange(projectId)), [projectId]);

  // Incrementing forces fresh timestamps in stats query while keeping stable cache keys
  const [refreshTrigger, setRefreshTrigger] = useState(0);

  // Real-time updates toggle (persisted in settings)
  const [realtimeEnabled, setRealtimeEnabled] = useState(() => {
    return settings.get<boolean>(HOME_REALTIME_KEY) ?? true;
  });

  useEffect(() => {
    return () => {
      if (refetchTimerRef.current) clearTimeout(refetchTimerRef.current);
    };
  }, []);

  const handleTimeRangeChange = (range: TimeRange) => {
    setTimeRange(range);
    saveTimeRange(projectId, range);
  };

  const timeRangeParams = useMemo(
    () => getTimeRangeParams(timeRange),
    // eslint-disable-next-line react-hooks/exhaustive-deps -- refreshTrigger intentionally forces recomputation
    [timeRange, refreshTrigger],
  );
  const {
    data: stats,
    isLoading: statsLoading,
    isError: statsError,
  } = useProjectStats(projectId, timeRangeParams, {
    refetchInterval: realtimeEnabled ? false : 30_000, // Disable interval when SSE is active
  });

  // Traces query for activity feed (always latest, regardless of time range)
  const {
    data: tracesData,
    isLoading: tracesLoading,
    isError: tracesError,
    refetch: refetchTraces,
  } = useTraces(
    projectId,
    { limit: 20, order_by: "end_time:desc" },
    { refetchInterval: realtimeEnabled ? false : 30_000 }, // Disable interval when SSE is active
  );

  const triggerRefresh = useCallback(() => {
    setRefreshTrigger((n) => n + 1);
    refetchTraces();
  }, [refetchTraces]);

  const handleSpanEvent = useCallback(() => {
    if (refetchTimerRef.current) clearTimeout(refetchTimerRef.current);
    refetchTimerRef.current = setTimeout(() => {
      refetchTimerRef.current = null;
      triggerRefresh();
    }, SSE_REFETCH_DEBOUNCE_MS);
  }, [triggerRefresh]);

  const { status: streamStatus } = useSpanStream({
    projectId,
    enabled: realtimeEnabled,
    onSpan: handleSpanEvent,
  });

  const [isRefreshing, setIsRefreshing] = useState(false);
  const handleRefresh = useCallback(async () => {
    setIsRefreshing(true);
    try {
      setRefreshTrigger((n) => n + 1);
      await refetchTraces();
    } finally {
      setIsRefreshing(false);
    }
  }, [refetchTraces]);

  const activityTraces = (tracesData?.data ?? []).slice(0, 5);

  if (statsError) {
    return (
      <div className="w-full mx-auto pt-header-offset sm:pt-header-offset-sm px-2 sm:px-4 pb-6">
        <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-2 pb-4">
          <div className="shrink-0">
            <h1 className="text-2xl font-semibold tracking-tight">Home</h1>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <ButtonGroup>
              <Button
                variant="outline"
                size="sm"
                className="px-2 gap-1.5 min-w-13 sm:min-w-17"
                onClick={() => {
                  setRealtimeEnabled((prev) => {
                    const next = !prev;
                    settings.set(HOME_REALTIME_KEY, next);
                    return next;
                  });
                }}
                aria-label={
                  realtimeEnabled ? "Disable real-time updates" : "Enable real-time updates"
                }
              >
                <span
                  className={cn(
                    "h-2 w-2 rounded-full shrink-0",
                    !realtimeEnabled
                      ? "bg-muted-foreground"
                      : streamStatus === "error"
                        ? "bg-destructive"
                        : "bg-primary",
                  )}
                />
                <span
                  className={cn("hidden sm:inline text-xs", realtimeEnabled && "font-semibold")}
                >
                  Live
                </span>
              </Button>
              <Button
                variant="outline"
                size="sm"
                className="h-8 w-8 px-0"
                onClick={handleRefresh}
                disabled={isRefreshing}
              >
                <RefreshCw className={`h-4 w-4 ${isRefreshing ? "animate-spin" : ""}`} />
              </Button>
            </ButtonGroup>
            <TimeRangeSelector value={timeRange} onChange={handleTimeRangeChange} />
          </div>
        </div>
        <StatsError onRetry={handleRefresh} />
      </div>
    );
  }

  return (
    <div className="w-full mx-auto pt-header-offset sm:pt-header-offset-sm px-2 sm:px-4 pb-6">
      <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-2 pb-4">
        <div className="shrink-0">
          <h1 className="text-2xl font-semibold tracking-tight">Home</h1>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <ButtonGroup>
            <Button
              variant="outline"
              size="sm"
              className="px-2 gap-1.5 min-w-13 sm:min-w-17"
              onClick={() => {
                setRealtimeEnabled((prev) => {
                  const next = !prev;
                  settings.set(HOME_REALTIME_KEY, next);
                  return next;
                });
              }}
              aria-label={
                realtimeEnabled ? "Disable real-time updates" : "Enable real-time updates"
              }
            >
              <span
                className={cn(
                  "h-2 w-2 rounded-full shrink-0",
                  !realtimeEnabled
                    ? "bg-muted-foreground"
                    : streamStatus === "error"
                      ? "bg-destructive"
                      : "bg-primary",
                )}
              />
              <span className={cn("hidden sm:inline text-xs", realtimeEnabled && "font-semibold")}>
                Live
              </span>
            </Button>
            <Button
              variant="outline"
              size="sm"
              className="h-8 w-8 px-0"
              onClick={handleRefresh}
              disabled={isRefreshing}
            >
              <RefreshCw className={`h-4 w-4 ${isRefreshing ? "animate-spin" : ""}`} />
            </Button>
          </ButtonGroup>
          <TimeRangeSelector value={timeRange} onChange={handleTimeRangeChange} />
        </div>
      </div>

      <div className="space-y-6">
        <div className="grid gap-6 lg:grid-cols-12">
          <div className="lg:col-span-8">
            <WidgetErrorBoundary title="Quick Stats">
              <QuickStats
                projectId={projectId}
                traces={stats?.counts.traces ?? 0}
                sessions={stats?.counts.sessions ?? 0}
                spans={stats?.counts.spans ?? 0}
                uniqueUsers={stats?.counts.unique_users ?? 0}
                isLoading={statsLoading}
              />
            </WidgetErrorBoundary>
          </div>
          <div className="lg:col-span-4">
            <WidgetErrorBoundary title="Cost">
              <FuelGauge
                projectId={projectId}
                timeRange={timeRange}
                costs={
                  stats?.costs ?? {
                    input: 0,
                    output: 0,
                    cache_read: 0,
                    cache_write: 0,
                    reasoning: 0,
                    total: 0,
                  }
                }
                isLoading={statsLoading}
              />
            </WidgetErrorBoundary>
          </div>
        </div>

        <div className="grid gap-6 lg:grid-cols-12">
          <div className="lg:col-span-6">
            <WidgetErrorBoundary title="Token Volume">
              <TokenTrend
                data={stats?.trend_data ?? []}
                total={stats?.tokens.total ?? 0}
                timeRange={timeRange}
                isLoading={statsLoading}
              />
            </WidgetErrorBoundary>
          </div>
          <div className="lg:col-span-6">
            <WidgetErrorBoundary title="Avg Trace Latency">
              <TraceLatency
                data={stats?.latency_trend_data ?? []}
                avgDurationMs={stats?.avg_trace_duration_ms ?? null}
                traceCount={stats?.counts.traces ?? 0}
                timeRange={timeRange}
                isLoading={statsLoading}
              />
            </WidgetErrorBoundary>
          </div>
        </div>

        <div className="grid gap-6 lg:grid-cols-12">
          <div className="lg:col-span-8">
            <WidgetErrorBoundary title="Recent Activity">
              <ActivityFeed
                projectId={projectId}
                traces={tracesError ? [] : activityTraces}
                recentCount={stats?.recent_activity_count}
                isLoading={tracesLoading}
              />
            </WidgetErrorBoundary>
          </div>
          <div className="lg:col-span-4">
            <WidgetErrorBoundary title="Insights">
              <InsightsPanel stats={stats} isLoading={statsLoading} />
            </WidgetErrorBoundary>
          </div>
        </div>

        <div className="grid grid-cols-1 gap-6 md:grid-cols-2">
          <WidgetErrorBoundary title="Framework Distribution">
            <FrameworkChart
              projectId={projectId}
              data={stats?.by_framework ?? []}
              isLoading={statsLoading}
            />
          </WidgetErrorBoundary>
          <WidgetErrorBoundary title="Model Mix">
            <ModelMix
              projectId={projectId}
              data={stats?.by_model ?? []}
              totalTokens={stats?.tokens.total ?? 0}
              traceCount={stats?.counts.traces ?? 0}
              isLoading={statsLoading}
            />
          </WidgetErrorBoundary>
        </div>
      </div>
    </div>
  );
}
