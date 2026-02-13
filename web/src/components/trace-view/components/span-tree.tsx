import { useCallback } from "react";
import { cn } from "@/lib/utils";
import { TreeView, type TreeNodeState } from "@/components/tree-view";
import type { TreeNode } from "../lib/types";
import {
  SPAN_TYPE_CONFIG,
  getDurationHeatmapColor,
  formatDuration,
  formatCost,
} from "../lib/span-config";
import { useTraceView } from "../contexts/use-trace-view";

export function SpanTree() {
  const {
    filteredTree,
    selectedSpanId,
    setSelectedSpanId,
    collapsedNodes,
    toggleCollapsed,
    rootDuration,
    rootCost,
    showDuration,
    showCost,
  } = useTraceView();

  const getRowClassName = useCallback(
    (node: TreeNode) =>
      node.span.status_code === "ERROR" ? "border border-destructive/50 my-0.5" : undefined,
    [],
  );

  const renderSpanContent = useCallback(
    (node: TreeNode, state: TreeNodeState) => {
      const config = SPAN_TYPE_CONFIG[node.type];
      const Icon = config.icon;
      const hasError = node.span.status_code === "ERROR";

      const durationColor =
        node.duration && rootDuration > 0
          ? getDurationHeatmapColor(node.duration, rootDuration)
          : "text-muted-foreground";

      const costStr = formatCost(node.totalCost);

      return (
        <div className="flex items-center gap-2.5">
          {/* Icon */}
          <Icon className={cn("h-4 w-4 shrink-0", hasError ? "text-destructive" : config.accent)} />

          {/* Content */}
          <div className="flex flex-col gap-0.5">
            <span className="whitespace-nowrap text-sm font-medium">{node.name}</span>

            {(showDuration || showCost) && (
              <div className="flex items-center gap-x-2 whitespace-nowrap text-xs text-muted-foreground">
                {showDuration && node.duration !== undefined && (
                  <span className={cn("shrink-0", durationColor)}>
                    {formatDuration(node.duration)}
                  </span>
                )}
                {showCost && costStr && (
                  <span
                    className={cn(
                      "shrink-0",
                      state.hasChildren &&
                        rootCost > 0 &&
                        getDurationHeatmapColor(node.totalCost, rootCost),
                    )}
                  >
                    {state.hasChildren && "\u2211 "}
                    {costStr}
                  </span>
                )}
              </div>
            )}
          </div>
        </div>
      );
    },
    [rootDuration, rootCost, showDuration, showCost],
  );

  return (
    <TreeView
      data={filteredTree}
      selectedId={selectedSpanId}
      onSelect={setSelectedSpanId}
      collapsedIds={collapsedNodes}
      onToggleCollapse={toggleCollapsed}
      renderContent={renderSpanContent}
      getRowClassName={getRowClassName}
    />
  );
}
