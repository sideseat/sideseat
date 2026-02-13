import { useState, useCallback, useMemo } from "react";
import { useNavigate } from "react-router-dom";
import { ChevronDown, ChevronUp } from "lucide-react";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { formatCompact, formatPercent, formatCurrencyFixed } from "@/lib/format";

interface ModelMixProps {
  projectId: string;
  data: Array<{ model: string | null; tokens: number; cost: number; percentage: number }>;
  totalTokens: number;
  traceCount: number;
  isLoading?: boolean;
}

// Pre-assigned colors for models (uses CSS variables from theme)
const MODEL_COLORS = [
  "var(--chart-1)",
  "var(--chart-2)",
  "var(--chart-3)",
  "var(--chart-4)",
  "var(--chart-5)",
];

export function ModelMix({ projectId, data, totalTokens, traceCount, isLoading }: ModelMixProps) {
  const navigate = useNavigate();
  const [isExpanded, setIsExpanded] = useState(false);

  const handleClick = useCallback(
    (model: string | null) => {
      const filters = model
        ? JSON.stringify([
            {
              type: "string_options",
              column: "gen_ai_request_model",
              operator: "any of",
              value: [model],
            },
          ])
        : "";
      const url = filters
        ? `/projects/${projectId}/observability/spans?filters=${encodeURIComponent(filters)}`
        : `/projects/${projectId}/observability/spans`;
      navigate(url);
    },
    [projectId, navigate],
  );

  const displayData = useMemo(() => {
    if (!data || data.length === 0) return [];
    return isExpanded ? data.slice(0, 5) : data.slice(0, 3);
  }, [data, isExpanded]);

  const ariaLabel = useMemo(
    () =>
      `Model distribution by tokens: ${displayData.map((d) => `${d.model ?? "Unknown"} ${d.percentage}%`).join(", ")}`,
    [displayData],
  );

  const maxVisible = data ? Math.min(5, data.length) : 0;
  const hasMore = data ? data.length > 3 : false;

  if (isLoading) {
    return (
      <Card className="h-full min-h-[280px]">
        <CardHeader className="pb-2">
          <CardTitle className="text-sm font-medium">Model Mix</CardTitle>
          <CardDescription>Token share and spend by model</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-1 flex-col gap-3">
          <Skeleton className="h-4 w-full" />
          <Skeleton className="h-4 w-full" />
          <Skeleton className="h-4 w-full" />
        </CardContent>
      </Card>
    );
  }

  if (displayData.length === 0) {
    return (
      <Card className="h-full min-h-[280px]">
        <CardHeader className="pb-2">
          <CardTitle className="text-sm font-medium">Model Mix</CardTitle>
          <CardDescription>Token share and spend by model</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-1 items-center justify-center">
          <div className="text-sm text-muted-foreground text-center">
            No model data for this period yet.
          </div>
        </CardContent>
      </Card>
    );
  }

  return (
    <Card className="h-full min-h-[280px]">
      <CardHeader className="pb-2">
        <CardTitle className="text-sm font-medium">Model Mix</CardTitle>
        <CardDescription>Token share and spend by model</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-1 flex-col gap-3" role="img" aria-label={ariaLabel}>
        <div className="space-y-3 flex-1">
          {displayData.map((entry, index) => {
            const color = MODEL_COLORS[index % MODEL_COLORS.length];
            const fullName = entry.model ?? "Unknown";
            const rowKey = `${entry.model ?? "unknown"}-${index}`;

            const rowContent = (
              <>
                <div className="flex items-center justify-between gap-2 mb-1">
                  <span className="min-w-0 text-sm font-medium truncate">{fullName}</span>
                  <span className="text-sm text-muted-foreground shrink-0">
                    {formatPercent(entry.percentage)}
                  </span>
                </div>
                <div className="h-2 bg-muted rounded-full overflow-hidden">
                  <div
                    className="h-full rounded-full transition-all"
                    style={{ width: `${entry.percentage}%`, backgroundColor: color }}
                  />
                </div>
                <div className="flex justify-between text-xs text-muted-foreground mt-1">
                  <span>{formatCompact(entry.tokens)} tokens</span>
                  <span className="min-w-[4.5rem] text-right tabular-nums">
                    {formatCurrencyFixed(entry.cost)}
                  </span>
                </div>
              </>
            );

            return (
              <Tooltip key={rowKey}>
                <TooltipTrigger asChild>
                  <button
                    onClick={() => handleClick(entry.model)}
                    className="w-full text-left hover:bg-muted/50 rounded p-1 -m-1 transition-colors"
                  >
                    {rowContent}
                  </button>
                </TooltipTrigger>
                <TooltipContent side="top" className="max-w-sm break-all font-mono text-xs">
                  {fullName}
                </TooltipContent>
              </Tooltip>
            );
          })}
        </div>

        {hasMore && (
          <Button
            variant="ghost"
            size="sm"
            className="w-full text-xs"
            onClick={() => setIsExpanded(!isExpanded)}
          >
            {isExpanded ? (
              <>
                Show less <ChevronUp className="ml-1 h-3 w-3" />
              </>
            ) : (
              <>
                +{maxVisible - 3} more <ChevronDown className="ml-1 h-3 w-3" />
              </>
            )}
          </Button>
        )}

        <div className="mt-auto border-t pt-2 text-xs text-muted-foreground space-y-0.5">
          <div>Total: {formatCompact(totalTokens)} tokens</div>
          {traceCount > 0 && (
            <div>Avg: {formatCompact(Math.round(totalTokens / traceCount))} tokens/trace</div>
          )}
        </div>
      </CardContent>
    </Card>
  );
}
