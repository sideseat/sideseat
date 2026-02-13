import { useState, useMemo } from "react";
import { Check, Columns3, Eye, EyeOff, RotateCcw, Search } from "lucide-react";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
  SheetTrigger,
} from "@/components/ui/sheet";

export interface ColumnConfig {
  id: string;
  label: string;
}

interface ColumnSelectorProps {
  columns: readonly ColumnConfig[];
  visibleColumns: string[];
  defaultColumns: readonly string[];
  onVisibilityChange: (columns: string[]) => void;
}

export function ColumnSelector({
  columns,
  visibleColumns,
  defaultColumns,
  onVisibilityChange,
}: ColumnSelectorProps) {
  const [search, setSearch] = useState("");
  const visibleSet = new Set(visibleColumns);

  const filteredColumns = useMemo(() => {
    if (!search.trim()) return columns;
    const q = search.toLowerCase();
    return columns.filter((col) => col.label.toLowerCase().includes(q));
  }, [columns, search]);

  const handleToggle = (columnId: string) => {
    const newSet = new Set(visibleColumns);
    if (newSet.has(columnId)) {
      newSet.delete(columnId);
    } else {
      newSet.add(columnId);
    }
    onVisibilityChange(Array.from(newSet));
  };

  const handleReset = () => {
    onVisibilityChange([...defaultColumns]);
  };

  const handleSelectAll = () => {
    onVisibilityChange(columns.map((col) => col.id));
  };

  const handleDeselectAll = () => {
    onVisibilityChange([]);
  };

  const visibleCount = visibleSet.size;
  const totalCount = columns.length;
  const isDefault =
    visibleCount === defaultColumns.length && defaultColumns.every((id) => visibleSet.has(id));

  const [open, setOpen] = useState(false);

  const handleOpenChange = (isOpen: boolean) => {
    setOpen(isOpen);
    if (!isOpen) {
      setSearch("");
    }
  };

  return (
    <Sheet open={open} onOpenChange={handleOpenChange}>
      <SheetTrigger asChild>
        <Button variant="outline" size="sm" className="gap-2">
          <Columns3 className="h-4 w-4" />
          <span className="hidden sm:inline">Columns</span>
          <span className="tabular-nums text-xs text-muted-foreground">
            {visibleCount}/{totalCount}
          </span>
        </Button>
      </SheetTrigger>
      <SheetContent side="right" className="flex w-full max-w-sm flex-col gap-0 p-0 sm:max-w-md">
        <SheetHeader className="border-b px-6 py-4">
          <SheetTitle className="flex items-center gap-2 text-base font-medium">
            <Columns3 className="h-4 w-4 text-muted-foreground" />
            Table Columns
          </SheetTitle>
          <SheetDescription className="sr-only">Show or hide table columns</SheetDescription>
        </SheetHeader>

        {/* Search and bulk actions */}
        <div className="flex flex-col gap-3 border-b px-6 py-4">
          <div className="relative">
            <Search className="absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
            <Input
              name="column-search"
              placeholder="Search columns..."
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              className="h-9 pl-9 text-sm"
            />
          </div>
          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={handleSelectAll}
              className="inline-flex items-center gap-1.5 rounded-md px-2.5 py-1.5 text-xs font-medium text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
            >
              <Eye className="h-3.5 w-3.5" />
              Show all
            </button>
            <button
              type="button"
              onClick={handleDeselectAll}
              className="inline-flex items-center gap-1.5 rounded-md px-2.5 py-1.5 text-xs font-medium text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
            >
              <EyeOff className="h-3.5 w-3.5" />
              Hide all
            </button>
            <button
              type="button"
              onClick={handleReset}
              disabled={isDefault}
              className="ml-auto inline-flex items-center gap-1.5 rounded-md px-2.5 py-1.5 text-xs font-medium text-muted-foreground transition-colors hover:bg-accent hover:text-foreground disabled:pointer-events-none disabled:opacity-40"
            >
              <RotateCcw className="h-3.5 w-3.5" />
              Reset
            </button>
          </div>
        </div>

        {/* Column list */}
        <div className="flex-1 overflow-y-auto">
          <div className="grid gap-0.5 p-3">
            {filteredColumns.length === 0 ? (
              <div className="py-8 text-center text-sm text-muted-foreground">
                No columns match "{search}"
              </div>
            ) : (
              filteredColumns.map((col) => {
                const isVisible = visibleSet.has(col.id);
                return (
                  <button
                    key={col.id}
                    type="button"
                    onClick={() => handleToggle(col.id)}
                    className={cn(
                      "group flex w-full items-center gap-3 rounded-lg px-3 py-2.5 text-left transition-all",
                      isVisible
                        ? "bg-accent/60 text-foreground"
                        : "text-muted-foreground hover:bg-accent/40 hover:text-foreground",
                    )}
                  >
                    <div
                      className={cn(
                        "flex h-5 w-5 shrink-0 items-center justify-center rounded-md border transition-colors",
                        isVisible
                          ? "border-primary bg-primary text-primary-foreground"
                          : "border-input bg-background group-hover:border-muted-foreground",
                      )}
                    >
                      {isVisible && <Check className="h-3 w-3" />}
                    </div>
                    <span className="flex-1 truncate text-sm font-medium">{col.label}</span>
                    <span
                      className={cn(
                        "text-xs transition-opacity",
                        isVisible
                          ? "text-muted-foreground opacity-100"
                          : "opacity-0 group-hover:opacity-60",
                      )}
                    >
                      {isVisible ? "visible" : "hidden"}
                    </span>
                  </button>
                );
              })
            )}
          </div>
        </div>

        {/* Footer */}
        <div className="flex items-center justify-between border-t bg-muted/30 px-6 py-3 text-xs text-muted-foreground">
          <span>
            {visibleCount} of {totalCount} columns visible
          </span>
          {!isDefault && (
            <span className="rounded-full bg-primary/10 px-2 py-0.5 text-primary">Modified</span>
          )}
        </div>
      </SheetContent>
    </Sheet>
  );
}
