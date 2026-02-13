import { useCallback, useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router";
import { useQueryParam, StringParam } from "use-query-params";
import {
  ChevronUp,
  ChevronDown,
  Maximize2,
  ExternalLink,
  X,
  RefreshCw,
  ChevronsUpDown,
  Check,
} from "lucide-react";
import { useCheckSpanFavorites, useToggleFavorite } from "@/api/favorites/hooks";
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
import { SpanDetail, SPAN_TABS, type SpanTab } from "./span-detail";
import type { ThreadTab } from "@/components/thread";
import type { SpanSummary } from "@/api/otel/types";
import { cn } from "@/lib/utils";

interface SpanDetailSheetProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  span: SpanSummary | null;
  projectId: string;
  onNavigatePrev?: () => void;
  onNavigateNext?: () => void;
  hasPrev?: boolean;
  hasNext?: boolean;
  realtimeEnabled?: boolean;
}

export function SpanDetailSheet({
  open,
  onOpenChange,
  span,
  projectId,
  onNavigatePrev,
  onNavigateNext,
  hasPrev = false,
  hasNext = false,
  realtimeEnabled = true,
}: SpanDetailSheetProps) {
  const navigate = useNavigate();
  const [activeTab, setActiveTab] = useQueryParam("tab", StringParam);
  const [threadTab, setThreadTab] = useQueryParam("threadTab", StringParam);
  const [refreshFn, setRefreshFn] = useState<(() => void) | null>(null);
  const [isRefreshing, setIsRefreshing] = useState(false);

  const effectiveActiveTab = activeTab ?? "overview";
  const effectiveThreadTab = threadTab ?? "messages";
  const activeTabConfig = SPAN_TABS.find((t) => t.value === effectiveActiveTab);

  // Favorites (spans use composite keys: trace_id:span_id)
  const spanIdentifiers = useMemo(
    () => (span ? [{ trace_id: span.trace_id, span_id: span.span_id }] : []),
    [span],
  );
  const { data: favoriteIds } = useCheckSpanFavorites(projectId, spanIdentifiers);
  const { mutate: toggleFavorite } = useToggleFavorite();
  const compositeId = span ? `${span.trace_id}:${span.span_id}` : "";
  const isFavorite = favoriteIds?.has(compositeId) ?? false;

  const handleToggleFavorite = useCallback(() => {
    if (!span) return;
    toggleFavorite({
      projectId,
      entityType: "span",
      entityId: span.trace_id,
      secondaryId: span.span_id,
      isFavorite,
    });
  }, [span, projectId, isFavorite, toggleFavorite]);

  const handleThreadTabChange = useCallback(
    (tab: ThreadTab) => {
      setThreadTab(tab);
    },
    [setThreadTab],
  );

  const handleActiveTabChange = useCallback(
    (tab: SpanTab) => {
      setActiveTab(tab);
      if (tab === "messages") {
        setThreadTab(undefined);
      }
    },
    [setActiveTab, setThreadTab],
  );

  const handleClose = useCallback(() => {
    setActiveTab(undefined);
    setThreadTab(undefined);
    onOpenChange(false);
  }, [setActiveTab, setThreadTab, onOpenChange]);

  const handleRefreshChange = useCallback((refetch: (() => void) | null, refreshing: boolean) => {
    setRefreshFn(() => refetch);
    setIsRefreshing(refreshing);
  }, []);

  const handleOpenFullPage = useCallback(() => {
    if (!span) return;
    navigate(`/projects/${projectId}/observability/spans/${span.trace_id}/${span.span_id}`);
    handleClose();
  }, [span, projectId, navigate, handleClose]);

  const handleOpenNewWindow = useCallback(() => {
    if (!span) return;
    window.open(
      `/ui/projects/${projectId}/observability/spans/${span.trace_id}/${span.span_id}`,
      "_blank",
      "noopener,noreferrer",
    );
  }, [span, projectId]);

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
          if (span) {
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
    span,
    handleOpenFullPage,
    handleClose,
  ]);

  return (
    <ResizableSheet
      open={open}
      storageKey="span-sheet-width"
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
                {activeTabConfig?.icon}
                <span className="text-xs">{activeTabConfig?.label}</span>
                <ChevronsUpDown className="h-3 w-3 opacity-50" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start">
              {SPAN_TABS.map((tab) => (
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
          {SPAN_TABS.map((tab) => (
            <button
              key={tab.value}
              type="button"
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

        {/* Span ID - hidden on narrow container */}
        {span && (
          <code className="hidden truncate rounded border border-border/50 bg-background/80 px-2 py-0.5 font-mono text-[10px] text-muted-foreground @[600px]:block @[600px]:max-w-[180px] @[800px]:max-w-[280px] @[1000px]:max-w-none">
            {span.span_id}
          </code>
        )}

        {/* Hidden title for accessibility */}
        <SheetTitle className="sr-only">Span Details</SheetTitle>

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
                    disabled={!span}
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
                    disabled={!span}
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
                    disabled={!span}
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
        {span ? (
          <SpanDetail
            traceId={span.trace_id}
            spanId={span.span_id}
            projectId={projectId}
            activeTab={effectiveActiveTab as SpanTab}
            threadTab={effectiveThreadTab as ThreadTab}
            onThreadTabChange={handleThreadTabChange}
            realtimeEnabled={realtimeEnabled}
            onRefreshChange={handleRefreshChange}
          />
        ) : (
          <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
            No span selected
          </div>
        )}
      </div>
    </ResizableSheet>
  );
}
