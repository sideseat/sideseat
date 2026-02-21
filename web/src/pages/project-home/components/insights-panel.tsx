import { Activity, Database, DollarSign, Hash } from "lucide-react";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import type { ProjectStats } from "@/api/otel/types";
import { formatCompact, formatCurrencyFixed, formatPercent } from "@/lib/format";

interface InsightsPanelProps {
  stats?: ProjectStats;
  isLoading?: boolean;
}

interface InsightItem {
  label: string;
  value: string;
  detail?: string;
  icon: React.ReactNode;
}

function InsightCard({ label, value, detail, icon }: InsightItem) {
  return (
    <div className="flex h-full flex-col rounded-xl border border-border/60 bg-background/70 p-3 shadow-sm">
      <div className="flex items-center justify-between text-[10px] font-semibold uppercase tracking-wide text-muted-foreground">
        <span>{label}</span>
        <span className="text-muted-foreground">{icon}</span>
      </div>
      <div
        className="mt-2 text-xl font-semibold tracking-tight tabular-nums truncate"
        title={value}
      >
        {value}
      </div>
      {detail && <div className="mt-auto text-xs text-muted-foreground">{detail}</div>}
    </div>
  );
}

export function InsightsPanel({ stats, isLoading }: InsightsPanelProps) {
  if (isLoading) {
    return (
      <Card className="h-full min-h-70">
        <CardHeader className="pb-2">
          <CardTitle className="text-sm font-medium">Insights</CardTitle>
          <CardDescription>Key signals from the selected time range</CardDescription>
        </CardHeader>
        <CardContent className="grid flex-1 gap-3 sm:grid-cols-2 sm:auto-rows-fr">
          {[1, 2, 3, 4].map((i) => (
            <div
              key={i}
              className="h-full rounded-xl border border-border/60 bg-background/70 p-3 flex flex-col"
            >
              <Skeleton className="h-3 w-20" />
              <Skeleton className="mt-3 h-6 w-24" />
              <Skeleton className="mt-auto h-3 w-28" />
            </div>
          ))}
        </CardContent>
      </Card>
    );
  }

  if (!stats) {
    return (
      <Card className="h-full min-h-70">
        <CardHeader className="pb-2">
          <CardTitle className="text-sm font-medium">Insights</CardTitle>
          <CardDescription>Key signals from the selected time range</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-1 items-center justify-center">
          <div className="text-sm text-muted-foreground text-center">
            No insights available yet.
          </div>
        </CardContent>
      </Card>
    );
  }

  const traces = stats.counts.traces;
  const avgCost = traces > 0 ? stats.costs.total / traces : 0;
  const avgTokens = traces > 0 ? stats.tokens.total / traces : 0;
  const cacheTokens = stats.tokens.cache_read + stats.tokens.cache_write;
  const cacheShare = stats.tokens.total > 0 ? (cacheTokens / stats.tokens.total) * 100 : 0;
  const items: InsightItem[] = [
    {
      label: "Live traces (5m)",
      value: formatCompact(stats.recent_activity_count),
      detail: "last 5 minutes",
      icon: <Activity className="h-4 w-4" />,
    },
    {
      label: "Avg cost / trace",
      value: formatCurrencyFixed(avgCost),
      detail: "based on total cost",
      icon: <DollarSign className="h-4 w-4" />,
    },
    {
      label: "Avg tokens / trace",
      value: formatCompact(Math.round(avgTokens)),
      detail: "tokens per trace",
      icon: <Hash className="h-4 w-4" />,
    },
    {
      label: "Cache share",
      value: formatPercent(cacheShare),
      detail: `${formatCompact(cacheTokens)} cached tokens`,
      icon: <Database className="h-4 w-4" />,
    },
  ];

  return (
    <Card className="h-full min-h-70">
      <CardHeader className="pb-2">
        <CardTitle className="text-sm font-medium">Insights</CardTitle>
        <CardDescription>Key signals from the selected time range</CardDescription>
      </CardHeader>
      <CardContent className="grid flex-1 gap-3 sm:grid-cols-2 sm:auto-rows-fr">
        {items.map((item) => (
          <InsightCard key={item.label} {...item} />
        ))}
      </CardContent>
    </Card>
  );
}
