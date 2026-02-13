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
import { useSpans, useSpanFilterOptions } from "@/api/otel/hooks/queries";
import { useCheckSpanFavorites, useFavoriteIds, useToggleFavorite } from "@/api/favorites";
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
  SPANS_COLUMN_VISIBILITY_KEY,
  SPANS_REALTIME_KEY,
  SPANS_SHOW_NON_GENAI_KEY,
} from "@/lib/settings";
import { cn } from "@/lib/utils";
import { SPAN_FILTER_CONFIGS } from "@/lib/filters";
import type { SpanSummary } from "@/api/otel/types";
import {
  columnDefs,
  defaultColDef,
  rowSelectionConfig,
  DEFAULT_VISIBLE_COLUMNS,
  HIDEABLE_COLUMNS,
  NON_HIDEABLE_COLUMNS,
} from "./spans-columns";
import { SpanDetailSheet } from "../span/span-detail-sheet";

ModuleRegistry.registerModules([AllCommunityModule]);

type SortState = { field: string; desc: boolean } | null;

export default function SpansPage() {
  const { projectId } = useCurrentProject();
  const colorScheme = useResolvedTheme();
  const gridRef = useRef<AgGridReact<SpanSummary>>(null);

  const [page, setPage] = useQueryParam("page", withDefault(NumberParam, 1));
  const [pageSize, setPageSize] = useState(() => {
    return settings.get<number>(GLOBAL_PAGE_SIZE_KEY) ?? DEFAULT_PAGE_SIZE;
  });
  const [sortParam, setSortParam] = useQueryParam(
    "sort",
    withDefault(StringParam, "timestamp_start:desc"),
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
  // viewId is composite: "traceId:spanId"
  const [viewId, setViewId] = useQueryParam("view", StringParam);
  const [realtimeEnabled, setRealtimeEnabled] = useState(() => {
    return settings.get<boolean>(SPANS_REALTIME_KEY) ?? true;
  });
  const [showNonGenAi, setShowNonGenAi] = useState(() => {
    return settings.get<boolean>(SPANS_SHOW_NON_GENAI_KEY) ?? false;
  });
  const [favoritesOnly, setFavoritesOnly] = useState(false);
  const [deleteDialogOpen, setDeleteDialogOpen] = useState(false);
  const queryClient = useQueryClient();

  // Shared hooks for grid state management
  const { recentlyDeletedIds, trackDeletedIds } = useRecentlyDeletedIds();
  const handleSseSpan = useSseDetailRefresh("span", viewId, queryClient);

  const { status: streamStatus } = useSpanStream({
    projectId,
    enabled: realtimeEnabled,
    onSpan: handleSseSpan,
  });

  const { filters, setFilters } = useFilters({
    onFiltersChange: () => setPage(1),
  });

  // Fetch all favorite IDs when "Favorites only" filter is enabled
  // Returns composite "trace_id:span_id" strings
  const { data: allFavoriteIds, isLoading: isFavoritesLoading } = useFavoriteIds(
    projectId,
    "span",
    { enabled: favoritesOnly },
  );

  const otelClient = useOtelClient();
  const [filtersPanelOpened, setFiltersPanelOpened] = useState(false);

  const [visibleColumns, setVisibleColumns] = useState<string[]>(() => {
    const saved = settings.get<string[]>(SPANS_COLUMN_VISIBILITY_KEY);
    return saved ?? [...DEFAULT_VISIBLE_COLUMNS];
  });

  const handleVisibilityChange = useCallback((columns: string[]) => {
    setVisibleColumns(columns);
    settings.set(SPANS_COLUMN_VISIBILITY_KEY, columns);
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

  // Extract valid span_ids from composite "trace_id:span_id" favorites
  const validSpanIds = useMemo(() => {
    if (!allFavoriteIds || allFavoriteIds.length === 0) return [];
    return allFavoriteIds.map((id) => id.split(":")[1]).filter((id): id is string => !!id);
  }, [allFavoriteIds]);

  // Combine user filters with favorites filter when enabled
  const combinedFilters = useMemo(() => {
    // Filter out any invalid filters (with null/undefined values)
    const validFilters = filters.filter((f) => f.value != null);
    const result = [...validFilters];
    // When favorites only is enabled and we have valid span IDs, add span_id filter
    if (favoritesOnly && validSpanIds.length > 0) {
      result.push({
        type: "string_options" as const,
        column: "span_id",
        operator: "any of" as const,
        value: validSpanIds,
      });
    }
    return result;
  }, [filters, favoritesOnly, validSpanIds]);

  // Show empty state when favorites filter is on but no valid span favorites exist
  const showEmptyFavorites = favoritesOnly && !isFavoritesLoading && validSpanIds.length === 0;

  // Disable query while favorites are loading or when showing empty favorites state
  const queryEnabled = !favoritesOnly || (!isFavoritesLoading && validSpanIds.length > 0);

  const { data, isLoading, isFetching, refetch } = useSpans(
    projectId,
    {
      page,
      limit: pageSize,
      order_by,
      from_timestamp: effectiveFromTimestamp,
      to_timestamp: toTimestamp ?? undefined,
      filters: combinedFilters.length > 0 ? combinedFilters : undefined,
      is_observation: !showNonGenAi, // Only filter to GenAI spans when not showing all spans
    },
    {
      enabled: queryEnabled,
    },
  );

  const filterOptionsParams = useMemo(
    () => ({
      from_timestamp: effectiveFromTimestamp,
      to_timestamp: toTimestamp ?? undefined,
      observations_only: !showNonGenAi,
    }),
    [effectiveFromTimestamp, toTimestamp, showNonGenAi],
  );

  const prefetchFilterOptions = useCallback(() => {
    queryClient.prefetchQuery({
      queryKey: otelKeys.spans.filterOptions(projectId, filterOptionsParams),
      queryFn: () => otelClient.getSpanFilterOptions(projectId, filterOptionsParams),
      staleTime: 60_000,
    });
  }, [queryClient, otelClient, projectId, filterOptionsParams]);

  const handleFiltersPanelOpen = useCallback(() => {
    setFiltersPanelOpened(true);
  }, []);

  const handleDeleteSelected = useCallback(() => {
    if (rowSelection.length === 0) return;
    setDeleteDialogOpen(true);
  }, [rowSelection.length]);

  const handleShowNonGenAiToggle = useCallback(() => {
    setShowNonGenAi((prev) => {
      const next = !prev;
      settings.set(SPANS_SHOW_NON_GENAI_KEY, next);
      return next;
    });
    setPage(1);
    setRowSelection([]);
    setViewId(null);
    gridRef.current?.api?.deselectAll();
  }, [setPage, setViewId]);

  const handleDeleteSuccess = useCallback(() => {
    const deletedIds = [...rowSelection];
    trackDeletedIds(deletedIds);
    toast.success(`Deleted ${deletedIds.length} span${deletedIds.length !== 1 ? "s" : ""}`);
    setRowSelection([]);
    gridRef.current?.api?.deselectAll();
  }, [rowSelection, trackDeletedIds]);

  const { data: filterOptionsData, isLoading: filterOptionsLoading } = useSpanFilterOptions(
    projectId,
    {
      ...filterOptionsParams,
      enabled: filtersPanelOpened,
    },
  );

  // Filter out recently deleted spans to prevent them from reappearing after SSE refetch
  const spans = useMemo(() => {
    const allSpans = data?.data ?? [];
    if (recentlyDeletedIds.size === 0) return allSpans;
    return allSpans.filter((s) => !recentlyDeletedIds.has(`${s.trace_id}:${s.span_id}`));
  }, [data?.data, recentlyDeletedIds]);
  const totalPages = showEmptyFavorites ? 1 : data?.meta?.total_pages;
  const totalItems = showEmptyFavorites ? 0 : (data?.meta?.total_items ?? spans.length);

  // Extract span identifiers for favorites check
  const spanIdentifiers = useMemo(
    () => spans.map((s) => ({ trace_id: s.trace_id, span_id: s.span_id })),
    [spans],
  );

  // Favorites
  const { data: favoriteIds } = useCheckSpanFavorites(projectId, spanIdentifiers);
  const { mutate: toggleFavoriteMutation } = useToggleFavorite();

  const handleToggleFavorite = useCallback(
    (entityId: string, isFavorite: boolean, secondaryId?: string) => {
      toggleFavoriteMutation({
        projectId,
        entityType: "span",
        entityId,
        secondaryId,
        isFavorite,
      });
    },
    [projectId, toggleFavoriteMutation],
  );

  // Refresh favorite cells when favoriteIds changes
  const renderedFavoritesRef = useRef<Set<string> | null>(null);
  useEffect(() => {
    if (!favoriteIds || spanIdentifiers.length === 0) return;

    const prevFavorites = renderedFavoritesRef.current;
    const favoritesChanged =
      !prevFavorites ||
      prevFavorites.size !== favoriteIds.size ||
      [...favoriteIds].some((id) => !prevFavorites.has(id));

    if (favoritesChanged) {
      gridRef.current?.api?.refreshCells({ columns: ["favorite"] });
      renderedFavoritesRef.current = favoriteIds;
    }
  }, [favoriteIds, spanIdentifiers]);

  const selectedSpanIndex = useMemo(() => {
    if (!viewId) return null;
    const index = spans.findIndex((s) => `${s.trace_id}:${s.span_id}` === viewId);
    return index >= 0 ? index : null;
  }, [viewId, spans]);

  const selectedSpan = selectedSpanIndex !== null ? spans[selectedSpanIndex] : null;
  const detailSheetOpen = !!viewId;

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
    (event: SortChangedEvent<SpanSummary>) => {
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

  const handleSelectionChanged = useCallback((event: SelectionChangedEvent<SpanSummary>) => {
    const selectedRows = event.api.getSelectedRows();
    setRowSelection(selectedRows.map((row) => `${row.trace_id}:${row.span_id}`));
  }, []);

  const handleRowClicked = useCallback(
    (event: RowClickedEvent<SpanSummary>) => {
      // Ignore clicks on checkbox inputs, sheet elements, favorite button, and actions cell
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
        setViewId(`${event.data.trace_id}:${event.data.span_id}`);
      }
    },
    [setViewId],
  );

  const handleNavigatePrev = useCallback(() => {
    if (selectedSpanIndex === null) return;

    if (selectedSpanIndex > 0) {
      const prev = spans[selectedSpanIndex - 1];
      setViewId(`${prev.trace_id}:${prev.span_id}`);
    } else if (page > 1) {
      pendingNavTargetRef.current = "last";
      setPage(page - 1);
    }
  }, [selectedSpanIndex, spans, setViewId, page, setPage]);

  const handleNavigateNext = useCallback(() => {
    if (selectedSpanIndex === null) return;

    if (selectedSpanIndex < spans.length - 1) {
      const next = spans[selectedSpanIndex + 1];
      setViewId(`${next.trace_id}:${next.span_id}`);
    } else if (totalPages && page < totalPages) {
      pendingNavTargetRef.current = "first";
      setPage(page + 1);
    }
  }, [selectedSpanIndex, spans, setViewId, page, totalPages, setPage]);

  const handleCloseSheet = useCallback(
    (open: boolean) => {
      if (!open) {
        setViewId(null);
      }
    },
    [setViewId],
  );

  // Memoize selected row style to avoid creating new objects on every render
  const selectedRowStyle = useMemo<RowStyle>(() => ({ backgroundColor: "var(--accent)" }), []);

  const getRowStyle = useCallback(
    (params: { rowIndex: number | null }) => {
      return params.rowIndex === selectedSpanIndex && detailSheetOpen
        ? selectedRowStyle
        : undefined;
    },
    [selectedSpanIndex, detailSheetOpen, selectedRowStyle],
  );

  useEffect(() => {
    const prev = prevClearingDepsRef.current;
    const currentFiltersKey = JSON.stringify(filters);
    const hasChanged =
      prev.page !== page ||
      prev.pageSize !== pageSize ||
      prev.sortParam !== sortParam ||
      prev.filtersKey !== currentFiltersKey;

    prevClearingDepsRef.current = { page, pageSize, sortParam, filtersKey: currentFiltersKey };

    if (!hasChanged) return;

    setRowSelection([]);
    if (!pendingNavTargetRef.current) {
      setViewId(null);
    }
    gridRef.current?.api?.deselectAll();
  }, [page, pageSize, sortParam, filters, setViewId]);

  // When page changes and we have a pending navigation target, select the appropriate span
  useEffect(() => {
    if (pendingNavTargetRef.current && !isLoading && !isFetching) {
      if (spans.length > 0) {
        if (pendingNavTargetRef.current === "first") {
          const first = spans[0];
          setViewId(`${first.trace_id}:${first.span_id}`);
        } else if (pendingNavTargetRef.current === "last") {
          const last = spans[spans.length - 1];
          setViewId(`${last.trace_id}:${last.span_id}`);
        }
      }
      pendingNavTargetRef.current = null;
    }
  }, [spans, isLoading, isFetching, setViewId]);

  return (
    <div className="h-screen w-full mx-auto pt-header-offset sm:pt-header-offset-sm px-2 sm:px-4">
      <div className="w-full h-full overflow-hidden grid grid-rows-[auto_1fr_auto]">
        <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-2 pb-4">
          <div className="shrink-0">
            <h1 className="text-2xl font-semibold tracking-tight">Spans</h1>
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
              filterConfigs={SPAN_FILTER_CONFIGS}
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
                setViewId(null);
                gridRef.current?.api?.deselectAll();
              }}
              aria-label={favoritesOnly ? "Show all spans" : "Show favorites only"}
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
                    settings.set(SPANS_REALTIME_KEY, next);
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
                    checked={showNonGenAi}
                    onCheckedChange={handleShowNonGenAiToggle}
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
        <AgGridReact<SpanSummary>
          ref={gridRef}
          theme={gridTheme}
          rowData={showEmptyFavorites ? [] : spans}
          columnDefs={visibleColumnDefs}
          defaultColDef={defaultColDef}
          rowSelection={rowSelectionConfig}
          getRowId={(params) => `${params.data.trace_id}:${params.data.span_id}`}
          rowClass="cursor-pointer"
          getRowStyle={getRowStyle}
          context={{
            projectId,
            entityType: "span" as const,
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
          noRowsOverlayComponentParams={{ entityName: "spans", projectId, realtimeEnabled }}
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

      <SpanDetailSheet
        open={detailSheetOpen}
        onOpenChange={handleCloseSheet}
        span={selectedSpan}
        projectId={projectId}
        onNavigatePrev={handleNavigatePrev}
        onNavigateNext={handleNavigateNext}
        hasPrev={selectedSpanIndex !== null && (selectedSpanIndex > 0 || page > 1)}
        hasNext={
          selectedSpanIndex !== null &&
          (selectedSpanIndex < spans.length - 1 || (totalPages != null && page < totalPages))
        }
        realtimeEnabled={realtimeEnabled}
      />

      <DeleteEntityDialog
        entityType="span"
        entityIds={rowSelection}
        projectId={projectId}
        open={deleteDialogOpen}
        onOpenChange={setDeleteDialogOpen}
        onSuccess={handleDeleteSuccess}
      />
    </div>
  );
}
