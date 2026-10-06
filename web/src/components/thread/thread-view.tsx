import {
  Fragment,
  useState,
  useMemo,
  useCallback,
  useRef,
  useLayoutEffect,
  type CSSProperties,
  type KeyboardEvent,
} from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
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
import { getBlockKey, renderBlockContent } from "./thread-utils";
import {
  getMessageCopyText,
  getMessagePreview,
  groupBlocksIntoMessages,
  messageFinishReason,
  messageHasError,
  messageModel,
  messageRowConfig,
  type ThreadMessage,
} from "./messages";
import { MediaGalleryProvider } from "./image-gallery-context";
import { useForcedOpenState } from "./use-forced-open-state";
import { ModelLink } from "@/components/model-link";
import { SpanErrorRow } from "./span-error-row";
import { placeSpanErrors } from "./span-errors";
import type { SpanEnvelope } from "@/api/otel/types";
import type { ThreadViewProps, ThreadTab } from "./types";

/**
 * Above this many messages, rows start collapsed. A collapsed row does not mount its Markdown, JSON
 * tree or media, which is what makes a long session cheap to open; the rows stay in the DOM so the
 * browser's find-in-page still reaches every header.
 */
export const LARGE_THREAD_BLOCKS = 200;

/**
 * Above this many messages the rows are virtualised: only what is near the viewport is in the DOM.
 *
 * Collapsing alone stops the bodies from mounting, but ten thousand headers are still ten thousand
 * cards to lay out and keep. The threshold is well above the collapse threshold because virtualising
 * costs the browser's find-in-page the rows it has not rendered, which is worth less than opening a
 * session of tens of thousands of messages at all.
 */
export const VIRTUALISED_THREAD_MESSAGES = 400;

/** The height a message row is assumed to have before it is measured. */
const ESTIMATED_ROW_HEIGHT = 44;

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

interface MessageRowsProps extends Omit<MessageRowProps, "message" | "index"> {
  messages: ThreadMessage[];
  spanErrorsAfter: Map<number, SpanEnvelope[]>;
  scrollContainerRef: React.RefObject<HTMLDivElement | null>;
}

interface MessageRowProps {
  message: ThreadMessage;
  index: number;
  startTime?: string;
  selectedIndex: number | null;
  onSelect: (index: number) => void;
  forceExpanded: boolean | null;
  openByDefault: boolean;
  onManualToggle: () => void;
  markdownEnabled: boolean;
  projectId?: string;
  showTraceLinks?: boolean;
  traceNumberMap: Map<string, number>;
}

/** One message of the conversation, with every block it is made of inside one card. */
function MessageRow({
  message,
  index,
  startTime,
  selectedIndex,
  onSelect,
  forceExpanded,
  openByDefault,
  onManualToggle,
  markdownEnabled,
  projectId,
  showTraceLinks,
  traceNumberMap,
}: MessageRowProps) {
  return (
    <TimelineRow
      block={message.lead}
      startTime={startTime}
      isSelected={selectedIndex === index}
      onSelect={() => onSelect(index)}
      forceExpanded={forceExpanded ?? undefined}
      defaultOpen={openByDefault}
      onManualToggle={onManualToggle}
      preview={getMessagePreview(message)}
      copyText={getMessageCopyText(message)}
      config={messageRowConfig(message)}
      isError={messageHasError(message)}
      model={messageModel(message)}
      finishReason={messageFinishReason(message)}
      traceNumber={
        showTraceLinks && message.lead.trace_id
          ? traceNumberMap.get(message.lead.trace_id)
          : undefined
      }
      projectId={showTraceLinks ? projectId : undefined}
    >
      {message.blocks.map((block) => (
        <Fragment key={getBlockKey(block)}>
          {renderBlockContent(block, markdownEnabled, projectId)}
        </Fragment>
      ))}
    </TimelineRow>
  );
}

/**
 * The thread's messages, virtualised once there are more than `VIRTUALISED_THREAD_MESSAGES`.
 *
 * Both paths render the same rows in the same order; the virtualised one keeps only the rows near the
 * viewport in the DOM, measuring each as it opens so a row that grows does not overlap the next.
 */
function MessageRows({ messages, spanErrorsAfter, scrollContainerRef, ...row }: MessageRowsProps) {
  const virtualise = messages.length > VIRTUALISED_THREAD_MESSAGES;
  // TanStack Virtual exposes mutable methods that React Compiler intentionally leaves unmemoized.
  // eslint-disable-next-line react-hooks/incompatible-library
  const virtualizer = useVirtualizer({
    count: virtualise ? messages.length : 0,
    getScrollElement: () => scrollContainerRef.current,
    estimateSize: () => ESTIMATED_ROW_HEIGHT,
    overscan: 12,
    // The gap the surrounding `space-y-3` leaves between rows, which the virtualiser must account for.
    gap: 12,
    getItemKey: (index) => messages[index].key,
  });

  if (!virtualise) {
    return (
      <>
        {messages.map((message, index) => (
          <Fragment key={message.key}>
            <div data-thread-row-index={index}>
              <MessageRow {...row} message={message} index={index} />
            </div>
            {spanErrorsAfter.get(index)?.map((envelope) => (
              <SpanErrorRow key={`${envelope.trace_id}-${envelope.span_id}`} envelope={envelope} />
            ))}
          </Fragment>
        ))}
      </>
    );
  }

  const items = virtualizer.getVirtualItems();
  return (
    <div
      className="relative h-(--list-size) w-full"
      style={{ "--list-size": `${virtualizer.getTotalSize()}px` } as CSSProperties}
    >
      {items.map((item) => (
        <div
          key={item.key}
          data-thread-row-index={item.index}
          data-index={item.index}
          ref={virtualizer.measureElement}
          className="absolute top-0 left-0 w-full translate-y-(--row-start)"
          style={{ "--row-start": `${item.start}px` } as CSSProperties}
        >
          <MessageRow {...row} message={messages[item.index]} index={item.index} />
          {spanErrorsAfter.get(item.index)?.map((envelope) => (
            <div key={`${envelope.trace_id}-${envelope.span_id}`} className="mt-3">
              <SpanErrorRow envelope={envelope} />
            </div>
          ))}
        </div>
      ))}
    </div>
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

  const messages = useMemo(() => groupBlocksIntoMessages(blocks), [blocks]);
  const openByDefault = messages.length <= LARGE_THREAD_BLOCKS;
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

  const blockErrors = useMemo(() => placeSpanErrors(blocks, envelopes ?? []), [blocks, envelopes]);

  // The errors a block index carries belong after the message that block ends.
  const spanErrors = useMemo(() => {
    const after = new Map<number, SpanEnvelope[]>();
    messages.forEach((message, index) => {
      const found = message.blocks.flatMap(
        (_, offset) =>
          blockErrors.after.get(message.lastBlockIndex - message.blocks.length + 1 + offset) ?? [],
      );
      if (found.length > 0) after.set(index, found);
    });
    return { leading: blockErrors.leading, after };
  }, [messages, blockErrors]);

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
    const trigger = scrollContainerRef.current?.querySelector<HTMLElement>(
      `[data-thread-row-index="${index}"] [data-thread-row-trigger]`,
    );
    // Moving focus also scrolls the row into view and tells assistive technology where the user is.
    trigger?.focus();
  }, []);

  // Keyboard navigation, scoped to the thread: a window-level handler took the arrow keys from every
  // other panel on the page and turned a plain Cmd/Ctrl+C into "copy the whole selected message".
  const handleKeyDown = useCallback(
    (e: KeyboardEvent<HTMLDivElement>) => {
      if (e.metaKey || e.ctrlKey || e.altKey || isEditableTarget(e.target)) return;
      if (messages.length === 0) return;
      const current =
        selectedIndex !== null && selectedIndex < messages.length ? selectedIndex : null;

      switch (e.key) {
        case "j":
        case "ArrowDown":
          e.preventDefault();
          selectRow(current === null ? 0 : Math.min(current + 1, messages.length - 1));
          break;
        case "k":
        case "ArrowUp":
          e.preventDefault();
          selectRow(current === null ? messages.length - 1 : Math.max(current - 1, 0));
          break;
        case "Escape":
          setSelectedIndex(null);
          break;
        case "c":
          if (current !== null) {
            navigator.clipboard.writeText(getMessageCopyText(messages[current]));
          }
          break;
      }
    },
    [messages, selectedIndex, selectRow],
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
              <MessageRows
                messages={messages}
                startTime={startTime}
                selectedIndex={selectedIndex}
                onSelect={setSelectedIndex}
                forceExpanded={forceExpandedState}
                openByDefault={openByDefault}
                onManualToggle={() => setForceExpandedState(null)}
                markdownEnabled={markdownEnabled}
                projectId={projectId}
                showTraceLinks={showTraceLinks}
                traceNumberMap={traceNumberMap}
                spanErrorsAfter={spanErrors.after}
                scrollContainerRef={scrollContainerRef}
              />
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
