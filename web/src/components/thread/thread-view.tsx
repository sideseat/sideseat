import { useState, useMemo, useCallback, useEffect, useRef, useLayoutEffect } from "react";
import {
  AlertCircle,
  MessageSquare,
  RefreshCw,
  Wrench,
  ChevronRight,
  Copy,
  Check,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { cn } from "@/lib/utils";
import { settings, MARKDOWN_ENABLED_KEY } from "@/lib/settings";
import { ThreadHeader } from "./thread-header";
import { TimelineRow } from "./timeline-row";
import { JsonContent } from "./content";
import { getBlockKey, getBlockPreview, getBlockCopyText, renderBlockContent } from "./thread-utils";
import { ImageGalleryProvider } from "./image-gallery-context";
import type { ThreadViewProps, ThreadTab } from "./types";

interface ToolCardProps {
  tool: Record<string, unknown>;
  index: number;
  forceExpanded?: boolean;
  onManualToggle?: () => void;
}

// Unwrap OpenAI format: {type: "function", function: {...}} -> {...}
function unwrapToolDef(tool: Record<string, unknown>): Record<string, unknown> {
  if (tool.function && typeof tool.function === "object") {
    return tool.function as Record<string, unknown>;
  }
  return tool;
}

function ToolCard({ tool, index, forceExpanded, onManualToggle }: ToolCardProps) {
  const [isOpenLocal, setIsOpenLocal] = useState(true);
  const [copied, setCopied] = useState(false);

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

  const unwrapped = unwrapToolDef(tool);
  const toolName = (unwrapped.name as string) ?? `Tool ${index + 1}`;
  const toolJson = JSON.stringify(unwrapped, null, 2);

  const handleCopy = useCallback(async () => {
    await navigator.clipboard.writeText(toolJson);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  }, [toolJson]);

  return (
    <Collapsible open={isOpen} onOpenChange={handleOpenChange}>
      <div className="@container group relative rounded-lg border bg-card transition-colors">
        <CollapsibleTrigger asChild>
          <div className="flex cursor-pointer items-center gap-2 px-3 py-2 hover:bg-muted/50 @[400px]:gap-3 @[400px]:px-4">
            <ChevronRight
              className={cn(
                "h-3.5 w-3.5 shrink-0 text-muted-foreground transition-transform @[400px]:h-4 @[400px]:w-4",
                isOpen && "rotate-90",
              )}
            />
            <span className="shrink-0 text-orange-600 dark:text-orange-400">
              <Wrench className="h-3.5 w-3.5 @[400px]:h-4 @[400px]:w-4" />
            </span>
            <span className="truncate text-xs font-medium text-orange-600 @[400px]:text-sm dark:text-orange-400">
              {toolName}
            </span>

            <div className="flex-1" />

            <Button
              variant="ghost"
              size="icon"
              className="h-6 w-6 shrink-0 @[400px]:h-7 @[400px]:w-7"
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
        </CollapsibleTrigger>

        <CollapsibleContent>
          <div className="border-t px-3 py-2 @[400px]:px-4 @[400px]:py-3">
            <JsonContent data={unwrapped} />
          </div>
        </CollapsibleContent>
      </div>
    </Collapsible>
  );
}

export function ThreadView({
  blocks,
  metadata,
  toolDefinitions,
  tokenBreakdown,
  costBreakdown,
  isLoading,
  error,
  onRetry,
  className,
  activeTab: controlledActiveTab,
  onTabChange,
  projectId,
  showTraceLinks,
}: ThreadViewProps) {
  const [internalActiveTab, setInternalActiveTab] = useState<ThreadTab>("messages");
  const activeTab = controlledActiveTab ?? internalActiveTab;
  const scrollContainerRef = useRef<HTMLDivElement>(null);

  const setActiveTab = useCallback(
    (tab: ThreadTab) => {
      setInternalActiveTab(tab);
      onTabChange?.(tab);
    },
    [onTabChange],
  );
  const [forceExpandedState, setForceExpandedState] = useState<boolean | null>(null);
  const [selectedIndex, setSelectedIndex] = useState<number | null>(null);

  // Scroll to top when blocks change (trace switch)
  useLayoutEffect(() => {
    if (scrollContainerRef.current) {
      scrollContainerRef.current.scrollTop = 0;
    }
  }, [blocks]);
  const [markdownEnabled, setMarkdownEnabled] = useState(
    () => settings.get<boolean>(MARKDOWN_ENABLED_KEY, true) ?? true,
  );

  const handleMarkdownToggle = useCallback(() => {
    const newValue = !markdownEnabled;
    setMarkdownEnabled(newValue);
    settings.set(MARKDOWN_ENABLED_KEY, newValue);
  }, [markdownEnabled]);

  const allExpanded = forceExpandedState !== false;
  const startTime = metadata?.start_time ?? blocks[0]?.timestamp;

  // Extract context info from blocks
  const contextInfo = useMemo(() => {
    const frameworks = new Set<string>();
    const models = new Set<string>();
    for (const b of blocks) {
      if (b.provider) frameworks.add(b.provider);
      if (b.model) models.add(b.model);
    }
    return {
      frameworks: [...frameworks],
      models: [...models],
    };
  }, [blocks]);

  // Build trace number map (1-based) for session view
  // Maps trace_id -> sequential number based on first occurrence
  const traceNumberMap = useMemo(() => {
    const map = new Map<string, number>();
    let counter = 1;
    for (const block of blocks) {
      if (block.trace_id && !map.has(block.trace_id)) {
        map.set(block.trace_id, counter++);
      }
    }
    return map;
  }, [blocks]);

  const handleToggleExpandAll = useCallback(() => {
    if (allExpanded) {
      setForceExpandedState(false);
    } else {
      setForceExpandedState(true);
    }
  }, [allExpanded]);

  // Keyboard navigation
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) {
        return;
      }

      switch (e.key) {
        case "j":
        case "ArrowDown":
          e.preventDefault();
          setSelectedIndex((prev) => (prev === null ? 0 : Math.min(prev + 1, blocks.length - 1)));
          break;
        case "k":
        case "ArrowUp":
          e.preventDefault();
          setSelectedIndex((prev) => (prev === null ? blocks.length - 1 : Math.max(prev - 1, 0)));
          break;
        case "Escape":
          setSelectedIndex(null);
          break;
        case "c":
          if (selectedIndex !== null && blocks[selectedIndex]) {
            const block = blocks[selectedIndex];
            navigator.clipboard.writeText(getBlockCopyText(block));
          }
          break;
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [blocks, selectedIndex]);

  // Loading state - render nothing, parent handles loading indicator
  if (isLoading) {
    return null;
  }

  // Error state
  if (error) {
    return (
      <div className={cn("flex h-full flex-col items-center justify-center gap-4 p-8", className)}>
        <AlertCircle className="h-12 w-12 text-destructive" />
        <div className="text-center">
          <h3 className="font-medium">Failed to load messages</h3>
          <p className="text-sm text-muted-foreground">{error.message}</p>
        </div>
        {onRetry && (
          <Button variant="outline" onClick={onRetry}>
            <RefreshCw className="mr-2 h-4 w-4" />
            Retry
          </Button>
        )}
      </div>
    );
  }

  // Empty state
  if (blocks.length === 0) {
    return (
      <div className={cn("flex h-full flex-col items-center justify-center gap-4 p-8", className)}>
        <MessageSquare className="h-12 w-12 text-muted-foreground/50" />
        <div className="text-center">
          <h3 className="font-medium text-muted-foreground">No messages</h3>
          <p className="text-sm text-muted-foreground">This trace has no conversation messages.</p>
        </div>
      </div>
    );
  }

  return (
    <div className={cn("thread-container flex h-full flex-col overflow-hidden", className)}>
      <ThreadHeader
        metadata={metadata}
        tokenBreakdown={tokenBreakdown}
        costBreakdown={costBreakdown}
        activeTab={activeTab}
        onTabChange={setActiveTab}
        allExpanded={allExpanded}
        onToggleExpandAll={handleToggleExpandAll}
        markdownEnabled={markdownEnabled}
        onMarkdownToggle={handleMarkdownToggle}
      />

      {activeTab === "messages" ? (
        <ImageGalleryProvider blocks={blocks} projectId={projectId}>
          <div ref={scrollContainerRef} className="flex-1 min-h-0 overflow-auto">
            <div className="space-y-3 p-4">
              {/* Framework/Model info */}
              {(contextInfo.frameworks.length > 0 || contextInfo.models.length > 0) && (
                <p className="text-sm text-muted-foreground">
                  {contextInfo.frameworks.length > 0 && contextInfo.frameworks.join(", ")}
                  {contextInfo.models.length > 0 && (
                    <span
                      className={contextInfo.frameworks.length > 0 ? "ml-1 font-mono" : "font-mono"}
                    >
                      {contextInfo.frameworks.length > 0 && "("}
                      {contextInfo.models.join(", ")}
                      {contextInfo.frameworks.length > 0 && ")"}
                    </span>
                  )}
                </p>
              )}
              {blocks.map((block, index) => (
                <TimelineRow
                  key={getBlockKey(block)}
                  block={block}
                  startTime={startTime}
                  isSelected={selectedIndex === index}
                  onSelect={() => setSelectedIndex(index)}
                  forceExpanded={forceExpandedState ?? undefined}
                  onManualToggle={() => setForceExpandedState(null)}
                  preview={getBlockPreview(block)}
                  copyText={getBlockCopyText(block)}
                  traceNumber={
                    showTraceLinks && block.trace_id
                      ? traceNumberMap.get(block.trace_id)
                      : undefined
                  }
                  projectId={showTraceLinks ? projectId : undefined}
                >
                  {renderBlockContent(block, markdownEnabled, projectId)}
                </TimelineRow>
              ))}
            </div>
          </div>
        </ImageGalleryProvider>
      ) : (
        <div ref={scrollContainerRef} className="flex-1 min-h-0 overflow-auto">
          {toolDefinitions && toolDefinitions.length > 0 ? (
            <div className="space-y-3 p-4">
              {toolDefinitions.map((tool, index) => (
                <ToolCard
                  key={index}
                  tool={tool}
                  index={index}
                  forceExpanded={forceExpandedState ?? undefined}
                  onManualToggle={() => setForceExpandedState(null)}
                />
              ))}
            </div>
          ) : (
            <div className="flex h-full flex-col items-center justify-center gap-2 text-muted-foreground">
              <Wrench className="h-12 w-12 text-muted-foreground/50" />
              <span className="text-sm">No tool definitions available</span>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
