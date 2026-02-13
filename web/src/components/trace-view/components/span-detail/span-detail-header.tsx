import { AlertCircle, Clock, Cpu, Coins, Layers } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import type { TreeNode } from "../../lib/types";
import { SPAN_TYPE_CONFIG, formatDuration, formatCost } from "../../lib/span-config";

interface SpanDetailHeaderProps {
  node: TreeNode;
}

interface TagBadgeProps {
  children: React.ReactNode;
  tooltip?: string;
  variant?: "secondary" | "destructive";
  className?: string;
}

function TagBadge({ children, tooltip, variant = "secondary", className }: TagBadgeProps) {
  const badge = (
    <Badge variant={variant} className={cn("gap-1 text-xs font-normal", className)}>
      {children}
    </Badge>
  );

  if (!tooltip) {
    return badge;
  }

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span className="inline-flex">{badge}</span>
      </TooltipTrigger>
      <TooltipContent side="bottom" className="max-w-sm break-all font-mono text-xs">
        {tooltip}
      </TooltipContent>
    </Tooltip>
  );
}

export function SpanDetailHeader({ node }: SpanDetailHeaderProps) {
  const config = SPAN_TYPE_CONFIG[node.type];
  const Icon = config.icon;
  const span = node.span;

  const hasError = span.status_code === "ERROR";
  const hasTokens = span.total_tokens > 0;
  const cost = node.totalCost > 0 ? node.totalCost : span.total_cost;
  const hasCost = cost > 0;

  const tokenDisplay = hasTokens
    ? span.input_tokens > 0 && span.output_tokens > 0
      ? `${span.input_tokens.toLocaleString()} → ${span.output_tokens.toLocaleString()} (Σ ${span.total_tokens.toLocaleString()})`
      : `${span.total_tokens.toLocaleString()} tokens`
    : null;

  return (
    <div className="@container space-y-2 border-b bg-background px-3 py-2.5 @[400px]:px-4 @[400px]:py-3">
      {/* Title row */}
      <div className="flex min-w-0 items-center gap-2">
        <Icon className={cn("h-4 w-4 shrink-0 @[400px]:h-5 @[400px]:w-5", config.accent)} />
        <h3 className="min-w-0 truncate text-sm font-semibold @[400px]:text-base">{node.name}</h3>
      </div>

      {/* Tags cloud */}
      <div className="flex flex-wrap items-center gap-1.5">
        <TagBadge>{config.label}</TagBadge>

        {span.framework && (
          <TagBadge>
            <Layers className="h-3 w-3" />
            {span.framework}
          </TagBadge>
        )}

        {node.duration !== undefined && (
          <TagBadge className="font-mono">
            <Clock className="h-3 w-3" />
            {formatDuration(node.duration)}
          </TagBadge>
        )}

        {tokenDisplay && (
          <TagBadge className="font-mono">
            <Cpu className="h-3 w-3" />
            {tokenDisplay}
          </TagBadge>
        )}

        {hasCost && (
          <TagBadge className="font-mono">
            <Coins className="h-3 w-3" />
            {formatCost(cost)}
          </TagBadge>
        )}

        {hasError && (
          <TagBadge variant="destructive">
            <AlertCircle className="h-3 w-3" />
            {span.status_code}
          </TagBadge>
        )}

        {span.model && (
          <TagBadge className="max-w-48 font-mono @[500px]:max-w-64" tooltip={span.model}>
            <span className="truncate">{span.model}</span>
          </TagBadge>
        )}

        {span.finish_reasons && span.finish_reasons.length > 0 && (
          <TagBadge>{span.finish_reasons.join(", ")}</TagBadge>
        )}

        {span.environment && <TagBadge>{span.environment}</TagBadge>}
      </div>
    </div>
  );
}
