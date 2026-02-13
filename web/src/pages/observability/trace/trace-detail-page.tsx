import { useCallback, useMemo, useState } from "react";
import { useParams } from "react-router";
import { useQueryParam, StringParam } from "use-query-params";
import { MessageSquare, Activity, FileJson, RefreshCw, ChevronsUpDown, Check } from "lucide-react";
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
import { TraceDetail, type TraceTab } from "./trace-detail";
import type { ViewMode } from "@/components/trace-view/lib/types";
import type { ThreadTab } from "@/components/thread";
import { cn } from "@/lib/utils";

const TABS: { value: TraceTab; label: string; icon: React.ReactNode }[] = [
  { value: "thread", label: "Thread", icon: <MessageSquare className="h-4 w-4" /> },
  { value: "trace", label: "Trace", icon: <Activity className="h-4 w-4" /> },
  { value: "raw", label: "Raw", icon: <FileJson className="h-4 w-4" /> },
];

export default function TraceDetailPage() {
  const { traceId } = useParams<{ traceId: string }>();
  const { projectId } = useCurrentProject();
  const [activeTab, setActiveTab] = useQueryParam("tab", StringParam);
  const [traceTab, setTraceTab] = useQueryParam("traceTab", StringParam);
  const [threadTab, setThreadTab] = useQueryParam("threadTab", StringParam);
  const [refetch, setRefetch] = useState<(() => void) | null>(null);
  const [isRefreshing, setIsRefreshing] = useState(false);

  const effectiveActiveTab = activeTab ?? "thread";
  const effectiveTraceTab = traceTab ?? "tree";
  const effectiveThreadTab = threadTab ?? "messages";
  const activeTabConfig = TABS.find((t) => t.value === effectiveActiveTab);

  // Favorites
  const traceIds = useMemo(() => (traceId ? [traceId] : []), [traceId]);
  const { data: favoriteIds } = useCheckFavorites(projectId, "trace", traceIds);
  const { mutate: toggleFavorite } = useToggleFavorite();
  const isFavorite = traceId ? (favoriteIds?.has(traceId) ?? false) : false;

  const handleToggleFavorite = useCallback(() => {
    if (!traceId) return;
    toggleFavorite({
      projectId,
      entityType: "trace",
      entityId: traceId,
      isFavorite,
    });
  }, [traceId, projectId, isFavorite, toggleFavorite]);

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

  const handleRefreshChange = useCallback((refetchFn: (() => void) | null, refreshing: boolean) => {
    setRefetch(() => refetchFn);
    setIsRefreshing(refreshing);
  }, []);

  if (!traceId) {
    return (
      <div className="flex h-screen items-center justify-center text-muted-foreground">
        No trace ID provided
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
                    onClick={() => setActiveTab(tab.value)}
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
                onClick={() => setActiveTab(tab.value)}
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

          {/* Right side - trace ID, favorite, and refresh */}
          <div className="flex items-center gap-2">
            <code className="hidden truncate rounded border border-border/50 bg-muted px-2 py-1 font-mono text-xs text-muted-foreground @[600px]:block @[600px]:max-w-[200px] @[800px]:max-w-[300px] @[1000px]:max-w-none">
              {traceId}
            </code>
            <FavoriteButton
              isFavorite={isFavorite}
              disabled={!traceId}
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
          <TraceDetail
            traceId={traceId}
            projectId={projectId}
            activeTab={effectiveActiveTab as TraceTab}
            traceTab={effectiveTraceTab as ViewMode}
            onTraceTabChange={handleTraceTabChange}
            threadTab={effectiveThreadTab as ThreadTab}
            onThreadTabChange={handleThreadTabChange}
            onRefreshChange={handleRefreshChange}
          />
        </div>
      </div>
    </div>
  );
}
