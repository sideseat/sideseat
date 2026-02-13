import { useMemo } from "react";
import {
  ChevronsDownUp,
  ChevronsUpDown,
  FileText,
  Code,
  Wrench,
  MessageSquare,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { ButtonGroup } from "@/components/ui/button-group";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import { formatDuration, formatTokens, formatCost } from "@/lib/format";
import { UsageBreakdownPopover, CostBreakdownPopover } from "@/components/breakdown-popover";
import type { ThreadHeaderProps, ThreadTab } from "./types";

const TABS: { value: ThreadTab; label: string; icon: typeof MessageSquare }[] = [
  { value: "messages", label: "Messages", icon: MessageSquare },
  { value: "tools", label: "Tools", icon: Wrench },
];

export function ThreadHeader({
  metadata,
  tokenBreakdown,
  costBreakdown,
  activeTab,
  onTabChange,
  allExpanded,
  onToggleExpandAll,
  markdownEnabled,
  onMarkdownToggle,
}: ThreadHeaderProps) {
  const stats = useMemo(() => {
    const durationMs =
      metadata?.start_time && metadata?.end_time
        ? new Date(metadata.end_time).getTime() - new Date(metadata.start_time).getTime()
        : null;

    return {
      tokens: tokenBreakdown?.total_tokens ?? metadata?.total_tokens ?? 0,
      cost: costBreakdown?.total_cost ?? metadata?.total_cost ?? 0,
      durationMs,
    };
  }, [metadata, tokenBreakdown, costBreakdown]);

  return (
    <div className="@container shrink-0 flex items-center gap-2 border-b bg-muted/30 px-2 py-1.5 @[400px]:px-3">
      {/* Main tabs */}
      <div className="flex h-7 items-center rounded-md border bg-muted p-0.5">
        {TABS.map((tab) => {
          const Icon = tab.icon;
          const isActive = activeTab === tab.value;
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
              onClick={() => onTabChange(tab.value)}
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
            <span>{formatCost(stats.cost)}</span>
          </CostBreakdownPopover>
        ) : (
          <span>{formatCost(stats.cost)}</span>
        )}
        {stats.tokens > 0 && (
          <>
            <span className="text-border">|</span>
            {tokenBreakdown ? (
              <UsageBreakdownPopover data={tokenBreakdown}>
                <span>{formatTokens(stats.tokens)} tok</span>
              </UsageBreakdownPopover>
            ) : (
              <span>{formatTokens(stats.tokens)} tok</span>
            )}
          </>
        )}
        {stats.durationMs !== null && (
          <>
            <span className="text-border">|</span>
            <span>{formatDuration(stats.durationMs)}</span>
          </>
        )}
      </div>

      {/* Right button group */}
      <ButtonGroup>
        {/* Markdown toggle */}
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="outline"
              size="sm"
              className={cn(
                "h-7 w-7 px-0",
                markdownEnabled && activeTab === "messages" && "bg-muted",
              )}
              onClick={onMarkdownToggle}
              disabled={activeTab === "tools"}
            >
              {markdownEnabled ? (
                <FileText className="h-3.5 w-3.5" />
              ) : (
                <Code className="h-3.5 w-3.5" />
              )}
            </Button>
          </TooltipTrigger>
          <TooltipContent>{markdownEnabled ? "Markdown enabled" : "Raw text"}</TooltipContent>
        </Tooltip>

        {/* Expand/Collapse */}
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="outline"
              size="sm"
              className="h-7 w-7 px-0"
              onClick={onToggleExpandAll}
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
