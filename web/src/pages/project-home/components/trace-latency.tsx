import { useId, useMemo } from "react";
import { Area, AreaChart, ResponsiveContainer, Tooltip, XAxis } from "recharts";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { formatCompact, formatDuration } from "@/lib/format";
import { formatBucketLabel, type TimeRange } from "@/lib/time-range";

interface TraceLatencyProps {
  data: Array<{ bucket: string; avg_duration_ms: number }>;
  avgDurationMs?: number | null;
  traceCount?: number;
  timeRange: TimeRange;
  isLoading?: boolean;
}

export function TraceLatency({
  data,
  avgDurationMs,
  traceCount,
  timeRange,
  isLoading,
}: TraceLatencyProps) {
  const chartId = useId().replace(/:/g, "");
  const gradientId = `gradient-${chartId}`;

  // Sort by bucket ascending (oldest first = left, newest = right)
  // Use index for X-axis positioning to avoid label collision issues
  const chartData = useMemo(() => {
    if (!data || data.length === 0) return [];
    const sorted = [...data].sort((a, b) => a.bucket.localeCompare(b.bucket));
    return sorted.map((entry, index) => {
      const label = formatBucketLabel(entry.bucket, timeRange);
      return {
        ...entry,
        index,
        label: label.short,
        labelLong: label.long,
      };
    });
  }, [data, timeRange]);

  if (isLoading) {
    return (
      <Card className="h-full min-h-70">
        <CardHeader className="pb-2">
          <CardTitle className="text-sm font-medium">Avg Trace Latency</CardTitle>
          <CardDescription>Average duration over time</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-1 flex-col gap-3">
          <Skeleton className="h-44 w-full" />
          <div className="mt-auto w-full border-t pt-2">
            <Skeleton className="h-3 w-24" />
          </div>
        </CardContent>
      </Card>
    );
  }

  const hasData =
    typeof avgDurationMs === "number" &&
    (traceCount ?? 0) > 0 &&
    chartData.some((entry) => entry.avg_duration_ms > 0);

  if (chartData.length === 0 || !hasData) {
    return (
      <Card className="h-full min-h-70">
        <CardHeader className="pb-2">
          <CardTitle className="text-sm font-medium">Avg Trace Latency</CardTitle>
          <CardDescription>Average duration over time</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-1 items-center justify-center">
          <div className="text-sm text-muted-foreground text-center">
            No trace duration for this period yet.
          </div>
        </CardContent>
      </Card>
    );
  }

  return (
    <Card className="h-full min-h-70">
      <CardHeader className="pb-2">
        <CardTitle className="text-sm font-medium">Avg Trace Latency</CardTitle>
        <CardDescription>Average duration over time</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-1 flex-col pt-2">
        <div
          className="flex-1"
          style={{ minHeight: 176 }}
          role="img"
          aria-label={`Average trace latency chart showing ${formatDuration(avgDurationMs ?? 0)} average duration`}
        >
          <ResponsiveContainer width="100%" height={176}>
            <AreaChart
              key={
                chartData.length > 0
                  ? `${chartData[0].bucket}-${chartData[chartData.length - 1].bucket}`
                  : "empty"
              }
              data={chartData}
              margin={{ top: 8, right: 10, left: 0, bottom: 0 }}
            >
              <defs>
                <linearGradient id={gradientId} x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%" style={{ stopColor: "var(--primary)", stopOpacity: 0.35 }} />
                  <stop offset="100%" style={{ stopColor: "var(--primary)", stopOpacity: 0.05 }} />
                </linearGradient>
              </defs>
              <XAxis
                dataKey="index"
                type="number"
                domain={[0, chartData.length - 1]}
                tickLine={false}
                axisLine={false}
                minTickGap={16}
                tick={{ fontSize: 11, fill: "var(--muted-foreground)" }}
                padding={{ left: 10, right: 10 }}
                tickFormatter={(index: number) => chartData[index]?.label ?? ""}
              />
              <Tooltip
                cursor={{ stroke: "var(--border)", strokeDasharray: "3 3" }}
                content={({ active, payload }) => {
                  if (!active || !payload?.length) return null;
                  const entry = payload[0].payload as (typeof chartData)[number];
                  return (
                    <div className="rounded-md border bg-popover px-3 py-2 text-sm shadow-md">
                      <div className="font-medium">{entry.labelLong}</div>
                      <div className="text-muted-foreground">
                        Avg {formatDuration(entry.avg_duration_ms)}
                      </div>
                    </div>
                  );
                }}
              />
              <Area
                type="monotone"
                dataKey="avg_duration_ms"
                stroke="var(--primary)"
                strokeWidth={2}
                fill={`url(#${gradientId})`}
                isAnimationActive={false}
              />
            </AreaChart>
          </ResponsiveContainer>
        </div>
        <div className="mt-2 border-t pt-2 text-xs text-muted-foreground">
          {formatCompact(traceCount ?? 0)} traces
        </div>
      </CardContent>
    </Card>
  );
}
