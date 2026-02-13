import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import { useNavigate } from "react-router";
import { useQueryParam, StringParam } from "use-query-params";
import {
  ChevronUp,
  ChevronDown,
  Maximize2,
  ExternalLink,
  X,
  MessageSquare,
  Activity,
  RefreshCw,
  Braces,
  ChevronsUpDown,
  Check,
} from "lucide-react";
import { useCheckFavorites, useToggleFavorite } from "@/api/favorites/hooks";
import { FavoriteButton } from "@/components/favorite-button";
import { SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { ResizableSheet } from "@/components/resizable-sheet";
import { Button } from "@/components/ui/button";
import { Kbd } from "@/components/ui/kbd";
import { Tooltip, TooltipContent, TooltipTrigger, TooltipProvider } from "@/components/ui/tooltip";
import { Separator } from "@/components/ui/separator";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { TraceDetail, type TraceTab } from "./trace-detail";
import type { ViewMode } from "@/components/trace-view/lib/types";
import type { ThreadTab } from "@/components/thread";
import type { TraceSummary } from "@/api/otel/types";
import { cn } from "@/lib/utils";

const TABS: { value: TraceTab; label: string; icon: ReactNode }[] = [
  { value: "thread", label: "Thread", icon: <MessageSquare className="h-4 w-4" /> },
  { value: "trace", label: "Trace", icon: <Activity className="h-4 w-4" /> },
  { value: "raw", label: "Raw", icon: <Braces className="h-4 w-4" /> },
];

interface TraceDetailSheetProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  trace: TraceSummary | null;
  projectId: string;
  onNavigatePrev?: () => void;
  onNavigateNext?: () => void;
  hasPrev?: boolean;
  hasNext?: boolean;
  /** Whether to enable SSE subscription in TraceDetail. Set to false if parent handles SSE. */
  realtimeEnabled?: boolean;
}

export function TraceDetailSheet({
  open,
  onOpenChange,
  trace,
  projectId,
  onNavigatePrev,
  onNavigateNext,
  hasPrev = false,
  hasNext = false,
  realtimeEnabled = true,
}: TraceDetailSheetProps) {
  const navigate = useNavigate();
  const [activeTab, setActiveTab] = useQueryParam("tab", StringParam);
  const [traceTab, setTraceTab] = useQueryParam("traceTab", StringParam);
  const [threadTab, setThreadTab] = useQueryParam("threadTab", StringParam);
  const [refreshFn, setRefreshFn] = useState<(() => void) | null>(null);
  const [isRefreshing, setIsRefreshing] = useState(false);

  const effectiveActiveTab = activeTab ?? "thread";
  const effectiveTraceTab = traceTab ?? "tree";
  const effectiveThreadTab = threadTab ?? "messages";

  // Favorites
  const traceIds = useMemo(() => (trace ? [trace.trace_id] : []), [trace]);
  const { data: favoriteIds } = useCheckFavorites(projectId, "trace", traceIds);
  const { mutate: toggleFavorite } = useToggleFavorite();
  const isFavorite = trace ? (favoriteIds?.has(trace.trace_id) ?? false) : false;

  const handleToggleFavorite = useCallback(() => {
    if (!trace) return;
    toggleFavorite({
      projectId,
      entityType: "trace",
      entityId: trace.trace_id,
      isFavorite,
    });
  }, [trace, projectId, isFavorite, toggleFavorite]);

  const handleTraceTabChange = useCallback(
    (tab: ViewMode) => {
      setTraceTab(tab);
    },
    [setTraceTab],
  );

  const handleThreadTabChange = useCallback(
    (tab: ThreadTab) => {
      setThreadTab(tab);
    },
    [setThreadTab],
  );

  const handleActiveTabChange = useCallback(
    (tab: TraceTab) => {
      setActiveTab(tab);
      // Reset sub-tabs to defaults when main tab changes
      if (tab === "trace") {
        setTraceTab(undefined);
      } else if (tab === "thread") {
        setThreadTab(undefined);
      }
    },
    [setActiveTab, setTraceTab, setThreadTab],
  );

  const handleClose = useCallback(() => {
    // Reset all tabs when closing
    setActiveTab(undefined);
    setTraceTab(undefined);
    setThreadTab(undefined);
    onOpenChange(false);
  }, [setActiveTab, setTraceTab, setThreadTab, onOpenChange]);

  const handleRefreshChange = useCallback((refetch: (() => void) | null, refreshing: boolean) => {
    setRefreshFn(() => refetch);
    setIsRefreshing(refreshing);
  }, []);

  const handleOpenFullPage = useCallback(() => {
    if (!trace) return;
    navigate(`/projects/${projectId}/observability/traces/${trace.trace_id}`);
    handleClose();
  }, [trace, projectId, navigate, handleClose]);

  const handleOpenNewWindow = useCallback(() => {
    if (!trace) return;
    window.open(
      `/ui/projects/${projectId}/observability/traces/${trace.trace_id}`,
      "_blank",
      "noopener,noreferrer",
    );
  }, [trace, projectId]);

  useEffect(() => {
    if (!open) return;

    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) {
        return;
      }

      switch (e.key) {
        case "ArrowUp":
        case "k":
          if (hasPrev && onNavigatePrev) {
            e.preventDefault();
            onNavigatePrev();
          }
          break;
        case "ArrowDown":
        case "j":
          if (hasNext && onNavigateNext) {
            e.preventDefault();
            onNavigateNext();
          }
          break;
        case "Enter":
          if (trace) {
            e.preventDefault();
            handleOpenFullPage();
          }
          break;
        case "Escape":
          e.preventDefault();
          handleClose();
          break;
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [
    open,
    hasPrev,
    hasNext,
    onNavigatePrev,
    onNavigateNext,
    trace,
    handleOpenFullPage,
    handleClose,
  ]);

  return (
    <ResizableSheet
      open={open}
      storageKey="trace-sheet-width"
      defaultWidth={800}
      minWidth={400}
      maxWidth={1400}
      onInteractOutside={(e) => e.preventDefault()}
      onPointerDownOutside={(e) => e.preventDefault()}
    >
      <SheetHeader className="@container flex h-11 shrink-0 flex-row items-center gap-2 border-b bg-muted/40 px-2 sm:h-12 sm:gap-3 sm:px-4">
        {/* Dropdown menu for narrow container */}
        <div className="@[600px]:hidden">
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="outline" size="sm" className="h-8 gap-1.5 px-2">
                {TABS.find((t) => t.value === effectiveActiveTab)?.icon}
                <span className="text-xs">
                  {TABS.find((t) => t.value === effectiveActiveTab)?.label}
                </span>
                <ChevronsUpDown className="h-3 w-3 opacity-50" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start">
              {TABS.map((tab) => (
                <DropdownMenuItem
                  key={tab.value}
                  onClick={() => handleActiveTabChange(tab.value)}
                  className="gap-2"
                >
                  {tab.icon}
                  {tab.label}
                  {effectiveActiveTab === tab.value && <Check className="ml-auto h-4 w-4" />}
                </DropdownMenuItem>
              ))}
            </DropdownMenuContent>
          </DropdownMenu>
        </div>

        {/* Inline tabs for wider container */}
        <div className="hidden h-9 items-center rounded-lg bg-muted/60 p-1 @[600px]:flex">
          {TABS.map((tab) => (
            <button
              key={tab.value}
              onClick={() => handleActiveTabChange(tab.value)}
              className={cn(
                "relative flex h-7 items-center justify-center gap-1.5 rounded-md px-3 text-sm font-medium transition-all",
                "focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
                effectiveActiveTab === tab.value
                  ? "bg-background text-foreground shadow-sm"
                  : "text-muted-foreground hover:text-foreground",
              )}
            >
              {tab.icon}
              {tab.label}
            </button>
          ))}
        </div>

        <Separator orientation="vertical" className="hidden h-5 @[600px]:block" />

        {/* Trace ID - hidden on narrow container */}
        {trace && (
          <code className="hidden truncate rounded border border-border/50 bg-background/80 px-2 py-0.5 font-mono text-[10px] text-muted-foreground @[600px]:block @[600px]:max-w-[180px] @[800px]:max-w-[280px] @[1000px]:max-w-none">
            {trace.trace_id}
          </code>
        )}

        {/* Hidden title for accessibility */}
        <SheetTitle className="sr-only">Trace Details</SheetTitle>

        <div className="ml-auto flex items-center gap-1">
          <TooltipProvider delayDuration={500} skipDelayDuration={0}>
            {/* Navigation and refresh controls */}
            <div className="flex items-center gap-0.5 rounded-md border border-border/50 bg-background/80 p-0.5">
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    className="h-6 w-6 sm:h-7 sm:w-7"
                    disabled={!hasPrev}
                    onClick={onNavigatePrev}
                  >
                    <ChevronUp className="h-3 w-3 sm:h-3.5 sm:w-3.5" />
                  </Button>
                </TooltipTrigger>
                <TooltipContent side="bottom" className="flex items-center gap-2">
                  Previous
                  <Kbd>↑</Kbd>
                </TooltipContent>
              </Tooltip>

              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    className="h-6 w-6 sm:h-7 sm:w-7"
                    disabled={!hasNext}
                    onClick={onNavigateNext}
                  >
                    <ChevronDown className="h-3 w-3 sm:h-3.5 sm:w-3.5" />
                  </Button>
                </TooltipTrigger>
                <TooltipContent side="bottom" className="flex items-center gap-2">
                  Next
                  <Kbd>↓</Kbd>
                </TooltipContent>
              </Tooltip>

              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    className="h-6 w-6 sm:h-7 sm:w-7"
                    disabled={!refreshFn || isRefreshing}
                    onClick={() => refreshFn?.()}
                  >
                    <RefreshCw
                      className={cn("h-3 w-3 sm:h-3.5 sm:w-3.5", isRefreshing && "animate-spin")}
                    />
                  </Button>
                </TooltipTrigger>
                <TooltipContent side="bottom">Refresh</TooltipContent>
              </Tooltip>
            </div>

            <Separator orientation="vertical" className="mx-0.5 h-5 sm:mx-1" />

            {/* Action buttons */}
            <div className="flex items-center gap-0.5">
              <Tooltip>
                <TooltipTrigger asChild>
                  <FavoriteButton
                    isFavorite={isFavorite}
                    disabled={!trace}
                    onToggle={handleToggleFavorite}
                    className="h-6 w-6 sm:h-7 sm:w-7"
                  />
                </TooltipTrigger>
                <TooltipContent side="bottom">
                  {isFavorite ? "Remove from favorites" : "Add to favorites"}
                </TooltipContent>
              </Tooltip>

              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    className="h-6 w-6 sm:h-7 sm:w-7"
                    onClick={handleOpenFullPage}
                    disabled={!trace}
                  >
                    <Maximize2 className="h-3 w-3 sm:h-3.5 sm:w-3.5" />
                  </Button>
                </TooltipTrigger>
                <TooltipContent side="bottom" className="flex items-center gap-2">
                  Open full page
                  <Kbd>Enter</Kbd>
                </TooltipContent>
              </Tooltip>

              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    className="hidden h-6 w-6 sm:inline-flex sm:h-7 sm:w-7"
                    onClick={handleOpenNewWindow}
                    disabled={!trace}
                  >
                    <ExternalLink className="h-3 w-3 sm:h-3.5 sm:w-3.5" />
                  </Button>
                </TooltipTrigger>
                <TooltipContent side="bottom">Open in new window</TooltipContent>
              </Tooltip>

              <Button
                variant="ghost"
                size="icon-sm"
                className="h-6 w-6 sm:h-7 sm:w-7"
                onClick={handleClose}
                autoFocus
                aria-label="Close"
              >
                <X className="h-3 w-3 sm:h-3.5 sm:w-3.5" />
              </Button>
            </div>
          </TooltipProvider>
        </div>
      </SheetHeader>

      <div className="flex-1 overflow-hidden">
        {trace ? (
          <TraceDetail
            traceId={trace.trace_id}
            projectId={projectId}
            activeTab={effectiveActiveTab as TraceTab}
            traceTab={effectiveTraceTab as ViewMode}
            onTraceTabChange={handleTraceTabChange}
            threadTab={effectiveThreadTab as ThreadTab}
            onThreadTabChange={handleThreadTabChange}
            realtimeEnabled={realtimeEnabled}
            onRefreshChange={handleRefreshChange}
          />
        ) : (
          <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
            No trace selected
          </div>
        )}
      </div>
    </ResizableSheet>
  );
}
