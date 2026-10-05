import { useState, useCallback, useMemo } from "react";
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
import { ButtonGroup } from "@/components/ui/button-group";
import { Tooltip, TooltipContent, TooltipTrigger, TooltipProvider } from "@/components/ui/tooltip";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { cn } from "@/lib/utils";
import type { Block } from "@/api/otel/types";
import { useForcedOpenState } from "./use-forced-open-state";
import { getIncompleteReason } from "./thread-utils";

// Role-based configuration (primary)
const ROLE_CONFIG: Record<
  string,
  { icon: typeof User; label: string; accent: string; showMetadata: boolean }
> = {
  system: {
    icon: Settings,
    label: "System",
    accent: "text-role-system",
    showMetadata: true,
  },
  user: {
    icon: User,
    label: "User",
    accent: "text-role-user",
    showMetadata: true,
  },
  assistant: {
    icon: Bot,
    label: "Assistant",
    accent: "text-role-assistant",
    showMetadata: true,
  },
  tool: {
    icon: CornerDownRight,
    label: "Tool Result",
    accent: "text-role-tool",
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
    accent: "text-role-tool-call",
    showMetadata: false,
  },
  tool_result: {
    icon: CornerDownRight,
    label: "Tool Result",
    accent: "text-role-tool",
    showMetadata: false,
  },
  thinking: {
    icon: Brain,
    label: "Thinking",
    accent: "text-role-thinking",
    showMetadata: false,
  },
  redacted_thinking: {
    icon: Brain,
    label: "Thinking",
    accent: "text-role-thinking/50",
    showMetadata: false,
  },
  tool_definitions: {
    icon: ListTree,
    label: "System",
    accent: "text-role-system",
    showMetadata: false,
  },
  refusal: {
    icon: AlertCircle,
    label: "Assistant",
    accent: "text-destructive",
    showMetadata: false,
  },
};

// Default config for unknown types
const DEFAULT_CONFIG = {
  icon: HelpCircle,
  label: "Assistant",
  accent: "text-role-assistant",
  showMetadata: false,
};

export interface TimelineRowProps {
  block: Block;
  startTime?: string;
  isSelected?: boolean;
  onSelect?: () => void;
  forceExpanded?: boolean;
  /** Whether the row starts open when nothing forces it either way. */
  defaultOpen?: boolean;
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
  defaultOpen = true,
  onManualToggle,
  preview,
  copyText,
  children,
  traceNumber,
  projectId,
}: TimelineRowProps) {
  const [copied, setCopied] = useState(false);
  const [isOpen, setIsOpen] = useForcedOpenState(forceExpanded, defaultOpen);
  const handleOpenChange = (open: boolean) => {
    onManualToggle?.();
    setIsOpen(open);
  };

  const isError = block.is_error;
  const incompleteReason = getIncompleteReason(block.finish_reason);

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
  const accentClass = isError ? "text-destructive" : config.accent;

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
          isError && "border-destructive/40",
          isSelected && "border-primary/50 bg-muted/30",
        )}
        onClick={onSelect}
      >
        {/*
          Only the label area is the disclosure button. The copy and trace controls are its siblings rather
          than its children, because interactive content nested inside a button is unreachable by keyboard
          and announced as one control by screen readers.
        */}
        <div className="flex items-center gap-2 px-3 py-2 hover:bg-muted/50 @[400px]:gap-3 @[400px]:px-4">
          <CollapsibleTrigger asChild>
            <button
              type="button"
              data-thread-row-trigger=""
              className="flex min-w-0 flex-1 cursor-pointer items-center gap-2 rounded-sm text-left focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring @[400px]:gap-3"
            >
              <ChevronRight
                aria-hidden="true"
                className={cn(
                  "h-3.5 w-3.5 shrink-0 text-muted-foreground transition-transform @[400px]:h-4 @[400px]:w-4",
                  isOpen && "rotate-90",
                )}
              />
              <span className={cn("shrink-0", accentClass)}>
                <Icon aria-hidden="true" className="h-3.5 w-3.5 @[400px]:h-4 @[400px]:w-4" />
              </span>
              <span
                className={cn("message-role text-xs font-medium @[400px]:text-sm", accentClass)}
              >
                {isError ? `${config.label} (error)` : config.label}
              </span>

              {!isOpen && (
                <span className="min-w-0 flex-1 truncate text-2xs text-muted-foreground @[400px]:text-xs">
                  {preview}
                </span>
              )}

              {isOpen && <span className="flex-1" />}

              {/* Model pill - hidden on small, truncate only when needed */}
              {config.showMetadata && block.model && (
                <span className="message-model-pill hidden min-w-0 shrink truncate rounded bg-muted px-1.5 py-0.5 font-mono text-xs text-muted-foreground @[450px]:inline">
                  {block.model}
                </span>
              )}
            </button>
          </CollapsibleTrigger>

          {incompleteReason && (
            <span
              className="shrink-0 rounded-sm bg-warning/10 px-1.5 py-0.5 text-3xs font-medium text-warning-foreground @[400px]:text-2xs"
              title={incompleteReason.description}
            >
              {incompleteReason.label}
            </span>
          )}

          <div className="flex shrink-0 items-center gap-1.5 text-3xs text-muted-foreground @[400px]:gap-2 @[400px]:text-xs">
            <TooltipProvider delayDuration={300}>
              <Tooltip>
                <TooltipTrigger asChild>
                  <span className="tabular-nums">{relativeTime}</span>
                </TooltipTrigger>
                <TooltipContent side="top">{absoluteTime}</TooltipContent>
              </Tooltip>
            </TooltipProvider>
          </div>

          {/* Button group: trace number + copy */}
          <ButtonGroup className="shrink-0">
            {traceNumber !== undefined && projectId && (
              <TooltipProvider delayDuration={300}>
                <Tooltip>
                  <TooltipTrigger asChild>
                    <Button
                      variant="ghost"
                      size="icon"
                      className="h-6 w-6 @[400px]:h-7 @[400px]:w-7"
                      aria-label={`Open trace ${traceNumber} in a new tab`}
                      onClick={handleOpenTrace}
                    >
                      <span className="text-3xs font-medium @[400px]:text-xs">#{traceNumber}</span>
                    </Button>
                  </TooltipTrigger>
                  <TooltipContent side="top">Open trace in new tab</TooltipContent>
                </Tooltip>
              </TooltipProvider>
            )}
            <Button
              variant="ghost"
              size="icon"
              className="h-6 w-6 @[400px]:h-7 @[400px]:w-7"
              aria-label={copied ? "Copied" : `Copy ${config.label.toLowerCase()}`}
              onClick={(e) => {
                e.stopPropagation();
                handleCopy();
              }}
            >
              {copied ? (
                <Check
                  aria-hidden="true"
                  className="h-3 w-3 text-success @[400px]:h-3.5 @[400px]:w-3.5"
                />
              ) : (
                <Copy aria-hidden="true" className="h-3 w-3 @[400px]:h-3.5 @[400px]:w-3.5" />
              )}
            </Button>
          </ButtonGroup>
        </div>

        {/* Content */}
        <CollapsibleContent>
          <div className="space-y-3 border-t px-3 py-2 @[400px]:px-4 @[400px]:py-3">{children}</div>
        </CollapsibleContent>
      </div>
    </Collapsible>
  );
}
