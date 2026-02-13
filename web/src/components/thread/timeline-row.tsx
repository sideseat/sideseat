import { useState, useCallback, useMemo, useEffect } from "react";
import {
  Copy,
  Check,
  ChevronRight,
  User,
  Bot,
  Settings,
  Wrench,
  CornerDownRight,
  Brain,
  ListTree,
  AlertCircle,
  HelpCircle,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger, TooltipProvider } from "@/components/ui/tooltip";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { cn } from "@/lib/utils";
import type { Block } from "@/api/otel/types";

// Role-based configuration (primary)
const ROLE_CONFIG: Record<
  string,
  { icon: typeof User; label: string; accent: string; showMetadata: boolean }
> = {
  system: {
    icon: Settings,
    label: "System",
    accent: "text-purple-600 dark:text-purple-400",
    showMetadata: true,
  },
  user: {
    icon: User,
    label: "User",
    accent: "text-blue-600 dark:text-blue-400",
    showMetadata: true,
  },
  assistant: {
    icon: Bot,
    label: "Assistant",
    accent: "text-emerald-600 dark:text-emerald-400",
    showMetadata: true,
  },
  tool: {
    icon: CornerDownRight,
    label: "Tool Result",
    accent: "text-teal-600 dark:text-teal-400",
    showMetadata: false,
  },
};

// Special entry types that override role-based labels
const SPECIAL_ENTRY_CONFIG: Record<
  string,
  { icon: typeof User; label: string; accent: string; showMetadata: boolean }
> = {
  tool_use: {
    icon: Wrench,
    label: "Tool Call",
    accent: "text-orange-600 dark:text-orange-400",
    showMetadata: false,
  },
  tool_result: {
    icon: CornerDownRight,
    label: "Tool Result",
    accent: "text-teal-600 dark:text-teal-400",
    showMetadata: false,
  },
  thinking: {
    icon: Brain,
    label: "Thinking",
    accent: "text-pink-600 dark:text-pink-400",
    showMetadata: false,
  },
  redacted_thinking: {
    icon: Brain,
    label: "Thinking",
    accent: "text-pink-600/50 dark:text-pink-400/50",
    showMetadata: false,
  },
  tool_definitions: {
    icon: ListTree,
    label: "System",
    accent: "text-purple-600 dark:text-purple-400",
    showMetadata: false,
  },
  refusal: {
    icon: AlertCircle,
    label: "Assistant",
    accent: "text-red-600 dark:text-red-400",
    showMetadata: false,
  },
};

// Default config for unknown types
const DEFAULT_CONFIG = {
  icon: HelpCircle,
  label: "Assistant",
  accent: "text-emerald-600 dark:text-emerald-400",
  showMetadata: false,
};

export interface TimelineRowProps {
  block: Block;
  startTime?: string;
  isSelected?: boolean;
  onSelect?: () => void;
  forceExpanded?: boolean;
  onManualToggle?: () => void;
  preview: string;
  copyText: string;
  children: React.ReactNode;
  /** Trace number in session (1-based) for navigation badge */
  traceNumber?: number;
  /** Project ID for building trace URL */
  projectId?: string;
}

export function TimelineRow({
  block,
  startTime,
  isSelected,
  onSelect,
  forceExpanded,
  onManualToggle,
  preview,
  copyText,
  children,
  traceNumber,
  projectId,
}: TimelineRowProps) {
  const [copied, setCopied] = useState(false);
  const [isOpenLocal, setIsOpenLocal] = useState(true);

  useEffect(() => {
    if (forceExpanded !== undefined) {
      setIsOpenLocal(forceExpanded);
    }
  }, [forceExpanded]);

  const isOpen = forceExpanded !== undefined ? forceExpanded : isOpenLocal;
  const handleOpenChange = (open: boolean) => {
    onManualToggle?.();
    setIsOpenLocal(open);
  };

  const isError = block.is_error;

  // Get config based on entry_type and role
  // Priority: special entry types (tool_use, thinking, etc.) > role-based > default
  const config = useMemo(() => {
    // Check for special entry types first
    const specialConfig = SPECIAL_ENTRY_CONFIG[block.entry_type];
    if (specialConfig) return specialConfig;

    // Fall back to role-based config
    const roleConfig = ROLE_CONFIG[block.role];
    if (roleConfig) return roleConfig;

    return DEFAULT_CONFIG;
  }, [block.entry_type, block.role]);

  const Icon = isError ? AlertCircle : config.icon;
  const accentClass = isError ? "text-red-600 dark:text-red-400" : config.accent;

  const relativeTime = useMemo(() => {
    if (!startTime || !block.timestamp) return null;
    const start = new Date(startTime).getTime();
    const current = new Date(block.timestamp).getTime();
    const diffMs = current - start;
    const diffSec = diffMs / 1000;
    if (diffSec < 0.01) return "+0.0s";
    if (diffSec < 10) return `+${diffSec.toFixed(1)}s`;
    if (diffSec < 60) return `+${diffSec.toFixed(0)}s`;
    return `+${(diffSec / 60).toFixed(1)}m`;
  }, [startTime, block.timestamp]);

  const absoluteTime = useMemo(() => {
    if (!block.timestamp) return null;
    return new Date(block.timestamp).toLocaleString();
  }, [block.timestamp]);

  const handleCopy = useCallback(async () => {
    await navigator.clipboard.writeText(copyText);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  }, [copyText]);

  const handleOpenTrace = useCallback(
    (e: React.MouseEvent) => {
      e.stopPropagation();
      if (projectId && block.trace_id) {
        window.open(`/ui/projects/${projectId}/observability/traces/${block.trace_id}`, "_blank");
      }
    },
    [projectId, block.trace_id],
  );

  return (
    <Collapsible open={isOpen} onOpenChange={handleOpenChange}>
      <div
        className={cn(
          "@container group relative rounded-lg border bg-card transition-colors",
          isError && "border-red-300 dark:border-red-800",
          isSelected && "border-primary/50 bg-muted/30",
        )}
        onClick={onSelect}
      >
        {/* Header */}
        <CollapsibleTrigger asChild>
          <div className="flex cursor-pointer items-center gap-2 px-3 py-2 hover:bg-muted/50 @[400px]:gap-3 @[400px]:px-4">
            <ChevronRight
              className={cn(
                "h-3.5 w-3.5 shrink-0 text-muted-foreground transition-transform @[400px]:h-4 @[400px]:w-4",
                isOpen && "rotate-90",
              )}
            />
            <span className={cn("shrink-0", accentClass)}>
              <Icon className="h-3.5 w-3.5 @[400px]:h-4 @[400px]:w-4" />
            </span>
            <span className={cn("message-role text-xs font-medium @[400px]:text-sm", accentClass)}>
              {config.label}
            </span>

            {!isOpen && (
              <span className="min-w-0 flex-1 truncate text-[11px] text-muted-foreground @[400px]:text-xs">
                {preview}
              </span>
            )}

            {isOpen && <div className="flex-1" />}

            {/* Model pill - hidden on small, truncate only when needed */}
            {config.showMetadata && block.model && (
              <span className="message-model-pill hidden min-w-0 shrink truncate rounded bg-muted px-1.5 py-0.5 font-mono text-xs text-muted-foreground @[450px]:inline">
                {block.model}
              </span>
            )}

            <div className="flex shrink-0 items-center gap-1.5 text-[10px] text-muted-foreground @[400px]:gap-2 @[400px]:text-xs">
              <TooltipProvider delayDuration={300}>
                <Tooltip>
                  <TooltipTrigger asChild>
                    <span className="tabular-nums">{relativeTime}</span>
                  </TooltipTrigger>
                  <TooltipContent side="top" className="text-xs">
                    {absoluteTime}
                  </TooltipContent>
                </Tooltip>
              </TooltipProvider>
            </div>

            {/* Button group: trace number + copy */}
            <div className="flex shrink-0">
              {traceNumber !== undefined && projectId && (
                <TooltipProvider delayDuration={300}>
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <Button
                        variant="ghost"
                        size="icon"
                        className="h-6 w-6 rounded-r-none @[400px]:h-7 @[400px]:w-7"
                        onClick={handleOpenTrace}
                      >
                        <span className="text-[10px] font-medium @[400px]:text-xs">
                          #{traceNumber}
                        </span>
                      </Button>
                    </TooltipTrigger>
                    <TooltipContent side="top" className="text-xs">
                      Open trace in new tab
                    </TooltipContent>
                  </Tooltip>
                </TooltipProvider>
              )}
              <Button
                variant="ghost"
                size="icon"
                className={cn(
                  "h-6 w-6 @[400px]:h-7 @[400px]:w-7",
                  traceNumber !== undefined && projectId && "rounded-l-none",
                )}
                onClick={(e) => {
                  e.stopPropagation();
                  handleCopy();
                }}
              >
                {copied ? (
                  <Check className="h-3 w-3 text-emerald-500 @[400px]:h-3.5 @[400px]:w-3.5" />
                ) : (
                  <Copy className="h-3 w-3 @[400px]:h-3.5 @[400px]:w-3.5" />
                )}
              </Button>
            </div>
          </div>
        </CollapsibleTrigger>

        {/* Content */}
        <CollapsibleContent>
          <div className="space-y-3 border-t px-3 py-2 @[400px]:px-4 @[400px]:py-3">{children}</div>
        </CollapsibleContent>
      </div>
    </Collapsible>
  );
}
