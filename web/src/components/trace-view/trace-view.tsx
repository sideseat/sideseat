import { useState, useEffect, useCallback } from "react";
import { AlertCircle, RefreshCw } from "lucide-react";
import { settings, getTraceViewLayoutKey } from "@/lib/settings";
import { TooltipProvider } from "@/components/ui/tooltip";
import { Button } from "@/components/ui/button";
import { ResizablePanelGroup, ResizablePanel, ResizableHandle } from "@/components/ui/resizable";
import type { SpanDetail } from "@/api/otel/types";
import type { TokenBreakdown, CostBreakdown } from "@/components/breakdown-popover";
import type { LayoutDirection, ViewMode } from "./lib/types";
import { TraceViewProvider } from "./contexts/trace-view-context";
import { useTraceView } from "./contexts/use-trace-view";
import { SpanTree } from "./components/span-tree";
import { SpanTimeline } from "./components/span-timeline";
import { SpanDiagram } from "./components/span-diagram";
import { SpanDetailPanel } from "./components/span-detail";
import { TraceViewHeader } from "./components/trace-view-header";

interface TraceViewProps {
  projectId: string;
  traceId: string;
  spans?: SpanDetail[];
  durationMs?: number | null;
  tokenBreakdown?: TokenBreakdown;
  costBreakdown?: CostBreakdown;
  isLoading?: boolean;
  error?: Error | null;
  onRetry?: () => void;
  viewMode?: ViewMode;
  onViewModeChange?: (mode: ViewMode) => void;
  /** Hide the header (view mode selector, stats, controls). Used when embedding in a parent with its own header. */
  hideHeader?: boolean;
}

export function TraceView({
  projectId,
  traceId,
  spans,
  durationMs,
  tokenBreakdown,
  costBreakdown,
  isLoading = false,
  error,
  onRetry,
  viewMode: controlledViewMode,
  onViewModeChange,
  hideHeader = false,
}: TraceViewProps) {
  // Loading state - render nothing, parent handles loading indicator
  if (isLoading) {
    return null;
  }

  if (error) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-4 p-8">
        <AlertCircle className="h-12 w-12 text-destructive" />
        <div className="text-center">
          <h3 className="font-medium">Failed to load trace</h3>
          <p className="text-sm text-muted-foreground">{error.message}</p>
        </div>
        {onRetry && (
          <Button variant="outline" size="sm" onClick={onRetry}>
            <RefreshCw className="mr-2 h-4 w-4" />
            Retry
          </Button>
        )}
      </div>
    );
  }

  if (!spans?.length) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-4 p-8">
        <AlertCircle className="h-12 w-12 text-muted-foreground/50" />
        <div className="text-center">
          <h3 className="font-medium text-muted-foreground">No spans found</h3>
          <p className="text-sm text-muted-foreground">This trace does not contain any spans.</p>
        </div>
      </div>
    );
  }

  return (
    <TooltipProvider>
      <TraceViewProvider
        spans={spans}
        initialViewMode={controlledViewMode}
        onViewModeChange={onViewModeChange}
      >
        <TraceViewContent
          projectId={projectId}
          traceId={traceId}
          durationMs={durationMs}
          tokenBreakdown={tokenBreakdown}
          costBreakdown={costBreakdown}
          hideHeader={hideHeader}
        />
      </TraceViewProvider>
    </TooltipProvider>
  );
}

function getDefaultLayout(viewMode: ViewMode): LayoutDirection {
  return viewMode === "tree" ? "horizontal" : "vertical";
}

function TraceViewContent({
  projectId,
  traceId,
  durationMs,
  tokenBreakdown,
  costBreakdown,
  hideHeader,
}: {
  projectId: string;
  traceId: string;
  durationMs?: number | null;
  tokenBreakdown?: TokenBreakdown;
  costBreakdown?: CostBreakdown;
  hideHeader?: boolean;
}) {
  const {
    viewMode,
    setViewMode,
    allExpanded,
    expandAll,
    collapseAll,
    showNonGenAiSpans,
    setShowNonGenAiSpans,
  } = useTraceView();

  // Load persisted layout for current view mode, fallback to default
  const [layoutDirection, setLayoutDirection] = useState<LayoutDirection>(() => {
    const saved = settings.get<LayoutDirection>(getTraceViewLayoutKey(viewMode));
    return saved ?? getDefaultLayout(viewMode);
  });

  // Update layout when view mode changes - load saved preference or use default
  useEffect(() => {
    const saved = settings.get<LayoutDirection>(getTraceViewLayoutKey(viewMode));
    setLayoutDirection(saved ?? getDefaultLayout(viewMode));
  }, [viewMode]);

  // Persist layout when user changes it
  const handleLayoutDirectionChange = useCallback(
    (direction: LayoutDirection) => {
      setLayoutDirection(direction);
      settings.set(getTraceViewLayoutKey(viewMode), direction);
    },
    [viewMode],
  );

  const handleToggleExpandAll = () => {
    if (allExpanded) {
      collapseAll();
    } else {
      expandAll();
    }
  };

  const renderViewContent = () => {
    switch (viewMode) {
      case "tree":
        return <SpanTree />;
      case "timeline":
        return <SpanTimeline />;
      case "diagram":
        return <SpanDiagram />;
      default:
        return <SpanTree />;
    }
  };

  return (
    <div className="@container flex h-full min-w-0 flex-col overflow-hidden">
      {!hideHeader && (
        <TraceViewHeader
          viewMode={viewMode}
          onViewModeChange={setViewMode}
          layoutDirection={layoutDirection}
          onLayoutDirectionChange={handleLayoutDirectionChange}
          duration={durationMs ?? 0}
          tokenBreakdown={tokenBreakdown}
          costBreakdown={costBreakdown}
          allExpanded={allExpanded}
          onToggleExpandAll={handleToggleExpandAll}
          showNonGenAiSpans={showNonGenAiSpans}
          onShowNonGenAiSpansChange={setShowNonGenAiSpans}
        />
      )}

      <ResizablePanelGroup
        direction={layoutDirection}
        className="flex-1"
        autoSaveId={`trace-view-panels-${layoutDirection}`}
      >
        <ResizablePanel defaultSize={40} minSize={20}>
          {renderViewContent()}
        </ResizablePanel>

        <ResizableHandle withHandle />

        <ResizablePanel defaultSize={60} minSize={30}>
          <SpanDetailPanel projectId={projectId} traceId={traceId} />
        </ResizablePanel>
      </ResizablePanelGroup>
    </div>
  );
}
