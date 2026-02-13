import { useNavigate } from "react-router-dom";
import { useState, useEffect, useMemo, useCallback } from "react";
import { PieChart, Pie, Cell, ResponsiveContainer, Tooltip } from "recharts";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { formatPercent } from "@/lib/format";

interface FrameworkChartProps {
  projectId: string;
  data: Array<{ framework: string | null; count: number; percentage: number }>;
  isLoading?: boolean;
}

// CSS variable names for chart colors
const CHART_COLOR_VARS = [
  "--chart-1",
  "--chart-2",
  "--chart-3",
  "--chart-4",
  "--chart-5",
  "--chart-6",
  "--chart-7",
  "--chart-8",
  "--chart-9",
  "--chart-10",
];

// Default fallback colors (used before CSS variables are resolved)
const DEFAULT_COLORS = [
  "#4a90a4",
  "#3d7a8c",
  "#5ba0b4",
  "#2d6a7c",
  "#6bb0c4",
  "#448494",
  "#5898a8",
  "#387080",
  "#64a8b8",
  "#2e5a6a",
];

// Hook to resolve CSS variables to actual color values (needed for SVG fills)
function useChartColors() {
  const [colors, setColors] = useState<string[]>(DEFAULT_COLORS);

  useEffect(() => {
    const computeColors = () => {
      const styles = getComputedStyle(document.documentElement);
      const resolvedColors = CHART_COLOR_VARS.map((varName) => {
        const value = styles.getPropertyValue(varName).trim();
        return value || "#888888";
      });
      // Only update state if colors actually changed
      setColors((prev) => {
        const changed = resolvedColors.some((c, i) => c !== prev[i]);
        return changed ? resolvedColors : prev;
      });
    };

    computeColors();

    // Re-compute on theme changes
    const observer = new MutationObserver(computeColors);
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["class"],
    });

    return () => observer.disconnect();
  }, []);

  return colors;
}

export function FrameworkChart({ projectId, data, isLoading }: FrameworkChartProps) {
  const navigate = useNavigate();
  const chartColors = useChartColors();

  const chartData = useMemo(() => {
    if (!data || data.length === 0) return [];
    return data.map((d, i) => ({
      name: d.framework ?? "Other",
      value: d.count,
      percentage: d.percentage,
      color: chartColors[i % chartColors.length] || "#888888",
      framework: d.framework,
    }));
  }, [data, chartColors]);

  const handleClick = useCallback(
    (framework: string | null) => {
      const filters = framework
        ? JSON.stringify([
            { type: "string_options", column: "framework", operator: "any of", value: [framework] },
          ])
        : "";
      const url = filters
        ? `/projects/${projectId}/observability/spans?filters=${encodeURIComponent(filters)}`
        : `/projects/${projectId}/observability/spans`;
      navigate(url);
    },
    [projectId, navigate],
  );

  const ariaLabel = useMemo(
    () =>
      `Framework distribution: ${chartData.map((d) => `${d.name} ${d.percentage}%`).join(", ")}`,
    [chartData],
  );

  if (isLoading) {
    return (
      <Card className="h-full min-h-70">
        <CardHeader className="pb-2">
          <CardTitle className="text-sm font-medium">Framework Distribution</CardTitle>
          <CardDescription>Share of traces by framework</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-1 items-center">
          <div className="flex items-center gap-4 w-full">
            <Skeleton className="h-32 w-32 rounded-full" />
            <div className="space-y-2 flex-1">
              <Skeleton className="h-4 w-full" />
              <Skeleton className="h-4 w-3/4" />
              <Skeleton className="h-4 w-1/2" />
            </div>
          </div>
        </CardContent>
      </Card>
    );
  }

  if (chartData.length === 0) {
    return (
      <Card className="h-full min-h-70">
        <CardHeader className="pb-2">
          <CardTitle className="text-sm font-medium">Framework Distribution</CardTitle>
          <CardDescription>Share of traces by framework</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-1 items-center justify-center">
          <div className="text-sm text-muted-foreground text-center">
            No framework data for this period yet.
          </div>
        </CardContent>
      </Card>
    );
  }

  return (
    <Card className="h-full min-h-70">
      <CardHeader className="pb-2">
        <CardTitle className="text-sm font-medium">Framework Distribution</CardTitle>
        <CardDescription>Share of traces by framework</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-1 items-center">
        <div className="flex items-center gap-4 w-full" role="img" aria-label={ariaLabel}>
          <div style={{ width: 128, height: 128 }}>
            <ResponsiveContainer width={128} height={128}>
              <PieChart>
                <Pie
                  data={chartData}
                  cx="50%"
                  cy="50%"
                  innerRadius={35}
                  outerRadius={55}
                  paddingAngle={2}
                  dataKey="value"
                  onClick={(_, index) => handleClick(chartData[index].framework)}
                  style={{ cursor: "pointer" }}
                >
                  {chartData.map((entry, index) => (
                    <Cell key={`cell-${index}`} style={{ fill: entry.color }} />
                  ))}
                </Pie>
                <Tooltip
                  content={({ active, payload }) => {
                    if (active && payload && payload.length) {
                      const data = payload[0].payload;
                      return (
                        <div className="bg-popover border rounded-md px-3 py-2 shadow-md text-sm">
                          <div className="font-medium">{data.name}</div>
                          <div className="text-muted-foreground">
                            {data.value} traces ({formatPercent(data.percentage)})
                          </div>
                        </div>
                      );
                    }
                    return null;
                  }}
                />
              </PieChart>
            </ResponsiveContainer>
          </div>
          <div className="flex-1 space-y-1.5">
            {chartData.slice(0, 5).map((entry, index) => (
              <button
                key={`${entry.framework ?? "other"}-${index}`}
                onClick={() => handleClick(entry.framework)}
                className="flex items-center gap-2 w-full text-left hover:bg-muted/50 rounded px-1 py-0.5 transition-colors"
              >
                <div className="h-3 w-3 rounded-sm" style={{ backgroundColor: entry.color }} />
                <span className="text-sm truncate flex-1">{entry.name}</span>
                <span className="text-sm text-muted-foreground">
                  {formatPercent(entry.percentage)}
                </span>
              </button>
            ))}
            {chartData.length > 5 && (
              <div className="text-xs text-muted-foreground pl-1">+{chartData.length - 5} more</div>
            )}
          </div>
        </div>
      </CardContent>
    </Card>
  );
}
