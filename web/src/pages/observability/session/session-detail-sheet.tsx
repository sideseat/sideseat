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
  RefreshCw,
  ChevronsUpDown,
  Check,
  GitBranch,
  Braces,
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
import { SessionDetail, type SessionTab } from "./session-detail";
import type { ThreadTab } from "@/components/thread";
import type { ViewMode } from "@/components/trace-view/lib/types";
import type { SessionSummary } from "@/api/otel/types";
import { cn } from "@/lib/utils";

const TABS: { value: SessionTab; label: string; icon: ReactNode }[] = [
  { value: "thread", label: "Thread", icon: <MessageSquare className="h-4 w-4" /> },
  { value: "trace", label: "Trace", icon: <GitBranch className="h-4 w-4" /> },
  { value: "raw", label: "Raw", icon: <Braces className="h-4 w-4" /> },
];

const SESSION_TABS = new Set<SessionTab>(["thread", "trace", "raw"]);
const THREAD_TABS = new Set<ThreadTab>(["messages", "tools"]);
const VIEW_MODES = new Set<ViewMode>(["tree", "timeline", "diagram"]);

function isSessionTab(value: string | null | undefined): value is SessionTab {
  return typeof value === "string" && SESSION_TABS.has(value as SessionTab);
}
function isThreadTab(value: string | null | undefined): value is ThreadTab {
  return typeof value === "string" && THREAD_TABS.has(value as ThreadTab);
}
function isViewMode(value: string | null | undefined): value is ViewMode {
  return typeof value === "string" && VIEW_MODES.has(value as ViewMode);
}

interface SessionDetailSheetProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  session: SessionSummary | null;
  projectId: string;
  onNavigatePrev?: () => void;
  onNavigateNext?: () => void;
  hasPrev?: boolean;
  hasNext?: boolean;
  /** Whether to enable SSE subscription in SessionDetail. Set to false if parent handles SSE. */
  realtimeEnabled?: boolean;
}

export function SessionDetailSheet({
  open,
  onOpenChange,
  session,
  projectId,
  onNavigatePrev,
  onNavigateNext,
  hasPrev = false,
  hasNext = false,
  realtimeEnabled = true,
}: SessionDetailSheetProps) {
  const navigate = useNavigate();
  const [activeTab, setActiveTab] = useQueryParam("tab", StringParam);
  const [threadTab, setThreadTab] = useQueryParam("threadTab", StringParam);
  const [traceTab, setTraceTab] = useQueryParam("traceTab", StringParam);
  const [refreshFn, setRefreshFn] = useState<(() => void) | null>(null);
  const [isRefreshing, setIsRefreshing] = useState(false);

  const effectiveActiveTab = isSessionTab(activeTab) ? activeTab : "thread";
  const effectiveThreadTab = isThreadTab(threadTab) ? threadTab : "messages";
  const effectiveTraceTab = isViewMode(traceTab) ? traceTab : "tree";
  const activeTabConfig = TABS.find((t) => t.value === effectiveActiveTab);

  // Favorites
  const sessionIds = useMemo(() => (session ? [session.session_id] : []), [session]);
  const { data: favoriteIds } = useCheckFavorites(projectId, "session", sessionIds);
  const { mutate: toggleFavorite } = useToggleFavorite();
  const isFavorite = session ? (favoriteIds?.has(session.session_id) ?? false) : false;

  const handleToggleFavorite = useCallback(() => {
    if (!session) return;
    toggleFavorite({
      projectId,
      entityType: "session",
      entityId: session.session_id,
      isFavorite,
    });
  }, [session, projectId, isFavorite, toggleFavorite]);

  const handleThreadTabChange = useCallback(
    (tab: ThreadTab) => {
      setThreadTab(tab);
    },
    [setThreadTab],
  );

  const handleTraceTabChange = useCallback(
    (tab: ViewMode) => {
      setTraceTab(tab);
    },
    [setTraceTab],
  );

  const handleActiveTabChange = useCallback(
    (tab: SessionTab) => {
      setActiveTab(tab);
      // Reset sub-tabs to defaults when main tab changes
      if (tab === "thread") {
        setThreadTab(undefined);
      }
      if (tab !== "trace") {
        setTraceTab(undefined);
      }
    },
    [setActiveTab, setThreadTab, setTraceTab],
  );

  const handleClose = useCallback(() => {
    // Reset all tabs when closing
    setActiveTab(undefined);
    setThreadTab(undefined);
    setTraceTab(undefined);
    onOpenChange(false);
  }, [setActiveTab, setThreadTab, setTraceTab, onOpenChange]);

  const handleRefreshChange = useCallback((refetch: (() => void) | null, refreshing: boolean) => {
    setRefreshFn(() => refetch);
    setIsRefreshing(refreshing);
  }, []);

  const handleOpenFullPage = useCallback(() => {
    if (!session) return;
    navigate(`/projects/${projectId}/observability/sessions/${session.session_id}`);
    handleClose();
  }, [session, projectId, navigate, handleClose]);

  const handleOpenNewWindow = useCallback(() => {
    if (!session) return;
    window.open(
      `/ui/projects/${projectId}/observability/sessions/${session.session_id}`,
      "_blank",
      "noopener,noreferrer",
    );
  }, [session, projectId]);

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
          if (session) {
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
    session,
    handleOpenFullPage,
    handleClose,
  ]);

  return (
    <ResizableSheet
      open={open}
      storageKey="session-sheet-width"
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

        {/* Session ID - hidden on narrow container */}
        {session && (
          <code className="hidden truncate rounded border border-border/50 bg-background/80 px-2 py-0.5 font-mono text-[10px] text-muted-foreground @[600px]:block @[600px]:max-w-[180px] @[800px]:max-w-[280px] @[1000px]:max-w-none">
            {session.session_id}
          </code>
        )}

        {/* Hidden title for accessibility */}
        <SheetTitle className="sr-only">Session Details</SheetTitle>

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
                    disabled={!session}
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
                    disabled={!session}
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
                    disabled={!session}
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
        {session ? (
          <SessionDetail
            sessionId={session.session_id}
            projectId={projectId}
            activeTab={effectiveActiveTab}
            threadTab={effectiveThreadTab}
            onThreadTabChange={handleThreadTabChange}
            traceTab={effectiveTraceTab}
            onTraceTabChange={handleTraceTabChange}
            realtimeEnabled={realtimeEnabled}
            onRefreshChange={handleRefreshChange}
          />
        ) : (
          <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
            No session selected
          </div>
        )}
      </div>
    </ResizableSheet>
  );
}
