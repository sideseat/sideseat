import { AlertCircle, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { TraceView } from "@/components/trace-view";
import type { ViewMode } from "@/components/trace-view/lib/types";
import type { TokenBreakdown, CostBreakdown } from "@/components/breakdown-popover";
import type { SpanDetail } from "@/api/otel/types";

interface SessionSpansViewProps {
  projectId: string;
  spans: SpanDetail[];
  viewMode: ViewMode;
  onViewModeChange: (mode: ViewMode) => void;
  isLoading: boolean;
  error?: Error;
  onRetry: () => void;
  durationMs?: number;
  tokenBreakdown?: TokenBreakdown;
  costBreakdown?: CostBreakdown;
}

export function SessionSpansView({
  projectId,
  spans,
  viewMode,
  onViewModeChange,
  isLoading,
  error,
  onRetry,
  durationMs,
  tokenBreakdown,
  costBreakdown,
}: SessionSpansViewProps) {
  if (isLoading) {
    return (
      <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
        Loading spans...
      </div>
    );
  }

  if (error) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-4 p-8">
        <AlertCircle className="h-12 w-12 text-destructive" />
        <div className="text-center">
          <h3 className="font-medium">Failed to load spans</h3>
          <p className="text-sm text-muted-foreground">{error.message}</p>
        </div>
        <Button variant="outline" size="sm" onClick={onRetry}>
          <RefreshCw className="mr-2 h-4 w-4" />
          Retry
        </Button>
      </div>
    );
  }

  return (
    <TraceView
      projectId={projectId}
      traceId={spans[0]?.trace_id ?? ""}
      spans={spans}
      durationMs={durationMs}
      tokenBreakdown={tokenBreakdown}
      costBreakdown={costBreakdown}
      viewMode={viewMode}
      onViewModeChange={onViewModeChange}
    />
  );
}
