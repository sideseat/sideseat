import { useCallback, useMemo, useState } from "react";
import { useParams } from "react-router";
import { useQueryParam, StringParam } from "use-query-params";
import { RefreshCw, ChevronsUpDown, Check } from "lucide-react";
import { useCheckSpanFavorites, useToggleFavorite } from "@/api/favorites/hooks";
import { FavoriteButton } from "@/components/favorite-button";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useCurrentProject } from "@/hooks/use-project";
import { SpanDetail, SPAN_TABS, type SpanTab } from "./span-detail";
import type { ThreadTab } from "@/components/thread";
import { cn } from "@/lib/utils";

export default function SpanDetailPage() {
  const { traceId, spanId } = useParams<{ traceId: string; spanId: string }>();
  const { projectId } = useCurrentProject();
  const [activeTab, setActiveTab] = useQueryParam("tab", StringParam);
  const [threadTab, setThreadTab] = useQueryParam("threadTab", StringParam);
  const [refetch, setRefetch] = useState<(() => void) | null>(null);
  const [isRefreshing, setIsRefreshing] = useState(false);

  const effectiveActiveTab = activeTab ?? "overview";
  const effectiveThreadTab = threadTab ?? "messages";
  const activeTabConfig = SPAN_TABS.find((t) => t.value === effectiveActiveTab);

  // Favorites (spans use composite keys: trace_id:span_id)
  const spanIdentifiers = useMemo(
    () => (traceId && spanId ? [{ trace_id: traceId, span_id: spanId }] : []),
    [traceId, spanId],
  );
  const { data: favoriteIds } = useCheckSpanFavorites(projectId, spanIdentifiers);
  const { mutate: toggleFavorite } = useToggleFavorite();
  const compositeId = traceId && spanId ? `${traceId}:${spanId}` : "";
  const isFavorite = favoriteIds?.has(compositeId) ?? false;

  const handleToggleFavorite = useCallback(() => {
    if (!traceId || !spanId) return;
    toggleFavorite({
      projectId,
      entityType: "span",
      entityId: traceId,
      secondaryId: spanId,
      isFavorite,
    });
  }, [traceId, spanId, projectId, isFavorite, toggleFavorite]);

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

  if (!traceId || !spanId) {
    return (
      <div className="flex h-screen items-center justify-center text-muted-foreground">
        No span ID provided
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
                {SPAN_TABS.map((tab) => (
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
            {SPAN_TABS.map((tab) => (
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

          {/* Right side - span ID, favorite, and refresh */}
          <div className="flex items-center gap-2">
            <code className="hidden truncate rounded border border-border/50 bg-muted px-2 py-1 font-mono text-xs text-muted-foreground @[600px]:block @[600px]:max-w-[200px] @[800px]:max-w-[300px] @[1000px]:max-w-none">
              {spanId}
            </code>
            <FavoriteButton
              isFavorite={isFavorite}
              disabled={!traceId || !spanId}
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
          <SpanDetail
            traceId={traceId}
            spanId={spanId}
            projectId={projectId}
            activeTab={effectiveActiveTab as SpanTab}
            threadTab={effectiveThreadTab as ThreadTab}
            onThreadTabChange={handleThreadTabChange}
            onRefreshChange={handleRefreshChange}
          />
        </div>
      </div>
    </div>
  );
}
