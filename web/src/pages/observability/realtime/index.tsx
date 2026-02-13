import { useState, useCallback, useMemo, useRef, useEffect, memo } from "react";
import { useParams } from "react-router";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  MessageSquare,
  FileJson,
  FileText,
  Code,
  Copy,
  RefreshCw,
  ChevronsUpDown,
  Check,
  ArrowDown,
  GitBranch,
  Layers,
  Users,
  Trash2,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { ButtonGroup } from "@/components/ui/button-group";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { WaitingIndicator } from "@/components/waiting-indicator";
import {
  TimelineRow,
  getBlockPreview,
  getBlockCopyText,
  renderBlockContent,
  ImageGalleryProvider,
} from "@/components/thread";
import { JsonContent } from "@/components/thread/content";
import { useQueryClient } from "@tanstack/react-query";
import { cn } from "@/lib/utils";
import { settings, MARKDOWN_ENABLED_KEY } from "@/lib/settings";
import { useOtelClient } from "@/lib/app-context";
import { useSpanStream } from "@/api/otel/hooks/streams";
import { useSseDetailRefresh } from "@/hooks/use-grid-helpers";
import { TraceDetailSheet } from "../trace/trace-detail-sheet";
import { SpanDetailSheet } from "../span/span-detail-sheet";
import { SessionDetailSheet } from "../session/session-detail-sheet";
import type {
  Block,
  SpanSummary,
  TraceSummary,
  SessionSummary,
  SseSpanEvent,
} from "@/api/otel/types";

type RealtimeTab = "messages" | "raw";

const TABS: { value: RealtimeTab; label: string; icon: React.ReactNode }[] = [
  { value: "messages", label: "Messages", icon: <MessageSquare className="h-4 w-4" /> },
  { value: "raw", label: "Raw", icon: <FileJson className="h-4 w-4" /> },
];

const MAX_BUFFER_SIZE = 1000;
const DEBOUNCE_MS = 100;
const MIN_REFETCH_INTERVAL_MS = 500; // Throttle: max 2 refetches per second
const REFETCH_LIMIT = 100; // Increased for high-throughput scenarios
const LATE_MESSAGE_BUFFER_MS = 30000; // 30s buffer for late-arriving spans (raw tab)
const ITEM_GAP = 12; // gap-3 = 12px
const CONTAINER_PADDING = 16; // p-4 = 16px

// ============================================================================
// TRACE-BASED BLOCK STORAGE
// ============================================================================
//
// Blocks are stored grouped by trace_id to preserve backend ordering.
// The backend's feed pipeline (sort_by_birth_time) handles all intra-trace
// ordering: birth_time → message_index → entry_index.
//
// Frontend NEVER re-sorts within a trace - backend order is canonical.
// We only sort traces relative to each other (by start time).
//
// This approach is robust because:
// 1. Backend handles complex ordering (tool chains, history, dedup)
// 2. Atomic trace replacement - no partial updates or merge corruption
// 3. Simple inter-trace ordering - just timestamp comparison
// ============================================================================

/** Trace data: blocks in backend order + metadata for sorting */
interface TraceData {
  blocks: Block[];
  startTime: string; // First message timestamp (for inter-trace ordering)
}

/** Convert trace map to flat display array (oldest trace first, backend order within) */
function tracesToDisplayBlocks(traceMap: Map<string, TraceData>, maxBlocks: number): Block[] {
  // Sort traces by start time ASC (oldest first for chat UI)
  const sortedTraces = Array.from(traceMap.values()).sort((a, b) =>
    a.startTime.localeCompare(b.startTime),
  );

  // Truncate by removing entire traces from the start (oldest) to stay under limit
  // This preserves trace integrity - never show partial conversations
  let totalBlocks = 0;
  let startIndex = 0;

  // Count total and find where to start to stay under limit
  for (const trace of sortedTraces) {
    totalBlocks += trace.blocks.length;
  }

  if (totalBlocks > maxBlocks) {
    let blocksToRemove = totalBlocks - maxBlocks;
    for (let i = 0; i < sortedTraces.length && blocksToRemove > 0; i++) {
      const traceBlockCount = sortedTraces[i].blocks.length;
      if (traceBlockCount <= blocksToRemove) {
        blocksToRemove -= traceBlockCount;
        startIndex = i + 1;
      } else {
        // Can't remove partial trace - stop here, slightly over limit is OK
        break;
      }
    }
  }

  // Flatten from startIndex onwards (backend order preserved within each trace)
  return sortedTraces.slice(startIndex).flatMap((trace) => trace.blocks);
}

// Sort spans: timestamp_start DESC, span_id ASC
function compareSpansDesc(a: SpanSummary, b: SpanSummary): number {
  const timeCompare = b.timestamp_start.localeCompare(a.timestamp_start);
  if (timeCompare !== 0) return timeCompare;
  return a.span_id.localeCompare(b.span_id);
}

// Estimate row height based on content type (includes gap)
function estimateBlockHeight(block: Block): number {
  let baseHeight: number;
  const content = block.content;

  if (content.type === "text") {
    const lines = content.text.split("\n").length;
    baseHeight = Math.max(80, Math.min(lines * 24 + 60, 400));
  } else if (content.type === "tool_use" || content.type === "tool_result") {
    baseHeight = 120;
  } else if (content.type === "thinking") {
    baseHeight = 100;
  } else {
    baseHeight = 80;
  }

  return baseHeight + ITEM_GAP;
}

// Rough estimate - actual height measured by virtualizer's measureElement
// Using constant avoids expensive JSON.stringify on every estimation call
const ESTIMATED_SPAN_HEIGHT = 52 + 300 + 32 + ITEM_GAP; // header + content + padding + gap

// Breadcrumb path showing session → trace → span navigation
// Only renders links for IDs that exist
const MessageBreadcrumb = memo(function MessageBreadcrumb({
  sessionId,
  traceId,
  spanId,
  onOpenSession,
  onOpenTrace,
  onOpenSpan,
}: {
  sessionId?: string;
  traceId?: string;
  spanId?: string;
  onOpenSession: (sessionId: string) => void;
  onOpenTrace: (traceId: string) => void;
  onOpenSpan: (traceId: string, spanId: string) => void;
}) {
  // Don't render breadcrumb if no IDs available
  if (!sessionId && !traceId && !spanId) return null;

  return (
    <div className="flex items-center gap-1 font-mono text-[10px] text-muted-foreground/70 mb-1.5 select-none">
      {sessionId && (
        <>
          <button
            type="button"
            onClick={() => onOpenSession(sessionId)}
            className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded hover:bg-muted hover:text-foreground transition-colors"
            title={`Session ${sessionId}`}
          >
            <Users className="h-3 w-3" />
            <span className="tracking-tight">session:{sessionId.slice(0, 8)}</span>
          </button>
          {(traceId || spanId) && <span className="text-muted-foreground/40">/</span>}
        </>
      )}
      {traceId && (
        <>
          <button
            type="button"
            onClick={() => onOpenTrace(traceId)}
            className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded hover:bg-muted hover:text-foreground transition-colors"
            title={`Trace ${traceId}`}
          >
            <GitBranch className="h-3 w-3" />
            <span className="tracking-tight">trace:{traceId.slice(0, 8)}</span>
          </button>
          {spanId && <span className="text-muted-foreground/40">/</span>}
        </>
      )}
      {spanId && traceId && (
        <button
          type="button"
          onClick={() => onOpenSpan(traceId, spanId)}
          className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded hover:bg-muted hover:text-foreground transition-colors"
          title={`Span ${spanId}`}
        >
          <Layers className="h-3 w-3" />
          <span className="tracking-tight">span:{spanId.slice(0, 8)}</span>
        </button>
      )}
    </div>
  );
});

// Block item component for virtualized list - renders single block directly
const FeedBlockItem = memo(function FeedBlockItem({
  block,
  startTime,
  markdownEnabled,
  projectId,
  onOpenSession,
  onOpenTrace,
  onOpenSpan,
}: {
  block: Block;
  startTime?: string;
  markdownEnabled: boolean;
  projectId: string;
  onOpenSession: (sessionId: string) => void;
  onOpenTrace: (traceId: string) => void;
  onOpenSpan: (traceId: string, spanId: string) => void;
}) {
  return (
    <div>
      <MessageBreadcrumb
        sessionId={block.session_id}
        traceId={block.trace_id}
        spanId={block.span_id}
        onOpenSession={onOpenSession}
        onOpenTrace={onOpenTrace}
        onOpenSpan={onOpenSpan}
      />
      <TimelineRow
        block={block}
        startTime={startTime}
        preview={getBlockPreview(block)}
        copyText={getBlockCopyText(block)}
      >
        {renderBlockContent(block, markdownEnabled, projectId)}
      </TimelineRow>
    </div>
  );
});

// Raw span item component for virtualized list
const FeedSpanItem = memo(function FeedSpanItem({ span }: { span: SpanSummary }) {
  const [copied, setCopied] = useState(false);
  const timeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Cleanup timeout on unmount
  useEffect(() => {
    return () => {
      if (timeoutRef.current) {
        clearTimeout(timeoutRef.current);
      }
    };
  }, []);

  const handleCopy = useCallback(async () => {
    try {
      const text = JSON.stringify(span.raw_span ?? span, null, 2);
      await navigator.clipboard.writeText(text);
      setCopied(true);
      if (timeoutRef.current) {
        clearTimeout(timeoutRef.current);
      }
      timeoutRef.current = setTimeout(() => setCopied(false), 2000);
    } catch {
      // Clipboard API failed silently
    }
  }, [span]);

  const displayData = span.raw_span ?? span;

  return (
    <div className="rounded-lg border bg-card">
      <div className="flex items-center justify-between gap-1 border-b px-2 py-1.5 sm:gap-2 sm:px-3 sm:py-2">
        <div className="min-w-0 flex-1">
          <div className="truncate text-xs font-medium sm:text-sm">{span.span_name}</div>
          <code className="block truncate text-xs text-muted-foreground">{span.span_id}</code>
        </div>
        <Button
          variant="ghost"
          size="sm"
          onClick={handleCopy}
          className="h-6 w-6 shrink-0 p-0 sm:h-7 sm:w-7"
          aria-label="Copy span"
        >
          {copied ? (
            <Check className="h-3 w-3 text-green-600 sm:h-3.5 sm:w-3.5" />
          ) : (
            <Copy className="h-3 w-3 sm:h-3.5 sm:w-3.5" />
          )}
        </Button>
      </div>
      <div className="overflow-x-auto p-2 sm:p-3">
        <JsonContent data={displayData} disableCollapse />
      </div>
    </div>
  );
});

export default function RealtimePage() {
  const { projectId = "default" } = useParams();
  const otelClient = useOtelClient();
  const queryClient = useQueryClient();
  const [activeTab, setActiveTab] = useState<RealtimeTab>("messages");
  const [markdownEnabled, setMarkdownEnabled] = useState(
    () => settings.get<boolean>(MARKDOWN_ENABLED_KEY, true) ?? true,
  );
  const [liveEnabled, setLiveEnabled] = useState(true);

  // Buffer state
  // - traceMap: Map<trace_id, TraceData> - blocks grouped by trace, backend order preserved
  // - spans: stored DESC (newest first), reversed for display
  const [traceMap, setTraceMap] = useState<Map<string, TraceData>>(() => new Map());
  const [spans, setSpans] = useState<SpanSummary[]>([]);

  // Derived: flat block array for display (computed from traceMap)
  // Named displayBlocks to clarify this is the display-ready array
  const displayBlocks = useMemo(() => tracesToDisplayBlocks(traceMap, MAX_BUFFER_SIZE), [traceMap]);

  // Sheet state for trace/span/session detail flyouts
  const [viewTrace, setViewTrace] = useState<TraceSummary | null>(null);
  const [viewSpan, setViewSpan] = useState<SpanSummary | null>(null);
  const [viewSession, setViewSession] = useState<SessionSummary | null>(null);

  // SSE handlers for refreshing detail sheets when relevant events arrive
  const viewTraceId = viewTrace?.trace_id ?? null;
  const viewSpanId = viewSpan ? `${viewSpan.trace_id}:${viewSpan.span_id}` : null;
  const viewSessionId = viewSession?.session_id ?? null;
  const handleSseTraceRefresh = useSseDetailRefresh("trace", viewTraceId, queryClient);
  const handleSseSpanRefresh = useSseDetailRefresh("span", viewSpanId, queryClient);
  const handleSseSessionRefresh = useSseDetailRefresh("session", viewSessionId, queryClient);

  // Page load time - only show data after this time (absolute minimum)
  const pageLoadTimeRef = useRef(new Date().toISOString());

  // Track last fetched span timestamp for efficient windowed queries (raw tab)
  // This allows catching late-arriving spans while avoiding full history re-fetch
  const lastSpanTimeRef = useRef<string | null>(null);

  // Auto-scroll mode: true = follow new content, false = user browsing history
  // This tracks USER INTENT, not physical position
  const autoScrollModeRef = useRef(true);

  // Track previous scroll positions to detect scroll direction (per tab)
  const prevScrollTopRef = useRef<Record<RealtimeTab, number>>({ messages: 0, raw: 0 });

  // Physical position state (for UI indicators)
  const [isAtBottom, setIsAtBottom] = useState(true);
  const [newCount, setNewCount] = useState(0);

  // Debounce and throttle refs for high-throughput protection
  const debounceTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const throttleTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const lastRefetchTimeRef = useRef(0);
  const isFetchingRef = useRef(false);
  const pendingRefetchRef = useRef(false);

  // Track pending trace_ids from SSE events that need refetching
  const pendingTraceIdsRef = useRef<Set<string>>(new Set());

  // Track component mount state to prevent state updates after unmount
  const isMountedRef = useRef(true);

  // Refs to avoid stale closures
  const activeTabRef = useRef(activeTab);
  activeTabRef.current = activeTab;

  // Track stale tabs (SSE events occurred while tab was inactive)
  const staleTabsRef = useRef<Set<RealtimeTab>>(new Set());

  // Scroll container refs
  const messagesScrollRef = useRef<HTMLDivElement>(null);
  const spansScrollRef = useRef<HTMLDivElement>(null);

  // Spans are stored DESC, reversed for display (oldest at top)
  const displaySpans = useMemo(() => [...spans].reverse(), [spans]);

  const startTime = useMemo(() => displayBlocks[0]?.timestamp, [displayBlocks]);

  // Virtualizers - overscan ensures items near edges are pre-rendered for smoother scrolling
  // +1 count for sentinel item at end (ensures scroll-to-end shows bottom padding)
  const blocksVirtualizer = useVirtualizer({
    count: displayBlocks.length + 1,
    getScrollElement: () => messagesScrollRef.current,
    estimateSize: (index) =>
      index < displayBlocks.length ? estimateBlockHeight(displayBlocks[index]) : CONTAINER_PADDING, // Sentinel height = bottom padding
    overscan: 8, // Higher overscan for smoother scroll-to-end
  });

  const spansVirtualizer = useVirtualizer({
    count: displaySpans.length + 1,
    getScrollElement: () => spansScrollRef.current,
    estimateSize: (index) =>
      index < displaySpans.length ? ESTIMATED_SPAN_HEIGHT : CONTAINER_PADDING,
    overscan: 5,
  });

  // Unified scroll-to-end function using virtualizer's scrollToIndex
  // Scrolls to sentinel item (last index) which ensures bottom padding is visible
  const scrollToEnd = useCallback(
    (tab: RealtimeTab) => {
      const virtualizer = tab === "messages" ? blocksVirtualizer : spansVirtualizer;
      const count = tab === "messages" ? displayBlocks.length : displaySpans.length;

      if (count === 0) return;

      // Scroll to sentinel item (index = count, since virtualizer has count+1 items)
      // This ensures the last real item plus bottom padding is fully visible
      virtualizer.scrollToIndex(count, { align: "end", behavior: "auto" });
    },
    [blocksVirtualizer, spansVirtualizer, displayBlocks.length, displaySpans.length],
  );

  const activeTabConfig = useMemo(() => TABS.find((t) => t.value === activeTab), [activeTab]);

  // Calculate windowed start_time for efficient queries that catch late messages
  // Uses: max(pageLoadTime, lastFetchedTime - buffer)
  const getWindowedStartTime = useCallback((lastTime: string | null): string => {
    const pageLoadMs = new Date(pageLoadTimeRef.current).getTime();
    if (!lastTime) {
      return pageLoadTimeRef.current;
    }
    const windowStart = new Date(lastTime).getTime() - LATE_MESSAGE_BUFFER_MS;
    return new Date(Math.max(pageLoadMs, windowStart)).toISOString();
  }, []);

  // Refetch trace messages for pending traces from SSE events
  // Backend handles sorting, we just replace blocks for each trace
  const refetchTraceMessages = useCallback(async () => {
    // Skip if already fetching - coalesce into pending
    if (isFetchingRef.current) {
      pendingRefetchRef.current = true;
      return;
    }

    // Get pending trace_ids and clear the set
    const traceIds = Array.from(pendingTraceIdsRef.current);
    if (traceIds.length === 0) return;
    pendingTraceIdsRef.current.clear();

    // Throttle: ensure minimum interval between fetches
    const now = Date.now();
    const timeSinceLastFetch = now - lastRefetchTimeRef.current;
    if (timeSinceLastFetch < MIN_REFETCH_INTERVAL_MS) {
      // Re-add trace_ids and schedule for later
      for (const id of traceIds) {
        pendingTraceIdsRef.current.add(id);
      }
      if (!pendingRefetchRef.current) {
        pendingRefetchRef.current = true;
        if (throttleTimerRef.current) clearTimeout(throttleTimerRef.current);
        throttleTimerRef.current = setTimeout(() => {
          throttleTimerRef.current = null;
          pendingRefetchRef.current = false;
          refetchTraceMessages();
        }, MIN_REFETCH_INTERVAL_MS - timeSinceLastFetch);
      }
      return;
    }

    // Mark as fetching
    isFetchingRef.current = true;
    lastRefetchTimeRef.current = now;

    const fetchTab = activeTabRef.current;

    try {
      if (fetchTab === "messages") {
        // Fetch messages for each trace (backend returns correctly sorted)
        const traceMessagesPromises = traceIds.map((traceId) =>
          otelClient.getTraceMessages(projectId, traceId).catch(() => ({ messages: [] })),
        );
        const results = await Promise.all(traceMessagesPromises);

        // Skip state updates if tab changed or component unmounted
        if (activeTabRef.current !== fetchTab || !isMountedRef.current) return;

        // Build new trace data from backend responses
        // Backend returns messages in canonical order - we preserve it exactly
        const newTraceData = new Map<string, TraceData>();
        for (let i = 0; i < results.length; i++) {
          const res = results[i];
          if (res.messages && res.messages.length > 0) {
            const messages = res.messages;
            newTraceData.set(traceIds[i], {
              blocks: messages, // Backend order - NEVER re-sort
              startTime: messages[0].timestamp,
            });
          }
        }

        if (newTraceData.size === 0) return;

        setTraceMap((prev) => {
          // Create new map with atomic trace replacement
          // Unchanged traces keep their data, updated traces get fresh backend data
          const next = new Map(prev);
          let addedCount = 0;

          for (const [traceId, traceData] of newTraceData) {
            const oldTrace = prev.get(traceId);
            const oldCount = oldTrace?.blocks.length ?? 0;
            addedCount += Math.max(0, traceData.blocks.length - oldCount);
            next.set(traceId, traceData);
          }

          // Track new count for scroll indicator
          if (!autoScrollModeRef.current && addedCount > 0) {
            setNewCount((c) => c + addedCount);
          }

          return next;
        });
      } else {
        // For raw tab, use the original feed-based approach
        const startTime = getWindowedStartTime(lastSpanTimeRef.current);
        const res = await otelClient.getFeedSpans(projectId, {
          limit: REFETCH_LIMIT,
          start_time: startTime,
          include_raw_span: true,
        });
        // Skip state updates if tab changed or component unmounted
        if (activeTabRef.current !== fetchTab || !isMountedRef.current) return;

        if (res.data.length > 0 && res.data[0].timestamp_start) {
          lastSpanTimeRef.current = res.data[0].timestamp_start;
        }

        setSpans((prev) => {
          const existingIds = new Set(prev.map((s) => s.span_id));
          const newUnique = res.data.filter((s) => !existingIds.has(s.span_id));
          if (newUnique.length > 0) {
            const merged = [...prev, ...newUnique];
            merged.sort(compareSpansDesc);
            const limited = merged.slice(0, MAX_BUFFER_SIZE);
            if (!autoScrollModeRef.current) {
              setNewCount((c) => c + newUnique.length);
            }
            return limited;
          }
          return prev;
        });
      }
    } catch (error) {
      console.error("Failed to refetch trace messages:", error);
    } finally {
      isFetchingRef.current = false;

      // If more events arrived during fetch, schedule another fetch (only if mounted)
      if (
        isMountedRef.current &&
        (pendingRefetchRef.current || pendingTraceIdsRef.current.size > 0)
      ) {
        pendingRefetchRef.current = false;
        if (throttleTimerRef.current) clearTimeout(throttleTimerRef.current);
        throttleTimerRef.current = setTimeout(() => {
          throttleTimerRef.current = null;
          refetchTraceMessages();
        }, MIN_REFETCH_INTERVAL_MS);
      }
    }
  }, [projectId, otelClient, getWindowedStartTime]);

  // Handle SSE events with debounce - collect trace_ids for batch refetch
  const handleSpanEvent = useCallback(
    (event: SseSpanEvent) => {
      // Mark inactive tab as stale
      const inactiveTab: RealtimeTab = activeTabRef.current === "messages" ? "raw" : "messages";
      staleTabsRef.current.add(inactiveTab);

      // Collect trace_id for batch refetch
      if (event.trace_id) {
        pendingTraceIdsRef.current.add(event.trace_id);
      }

      if (debounceTimerRef.current) {
        clearTimeout(debounceTimerRef.current);
      }

      debounceTimerRef.current = setTimeout(refetchTraceMessages, DEBOUNCE_MS);

      // Also refresh detail sheets if the event matches the viewed entity
      handleSseTraceRefresh(event);
      handleSseSpanRefresh(event);
      handleSseSessionRefresh(event);
    },
    [refetchTraceMessages, handleSseTraceRefresh, handleSseSpanRefresh, handleSseSessionRefresh],
  );

  // SSE subscription
  const { status: sseStatus } = useSpanStream({
    projectId,
    enabled: liveEnabled,
    onSpan: handleSpanEvent,
  });

  // Cleanup debounce and throttle timers, mark unmounted
  useEffect(() => {
    isMountedRef.current = true;
    return () => {
      isMountedRef.current = false;
      if (debounceTimerRef.current) {
        clearTimeout(debounceTimerRef.current);
      }
      if (throttleTimerRef.current) {
        clearTimeout(throttleTimerRef.current);
      }
    };
  }, []);

  // Auto-scroll when in auto-scroll mode and new content arrives
  // Uses virtualizer.scrollToIndex for reliable positioning with dynamic heights
  useEffect(() => {
    if (!autoScrollModeRef.current) return;
    if (displayBlocks.length === 0) return;
    if (activeTab !== "messages") return;

    // Defer to allow virtualizer to update its count
    queueMicrotask(() => {
      scrollToEnd("messages");
      // Second scroll after paint to handle any measurement updates
      requestAnimationFrame(() => scrollToEnd("messages"));
    });
  }, [displayBlocks.length, activeTab, scrollToEnd]);

  useEffect(() => {
    if (!autoScrollModeRef.current) return;
    if (displaySpans.length === 0) return;
    if (activeTab !== "raw") return;

    queueMicrotask(() => {
      scrollToEnd("raw");
      requestAnimationFrame(() => scrollToEnd("raw"));
    });
  }, [displaySpans.length, activeTab, scrollToEnd]);

  // Scroll position detection - detects user intent via scroll direction
  // State updates are deferred via queueMicrotask to avoid flushSync conflicts with TanStack Virtual
  const handleScroll = useCallback((element: HTMLDivElement | null, tab: RealtimeTab) => {
    if (!element) return;

    const { scrollTop, scrollHeight, clientHeight } = element;
    const distanceFromBottom = scrollHeight - scrollTop - clientHeight;
    const threshold = 50;
    const atBottom = distanceFromBottom < threshold;

    // Detect scroll direction by comparing with previous position
    const prevScrollTop = prevScrollTopRef.current[tab];
    const scrolledUp = scrollTop < prevScrollTop - 5; // 5px threshold to filter noise
    prevScrollTopRef.current[tab] = scrollTop;

    // User explicitly scrolled up → disable auto-scroll (browsing history)
    if (scrolledUp && !atBottom) {
      autoScrollModeRef.current = false;
    }

    // User reached bottom → re-enable auto-scroll
    if (atBottom) {
      autoScrollModeRef.current = true;
    }

    // Defer state updates to avoid flushSync conflict with TanStack Virtual measurements
    queueMicrotask(() => {
      if (atBottom) {
        setNewCount(0);
      }
      setIsAtBottom(atBottom);
    });
  }, []);

  // Re-scroll when content height changes (e.g., images load, content expands)
  // Uses ResizeObserver to detect size changes and re-scrolls using virtualizer
  useEffect(() => {
    const scrollEl = activeTab === "messages" ? messagesScrollRef.current : spansScrollRef.current;
    if (!scrollEl) return;

    // Observe the first child (the content container with dynamic height)
    const contentEl = scrollEl.firstElementChild;
    if (!contentEl) return;

    let prevHeight = contentEl.clientHeight;

    const observer = new ResizeObserver((entries) => {
      // Only scroll if in auto-scroll mode
      if (!autoScrollModeRef.current) return;

      for (const entry of entries) {
        const newHeight = entry.contentRect.height;
        // Only scroll if height increased (images loaded, content expanded)
        if (newHeight > prevHeight) {
          queueMicrotask(() => scrollToEnd(activeTab));
        }
        prevHeight = newHeight;
      }
    });

    observer.observe(contentEl);
    return () => observer.disconnect();
  }, [activeTab, scrollToEnd]);

  // Jump to bottom - re-enables auto-scroll mode
  const jumpToBottom = useCallback(() => {
    autoScrollModeRef.current = true;
    setIsAtBottom(true);
    setNewCount(0);

    // Use virtualizer's scrollToIndex for reliable positioning
    queueMicrotask(() => {
      scrollToEnd(activeTab);
      requestAnimationFrame(() => scrollToEnd(activeTab));
    });
  }, [activeTab, scrollToEnd]);

  // Manual refresh - refetch all current traces
  const handleRefresh = useCallback(() => {
    // Add all current trace IDs to pending set for refetch
    for (const traceId of traceMap.keys()) {
      pendingTraceIdsRef.current.add(traceId);
    }
    refetchTraceMessages();
  }, [traceMap, refetchTraceMessages]);

  // Clear all blocks and spans
  const handleClear = useCallback(() => {
    setTraceMap(new Map());
    setSpans([]);
    // Reset the start time to now so cleared data isn't refetched
    pageLoadTimeRef.current = new Date().toISOString();
    lastSpanTimeRef.current = null;
    pendingTraceIdsRef.current.clear();
    setNewCount(0);
    setIsAtBottom(true);
    autoScrollModeRef.current = true;
  }, []);

  // Handle tab change - reset state, refetch if stale
  // Scrolling happens via the auto-scroll effects after re-render
  const handleTabChange = useCallback(
    (newTab: RealtimeTab) => {
      if (newTab === activeTab) return;

      // Update ref immediately so refetch uses correct tab
      activeTabRef.current = newTab;
      autoScrollModeRef.current = true;

      setActiveTab(newTab);
      setNewCount(0);
      setIsAtBottom(true);

      // If the new tab has stale data, trigger refetch
      if (staleTabsRef.current.has(newTab)) {
        staleTabsRef.current.delete(newTab);
        // Add all current trace IDs to refetch
        for (const traceId of traceMap.keys()) {
          pendingTraceIdsRef.current.add(traceId);
        }
        refetchTraceMessages();
      }

      // Scroll after React re-renders and virtualizer updates
      // Double rAF ensures the new tab's DOM is ready
      requestAnimationFrame(() => {
        requestAnimationFrame(() => {
          scrollToEnd(newTab);
        });
      });
    },
    [activeTab, traceMap, refetchTraceMessages, scrollToEnd],
  );

  // Toggle markdown
  const handleMarkdownToggle = useCallback(() => {
    const newValue = !markdownEnabled;
    setMarkdownEnabled(newValue);
    settings.set(MARKDOWN_ENABLED_KEY, newValue);
  }, [markdownEnabled]);

  // Toggle live mode
  const toggleLive = useCallback(() => {
    setLiveEnabled((v) => !v);
  }, []);

  // Connection status color (matches traces/sessions/spans pages)
  const liveIndicatorClass = useMemo(() => {
    if (!liveEnabled) return "bg-muted-foreground";
    if (sseStatus === "error") return "bg-destructive";
    return "bg-primary";
  }, [liveEnabled, sseStatus]);

  // Show waiting indicator if no data yet (starts empty, fills as events arrive)
  const showWaitingIndicator = useMemo(
    () =>
      (activeTab === "messages" && displayBlocks.length === 0) ||
      (activeTab === "raw" && spans.length === 0),
    [activeTab, displayBlocks.length, spans.length],
  );

  // Callbacks for opening session/trace/span detail sheets
  const handleOpenSession = useCallback((sessionId: string) => {
    setViewSession({ session_id: sessionId } as SessionSummary);
    setViewTrace(null);
    setViewSpan(null);
  }, []);

  const handleOpenTrace = useCallback((traceId: string) => {
    setViewTrace({ trace_id: traceId } as TraceSummary);
    setViewSession(null);
    setViewSpan(null);
  }, []);

  const handleOpenSpan = useCallback((traceId: string, spanId: string) => {
    setViewSpan({ trace_id: traceId, span_id: spanId } as SpanSummary);
    setViewSession(null);
    setViewTrace(null);
  }, []);

  const handleCloseSessionSheet = useCallback((open: boolean) => {
    if (!open) setViewSession(null);
  }, []);

  const handleCloseTraceSheet = useCallback((open: boolean) => {
    if (!open) setViewTrace(null);
  }, []);

  const handleCloseSpanSheet = useCallback((open: boolean) => {
    if (!open) setViewSpan(null);
  }, []);

  return (
    <div className="h-screen w-full mx-auto pt-header-offset sm:pt-header-offset-sm px-2 sm:px-4 overflow-hidden">
      <div className="flex h-full w-full min-w-0 flex-col overflow-hidden pb-3">
        {/* Toolbar */}
        <div className="@container flex shrink-0 min-w-0 items-center justify-between gap-3 pb-4">
          {/* Left: Tabs */}
          <div className="flex items-center gap-2">
            {/* Dropdown on narrow */}
            <div className="@[500px]:hidden">
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button variant="outline" size="sm" className="h-9 gap-2 px-3">
                    {activeTabConfig?.icon}
                    <span>{activeTabConfig?.label}</span>
                    <ChevronsUpDown className="h-3.5 w-3.5 opacity-50" />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="start">
                  {TABS.map((tab) => (
                    <DropdownMenuItem
                      key={tab.value}
                      onClick={() => handleTabChange(tab.value)}
                      className="gap-2"
                    >
                      {tab.icon}
                      {tab.label}
                      {activeTab === tab.value && <Check className="ml-auto h-4 w-4" />}
                    </DropdownMenuItem>
                  ))}
                </DropdownMenuContent>
              </DropdownMenu>
            </div>

            {/* Inline tabs on wide */}
            <div className="hidden h-9 items-center rounded-lg bg-muted p-1 @[500px]:flex">
              {TABS.map((tab) => (
                <button
                  key={tab.value}
                  type="button"
                  onClick={() => handleTabChange(tab.value)}
                  className={cn(
                    "relative flex h-7 items-center justify-center gap-2 rounded-md px-3 text-sm font-medium transition-all",
                    "focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
                    activeTab === tab.value
                      ? "bg-background text-foreground shadow-sm"
                      : "text-muted-foreground hover:text-foreground",
                  )}
                >
                  {tab.icon}
                  {tab.label}
                </button>
              ))}
            </div>
          </div>

          {/* Right: Controls */}
          <ButtonGroup>
            {/* Scroll to end button */}
            <Button
              variant="outline"
              size="sm"
              className="h-9 px-2"
              onClick={jumpToBottom}
              disabled={
                (activeTab === "messages" ? displayBlocks.length : displaySpans.length) === 0
              }
            >
              <ArrowDown className="h-4 w-4" />
            </Button>

            {/* Markdown toggle */}
            <Button
              variant="outline"
              size="sm"
              className={cn("h-9 px-2", markdownEnabled && activeTab === "messages" && "bg-muted")}
              onClick={handleMarkdownToggle}
              disabled={activeTab === "raw"}
            >
              {markdownEnabled ? <FileText className="h-4 w-4" /> : <Code className="h-4 w-4" />}
            </Button>

            {/* Live toggle */}
            <Button
              variant="outline"
              size="sm"
              className="h-9 px-2 gap-1.5 min-w-13 sm:min-w-17"
              onClick={toggleLive}
            >
              <span
                className={cn(
                  "h-2 w-2 rounded-full shrink-0 transition-colors",
                  liveIndicatorClass,
                )}
              />
              <span className={cn("hidden sm:inline text-xs", liveEnabled && "font-semibold")}>
                Live
              </span>
            </Button>

            {/* Clear - clears both blocks and spans */}
            <Button
              variant="outline"
              size="sm"
              className="h-9 px-2"
              onClick={handleClear}
              disabled={displayBlocks.length === 0 && spans.length === 0}
            >
              <Trash2 className="h-4 w-4" />
            </Button>

            {/* Refresh */}
            <Button variant="outline" size="sm" className="h-9 px-2" onClick={handleRefresh}>
              <RefreshCw className="h-4 w-4" />
            </Button>
          </ButtonGroup>
        </div>

        {/* Content container */}
        <div className="relative min-h-0 flex-1 overflow-hidden rounded-lg border">
          {showWaitingIndicator ? (
            <div className="h-full flex items-center justify-center">
              <WaitingIndicator
                entityName={activeTab === "messages" ? "messages" : "spans"}
                projectId={projectId}
              />
            </div>
          ) : activeTab === "messages" ? (
            <ImageGalleryProvider blocks={displayBlocks} projectId={projectId}>
              <div
                ref={messagesScrollRef}
                className="h-full overflow-auto"
                onScroll={(e) => handleScroll(e.currentTarget, "messages")}
              >
                <div
                  style={{
                    // getTotalSize includes sentinel (bottom padding), add only top padding
                    height: blocksVirtualizer.getTotalSize() + CONTAINER_PADDING,
                    width: "100%",
                    position: "relative",
                  }}
                >
                  {blocksVirtualizer.getVirtualItems().map((virtualItem) => {
                    // Skip sentinel item (last index) - it's just a spacer
                    if (virtualItem.index >= displayBlocks.length) {
                      return (
                        <div
                          key={virtualItem.key}
                          data-index={virtualItem.index}
                          ref={blocksVirtualizer.measureElement}
                          style={{
                            position: "absolute",
                            top: 0,
                            left: 0,
                            width: "100%",
                            height: CONTAINER_PADDING,
                            transform: `translateY(${virtualItem.start + CONTAINER_PADDING}px)`,
                          }}
                        />
                      );
                    }
                    const block = displayBlocks[virtualItem.index];
                    return (
                      <div
                        key={virtualItem.key}
                        data-index={virtualItem.index}
                        ref={blocksVirtualizer.measureElement}
                        style={{
                          position: "absolute",
                          top: 0,
                          left: 0,
                          width: "100%",
                          transform: `translateY(${virtualItem.start + CONTAINER_PADDING}px)`,
                          paddingLeft: CONTAINER_PADDING,
                          paddingRight: CONTAINER_PADDING,
                          paddingBottom: ITEM_GAP,
                        }}
                      >
                        <FeedBlockItem
                          block={block}
                          startTime={startTime}
                          markdownEnabled={markdownEnabled}
                          projectId={projectId}
                          onOpenSession={handleOpenSession}
                          onOpenTrace={handleOpenTrace}
                          onOpenSpan={handleOpenSpan}
                        />
                      </div>
                    );
                  })}
                </div>
              </div>
            </ImageGalleryProvider>
          ) : (
            <div
              ref={spansScrollRef}
              className="h-full overflow-auto"
              onScroll={(e) => handleScroll(e.currentTarget, "raw")}
            >
              <div
                style={{
                  // getTotalSize includes sentinel (bottom padding), add only top padding
                  height: spansVirtualizer.getTotalSize() + CONTAINER_PADDING,
                  width: "100%",
                  position: "relative",
                }}
              >
                {spansVirtualizer.getVirtualItems().map((virtualItem) => {
                  // Skip sentinel item (last index) - it's just a spacer
                  if (virtualItem.index >= displaySpans.length) {
                    return (
                      <div
                        key={virtualItem.key}
                        data-index={virtualItem.index}
                        ref={spansVirtualizer.measureElement}
                        style={{
                          position: "absolute",
                          top: 0,
                          left: 0,
                          width: "100%",
                          height: CONTAINER_PADDING,
                          transform: `translateY(${virtualItem.start + CONTAINER_PADDING}px)`,
                        }}
                      />
                    );
                  }
                  const span = displaySpans[virtualItem.index];
                  return (
                    <div
                      key={virtualItem.key}
                      data-index={virtualItem.index}
                      ref={spansVirtualizer.measureElement}
                      style={{
                        position: "absolute",
                        top: 0,
                        left: 0,
                        width: "100%",
                        transform: `translateY(${virtualItem.start + CONTAINER_PADDING}px)`,
                        paddingLeft: CONTAINER_PADDING,
                        paddingRight: CONTAINER_PADDING,
                        paddingBottom: ITEM_GAP,
                      }}
                    >
                      <FeedSpanItem span={span} />
                    </div>
                  );
                })}
              </div>
            </div>
          )}

          {/* Jump to bottom button */}
          {!isAtBottom && newCount > 0 && (
            <Button
              variant="secondary"
              size="sm"
              className="absolute bottom-4 left-1/2 -translate-x-1/2 shadow-lg gap-1.5 cursor-pointer"
              onClick={jumpToBottom}
            >
              <ArrowDown className="h-3.5 w-3.5" />
              New {activeTab === "messages" ? "Messages" : "Spans"}
            </Button>
          )}
        </div>
      </div>

      {/* Detail flyout sheets */}
      <TraceDetailSheet
        open={!!viewTrace}
        onOpenChange={handleCloseTraceSheet}
        trace={viewTrace}
        projectId={projectId}
        realtimeEnabled={false}
      />

      <SpanDetailSheet
        open={!!viewSpan}
        onOpenChange={handleCloseSpanSheet}
        span={viewSpan}
        projectId={projectId}
        realtimeEnabled={false}
      />

      <SessionDetailSheet
        open={!!viewSession}
        onOpenChange={handleCloseSessionSheet}
        session={viewSession}
        projectId={projectId}
        realtimeEnabled={false}
      />
    </div>
  );
}
