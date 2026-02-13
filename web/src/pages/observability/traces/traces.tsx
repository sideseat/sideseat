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
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Pagination, DEFAULT_PAGE_SIZE } from "@/components/pagination";
import { ColumnSelector } from "@/components/column-selector";
import { FiltersPanel } from "@/components/filters-panel";
import { GridEmptyOverlay, DeleteEntityDialog } from "@/components/grid";
import { toast } from "sonner";
import { useTraces, useTraceFilterOptions } from "@/api/otel/hooks/queries";
import { useCheckFavorites, useFavoriteIds, useToggleFavorite } from "@/api/favorites";
import { otelKeys } from "@/api/otel/keys";
import { useSpanStream } from "@/api/otel/hooks/streams";
import { useCurrentProject } from "@/hooks/use-project";
import { useOtelClient } from "@/lib/app-context";
import { useResolvedTheme } from "@/hooks";
import { useFilters } from "@/hooks/use-filters";
import { useRecentlyDeletedIds, useSseDetailRefresh } from "@/hooks/use-grid-helpers";
import {
  settings,
  GLOBAL_PAGE_SIZE_KEY,
  TRACES_COLUMN_VISIBILITY_KEY,
  TRACES_REALTIME_KEY,
  TRACES_SHOW_NON_GENAI_KEY,
} from "@/lib/settings";
import { cn } from "@/lib/utils";
import { TRACE_FILTER_CONFIGS } from "@/lib/filters";
import type { TraceSummary } from "@/api/otel/types";
import {
  columnDefs,
  defaultColDef,
  rowSelectionConfig,
  DEFAULT_VISIBLE_COLUMNS,
  HIDEABLE_COLUMNS,
  NON_HIDEABLE_COLUMNS,
} from "./traces-columns";
import { TraceDetailSheet } from "../trace/trace-detail-sheet";

ModuleRegistry.registerModules([AllCommunityModule]);

type SortState = { field: string; desc: boolean } | null;

export default function TracesPage() {
  const { projectId } = useCurrentProject();
  const colorScheme = useResolvedTheme();
  const gridRef = useRef<AgGridReact<TraceSummary>>(null);

  const [page, setPage] = useQueryParam("page", withDefault(NumberParam, 1));
  const [pageSize, setPageSize] = useState(() => {
    return settings.get<number>(GLOBAL_PAGE_SIZE_KEY) ?? DEFAULT_PAGE_SIZE;
  });
  const [sortParam, setSortParam] = useQueryParam(
    "sort",
    withDefault(StringParam, "start_time:desc"),
  );
  const [timePreset, setTimePreset] = useQueryParam(
    "time",
    withDefault(StringParam, DEFAULT_TIME_PRESET),
  );
  const [fromTimestamp, setFromTimestamp] = useQueryParam("from", StringParam);
  const [toTimestamp, setToTimestamp] = useQueryParam("to", StringParam);

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
  const [viewTraceId, setViewTraceId] = useQueryParam("view", StringParam);
  const [realtimeEnabled, setRealtimeEnabled] = useState(() => {
    return settings.get<boolean>(TRACES_REALTIME_KEY) ?? true;
  });
  const [showNonGenAiSpans, setShowNonGenAiSpans] = useState(() => {
    return settings.get<boolean>(TRACES_SHOW_NON_GENAI_KEY) ?? false;
  });
  const [favoritesOnly, setFavoritesOnly] = useState(false);
  const [deleteDialogOpen, setDeleteDialogOpen] = useState(false);
  const queryClient = useQueryClient();

  // Shared hooks for grid state management
  const { recentlyDeletedIds, trackDeletedIds } = useRecentlyDeletedIds();
  const handleSseSpan = useSseDetailRefresh("trace", viewTraceId, queryClient);

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
    "trace",
    { enabled: favoritesOnly },
  );

  const otelClient = useOtelClient();
  const [filtersPanelOpened, setFiltersPanelOpened] = useState(false);

  const [visibleColumns, setVisibleColumns] = useState<string[]>(() => {
    const saved = settings.get<string[]>(TRACES_COLUMN_VISIBILITY_KEY);
    return saved ?? [...DEFAULT_VISIBLE_COLUMNS];
  });

  const handleVisibilityChange = useCallback((columns: string[]) => {
    setVisibleColumns(columns);
    settings.set(TRACES_COLUMN_VISIBILITY_KEY, columns);
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
    // When favorites only is enabled and we have favorite IDs, add trace_id filter
    if (favoritesOnly && allFavoriteIds && allFavoriteIds.length > 0) {
      result.push({
        type: "string_options" as const,
        column: "trace_id",
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

  const { data, isLoading, isFetching, refetch } = useTraces(
    projectId,
    {
      page,
      limit: pageSize,
      order_by,
      from_timestamp: effectiveFromTimestamp,
      to_timestamp: toTimestamp ?? undefined,
      filters: combinedFilters.length > 0 ? combinedFilters : undefined,
      include_nongenai: showNonGenAiSpans,
    },
    {
      enabled: queryEnabled,
    },
  );

  const filterOptionsParams = useMemo(
    () => ({
      from_timestamp: effectiveFromTimestamp,
      to_timestamp: toTimestamp ?? undefined,
    }),
    [effectiveFromTimestamp, toTimestamp],
  );

  const prefetchFilterOptions = useCallback(() => {
    queryClient.prefetchQuery({
      queryKey: otelKeys.traces.filterOptions(projectId, filterOptionsParams),
      queryFn: () => otelClient.getTraceFilterOptions(projectId, filterOptionsParams),
      staleTime: 60_000,
    });
  }, [queryClient, otelClient, projectId, filterOptionsParams]);

  const handleFiltersPanelOpen = useCallback(() => {
    setFiltersPanelOpened(true);
  }, []);

  const { data: filterOptionsData, isLoading: filterOptionsLoading } = useTraceFilterOptions(
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

  const handleShowNonGenAiSpansToggle = useCallback(() => {
    setShowNonGenAiSpans((prev) => {
      const next = !prev;
      settings.set(TRACES_SHOW_NON_GENAI_KEY, next);
      return next;
    });
    setPage(1);
    setRowSelection([]);
    setViewTraceId(null);
    gridRef.current?.api?.deselectAll();
  }, [setPage, setViewTraceId]);

  const handleDeleteSuccess = useCallback(() => {
    const deletedIds = [...rowSelection];
    trackDeletedIds(deletedIds);
    toast.success(`Deleted ${deletedIds.length} trace${deletedIds.length !== 1 ? "s" : ""}`);
    setRowSelection([]);
    gridRef.current?.api?.deselectAll();
  }, [rowSelection, trackDeletedIds]);

  // Filter out recently deleted traces to prevent them from reappearing after SSE refetch
  // Also return empty when favorites only mode is on but no favorites exist
  const traces = useMemo(() => {
    if (showEmptyFavorites) return [];
    const allTraces = data?.data ?? [];
    if (recentlyDeletedIds.size === 0) return allTraces;
    return allTraces.filter((t) => !recentlyDeletedIds.has(t.trace_id));
  }, [data?.data, recentlyDeletedIds, showEmptyFavorites]);
  const totalPages = showEmptyFavorites ? 1 : data?.meta?.total_pages;
  const totalItems = showEmptyFavorites ? 0 : (data?.meta?.total_items ?? traces.length);

  // Extract trace IDs for favorites check
  const traceIds = useMemo(() => traces.map((t) => t.trace_id), [traces]);

  // Favorites
  const { data: favoriteIds } = useCheckFavorites(projectId, "trace", traceIds);
  const { mutate: toggleFavoriteMutation } = useToggleFavorite();

  const handleToggleFavorite = useCallback(
    (entityId: string, isFavorite: boolean) => {
      toggleFavoriteMutation({
        projectId,
        entityType: "trace",
        entityId,
        isFavorite,
      });
    },
    [projectId, toggleFavoriteMutation],
  );

  const renderedFavoritesRef = useRef<Set<string> | null>(null);
  useEffect(() => {
    if (!favoriteIds || traceIds.length === 0) return;

    const prevFavorites = renderedFavoritesRef.current;
    const favoritesChanged =
      !prevFavorites ||
      prevFavorites.size !== favoriteIds.size ||
      [...favoriteIds].some((id) => !prevFavorites.has(id));

    if (favoritesChanged) {
      gridRef.current?.api?.refreshCells({ columns: ["favorite"] });
      renderedFavoritesRef.current = favoriteIds;
    }
  }, [favoriteIds, traceIds]);

  const selectedTraceIndex = useMemo(() => {
    if (!viewTraceId) return null;
    const index = traces.findIndex((t) => t.trace_id === viewTraceId);
    return index >= 0 ? index : null;
  }, [viewTraceId, traces]);

  const selectedTrace = selectedTraceIndex !== null ? traces[selectedTraceIndex] : null;
  const detailSheetOpen = !!viewTraceId;

  // Memoize selected row style to avoid creating new objects on every render
  const selectedRowStyle = useMemo<RowStyle>(() => ({ backgroundColor: "var(--accent)" }), []);

  const getRowStyle = useCallback(
    (params: { rowIndex: number | null }) => {
      return params.rowIndex === selectedTraceIndex && detailSheetOpen
        ? selectedRowStyle
        : undefined;
    },
    [selectedTraceIndex, detailSheetOpen, selectedRowStyle],
  );

  const getRowClass = useCallback((params: { data?: TraceSummary }) => {
    return params.data?.has_error ? "ag-row-error" : undefined;
  }, []);

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
    (event: SortChangedEvent<TraceSummary>) => {
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

  const handleSelectionChanged = useCallback((event: SelectionChangedEvent<TraceSummary>) => {
    const selectedRows = event.api.getSelectedRows();
    setRowSelection(selectedRows.map((row) => row.trace_id));
  }, []);

  const handleRowClicked = useCallback(
    (event: RowClickedEvent<TraceSummary>) => {
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
        setViewTraceId(event.data.trace_id);
      }
    },
    [setViewTraceId],
  );

  const handleNavigatePrev = useCallback(() => {
    if (selectedTraceIndex === null) return;

    if (selectedTraceIndex > 0) {
      // Navigate within current page
      setViewTraceId(traces[selectedTraceIndex - 1].trace_id);
    } else if (page > 1) {
      // Navigate to previous page, select last item
      pendingNavTargetRef.current = "last";
      setPage(page - 1);
    }
  }, [selectedTraceIndex, traces, setViewTraceId, page, setPage]);

  const handleNavigateNext = useCallback(() => {
    if (selectedTraceIndex === null) return;

    if (selectedTraceIndex < traces.length - 1) {
      // Navigate within current page
      setViewTraceId(traces[selectedTraceIndex + 1].trace_id);
    } else if (totalPages && page < totalPages) {
      // Navigate to next page, select first item
      pendingNavTargetRef.current = "first";
      setPage(page + 1);
    }
  }, [selectedTraceIndex, traces, setViewTraceId, page, totalPages, setPage]);

  const handleCloseSheet = useCallback(
    (open: boolean) => {
      if (!open) {
        setViewTraceId(null);
      }
    },
    [setViewTraceId],
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
    // Only clear viewTraceId if not navigating across pages
    if (!pendingNavTargetRef.current) {
      setViewTraceId(null);
    }
    gridRef.current?.api?.deselectAll();
  }, [page, pageSize, sortParam, filters, setViewTraceId]);

  // When page changes and we have a pending navigation target, select the appropriate trace
  // This effect must be defined AFTER the clearing effect above to ensure correct execution order
  // Check !isFetching to avoid selecting from placeholder/stale data during page transitions
  useEffect(() => {
    if (pendingNavTargetRef.current && !isLoading && !isFetching) {
      if (traces.length > 0) {
        if (pendingNavTargetRef.current === "first") {
          setViewTraceId(traces[0].trace_id);
        } else if (pendingNavTargetRef.current === "last") {
          setViewTraceId(traces[traces.length - 1].trace_id);
        }
      }
      // Always clear ref when fetch completes, even if page is empty
      pendingNavTargetRef.current = null;
    }
  }, [traces, isLoading, isFetching, setViewTraceId]);

  return (
    <div className="h-screen w-full mx-auto pt-header-offset sm:pt-header-offset-sm px-2 sm:px-4">
      <div className="w-full h-full overflow-hidden grid grid-rows-[auto_1fr_auto]">
        <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-2 pb-4">
          <div className="shrink-0">
            <h1 className="text-2xl font-semibold tracking-tight">Traces</h1>
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
              filterConfigs={TRACE_FILTER_CONFIGS}
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
                setViewTraceId(null);
                gridRef.current?.api?.deselectAll();
              }}
              aria-label={favoritesOnly ? "Show all traces" : "Show favorites only"}
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
                className="px-2 gap-1.5 min-w-[52px] sm:min-w-[68px]"
                onClick={() => {
                  setRealtimeEnabled((prev) => {
                    const next = !prev;
                    settings.set(TRACES_REALTIME_KEY, next);
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
                  <DropdownMenuCheckboxItem
                    checked={showNonGenAiSpans}
                    onCheckedChange={handleShowNonGenAiSpansToggle}
                  >
                    Show non-GenAI
                  </DropdownMenuCheckboxItem>
                  <DropdownMenuSeparator />
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
        <AgGridReact<TraceSummary>
          ref={gridRef}
          theme={gridTheme}
          rowData={showEmptyFavorites ? [] : traces}
          columnDefs={visibleColumnDefs}
          defaultColDef={defaultColDef}
          rowSelection={rowSelectionConfig}
          getRowId={(params) => params.data.trace_id}
          rowClass="cursor-pointer"
          getRowStyle={getRowStyle}
          getRowClass={getRowClass}
          context={{
            projectId,
            entityType: "trace" as const,
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
          noRowsOverlayComponentParams={{ entityName: "traces", projectId, realtimeEnabled }}
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

      <TraceDetailSheet
        open={detailSheetOpen}
        onOpenChange={handleCloseSheet}
        trace={selectedTrace}
        projectId={projectId}
        onNavigatePrev={handleNavigatePrev}
        onNavigateNext={handleNavigateNext}
        hasPrev={selectedTraceIndex !== null && (selectedTraceIndex > 0 || page > 1)}
        hasNext={
          selectedTraceIndex !== null &&
          (selectedTraceIndex < traces.length - 1 || (totalPages != null && page < totalPages))
        }
        realtimeEnabled={false}
      />

      <DeleteEntityDialog
        entityType="trace"
        entityIds={rowSelection}
        projectId={projectId}
        open={deleteDialogOpen}
        onOpenChange={setDeleteDialogOpen}
        onSuccess={handleDeleteSuccess}
      />
    </div>
  );
}
