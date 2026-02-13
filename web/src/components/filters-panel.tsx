import { useState, useMemo, useCallback } from "react";
import { RotateCcw, Search, SlidersHorizontal } from "lucide-react";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
  SheetTrigger,
} from "@/components/ui/sheet";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Badge } from "@/components/ui/badge";
import { FilterSection } from "@/components/filter-section";
import type { Filter, FilterOption } from "@/api/otel/types";
import type { FilterConfig } from "@/lib/filters";
import { getFiltersForColumn, updateColumnFilters } from "@/lib/filters";

export interface FiltersPanelProps {
  filters: Filter[];
  onFiltersChange: (filters: Filter[]) => void;
  filterConfigs: readonly FilterConfig[];
  filterOptions?: Record<string, FilterOption[]>;
  trigger?: React.ReactNode;
  onTriggerMouseEnter?: () => void;
  onOpen?: () => void;
  isLoading?: boolean;
}

export function FiltersPanel({
  filters,
  onFiltersChange,
  filterConfigs,
  filterOptions,
  trigger,
  onTriggerMouseEnter,
  onOpen,
  isLoading,
}: FiltersPanelProps) {
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState("");

  const activeCount = new Set(filters.map((f) => f.column)).size;

  const filteredConfigs = useMemo(() => {
    if (!search.trim()) return filterConfigs;
    const lowerSearch = search.toLowerCase();
    return filterConfigs.filter(
      (config) =>
        config.label.toLowerCase().includes(lowerSearch) ||
        config.column.toLowerCase().includes(lowerSearch),
    );
  }, [filterConfigs, search]);

  const handleFilterChange = useCallback(
    (column: string, columnFilters: Filter[]) => {
      const updated = updateColumnFilters(filters, column, columnFilters);
      onFiltersChange(updated);
    },
    [filters, onFiltersChange],
  );

  const handleClearAll = useCallback(() => {
    onFiltersChange([]);
  }, [onFiltersChange]);

  const handleOpenChange = useCallback(
    (isOpen: boolean) => {
      setOpen(isOpen);
      if (isOpen) {
        onOpen?.();
      }
      if (!isOpen) {
        setSearch("");
      }
    },
    [onOpen],
  );

  return (
    <Sheet open={open} onOpenChange={handleOpenChange}>
      <SheetTrigger asChild onMouseEnter={onTriggerMouseEnter}>
        {trigger ?? (
          <Button variant="outline" size="sm" className="gap-2">
            <SlidersHorizontal className="h-4 w-4" />
            <span className="hidden sm:inline">Filters</span>
            {activeCount > 0 && <Badge variant="secondary">{activeCount}</Badge>}
          </Button>
        )}
      </SheetTrigger>
      <SheetContent side="right" className="flex w-full max-w-sm flex-col gap-0 p-0 sm:max-w-md">
        <SheetHeader className="flex h-14 flex-row items-center border-b px-6">
          <SheetTitle className="flex items-center gap-2 text-base font-medium">
            <SlidersHorizontal className="h-4 w-4 text-muted-foreground" />
            Filters
          </SheetTitle>
          <SheetDescription className="sr-only">
            Filter and search through available options
          </SheetDescription>
        </SheetHeader>

        <div className="flex flex-col gap-3 border-b px-6 py-4">
          <div className="relative">
            <Search className="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
            <Input
              name="filter-search"
              placeholder="Search filters..."
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              className="h-9 pl-9 text-sm"
            />
          </div>
          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={handleClearAll}
              disabled={activeCount === 0}
              className="ml-auto inline-flex items-center gap-1.5 rounded-md px-2.5 py-1.5 text-xs font-medium text-muted-foreground transition-colors hover:bg-accent hover:text-foreground disabled:pointer-events-none disabled:opacity-40"
            >
              <RotateCcw className="h-3.5 w-3.5" />
              Clear all
            </button>
          </div>
        </div>

        <div className="flex-1 overflow-y-auto">
          {filteredConfigs.map((config) => (
            <FilterSection
              key={config.id}
              config={config}
              filters={getFiltersForColumn(filters, config.column)}
              options={filterOptions?.[config.id]}
              onChange={(columnFilters) => handleFilterChange(config.column, columnFilters)}
              isLoading={isLoading}
            />
          ))}
          {filteredConfigs.length === 0 && (
            <div className="py-8 text-center text-sm text-muted-foreground">
              No filters match "{search}"
            </div>
          )}
        </div>

        <div className="flex items-center justify-between border-t bg-muted/30 px-6 py-3 text-xs text-muted-foreground">
          <span>
            {activeCount} active filter{activeCount !== 1 ? "s" : ""}
          </span>
          {activeCount > 0 && (
            <span className="rounded-full bg-primary/10 px-2 py-0.5 text-primary">Filtered</span>
          )}
        </div>
      </SheetContent>
    </Sheet>
  );
}
