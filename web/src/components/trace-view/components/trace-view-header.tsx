import {
  List,
  GanttChart,
  GitBranch,
  ChevronsDownUp,
  ChevronsUpDown,
  PanelLeftDashed,
  PanelTopDashed,
  Layers,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { ButtonGroup } from "@/components/ui/button-group";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import { formatDuration, formatTokens, formatCost } from "@/lib/format";
import {
  UsageBreakdownPopover,
  CostBreakdownPopover,
  type TokenBreakdown,
  type CostBreakdown,
} from "@/components/breakdown-popover";
import type { ViewMode, LayoutDirection } from "../lib/types";

interface TraceViewHeaderProps {
  viewMode: ViewMode;
  onViewModeChange: (mode: ViewMode) => void;
  layoutDirection: LayoutDirection;
  onLayoutDirectionChange: (direction: LayoutDirection) => void;
  duration: number;
  tokenBreakdown?: TokenBreakdown;
  costBreakdown?: CostBreakdown;
  allExpanded: boolean;
  onToggleExpandAll: () => void;
  showNonGenAiSpans: boolean;
  onShowNonGenAiSpansChange: (show: boolean) => void;
}

const VIEW_TABS: { value: ViewMode; label: string; icon: typeof List }[] = [
  { value: "tree", label: "Tree", icon: List },
  { value: "timeline", label: "Timeline", icon: GanttChart },
  { value: "diagram", label: "Diagram", icon: GitBranch },
];

export function TraceViewHeader({
  viewMode,
  onViewModeChange,
  layoutDirection,
  onLayoutDirectionChange,
  duration,
  tokenBreakdown,
  costBreakdown,
  allExpanded,
  onToggleExpandAll,
  showNonGenAiSpans,
  onShowNonGenAiSpansChange,
}: TraceViewHeaderProps) {
  const toggleLayout = () => {
    onLayoutDirectionChange(layoutDirection === "horizontal" ? "vertical" : "horizontal");
  };

  const totalTokens = tokenBreakdown?.total_tokens ?? 0;
  const totalCost = costBreakdown?.total_cost ?? 0;

  return (
    <div className="@container shrink-0 flex items-center gap-2 border-b bg-muted/30 px-2 py-1.5 @[400px]:px-3">
      {/* View mode tabs */}
      <div className="flex h-7 items-center rounded-md border bg-muted p-0.5">
        {VIEW_TABS.map((tab) => {
          const Icon = tab.icon;
          const isActive = viewMode === tab.value;
          return (
            <button
              key={tab.value}
              type="button"
              className={cn(
                "flex h-6 items-center justify-center rounded px-1.5 text-xs font-medium transition-all @[500px]:gap-1.5 @[500px]:px-2",
                "focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
                isActive
                  ? "bg-background text-foreground shadow-sm"
                  : "text-muted-foreground hover:text-foreground",
              )}
              onClick={() => onViewModeChange(tab.value)}
            >
              <Icon className="h-3.5 w-3.5" />
              <span className="hidden @[500px]:inline">{tab.label}</span>
            </button>
          );
        })}
      </div>

      <div className="flex-1" />

      {/* Stats */}
      <div className="hidden items-center gap-2 text-xs text-muted-foreground whitespace-nowrap @[550px]:flex">
        {costBreakdown ? (
          <CostBreakdownPopover data={costBreakdown}>
            <span>{formatCost(totalCost)}</span>
          </CostBreakdownPopover>
        ) : (
          <span>{formatCost(totalCost)}</span>
        )}
        {totalTokens > 0 && tokenBreakdown && (
          <>
            <span className="text-border">|</span>
            <UsageBreakdownPopover data={tokenBreakdown}>
              <span>{formatTokens(totalTokens)} tok</span>
            </UsageBreakdownPopover>
          </>
        )}
        {duration > 0 && (
          <>
            <span className="text-border">|</span>
            <span>{formatDuration(duration)}</span>
          </>
        )}
      </div>

      {/* Right button group */}
      <ButtonGroup>
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="outline"
              size="sm"
              className={cn("h-7 w-7 px-0", !showNonGenAiSpans && "bg-muted")}
              onClick={() => onShowNonGenAiSpansChange(!showNonGenAiSpans)}
            >
              <Layers className="h-3.5 w-3.5" />
            </Button>
          </TooltipTrigger>
          <TooltipContent>
            {showNonGenAiSpans ? "Show GenAI only" : "Show all spans"}
          </TooltipContent>
        </Tooltip>

        <Tooltip>
          <TooltipTrigger asChild>
            <Button variant="outline" size="sm" className="h-7 w-7 px-0" onClick={toggleLayout}>
              {layoutDirection === "horizontal" ? (
                <PanelLeftDashed className="h-3.5 w-3.5" />
              ) : (
                <PanelTopDashed className="h-3.5 w-3.5" />
              )}
            </Button>
          </TooltipTrigger>
          <TooltipContent>
            {layoutDirection === "horizontal" ? "Vertical layout" : "Horizontal layout"}
          </TooltipContent>
        </Tooltip>

        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="outline"
              size="sm"
              className="h-7 w-7 px-0"
              onClick={onToggleExpandAll}
              disabled={viewMode === "diagram"}
            >
              {allExpanded ? (
                <ChevronsDownUp className="h-3.5 w-3.5" />
              ) : (
                <ChevronsUpDown className="h-3.5 w-3.5" />
              )}
            </Button>
          </TooltipTrigger>
          <TooltipContent>{allExpanded ? "Collapse all" : "Expand all"}</TooltipContent>
        </Tooltip>
      </ButtonGroup>
    </div>
  );
}
