import { useState, useEffect } from "react";
import { Check, X, Pencil, Trash2 } from "lucide-react";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { formatCurrencyFixed } from "@/lib/format";
import { getTimeRangeLabel, type TimeRange } from "@/lib/time-range";
import { cn } from "@/lib/utils";

interface FuelGaugeProps {
  projectId: string;
  timeRange: TimeRange;
  costs: {
    input: number;
    output: number;
    cache_read: number;
    cache_write: number;
    reasoning: number;
    total: number;
  };
  isLoading?: boolean;
}

function getBudgetStorageKey(projectId: string): string {
  return `sideseat_budget_${projectId}`;
}

function loadBudget(projectId: string): number | null {
  if (typeof localStorage === "undefined") return null;
  const stored = localStorage.getItem(getBudgetStorageKey(projectId));
  if (stored === null) return null;
  const parsed = parseFloat(stored);
  return isNaN(parsed) ? null : parsed;
}

function saveBudget(projectId: string, budget: number | null): void {
  if (typeof localStorage === "undefined") return;
  if (budget === null) {
    localStorage.removeItem(getBudgetStorageKey(projectId));
  } else {
    localStorage.setItem(getBudgetStorageKey(projectId), budget.toString());
  }
}

export function FuelGauge({ projectId, timeRange, costs, isLoading }: FuelGaugeProps) {
  const [budget, setBudget] = useState<number | null>(null);
  const [isEditing, setIsEditing] = useState(false);
  const [editValue, setEditValue] = useState("");

  // Load budget from localStorage on mount
  useEffect(() => {
    setBudget(loadBudget(projectId));
  }, [projectId]);

  const handleSetBudget = () => {
    // Default to $50 for new budgets as a reasonable starting suggestion
    setEditValue(budget?.toString() ?? "50");
    setIsEditing(true);
  };

  const parsedEditValue = parseFloat(editValue);
  const isValidBudget = !isNaN(parsedEditValue) && parsedEditValue > 0 && parsedEditValue <= 999999;

  const handleSave = () => {
    if (isValidBudget) {
      const rounded = Math.round(parsedEditValue * 100) / 100;
      setBudget(rounded);
      saveBudget(projectId, rounded);
      setIsEditing(false);
    }
  };

  const handleCancel = () => {
    setIsEditing(false);
    setEditValue("");
  };

  const handleClear = () => {
    setBudget(null);
    saveBudget(projectId, null);
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter") handleSave();
    if (e.key === "Escape") handleCancel();
  };

  if (isLoading) {
    return (
      <Card className="h-full min-h-70">
        <CardHeader className="pb-2">
          <CardTitle className="text-sm font-medium">Cost</CardTitle>
          <CardDescription>Loading budget status</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-1 flex-col pt-2">
          {/* Primary metric skeleton */}
          <div className="space-y-1">
            <Skeleton className="h-9 w-28" />
            <Skeleton className="h-3 w-20" />
          </div>

          {/* Budget section skeleton */}
          <div className="mt-4 space-y-2">
            <Skeleton className="h-2.5 w-full rounded-full" />
            <div className="flex items-center justify-between">
              <Skeleton className="h-3 w-40" />
              <Skeleton className="h-8 w-24" />
            </div>
          </div>

          {/* Footer skeleton */}
          <div className="mt-auto border-t pt-2">
            <div className="flex flex-wrap gap-x-3 gap-y-1">
              <Skeleton className="h-3 w-20" />
              <Skeleton className="h-3 w-20" />
              <Skeleton className="h-3 w-20" />
            </div>
          </div>
        </CardContent>
      </Card>
    );
  }

  // Always show total spend for all time ranges
  const spendLabel = "Total spend";
  const spendValue = costs.total;

  const percentage = budget ? Math.min(100, (costs.total / budget) * 100) : 0;
  const isOverBudget = budget ? costs.total > budget : false;
  const cacheTotal = costs.cache_read + costs.cache_write;

  return (
    <Card className="h-full min-h-70">
      <CardHeader className="pb-2">
        <CardTitle className="text-sm font-medium">Cost</CardTitle>
        <CardDescription>{getTimeRangeLabel(timeRange)}</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-1 flex-col pt-2">
        {/* Primary metric - prominent display */}
        <div>
          <div className="text-3xl font-semibold tabular-nums">
            {formatCurrencyFixed(spendValue)}
          </div>
          <div className="text-xs text-muted-foreground">{spendLabel}</div>
        </div>

        {/* Budget section */}
        <div className="mt-4 mb-4 space-y-2">
          <div className="h-2.5 bg-muted rounded-full overflow-hidden">
            <div
              className={cn(
                "h-full rounded-full transition-all",
                !budget
                  ? "bg-transparent"
                  : isOverBudget
                    ? "bg-destructive"
                    : percentage > 75
                      ? "bg-linear-to-r from-amber-500 to-red-500"
                      : percentage > 50
                        ? "bg-linear-to-r from-emerald-500 to-amber-500"
                        : "bg-emerald-500",
              )}
              style={{ width: budget ? `${Math.min(percentage, 100)}%` : "0%" }}
            />
          </div>

          {isEditing ? (
            <div className="flex items-center gap-2">
              <label htmlFor="budget-input" className="text-xs text-muted-foreground">
                Budget:
              </label>
              <span className="text-muted-foreground" aria-hidden="true">
                $
              </span>
              <Input
                id="budget-input"
                type="number"
                value={editValue}
                onChange={(e) => setEditValue(e.target.value)}
                onKeyDown={handleKeyDown}
                placeholder="50.00"
                className="h-7 w-20 text-right text-sm"
                min={0}
                max={999999}
                step={0.01}
                autoFocus
                aria-label="Budget amount in dollars"
              />
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label="Save budget"
                title="Save budget"
                onClick={handleSave}
                disabled={!isValidBudget}
              >
                <Check className="h-4 w-4" />
              </Button>
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label="Cancel budget edit"
                title="Cancel budget edit"
                onClick={handleCancel}
              >
                <X className="h-4 w-4" />
              </Button>
            </div>
          ) : budget ? (
            <div className="flex items-center justify-between">
              <div
                className={cn(
                  "text-xs",
                  isOverBudget ? "text-destructive" : "text-muted-foreground",
                )}
              >
                {isOverBudget
                  ? `Over budget by ${formatCurrencyFixed(costs.total - budget)}`
                  : `${formatCurrencyFixed(costs.total)} of ${formatCurrencyFixed(budget)} (${percentage.toFixed(1)}%)`}
              </div>
              <div className="flex items-center gap-1">
                <Button
                  variant="ghost"
                  size="icon-sm"
                  aria-label="Edit budget"
                  title="Edit budget"
                  onClick={handleSetBudget}
                >
                  <Pencil className="h-3.5 w-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon-sm"
                  aria-label="Clear budget"
                  title="Clear budget"
                  onClick={handleClear}
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </Button>
              </div>
            </div>
          ) : (
            <div className="flex items-center justify-between gap-2">
              <span className="text-xs text-muted-foreground">No budget set</span>
              <Button variant="outline" size="sm" onClick={handleSetBudget}>
                Set budget
              </Button>
            </div>
          )}
        </div>

        {/* Cost breakdown footer */}
        <div className="mt-auto border-t pt-2 text-xs text-muted-foreground">
          <div className="flex flex-wrap gap-x-3 gap-y-1">
            <span>Input: {formatCurrencyFixed(costs.input)}</span>
            <span>Output: {formatCurrencyFixed(costs.output)}</span>
            <span>Cache: {formatCurrencyFixed(cacheTotal)}</span>
          </div>
        </div>
      </CardContent>
    </Card>
  );
}
