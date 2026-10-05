import {
  Fragment,
  useState,
  useMemo,
  useCallback,
  useRef,
  useLayoutEffect,
  type KeyboardEvent,
} from "react";
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
import { Skeleton } from "@/components/ui/skeleton";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { cn } from "@/lib/utils";
import { settings, MARKDOWN_ENABLED_KEY } from "@/lib/settings";
import { ThreadHeader } from "./thread-header";
import { TimelineRow } from "./timeline-row";
import { JsonContent } from "./content";
import { getBlockKey, getBlockPreview, getBlockCopyText, renderBlockContent } from "./thread-utils";
import { MediaGalleryProvider } from "./image-gallery-context";
import { useForcedOpenState } from "./use-forced-open-state";
import { ModelLink } from "@/components/model-link";
import { SpanErrorRow } from "./span-error-row";
import { placeSpanErrors } from "./span-errors";
import type { ThreadViewProps, ThreadTab } from "./types";

/**
 * Above this many blocks, rows start collapsed. A collapsed row does not mount its Markdown, JSON
 * tree or media, which is what makes a long session cheap to open; the rows stay in the DOM so the
 * browser's find-in-page still reaches every header.
 */
export const LARGE_THREAD_BLOCKS = 200;

/** Keys the thread handles itself; anything typed into a control keeps its own meaning. */
function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return (
    target instanceof HTMLInputElement ||
    target instanceof HTMLTextAreaElement ||
    target instanceof HTMLSelectElement ||
    target.isContentEditable
  );
}

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
  const [copied, setCopied] = useState(false);
  const [isOpen, setIsOpen] = useForcedOpenState(forceExpanded);
  const handleOpenChange = (open: boolean) => {
    onManualToggle?.();
    setIsOpen(open);
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
        <div className="flex items-center gap-2 px-3 py-2 hover:bg-muted/50 @[400px]:gap-3 @[400px]:px-4">
          <CollapsibleTrigger asChild>
            <button
              type="button"
              className="flex min-w-0 flex-1 cursor-pointer items-center gap-2 rounded-sm text-left focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring @[400px]:gap-3"
            >
              <ChevronRight
                aria-hidden="true"
                className={cn(
                  "h-3.5 w-3.5 shrink-0 text-muted-foreground transition-transform @[400px]:h-4 @[400px]:w-4",
                  isOpen && "rotate-90",
                )}
              />
              <span className="shrink-0 text-role-tool-call">
                <Wrench aria-hidden="true" className="h-3.5 w-3.5 @[400px]:h-4 @[400px]:w-4" />
              </span>
              <span className="truncate text-xs font-medium text-role-tool-call @[400px]:text-sm">
                {toolName}
              </span>
            </button>
          </CollapsibleTrigger>

          <Button
            variant="ghost"
            size="icon"
            className="h-6 w-6 shrink-0 @[400px]:h-7 @[400px]:w-7"
            aria-label={copied ? "Copied" : `Copy ${toolName} definition`}
            onClick={(e) => {
              e.stopPropagation();
              handleCopy();
            }}
          >
            {copied ? (
              <Check className="h-3 w-3 text-success @[400px]:h-3.5 @[400px]:w-3.5" />
            ) : (
              <Copy className="h-3 w-3 @[400px]:h-3.5 @[400px]:w-3.5" />
            )}
          </Button>
        </div>

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
  toolNames,
  envelopes,
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

  const openByDefault = blocks.length <= LARGE_THREAD_BLOCKS;
  const allExpanded = forceExpandedState ?? openByDefault;
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

  const spanErrors = useMemo(() => placeSpanErrors(blocks, envelopes ?? []), [blocks, envelopes]);

  // Tool names without schemas: some frameworks report which tools were offered but not their
  // definitions, and an empty Tools tab would claim no tools were available at all.
  const undefinedToolNames = useMemo(() => {
    const defined = new Set(
      (toolDefinitions ?? []).map((tool) => unwrapToolDef(tool).name).filter(Boolean),
    );
    return [...new Set(toolNames ?? [])].filter((name) => !defined.has(name));
  }, [toolDefinitions, toolNames]);

  const handleToggleExpandAll = useCallback(() => {
    setForceExpandedState(!allExpanded);
  }, [allExpanded]);

  const selectRow = useCallback((index: number) => {
    setSelectedIndex(index);
    const triggers = scrollContainerRef.current?.querySelectorAll<HTMLElement>(
      "[data-thread-row-trigger]",
    );
    // Moving focus also scrolls the row into view and tells assistive technology where the user is.
    triggers?.[index]?.focus();
  }, []);

  // Keyboard navigation, scoped to the thread: a window-level handler took the arrow keys from every
  // other panel on the page and turned a plain Cmd/Ctrl+C into "copy the whole selected block".
  const handleKeyDown = useCallback(
    (e: KeyboardEvent<HTMLDivElement>) => {
      if (e.metaKey || e.ctrlKey || e.altKey || isEditableTarget(e.target)) return;
      if (blocks.length === 0) return;
      const current =
        selectedIndex !== null && selectedIndex < blocks.length ? selectedIndex : null;

      switch (e.key) {
        case "j":
        case "ArrowDown":
          e.preventDefault();
          selectRow(current === null ? 0 : Math.min(current + 1, blocks.length - 1));
          break;
        case "k":
        case "ArrowUp":
          e.preventDefault();
          selectRow(current === null ? blocks.length - 1 : Math.max(current - 1, 0));
          break;
        case "Escape":
          setSelectedIndex(null);
          break;
        case "c":
          if (current !== null) {
            navigator.clipboard.writeText(getBlockCopyText(blocks[current]));
          }
          break;
      }
    },
    [blocks, selectedIndex, selectRow],
  );

  if (isLoading) {
    return (
      <div
        role="status"
        aria-label="Loading messages"
        className={cn("flex h-full flex-col gap-3 p-4", className)}
      >
        <Skeleton className="h-10 w-full" />
        <Skeleton className="h-24 w-full" />
        <Skeleton className="h-10 w-full" />
      </div>
    );
  }

  // Error state
  if (error) {
    return (
      <div
        role="alert"
        className={cn("flex h-full flex-col items-center justify-center gap-4 p-8", className)}
      >
        <AlertCircle aria-hidden="true" className="h-12 w-12 text-destructive" />
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

  // Empty state. A span that failed without recording a message still has something to show.
  if (blocks.length === 0 && spanErrors.leading.length === 0) {
    return (
      <div className={cn("flex h-full flex-col items-center justify-center gap-4 p-8", className)}>
        <MessageSquare aria-hidden="true" className="h-12 w-12 text-muted-foreground/50" />
        <div className="text-center">
          <h3 className="font-medium text-muted-foreground">No messages</h3>
          <p className="text-sm text-muted-foreground">
            No conversation messages were recorded here.
          </p>
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

      {/*
        An incomplete answer must not look complete. `replay_matching_complete` is false when cross-trace
        replay matching hit its search budget, so this thread may repeat history it would otherwise have
        collapsed. The server omits the flag when true, so only an explicit `false` warns. Without this the
        duplicated turns are indistinguishable from a model that actually repeated itself - exactly the wrong
        conclusion to hand someone debugging one.
      */}
      {metadata?.replay_matching_complete === false && (
        <div
          role="status"
          className="mx-4 mt-3 rounded-md border border-warning/40 bg-warning/10 px-3 py-2 text-sm text-warning-foreground"
        >
          Repeated history may appear twice below: this conversation was large enough that
          duplicate-detection stopped short of a complete answer.
        </div>
      )}

      {activeTab === "messages" ? (
        <MediaGalleryProvider blocks={blocks} projectId={projectId}>
          <div
            ref={scrollContainerRef}
            role="region"
            aria-label="Conversation messages"
            tabIndex={0}
            onKeyDown={handleKeyDown}
            className="flex-1 min-h-0 overflow-auto focus-visible:outline-none"
          >
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
                      {contextInfo.models.map((model, i) => (
                        <span key={model}>
                          {i > 0 && ", "}
                          <ModelLink model={model} />
                        </span>
                      ))}
                      {contextInfo.frameworks.length > 0 && ")"}
                    </span>
                  )}
                </p>
              )}
              {spanErrors.leading.map((envelope) => (
                <SpanErrorRow
                  key={`${envelope.trace_id}-${envelope.span_id}`}
                  envelope={envelope}
                />
              ))}
              {blocks.map((block, index) => (
                <Fragment key={getBlockKey(block)}>
                  <TimelineRow
                    block={block}
                    startTime={startTime}
                    isSelected={selectedIndex === index}
                    onSelect={() => setSelectedIndex(index)}
                    forceExpanded={forceExpandedState ?? undefined}
                    defaultOpen={openByDefault}
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
                  {spanErrors.after.get(index)?.map((envelope) => (
                    <SpanErrorRow
                      key={`${envelope.trace_id}-${envelope.span_id}`}
                      envelope={envelope}
                    />
                  ))}
                </Fragment>
              ))}
            </div>
          </div>
        </MediaGalleryProvider>
      ) : (
        <div ref={scrollContainerRef} className="flex-1 min-h-0 overflow-auto">
          {(toolDefinitions && toolDefinitions.length > 0) || undefinedToolNames.length > 0 ? (
            <div className="space-y-3 p-4">
              {toolDefinitions?.map((tool, index) => (
                <ToolCard
                  key={index}
                  tool={tool}
                  index={index}
                  forceExpanded={forceExpandedState ?? undefined}
                  onManualToggle={() => setForceExpandedState(null)}
                />
              ))}
              {undefinedToolNames.length > 0 && (
                <div className="rounded-lg border bg-card px-3 py-2 @[400px]:px-4">
                  <p className="text-xs text-muted-foreground">
                    Named in telemetry without a recorded definition
                  </p>
                  <ul className="mt-1 flex flex-wrap gap-2">
                    {undefinedToolNames.map((name) => (
                      <li key={name} className="font-mono text-xs text-role-tool-call">
                        {name}
                      </li>
                    ))}
                  </ul>
                </div>
              )}
            </div>
          ) : (
            <div className="flex h-full flex-col items-center justify-center gap-2 text-muted-foreground">
              <Wrench aria-hidden="true" className="h-12 w-12 text-muted-foreground/50" />
              <span className="text-sm">No tool definitions available</span>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
