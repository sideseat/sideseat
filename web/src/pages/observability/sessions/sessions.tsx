import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useQueryParam, NumberParam, StringParam, withDefault } from "use-query-params";
import { TimeFilter } from "@/components/time-filter";
import { DEFAULT_TIME_PRESET, getPresetRange } from "@/lib/time-filter";
import { AgGridReact } from "ag-grid-react";
import {
  AllCommunityModule,
  ModuleRegistry,
  themeQuartz,
  colorSchemeDark,
  colorSchemeLight,
} from "ag-grid-community";
import type {
  SelectionChangedEvent,
  SortChangedEvent,
  RowClickedEvent,
  RowStyle,
} from "ag-grid-community";
import { MoreHorizontal, RefreshCw, Star, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ButtonGroup } from "@/components/ui/button-group";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Pagination, DEFAULT_PAGE_SIZE } from "@/components/pagination";
import { ColumnSelector } from "@/components/column-selector";
import { FiltersPanel } from "@/components/filters-panel";
import { GridEmptyOverlay, DeleteEntityDialog } from "@/components/grid";
import { toast } from "sonner";
import { useSessions, useSessionFilterOptions } from "@/api/otel/hooks/queries";
import { useCheckFavorites, useFavoriteIds, useToggleFavorite } from "@/api/favorites";
import { otelKeys } from "@/api/otel/keys";
import { useSpanStream } from "@/api/otel/hooks/streams";
import { useOtelClient } from "@/lib/app-context";
import { useCurrentProject } from "@/hooks/use-project";
import { useResolvedTheme } from "@/hooks";
import { useFilters } from "@/hooks/use-filters";
import { useRecentlyDeletedIds, useSseDetailRefresh } from "@/hooks/use-grid-helpers";
import {
  settings,
  GLOBAL_PAGE_SIZE_KEY,
  GLOBAL_TIME_PRESET_KEY,
  SESSIONS_COLUMN_VISIBILITY_KEY,
  SESSIONS_REALTIME_KEY,
} from "@/lib/settings";
import { cn } from "@/lib/utils";
import { SESSION_FILTER_CONFIGS } from "@/lib/filters";
import type { SessionSummary } from "@/api/otel/types";
import {
  columnDefs,
  defaultColDef,
  rowSelectionConfig,
  DEFAULT_VISIBLE_COLUMNS,
  HIDEABLE_COLUMNS,
  NON_HIDEABLE_COLUMNS,
} from "./sessions-columns";
import { SessionDetailSheet } from "../session/session-detail-sheet";

ModuleRegistry.registerModules([AllCommunityModule]);

type SortState = { field: string; desc: boolean } | null;

export default function SessionsPage() {
  const { projectId } = useCurrentProject();
  const colorScheme = useResolvedTheme();
  const otelClient = useOtelClient();
  const gridRef = useRef<AgGridReact<SessionSummary>>(null);

  const [page, setPage] = useQueryParam("page", withDefault(NumberParam, 1));
  const [pageSize, setPageSize] = useState(() => {
    return settings.get<number>(GLOBAL_PAGE_SIZE_KEY) ?? DEFAULT_PAGE_SIZE;
  });
  const [sortParam, setSortParam] = useQueryParam(
    "sort",
    withDefault(StringParam, "start_time:desc"),
  );
  const savedTimePresetRef = useRef(
    settings.get<string>(GLOBAL_TIME_PRESET_KEY) ?? DEFAULT_TIME_PRESET,
  );
  const [timePreset, setTimePreset] = useQueryParam(
    "time",
    withDefault(StringParam, savedTimePresetRef.current),
  );
  const [fromTimestamp, setFromTimestamp] = useQueryParam("from", StringParam);
  const [toTimestamp, setToTimestamp] = useQueryParam("to", StringParam);

  // Persist time preset to settings whenever it changes
  useEffect(() => {
    if (timePreset && timePreset !== savedTimePresetRef.current) {
      settings.set(GLOBAL_TIME_PRESET_KEY, timePreset);
      savedTimePresetRef.current = timePreset;
    }
  }, [timePreset]);

  // Compute effective fromTimestamp: use URL param if set, otherwise derive from preset
  const effectiveFromTimestamp = useMemo(() => {
    if (fromTimestamp) return fromTimestamp;
    if (timePreset && timePreset !== "custom") {
      const range = getPresetRange(timePreset);
      return range?.from ?? undefined;
    }
    return undefined;
  }, [fromTimestamp, timePreset]);

  const sortState = useMemo<SortState>(() => {
    if (!sortParam) return null;
    const [field, dir] = sortParam.split(":");
    return { field, desc: dir === "desc" };
  }, [sortParam]);

  const [rowSelection, setRowSelection] = useState<string[]>([]);
  const [viewSessionId, setViewSessionId] = useQueryParam("view", StringParam);
  const [realtimeEnabled, setRealtimeEnabled] = useState(() => {
    return settings.get<boolean>(SESSIONS_REALTIME_KEY) ?? true;
  });
  const [favoritesOnly, setFavoritesOnly] = useState(false);
  const [deleteDialogOpen, setDeleteDialogOpen] = useState(false);
  const [filtersPanelOpened, setFiltersPanelOpened] = useState(false);
  const queryClient = useQueryClient();

  // Shared hooks for grid state management
  const { recentlyDeletedIds, trackDeletedIds } = useRecentlyDeletedIds();
  const handleSseSpan = useSseDetailRefresh("session", viewSessionId, queryClient);

  const { status: streamStatus } = useSpanStream({
    projectId,
    enabled: realtimeEnabled,
    onSpan: handleSseSpan,
  });

  const { filters, setFilters } = useFilters({
    onFiltersChange: () => setPage(1),
  });

  // Fetch all favorite IDs when "Favorites only" filter is enabled
  const { data: allFavoriteIds, isLoading: isFavoritesLoading } = useFavoriteIds(
    projectId,
    "session",
    { enabled: favoritesOnly },
  );

  const [visibleColumns, setVisibleColumns] = useState<string[]>(() => {
    const saved = settings.get<string[]>(SESSIONS_COLUMN_VISIBILITY_KEY);
    return saved ?? [...DEFAULT_VISIBLE_COLUMNS];
  });

  const handleVisibilityChange = useCallback((columns: string[]) => {
    setVisibleColumns(columns);
    settings.set(SESSIONS_COLUMN_VISIBILITY_KEY, columns);
  }, []);

  const visibleColumnDefs = useMemo(() => {
    return columnDefs.map((col) => {
      const colId = col.colId ?? col.field;
      if (!colId) return col;

      if (NON_HIDEABLE_COLUMNS.has(colId)) return col;

      return {
        ...col,
        hide: !visibleColumns.includes(colId),
      };
    });
  }, [visibleColumns]);

  const gridTheme = useMemo(
    () => themeQuartz.withPart(colorScheme === "dark" ? colorSchemeDark : colorSchemeLight),
    [colorScheme],
  );

  const order_by = sortState ? `${sortState.field}:${sortState.desc ? "desc" : "asc"}` : undefined;

  // Combine user filters with favorites filter when enabled
  const combinedFilters = useMemo(() => {
    // Filter out any invalid filters (with null/undefined values)
    const validFilters = filters.filter((f) => f.value != null);
    const result = [...validFilters];
    // When favorites only is enabled and we have favorite IDs, add session_id filter
    if (favoritesOnly && allFavoriteIds && allFavoriteIds.length > 0) {
      result.push({
        type: "string_options" as const,
        column: "session_id",
        operator: "any of" as const,
        value: allFavoriteIds,
      });
    }
    return result;
  }, [filters, favoritesOnly, allFavoriteIds]);

  // Show empty state when favorites filter is on but no favorites exist
  const showEmptyFavorites =
    favoritesOnly && !isFavoritesLoading && (!allFavoriteIds || allFavoriteIds.length === 0);

  // Disable query while favorites are loading or when showing empty favorites state
  const queryEnabled =
    !favoritesOnly || (!isFavoritesLoading && allFavoriteIds && allFavoriteIds.length > 0);

  const { data, isLoading, isFetching, refetch } = useSessions(
    projectId,
    {
      page,
      limit: pageSize,
      order_by,
      from_timestamp: effectiveFromTimestamp,
      to_timestamp: toTimestamp ?? undefined,
      filters: combinedFilters.length > 0 ? combinedFilters : undefined,
    },
    {
      enabled: queryEnabled,
    },
  );

  // Filter options for dropdown filters
  const filterOptionsParams = useMemo(
    () => ({
      from_timestamp: effectiveFromTimestamp,
      to_timestamp: toTimestamp ?? undefined,
    }),
    [effectiveFromTimestamp, toTimestamp],
  );

  const prefetchFilterOptions = useCallback(() => {
    queryClient.prefetchQuery({
      queryKey: otelKeys.sessions.filterOptions(projectId, filterOptionsParams),
      queryFn: () => otelClient.getSessionFilterOptions(projectId, filterOptionsParams),
      staleTime: 60_000,
    });
  }, [queryClient, otelClient, projectId, filterOptionsParams]);

  const handleFiltersPanelOpen = useCallback(() => {
    setFiltersPanelOpened(true);
  }, []);

  const { data: filterOptionsData, isLoading: filterOptionsLoading } = useSessionFilterOptions(
    projectId,
    {
      ...filterOptionsParams,
      enabled: filtersPanelOpened,
    },
  );

  const handleDeleteSelected = useCallback(() => {
    if (rowSelection.length === 0) return;
    setDeleteDialogOpen(true);
  }, [rowSelection.length]);

  const handleDeleteSuccess = useCallback(() => {
    const deletedIds = [...rowSelection];
    trackDeletedIds(deletedIds);
    toast.success(`Deleted ${deletedIds.length} session${deletedIds.length !== 1 ? "s" : ""}`);
    setRowSelection([]);
    gridRef.current?.api?.deselectAll();
  }, [rowSelection, trackDeletedIds]);

  // Filter out recently deleted sessions to prevent them from reappearing after SSE refetch
  // Also return empty when favorites only mode is on but no favorites exist
  const sessions = useMemo(() => {
    if (showEmptyFavorites) return [];
    const allSessions = data?.data ?? [];
    if (recentlyDeletedIds.size === 0) return allSessions;
    return allSessions.filter((s) => !recentlyDeletedIds.has(s.session_id));
  }, [data?.data, recentlyDeletedIds, showEmptyFavorites]);
  const totalPages = showEmptyFavorites ? 1 : data?.meta?.total_pages;
  const totalItems = showEmptyFavorites ? 0 : (data?.meta?.total_items ?? sessions.length);

  // Extract session IDs for favorites check
  const sessionIds = useMemo(() => sessions.map((s) => s.session_id), [sessions]);

  // Favorites
  const { data: favoriteIds } = useCheckFavorites(projectId, "session", sessionIds);
  const { mutate: toggleFavoriteMutation } = useToggleFavorite();

  const handleToggleFavorite = useCallback(
    (entityId: string, isFavorite: boolean) => {
      toggleFavoriteMutation({
        projectId,
        entityType: "session",
        entityId,
        isFavorite,
      });
    },
    [projectId, toggleFavoriteMutation],
  );

  const renderedFavoritesRef = useRef<Set<string> | null>(null);
  useEffect(() => {
    if (!favoriteIds || sessionIds.length === 0) return;

    const prevFavorites = renderedFavoritesRef.current;
    const favoritesChanged =
      !prevFavorites ||
      prevFavorites.size !== favoriteIds.size ||
      [...favoriteIds].some((id) => !prevFavorites.has(id));

    if (favoritesChanged) {
      gridRef.current?.api?.refreshCells({ columns: ["favorite"] });
      renderedFavoritesRef.current = favoriteIds;
    }
  }, [favoriteIds, sessionIds]);

  const selectedSessionIndex = useMemo(() => {
    if (!viewSessionId) return null;
    const index = sessions.findIndex((s) => s.session_id === viewSessionId);
    return index >= 0 ? index : null;
  }, [viewSessionId, sessions]);

  const selectedSession = selectedSessionIndex !== null ? sessions[selectedSessionIndex] : null;
  const detailSheetOpen = !!viewSessionId;

  // Memoize selected row style to avoid creating new objects on every render
  const selectedRowStyle = useMemo<RowStyle>(() => ({ backgroundColor: "var(--accent)" }), []);

  const getRowStyle = useCallback(
    (params: { rowIndex: number | null }) => {
      return params.rowIndex === selectedSessionIndex && detailSheetOpen
        ? selectedRowStyle
        : undefined;
    },
    [selectedSessionIndex, detailSheetOpen, selectedRowStyle],
  );

  // Track pending navigation target for cross-page navigation
  const pendingNavTargetRef = useRef<"first" | "last" | null>(null);

  // Track previous values to detect actual changes (not initial mount)
  const prevClearingDepsRef = useRef({
    page,
    pageSize,
    sortParam,
    filtersKey: JSON.stringify(filters),
  });

  const handleSortChanged = useCallback(
    (event: SortChangedEvent<SessionSummary>) => {
      const primarySort = event.api
        .getColumnState()
        .filter((col) => col.sort)
        .sort((a, b) => (a.sortIndex ?? 0) - (b.sortIndex ?? 0))[0];

      if (primarySort) {
        setSortParam(`${primarySort.colId}:${primarySort.sort === "desc" ? "desc" : "asc"}`);
      } else {
        setSortParam(undefined);
      }
    },
    [setSortParam],
  );

  const handleSelectionChanged = useCallback((event: SelectionChangedEvent<SessionSummary>) => {
    const selectedRows = event.api.getSelectedRows();
    setRowSelection(selectedRows.map((row) => row.session_id));
  }, []);

  const handleRowClicked = useCallback(
    (event: RowClickedEvent<SessionSummary>) => {
      // Ignore clicks on checkbox inputs, sheet elements, favorite button, and action buttons
      const target = event.event?.target as HTMLElement | null;
      if (
        target instanceof HTMLInputElement ||
        target?.closest('[data-slot="sheet"]') ||
        target?.closest("[data-favorite-cell]") ||
        target?.closest("[data-actions-cell]")
      ) {
        return;
      }
      if (event.data) {
        setViewSessionId(event.data.session_id);
      }
    },
    [setViewSessionId],
  );

  const handleNavigatePrev = useCallback(() => {
    if (selectedSessionIndex === null) return;

    if (selectedSessionIndex > 0) {
      // Navigate within current page
      setViewSessionId(sessions[selectedSessionIndex - 1].session_id);
    } else if (page > 1) {
      // Navigate to previous page, select last item
      pendingNavTargetRef.current = "last";
      setPage(page - 1);
    }
  }, [selectedSessionIndex, sessions, setViewSessionId, page, setPage]);

  const handleNavigateNext = useCallback(() => {
    if (selectedSessionIndex === null) return;

    if (selectedSessionIndex < sessions.length - 1) {
      // Navigate within current page
      setViewSessionId(sessions[selectedSessionIndex + 1].session_id);
    } else if (totalPages && page < totalPages) {
      // Navigate to next page, select first item
      pendingNavTargetRef.current = "first";
      setPage(page + 1);
    }
  }, [selectedSessionIndex, sessions, setViewSessionId, page, totalPages, setPage]);

  const handleCloseSheet = useCallback(
    (open: boolean) => {
      if (!open) {
        setViewSessionId(null);
      }
    },
    [setViewSessionId],
  );

  useEffect(() => {
    const prev = prevClearingDepsRef.current;
    const currentFiltersKey = JSON.stringify(filters);
    const hasChanged =
      prev.page !== page ||
      prev.pageSize !== pageSize ||
      prev.sortParam !== sortParam ||
      prev.filtersKey !== currentFiltersKey;

    // Update previous values
    prevClearingDepsRef.current = { page, pageSize, sortParam, filtersKey: currentFiltersKey };

    // Only clear if values actually changed (skip on initial mount)
    if (!hasChanged) return;

    setRowSelection([]);
    // Only clear viewSessionId if not navigating across pages
    if (!pendingNavTargetRef.current) {
      setViewSessionId(null);
    }
    gridRef.current?.api?.deselectAll();
  }, [page, pageSize, sortParam, filters, setViewSessionId]);

  // When page changes and we have a pending navigation target, select the appropriate session
  // This effect must be defined AFTER the clearing effect above to ensure correct execution order
  // Check !isFetching to avoid selecting from placeholder/stale data during page transitions
  useEffect(() => {
    if (pendingNavTargetRef.current && !isLoading && !isFetching) {
      if (sessions.length > 0) {
        if (pendingNavTargetRef.current === "first") {
          setViewSessionId(sessions[0].session_id);
        } else if (pendingNavTargetRef.current === "last") {
          setViewSessionId(sessions[sessions.length - 1].session_id);
        }
      }
      // Always clear ref when fetch completes, even if page is empty
      pendingNavTargetRef.current = null;
    }
  }, [sessions, isLoading, isFetching, setViewSessionId]);

  return (
    <div className="h-screen w-full mx-auto pt-header-offset sm:pt-header-offset-sm px-2 sm:px-4">
      <div className="w-full h-full overflow-hidden grid grid-rows-[auto_1fr_auto]">
        <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-2 pb-4">
          <div className="shrink-0">
            <h1 className="text-2xl font-semibold tracking-tight">Sessions</h1>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <TimeFilter
              preset={timePreset ?? undefined}
              fromTimestamp={fromTimestamp ?? undefined}
              toTimestamp={toTimestamp ?? undefined}
              onFilterChange={(preset, from, to) => {
                setTimePreset(preset ?? null);
                setFromTimestamp(from ?? null);
                setToTimestamp(to ?? null);
                setPage(1);
              }}
            />
            <FiltersPanel
              filters={filters}
              onFiltersChange={setFilters}
              filterConfigs={SESSION_FILTER_CONFIGS}
              filterOptions={filterOptionsData?.options}
              onTriggerMouseEnter={prefetchFilterOptions}
              onOpen={handleFiltersPanelOpen}
              isLoading={filterOptionsLoading}
            />
            <Button
              variant={favoritesOnly ? "default" : "outline"}
              size="sm"
              className="px-2 gap-1.5"
              onClick={() => {
                setFavoritesOnly((prev) => !prev);
                setPage(1);
                setRowSelection([]);
                setViewSessionId(null);
                gridRef.current?.api?.deselectAll();
              }}
              aria-label={favoritesOnly ? "Show all sessions" : "Show favorites only"}
            >
              <Star className={cn("h-4 w-4", favoritesOnly && "fill-current")} />
              <span className="hidden sm:inline text-xs">Favorites</span>
            </Button>
            <ColumnSelector
              columns={HIDEABLE_COLUMNS}
              visibleColumns={visibleColumns}
              defaultColumns={DEFAULT_VISIBLE_COLUMNS}
              onVisibilityChange={handleVisibilityChange}
            />
            <ButtonGroup>
              <Button
                variant="outline"
                size="sm"
                className="px-2 gap-1.5 min-w-13 sm:min-w-17"
                onClick={() => {
                  setRealtimeEnabled((prev) => {
                    const next = !prev;
                    settings.set(SESSIONS_REALTIME_KEY, next);
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
                className="px-2"
                onClick={() => refetch()}
                disabled={isFetching}
                aria-label="Refresh"
              >
                <RefreshCw className={isFetching ? "h-4 w-4 animate-spin" : "h-4 w-4"} />
              </Button>
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button variant="outline" size="sm" className="px-2" aria-label="More actions">
                    <MoreHorizontal className="h-4 w-4" />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  <DropdownMenuItem
                    variant="destructive"
                    disabled={rowSelection.length === 0}
                    onClick={handleDeleteSelected}
                  >
                    <Trash2 className="h-4 w-4" />
                    Delete Selected
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            </ButtonGroup>
          </div>
        </div>
        <AgGridReact<SessionSummary>
          ref={gridRef}
          theme={gridTheme}
          rowData={showEmptyFavorites ? [] : sessions}
          columnDefs={visibleColumnDefs}
          defaultColDef={defaultColDef}
          rowSelection={rowSelectionConfig}
          getRowId={(params) => params.data.session_id}
          rowClass="cursor-pointer"
          getRowStyle={getRowStyle}
          context={{
            projectId,
            entityType: "session" as const,
            trackDeletedIds,
            realtimeEnabled,
            favoriteIds,
            toggleFavorite: handleToggleFavorite,
          }}
          onSortChanged={handleSortChanged}
          onSelectionChanged={handleSelectionChanged}
          onRowClicked={handleRowClicked}
          suppressMultiSort
          suppressCellFocus
          loading={isLoading || (favoritesOnly && isFavoritesLoading)}
          domLayout="normal"
          noRowsOverlayComponent={GridEmptyOverlay}
          noRowsOverlayComponentParams={{ entityName: "sessions", projectId, realtimeEnabled }}
          animateRows={false}
        />
        <div className="pb-3 pt-2">
          <Pagination
            currentPage={page}
            pageSize={pageSize}
            totalItems={totalItems}
            totalPages={totalPages}
            onPageChange={(nextPage) => setPage(nextPage)}
            onPageSizeChange={(size) => {
              setPage(1);
              setPageSize(size);
              settings.set(GLOBAL_PAGE_SIZE_KEY, size);
            }}
            isLoading={isFetching}
            selectedCount={rowSelection.length}
          />
        </div>
      </div>

      <SessionDetailSheet
        open={detailSheetOpen}
        onOpenChange={handleCloseSheet}
        session={selectedSession}
        projectId={projectId}
        onNavigatePrev={handleNavigatePrev}
        onNavigateNext={handleNavigateNext}
        hasPrev={selectedSessionIndex !== null && (selectedSessionIndex > 0 || page > 1)}
        hasNext={
          selectedSessionIndex !== null &&
          (selectedSessionIndex < sessions.length - 1 || (totalPages != null && page < totalPages))
        }
        realtimeEnabled={false}
      />

      <DeleteEntityDialog
        entityType="session"
        entityIds={rowSelection}
        projectId={projectId}
        open={deleteDialogOpen}
        onOpenChange={setDeleteDialogOpen}
        onSuccess={handleDeleteSuccess}
      />
    </div>
  );
}
