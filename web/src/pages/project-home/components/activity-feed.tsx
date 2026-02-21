import { useState, useEffect, useRef } from "react";
import { Link } from "react-router";
import { ArrowUpRight } from "lucide-react";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { formatRelativeTime, formatCost, formatCompact } from "@/lib/format";
import type { TraceSummary } from "@/api/otel/types";
import { cn } from "@/lib/utils";

/** Auto-updating relative time display */
function RelativeTime({ date }: { date: Date | string }) {
  const [, setTick] = useState(0);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Convert date to timestamp string for stable dependency
  const dateKey = typeof date === "string" ? date : date.toISOString();

  useEffect(() => {
    const tick = () => {
      const then = typeof date === "string" ? new Date(date) : date;
      const ageMs = Date.now() - then.getTime();
      // Interval: <1m = 10s, <1h = 30s, else 60s
      const interval = ageMs < 60_000 ? 10_000 : ageMs < 3_600_000 ? 30_000 : 60_000;

      timerRef.current = setTimeout(() => {
        setTick((t) => t + 1);
        tick();
      }, interval);
    };

    tick();
    return () => {
      if (timerRef.current) clearTimeout(timerRef.current);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- dateKey is derived from date, ensures stable comparison
  }, [dateKey]);

  return <>{formatRelativeTime(date)}</>;
}

interface ActivityFeedProps {
  projectId: string;
  traces: TraceSummary[];
  recentCount?: number;
  isLoading?: boolean;
}

export function ActivityFeed({ projectId, traces, recentCount, isLoading }: ActivityFeedProps) {
  if (isLoading) {
    return (
      <Card className="h-full min-h-70">
        <CardHeader className="pb-2">
          <CardTitle className="text-sm font-medium">Recent Activity</CardTitle>
        </CardHeader>
        <CardContent className="flex flex-1 flex-col space-y-2">
          {[1, 2, 3].map((i) => (
            <div key={i} className="flex items-center gap-3 py-2">
              <Skeleton className="h-2 w-2 rounded-full" />
              <Skeleton className="h-4 w-12" />
              <Skeleton className="h-4 flex-1" />
              <Skeleton className="h-4 w-14" />
              <Skeleton className="h-4 w-16" />
            </div>
          ))}
        </CardContent>
      </Card>
    );
  }

  if (!traces || traces.length === 0) {
    return (
      <Card className="h-full min-h-70">
        <CardHeader className="pb-2">
          <CardTitle className="text-sm font-medium">Recent Activity</CardTitle>
          <CardDescription>Latest traces across your project</CardDescription>
          {typeof recentCount === "number" && (
            <CardAction>
              <Badge variant="secondary">{formatCompact(recentCount)} in last 5m</Badge>
            </CardAction>
          )}
        </CardHeader>
        <CardContent className="flex flex-1 items-center justify-center">
          <div className="text-center text-muted-foreground">
            <p>No recent traces</p>
            <p className="text-sm mt-1">Activity will appear here as traces are received</p>
          </div>
        </CardContent>
      </Card>
    );
  }

  return (
    <Card className="h-full min-h-70">
      <CardHeader className="pb-2">
        <CardTitle className="text-sm font-medium">Recent Activity</CardTitle>
        <CardDescription>Latest traces across your project</CardDescription>
        {typeof recentCount === "number" && (
          <CardAction>
            <Badge variant="secondary">{formatCompact(recentCount)} in last 5m</Badge>
          </CardAction>
        )}
      </CardHeader>
      <CardContent className="flex flex-1 flex-col">
        <div className="space-y-1 flex-1">
          {traces.map((trace) => {
            const isInProgress = trace.end_time === null;
            const activityTime = trace.end_time ?? trace.start_time;

            return (
              <Link
                key={trace.trace_id}
                to={`/projects/${projectId}/observability/traces/${trace.trace_id}`}
                className="flex items-center gap-3 py-2 px-1 -mx-1 rounded hover:bg-muted/50 transition-colors"
              >
                <div
                  className={cn(
                    "h-2 w-2 rounded-full shrink-0",
                    isInProgress ? "bg-emerald-500 animate-pulse" : "bg-muted-foreground/50",
                  )}
                  role="img"
                  aria-label={isInProgress ? "In progress" : "Complete"}
                />
                <span className="text-xs text-muted-foreground w-14 shrink-0">
                  <RelativeTime date={activityTime} />
                </span>
                <span
                  className="text-sm truncate flex-1"
                  title={trace.trace_name ?? "Unnamed trace"}
                >
                  {trace.trace_name ?? "Unnamed trace"}
                </span>
                <span className="text-xs text-muted-foreground w-20 text-right shrink-0 tabular-nums whitespace-nowrap">
                  {formatCost(trace.total_cost)}
                </span>
                <span className="text-xs text-muted-foreground w-16 text-right shrink-0">
                  {formatCompact(trace.total_tokens)} tok
                </span>
              </Link>
            );
          })}
        </div>

        <Button asChild variant="ghost" size="sm" className="w-full mt-2">
          <Link to={`/projects/${projectId}/observability/traces`}>
            View All
            <ArrowUpRight className="ml-1 h-4 w-4" />
          </Link>
        </Button>
      </CardContent>
    </Card>
  );
}
