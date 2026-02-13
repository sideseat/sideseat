import { memo } from "react";
import { ChevronRight } from "lucide-react";
import { cn } from "@/lib/utils";
import type { TimelineMetrics } from "@/components/timeline";
import type { TreeNode } from "../lib/types";
import {
  SPAN_TYPE_CONFIG,
  formatDuration,
  formatCost,
  getDurationHeatmapColor,
} from "../lib/span-config";

interface SpanTimelineRowProps {
  node: TreeNode;
  metrics: TimelineMetrics;
  isSelected: boolean;
  isCollapsed: boolean;
  onSelect: (id: string) => void;
  onToggleCollapse: (id: string) => void;
  rootDuration: number;
  showDuration: boolean;
  showCost: boolean;
}

export const SpanTimelineRow = memo(function SpanTimelineRow({
  node,
  metrics,
  isSelected,
  isCollapsed,
  onSelect,
  onToggleCollapse,
  rootDuration,
  showDuration,
  showCost,
}: SpanTimelineRowProps) {
  const config = SPAN_TYPE_CONFIG[node.type];
  const Icon = config.icon;
  const hasChildren = node.children.length > 0;
  const hasError = node.span.status_code === "ERROR";

  const durationColor =
    node.duration !== undefined && rootDuration > 0
      ? getDurationHeatmapColor(node.duration, rootDuration)
      : "text-muted-foreground";

  return (
    <div className="group flex min-w-fit cursor-pointer flex-row items-center py-1 pr-4">
      <div style={{ marginLeft: `${metrics.marginLeft}px` }} onClick={() => onSelect(node.id)}>
        <div
          className={cn(
            "relative flex h-8 items-center rounded-sm border bg-transparent",
            hasError && !isSelected ? "border-destructive/50" : "border-border",
            isSelected
              ? "ring-2 ring-primary"
              : "group-hover:ring-1 group-hover:ring-muted-foreground/50",
          )}
          style={{
            minWidth: `${metrics.barWidth}px`,
          }}
        >
          <div
            className="absolute inset-y-px left-px rounded-sm bg-muted"
            style={{ width: `calc(${metrics.barWidth}px - 4px)` }}
          />

          <div
            className={cn(
              "relative flex flex-row items-center gap-2 px-2 text-xs text-muted-foreground",
              hasChildren && "pl-1",
            )}
          >
            {hasChildren && (
              <button
                type="button"
                className="shrink-0 rounded hover:bg-background/50"
                onClick={(e) => {
                  e.stopPropagation();
                  onToggleCollapse(node.id);
                }}
              >
                <ChevronRight className={cn("h-4 w-4", !isCollapsed && "rotate-90")} />
              </button>
            )}

            <Icon
              className={cn("h-4 w-4 shrink-0", hasError ? "text-destructive" : config.accent)}
            />

            <span className="whitespace-nowrap text-sm font-medium text-foreground">
              {node.name}
            </span>

            {showDuration && node.duration !== undefined && (
              <span className={durationColor}>{formatDuration(node.duration)}</span>
            )}
            {showCost && node.totalCost > 0 && <span>{formatCost(node.totalCost)}</span>}
          </div>
        </div>
      </div>
    </div>
  );
});
