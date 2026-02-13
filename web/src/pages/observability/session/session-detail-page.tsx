import { useCallback, useMemo, useState } from "react";
import { useParams } from "react-router";
import { useQueryParam, StringParam } from "use-query-params";
import { MessageSquare, RefreshCw, ChevronsUpDown, Check, GitBranch, Braces } from "lucide-react";
import { useCheckFavorites, useToggleFavorite } from "@/api/favorites/hooks";
import { FavoriteButton } from "@/components/favorite-button";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useCurrentProject } from "@/hooks/use-project";
import { SessionDetail, type SessionTab } from "./session-detail";
import type { ThreadTab } from "@/components/thread";
import type { ViewMode } from "@/components/trace-view/lib/types";
import { cn } from "@/lib/utils";

const TABS: { value: SessionTab; label: string; icon: React.ReactNode }[] = [
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

export default function SessionDetailPage() {
  const { sessionId } = useParams<{ sessionId: string }>();
  const { projectId } = useCurrentProject();
  const [activeTab, setActiveTab] = useQueryParam("tab", StringParam);
  const [threadTab, setThreadTab] = useQueryParam("threadTab", StringParam);
  const [traceTab, setTraceTab] = useQueryParam("traceTab", StringParam);
  const [refetch, setRefetch] = useState<(() => void) | null>(null);
  const [isRefreshing, setIsRefreshing] = useState(false);

  const effectiveActiveTab = isSessionTab(activeTab) ? activeTab : "thread";
  const effectiveThreadTab = isThreadTab(threadTab) ? threadTab : "messages";
  const effectiveTraceTab = isViewMode(traceTab) ? traceTab : "tree";
  const activeTabConfig = TABS.find((t) => t.value === effectiveActiveTab);

  // Favorites
  const sessionIds = useMemo(() => (sessionId ? [sessionId] : []), [sessionId]);
  const { data: favoriteIds } = useCheckFavorites(projectId, "session", sessionIds);
  const { mutate: toggleFavorite } = useToggleFavorite();
  const isFavorite = sessionId ? (favoriteIds?.has(sessionId) ?? false) : false;

  const handleToggleFavorite = useCallback(() => {
    if (!sessionId) return;
    toggleFavorite({
      projectId,
      entityType: "session",
      entityId: sessionId,
      isFavorite,
    });
  }, [sessionId, projectId, isFavorite, toggleFavorite]);

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
      if (tab === "thread") setThreadTab(undefined);
      if (tab !== "trace") setTraceTab(undefined);
    },
    [setActiveTab, setThreadTab, setTraceTab],
  );

  const handleRefreshChange = useCallback((refetchFn: (() => void) | null, refreshing: boolean) => {
    setRefetch(() => refetchFn);
    setIsRefreshing(refreshing);
  }, []);

  if (!sessionId) {
    return (
      <div className="flex h-screen items-center justify-center text-muted-foreground">
        No session ID provided
      </div>
    );
  }

  return (
    <div className="h-screen w-full mx-auto pt-header-offset sm:pt-header-offset-sm px-2 sm:px-4 overflow-hidden">
      <div className="flex h-full w-full min-w-0 flex-col overflow-hidden pb-3">
        {/* Toolbar */}
        <div className="@container flex shrink-0 min-w-0 items-center justify-between gap-3 pb-4">
          {/* Tabs - dropdown on narrow containers, inline on wider */}
          <div className="@[500px]:hidden">
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button variant="outline" size="sm" className="h-9 gap-2 px-3">
                  {activeTabConfig?.icon}
                  <span>{activeTabConfig?.label}</span>
                  <ChevronsUpDown className="h-3.5 w-3.5 opacity-50" />
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

          {/* Inline tabs for wider containers */}
          <div className="hidden h-9 items-center rounded-lg bg-muted p-1 @[500px]:flex">
            {TABS.map((tab) => (
              <button
                key={tab.value}
                type="button"
                onClick={() => handleActiveTabChange(tab.value)}
                className={cn(
                  "relative flex h-7 items-center justify-center gap-2 rounded-md px-3 text-sm font-medium transition-all",
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

          {/* Right side - session ID, favorite, and refresh */}
          <div className="flex items-center gap-2">
            <code className="hidden truncate rounded border border-border/50 bg-muted px-2 py-1 font-mono text-xs text-muted-foreground @[600px]:block @[600px]:max-w-[200px] @[800px]:max-w-[300px] @[1000px]:max-w-none">
              {sessionId}
            </code>
            <FavoriteButton
              isFavorite={isFavorite}
              disabled={!sessionId}
              onToggle={handleToggleFavorite}
              className="h-9 w-9"
            />
            <Button
              variant="outline"
              size="sm"
              className="h-9 w-9 shrink-0 p-0"
              onClick={() => refetch?.()}
              disabled={!refetch || isRefreshing}
              aria-label="Refresh"
            >
              <RefreshCw className={cn("h-4 w-4", isRefreshing && "animate-spin")} />
            </Button>
          </div>
        </div>

        {/* Content */}
        <div className="min-h-0 flex-1 overflow-hidden rounded-lg border">
          <SessionDetail
            sessionId={sessionId}
            projectId={projectId}
            activeTab={effectiveActiveTab}
            threadTab={effectiveThreadTab}
            onThreadTabChange={handleThreadTabChange}
            traceTab={effectiveTraceTab}
            onTraceTabChange={handleTraceTabChange}
            onRefreshChange={handleRefreshChange}
          />
        </div>
      </div>
    </div>
  );
}
