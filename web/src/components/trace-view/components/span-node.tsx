import { memo } from "react";
import { Handle, Position, type NodeProps, type Node } from "@xyflow/react";
import { cn } from "@/lib/utils";
import { SPAN_TYPE_CONFIG, formatDuration, formatTokens, formatCost } from "../lib/span-config";
import type { SpanType } from "../lib/types";

export interface SpanNodeData extends Record<string, unknown> {
  label: string;
  type: SpanType;
  duration?: number;
  tokens?: number;
  cost?: number;
  startTime?: number;
  isError?: boolean;
  animationState?: "active" | "visited" | "inactive";
}

type SpanNodeType = Node<SpanNodeData, "span">;

export const SpanNode = memo(function SpanNode({ data, selected }: NodeProps<SpanNodeType>) {
  const config = SPAN_TYPE_CONFIG[data.type];
  const Icon = config.icon;

  return (
    <>
      <Handle type="target" position={Position.Left} className="bg-border!" />
      <div
        className={cn(
          "flex min-w-45 flex-col rounded-md border bg-background p-3 shadow-sm transition-all duration-300",
          data.isError && !selected ? "border-destructive/50" : "",
          selected && "ring-2 ring-primary",
          data.animationState === "active" && "ring-2 ring-primary shadow-lg shadow-primary/25",
          data.animationState === "visited" && "border-primary/50 bg-primary/5",
          data.animationState === "inactive" && "opacity-40",
        )}
      >
        <div className="flex items-center gap-2">
          <Icon
            className={cn("h-4 w-4 shrink-0", data.isError ? "text-destructive" : config.accent)}
          />
          <span className="truncate text-sm font-medium">{data.label}</span>
        </div>
        {(data.duration !== undefined ||
          (data.tokens && data.tokens > 0) ||
          (data.cost && data.cost > 0)) && (
          <div className="mt-1 flex flex-wrap gap-x-2 text-xs text-muted-foreground">
            {data.duration !== undefined && <span>{formatDuration(data.duration)}</span>}
            {data.tokens !== undefined && data.tokens > 0 && (
              <span>{formatTokens(0, 0, data.tokens)}</span>
            )}
            {data.cost !== undefined && data.cost > 0 && <span>{formatCost(data.cost)}</span>}
          </div>
        )}
      </div>
      <Handle type="source" position={Position.Right} className="bg-border!" />
    </>
  );
});
