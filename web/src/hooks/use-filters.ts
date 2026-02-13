import { useCallback, useMemo } from "react";
import { useQueryParam, StringParam } from "use-query-params";
import type { Filter } from "@/api/otel/types";
import { serializeFilters, parseFilters } from "@/lib/filters";

interface UseFiltersOptions {
  paramName?: string;
  onFiltersChange?: () => void;
}

interface UseFiltersReturn {
  filters: Filter[];
  setFilters: (filters: Filter[]) => void;
  clearFilters: () => void;
  hasActiveFilters: boolean;
  activeFilterCount: number;
}

export function useFilters(options: UseFiltersOptions = {}): UseFiltersReturn {
  const { paramName = "filters", onFiltersChange } = options;

  const [filtersParam, setFiltersParam] = useQueryParam(paramName, StringParam);

  const filters = useMemo<Filter[]>(() => {
    if (!filtersParam) return [];
    return parseFilters(filtersParam);
  }, [filtersParam]);

  const setFilters = useCallback(
    (newFilters: Filter[]) => {
      setFiltersParam(newFilters.length > 0 ? serializeFilters(newFilters) : null);
      onFiltersChange?.();
    },
    [setFiltersParam, onFiltersChange],
  );

  const clearFilters = useCallback(() => {
    setFiltersParam(null);
    onFiltersChange?.();
  }, [setFiltersParam, onFiltersChange]);

  const activeFilterCount = new Set(filters.map((f) => f.column)).size;

  return {
    filters,
    setFilters,
    clearFilters,
    hasActiveFilters: activeFilterCount > 0,
    activeFilterCount,
  };
}
