import { useState, useMemo, useCallback } from "react";
import { ChevronRight, ChevronDown, Copy, Check } from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { useCopy } from "@/hooks";
import {
  transformToRows,
  getChildRows,
  getAllExpandablePaths,
  flattenSingleChildren,
  formatValue,
  type DataInspectorRow,
  type ValueType,
} from "@/lib/data-inspector";

export interface DataInspectorProps {
  data: Record<string, unknown>;
  title?: string;
  defaultExpanded?: boolean | string[];
  maxDepth?: number;
  /** Flatten single-child objects to show value directly */
  flatten?: boolean;
  /** Expand only the last top-level item */
  expandLastItem?: boolean;
  className?: string;
}

const INDENT_PX = 16;
const BASE_INDENT_PX = 8;

function getValueColorClass(type: ValueType): string {
  switch (type) {
    case "string":
      return "text-green-600 dark:text-green-500";
    case "number":
      return "text-blue-600 dark:text-blue-500";
    case "boolean":
      return "text-amber-600 dark:text-amber-500";
    case "null":
      return "text-muted-foreground italic";
    case "object":
    case "array":
      return "text-muted-foreground italic";
    default:
      return "";
  }
}

interface RowProps {
  row: DataInspectorRow;
  isExpanded: boolean;
  onToggle: (path: string) => void;
  maxDepth: number;
  expandedPaths: Set<string>;
}

function Row({ row, isExpanded, onToggle, maxDepth, expandedPaths }: RowProps) {
  const { copied, copy } = useCopy();
  const indentStyle = { paddingLeft: `${row.depth * INDENT_PX + BASE_INDENT_PX}px` };

  const displayValue = formatValue(row.value, row.type);
  const copyValue =
    row.type === "object" || row.type === "array"
      ? JSON.stringify(row.value, null, 2)
      : String(row.value ?? "");

  const handleCopy = useCallback(
    (e: React.MouseEvent) => {
      e.stopPropagation();
      copy(copyValue);
    },
    [copy, copyValue],
  );

  const handleToggle = useCallback(() => {
    if (row.hasChildren) {
      onToggle(row.path);
    }
  }, [row.hasChildren, row.path, onToggle]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (row.hasChildren && (e.key === "Enter" || e.key === " ")) {
        e.preventDefault();
        onToggle(row.path);
      }
    },
    [row.hasChildren, row.path, onToggle],
  );

  const childRows = useMemo(() => {
    if (!isExpanded || !row.hasChildren) return [];
    return getChildRows(row, maxDepth);
  }, [isExpanded, row, maxDepth]);

  return (
    <>
      <div
        className={cn(
          "group/row flex min-h-8 flex-col border-b border-border/50 last:border-b-0 hover:bg-muted/50 @[420px]:flex-row @[420px]:items-stretch",
          row.hasChildren && "cursor-pointer",
        )}
        role={row.hasChildren ? "button" : undefined}
        tabIndex={row.hasChildren ? 0 : undefined}
        onClick={handleToggle}
        onKeyDown={handleKeyDown}
      >
        {/* Path column */}
        <div
          className="flex min-w-0 flex-1 items-center gap-1 py-1.5 pr-2 @[420px]:w-2/5 @[420px]:flex-none"
          style={indentStyle}
        >
          {row.hasChildren ? (
            <span className="flex h-4 w-4 shrink-0 items-center justify-center">
              {isExpanded ? (
                <ChevronDown className="h-3 w-3 text-muted-foreground" />
              ) : (
                <ChevronRight className="h-3 w-3 text-muted-foreground" />
              )}
            </span>
          ) : (
            <span className="w-4 shrink-0" />
          )}
          <span className="break-all text-sm">{row.name}</span>
        </div>

        {/* Value column */}
        <div className="flex min-w-0 flex-1 items-center gap-1 border-t border-border/50 py-1.5 pl-7 pr-2 @[420px]:border-l @[420px]:border-t-0 @[420px]:w-3/5 @[420px]:flex-none @[420px]:pl-3">
          <span className={cn("break-all font-mono text-sm", getValueColorClass(row.type))}>
            {displayValue}
          </span>
          <Button
            variant="ghost"
            size="sm"
            onClick={handleCopy}
            aria-label="Copy value"
            className="ml-auto h-5 w-5 shrink-0 p-0 opacity-0 transition-opacity group-hover/row:opacity-100"
          >
            {copied ? (
              <Check className="h-3 w-3 text-green-600 dark:text-green-500" />
            ) : (
              <Copy className="h-3 w-3" />
            )}
          </Button>
        </div>
      </div>

      {/* Render children if expanded */}
      {isExpanded &&
        childRows.map((childRow) => (
          <Row
            key={childRow.id}
            row={childRow}
            isExpanded={expandedPaths.has(childRow.path)}
            onToggle={onToggle}
            maxDepth={maxDepth}
            expandedPaths={expandedPaths}
          />
        ))}
    </>
  );
}

export function DataInspector({
  data,
  title,
  defaultExpanded = true,
  maxDepth = 10,
  flatten = false,
  expandLastItem = false,
  className,
}: DataInspectorProps) {
  // Apply flattening if enabled
  const processedData = useMemo(
    () => (flatten ? flattenSingleChildren(data) : data),
    [data, flatten],
  );

  const [expandedPaths, setExpandedPaths] = useState<Set<string>>(() => {
    if (expandLastItem) {
      // Expand only the last top-level item
      const keys = Object.keys(processedData);
      if (keys.length > 0) {
        const lastKey = keys[keys.length - 1];
        return new Set([lastKey]);
      }
      return new Set();
    }
    if (defaultExpanded === true) {
      return new Set(getAllExpandablePaths(processedData, "", 0, maxDepth));
    }
    if (Array.isArray(defaultExpanded)) {
      return new Set(defaultExpanded);
    }
    return new Set();
  });

  const toggleExpand = useCallback((path: string) => {
    setExpandedPaths((prev) => {
      const next = new Set(prev);
      if (next.has(path)) {
        next.delete(path);
      } else {
        next.add(path);
      }
      return next;
    });
  }, []);

  const rootRows = useMemo(
    () => transformToRows(processedData, "", 0, maxDepth),
    [processedData, maxDepth],
  );

  if (!data || Object.keys(data).length === 0) {
    return <div className={cn("text-sm text-muted-foreground", className)}>No data available</div>;
  }

  return (
    <div className={cn("@container", className)}>
      {title && (
        <div className="mb-2 text-xs font-medium uppercase tracking-wide text-muted-foreground">
          {title}
        </div>
      )}
      <div className="rounded-md border bg-muted/30">
        {/* Header */}
        <div className="hidden border-b border-border bg-muted/50 text-xs font-medium uppercase tracking-wide text-muted-foreground @[420px]:flex">
          <div className="w-2/5 px-2 py-2">Name</div>
          <div className="w-3/5 border-l border-border/50 px-3 py-2">Value</div>
        </div>

        {/* Rows */}
        <div>
          {rootRows.map((row) => (
            <Row
              key={row.id}
              row={row}
              isExpanded={expandedPaths.has(row.path)}
              onToggle={toggleExpand}
              maxDepth={maxDepth}
              expandedPaths={expandedPaths}
            />
          ))}
        </div>
      </div>
    </div>
  );
}
