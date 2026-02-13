import { useState, useEffect, useCallback, useMemo, useRef } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  AlertCircle,
  Copy,
  Check,
  RefreshCw,
  Download,
  Search,
  X,
  ChevronUp,
  ChevronDown,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { ButtonGroup } from "@/components/ui/button-group";
import { Input } from "@/components/ui/input";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { JsonContent, highlightText, MAX_SEARCH_LENGTH } from "@/components/thread";
import { deepParseJsonStrings, downloadFile } from "@/lib/utils";

interface RawSpan {
  span_id: string;
  span_name: string;
  raw_span?: Record<string, unknown>;
}

// Search only in actual values (strings, numbers, booleans) and keys, not JSON structure
function searchInValues(obj: unknown, searchLower: string): boolean {
  if (obj === null || obj === undefined) return false;
  if (typeof obj === "string") return obj.toLowerCase().includes(searchLower);
  if (typeof obj === "number") return String(obj).toLowerCase().includes(searchLower);
  if (typeof obj === "boolean") return String(obj).toLowerCase().includes(searchLower);
  if (Array.isArray(obj)) return obj.some((item) => searchInValues(item, searchLower));
  if (typeof obj === "object") {
    return Object.entries(obj).some(
      ([key, value]) =>
        key.toLowerCase().includes(searchLower) || searchInValues(value, searchLower),
    );
  }
  return false;
}

interface RawSpansViewProps {
  spans: RawSpan[];
  entityId: string;
  /** Prefix for download filename. Defaults to "trace" (results in "trace-{entityId}.json") */
  downloadPrefix?: string;
  isLoading: boolean;
  error?: Error | null;
  onRetry: () => void;
}

export function RawSpansView({
  spans,
  entityId,
  downloadPrefix = "trace",
  isLoading,
  error,
  onRetry,
}: RawSpansViewProps) {
  const [copiedSpanId, setCopiedSpanId] = useState<string | null>(null);
  const [copiedAll, setCopiedAll] = useState(false);
  const [search, setSearch] = useState("");
  const [debouncedSearch, setDebouncedSearch] = useState("");
  const [currentMatchIndex, setCurrentMatchIndex] = useState(0);

  const scrollRef = useRef<HTMLDivElement>(null);
  const copyTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const copyAllTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const searchDebounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const scrollTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const scrollAnimationRef = useRef<number | null>(null);
  const scrollToFirstMatchRef = useRef<(() => void) | undefined>(undefined);

  // Debounce search to avoid re-renders on every keystroke
  useEffect(() => {
    if (searchDebounceRef.current) clearTimeout(searchDebounceRef.current);
    searchDebounceRef.current = setTimeout(() => {
      setDebouncedSearch(search);
    }, 150);
    return () => {
      if (searchDebounceRef.current) clearTimeout(searchDebounceRef.current);
    };
  }, [search]);

  // Cleanup timeouts and animations on unmount
  useEffect(() => {
    return () => {
      if (copyTimeoutRef.current) clearTimeout(copyTimeoutRef.current);
      if (copyAllTimeoutRef.current) clearTimeout(copyAllTimeoutRef.current);
      if (scrollTimeoutRef.current) clearTimeout(scrollTimeoutRef.current);
      if (scrollAnimationRef.current) cancelAnimationFrame(scrollAnimationRef.current);
    };
  }, []);

  // Parse JSON strings in raw spans
  const spansWithParsedRaw = useMemo(() => {
    return spans.map((s) => ({
      ...s,
      raw_span: deepParseJsonStrings(s.raw_span),
    }));
  }, [spans]);

  // Trimmed search term for consistent usage
  const trimmedSearch = useMemo(() => debouncedSearch.trim(), [debouncedSearch]);

  // Check if search is pure JSON syntax (would match all spans but nothing highlights)
  const isJsonSyntaxSearch = useMemo(() => {
    return trimmedSearch.length > 0 && /^[{}[\]:,"]+$/.test(trimmedSearch);
  }, [trimmedSearch]);

  // Find indices of spans that contain matches (for navigation)
  const matchingSpanIndices = useMemo(() => {
    if (!trimmedSearch || isJsonSyntaxSearch || trimmedSearch.length > MAX_SEARCH_LENGTH) return [];
    const searchLower = trimmedSearch.toLowerCase();
    return spansWithParsedRaw
      .map((span, index) => {
        const nameMatch = span.span_name?.toLowerCase().includes(searchLower);
        const idMatch = span.span_id?.toLowerCase().includes(searchLower);
        const jsonMatch = searchInValues(span.raw_span, searchLower);
        return nameMatch || idMatch || jsonMatch ? index : -1;
      })
      .filter((index) => index !== -1);
  }, [spansWithParsedRaw, trimmedSearch, isJsonSyntaxSearch]);

  const rawSpanData = useMemo(() => {
    return spansWithParsedRaw.map((s) => s.raw_span);
  }, [spansWithParsedRaw]);

  const estimatedHeights = useMemo(() => {
    return spansWithParsedRaw.map((span) => {
      if (!span.raw_span) return 200;
      const str = JSON.stringify(span.raw_span);
      const lineCount = (str.match(/[{}[\],]/g)?.length ?? 0) + 1;
      const headerHeight = 52;
      const lineHeight = 22;
      const padding = 32;
      return headerHeight + Math.min(lineCount * lineHeight, 5000) + padding;
    });
  }, [spansWithParsedRaw]);

  const virtualizer = useVirtualizer({
    count: spansWithParsedRaw.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (index) => estimatedHeights[index] ?? 200,
    overscan: 2,
  });

  const hasMatches = matchingSpanIndices.length > 0;

  const goToMatch = useCallback(
    (matchIndex: number) => {
      if (matchingSpanIndices.length === 0) return;
      const clampedMatchIndex = Math.max(0, Math.min(matchIndex, matchingSpanIndices.length - 1));
      setCurrentMatchIndex(clampedMatchIndex);

      const spanIndex = matchingSpanIndices[clampedMatchIndex];
      if (spanIndex === undefined) return;

      // Cancel any pending scroll animation
      if (scrollAnimationRef.current) {
        cancelAnimationFrame(scrollAnimationRef.current);
      }

      const container = scrollRef.current;
      if (!container) return;

      // Get the offset for this span from virtualizer
      const offsetResult = virtualizer.getOffsetForIndex(spanIndex, "start");
      if (offsetResult) {
        const [offset] = offsetResult;
        container.scrollTop = offset;
      }

      let attempts = 0;
      const maxAttempts = 30;

      const tryScrollToMark = () => {
        const card = container.querySelector(`[data-index="${spanIndex}"]`);
        const mark = card?.querySelector("mark");

        if (mark) {
          // Calculate mark position relative to container
          const containerRect = container.getBoundingClientRect();
          const markRect = mark.getBoundingClientRect();
          const markRelativeTop = markRect.top - containerRect.top;

          // If mark is not visible, scroll to it
          if (markRelativeTop < 0 || markRelativeTop > container.clientHeight - 50) {
            const newScrollTop =
              container.scrollTop + markRelativeTop - container.clientHeight * 0.3;
            container.scrollTop = Math.max(0, newScrollTop);
          }
          scrollAnimationRef.current = null;
        } else if (attempts < maxAttempts) {
          attempts++;
          scrollAnimationRef.current = requestAnimationFrame(tryScrollToMark);
        } else {
          scrollAnimationRef.current = null;
        }
      };

      // Give time for the scroll and render to settle
      if (scrollTimeoutRef.current) clearTimeout(scrollTimeoutRef.current);
      scrollTimeoutRef.current = setTimeout(() => {
        scrollAnimationRef.current = requestAnimationFrame(tryScrollToMark);
      }, 50);
    },
    [matchingSpanIndices, virtualizer],
  );

  const goToPrevMatch = useCallback(() => {
    const newIndex =
      currentMatchIndex <= 0 ? matchingSpanIndices.length - 1 : currentMatchIndex - 1;
    goToMatch(newIndex);
  }, [currentMatchIndex, matchingSpanIndices.length, goToMatch]);

  const goToNextMatch = useCallback(() => {
    const newIndex =
      currentMatchIndex >= matchingSpanIndices.length - 1 ? 0 : currentMatchIndex + 1;
    goToMatch(newIndex);
  }, [currentMatchIndex, matchingSpanIndices.length, goToMatch]);

  // Scroll to first match when search changes
  scrollToFirstMatchRef.current = () => {
    if (matchingSpanIndices.length > 0) {
      goToMatch(0);
    } else {
      setCurrentMatchIndex(0);
    }
  };

  useEffect(() => {
    // Only scroll when there's an active search
    if (!trimmedSearch) {
      setCurrentMatchIndex(0);
      return;
    }

    // Delay scroll to run after virtual list stabilizes
    let rafId1: number | null = null;
    let rafId2: number | null = null;
    const timeoutId = setTimeout(() => {
      // Double RAF to ensure DOM is painted before scrolling
      rafId1 = requestAnimationFrame(() => {
        rafId2 = requestAnimationFrame(() => {
          scrollToFirstMatchRef.current?.();
        });
      });
    }, 50);
    return () => {
      clearTimeout(timeoutId);
      if (rafId1) cancelAnimationFrame(rafId1);
      if (rafId2) cancelAnimationFrame(rafId2);
    };
  }, [trimmedSearch]);

  const handleCopySpan = useCallback(async (spanId: string, rawSpan: Record<string, unknown>) => {
    try {
      await navigator.clipboard.writeText(JSON.stringify(rawSpan, null, 2));
      setCopiedSpanId(spanId);
      if (copyTimeoutRef.current) clearTimeout(copyTimeoutRef.current);
      copyTimeoutRef.current = setTimeout(() => setCopiedSpanId(null), 2000);
    } catch {
      // Clipboard API not available or failed
    }
  }, []);

  const handleCopyAll = useCallback(async () => {
    if (rawSpanData.length === 0) return;
    try {
      await navigator.clipboard.writeText(JSON.stringify(rawSpanData, null, 2));
      setCopiedAll(true);
      if (copyAllTimeoutRef.current) clearTimeout(copyAllTimeoutRef.current);
      copyAllTimeoutRef.current = setTimeout(() => setCopiedAll(false), 2000);
    } catch {
      // Clipboard API not available or failed
    }
  }, [rawSpanData]);

  const handleDownloadAll = useCallback(() => {
    if (rawSpanData.length === 0) return;
    downloadFile(
      JSON.stringify(rawSpanData, null, 2),
      `${downloadPrefix}-${entityId}.json`,
      "application/json",
    );
  }, [rawSpanData, downloadPrefix, entityId]);

  // Loading state - render nothing, parent handles loading indicator
  if (isLoading) {
    return null;
  }

  if (error) {
    return (
      <div className="@container flex h-full flex-col items-center justify-center gap-4 p-4 @[400px]:p-8">
        <AlertCircle className="h-10 w-10 text-destructive @[400px]:h-12 @[400px]:w-12" />
        <div className="text-center">
          <h3 className="font-medium">Failed to load spans</h3>
          <p className="text-sm text-muted-foreground">{error.message}</p>
        </div>
        <Button variant="outline" size="sm" onClick={onRetry}>
          <RefreshCw className="mr-2 h-4 w-4" />
          Retry
        </Button>
      </div>
    );
  }

  if (spansWithParsedRaw.length === 0) {
    return (
      <div className="@container flex h-full flex-col items-center justify-center gap-4 p-4 @[400px]:p-8">
        <AlertCircle className="h-10 w-10 text-muted-foreground/50 @[400px]:h-12 @[400px]:w-12" />
        <div className="text-center">
          <h3 className="font-medium text-muted-foreground">No raw data</h3>
          <p className="text-sm text-muted-foreground">
            Raw span data is not available for this trace.
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className="@container flex h-full flex-col overflow-hidden">
      <div className="@container shrink-0 flex items-center gap-2 border-b bg-muted/30 px-2 py-1.5 @[400px]:px-3">
        <div className="relative shrink min-w-20 w-32 @[400px]:w-40 @[500px]:w-56">
          <Search className="absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
          <Input
            type="text"
            placeholder="Search..."
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && hasMatches) {
                e.preventDefault();
                if (e.shiftKey) {
                  goToPrevMatch();
                } else {
                  goToNextMatch();
                }
              }
            }}
            className="h-7 pl-7 pr-7 text-xs"
          />
          {search && (
            <Button
              variant="ghost"
              size="sm"
              onClick={() => setSearch("")}
              className="absolute right-0.5 top-1/2 h-6 w-6 -translate-y-1/2 p-0"
              aria-label="Clear search"
            >
              <X className="h-3 w-3" />
            </Button>
          )}
        </div>
        {trimmedSearch && !isJsonSyntaxSearch && (
          <div className="flex items-center gap-0.5">
            <span className="text-xs text-muted-foreground tabular-nums whitespace-nowrap">
              {hasMatches ? `${currentMatchIndex + 1}/${matchingSpanIndices.length}` : "0 results"}
            </span>
            {hasMatches && (
              <>
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={goToPrevMatch}
                  className="h-6 w-6 p-0"
                  aria-label="Previous match"
                >
                  <ChevronUp className="h-4 w-4" />
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={goToNextMatch}
                  className="h-6 w-6 p-0"
                  aria-label="Next match"
                >
                  <ChevronDown className="h-4 w-4" />
                </Button>
              </>
            )}
          </div>
        )}
        <div className="flex-1" />
        <span className="hidden text-xs text-muted-foreground whitespace-nowrap @[450px]:inline">
          {spansWithParsedRaw.length} span{spansWithParsedRaw.length !== 1 ? "s" : ""}
        </span>
        <ButtonGroup>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="outline"
                size="sm"
                onClick={handleCopyAll}
                className="h-7 w-7 px-0"
                aria-label="Copy all spans"
              >
                {copiedAll ? <Check className="h-3.5 w-3.5" /> : <Copy className="h-3.5 w-3.5" />}
              </Button>
            </TooltipTrigger>
            <TooltipContent>Copy all</TooltipContent>
          </Tooltip>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="outline"
                size="sm"
                onClick={handleDownloadAll}
                className="h-7 w-7 px-0"
                aria-label="Download all spans"
              >
                <Download className="h-3.5 w-3.5" />
              </Button>
            </TooltipTrigger>
            <TooltipContent>Download</TooltipContent>
          </Tooltip>
        </ButtonGroup>
      </div>
      <div ref={scrollRef} className="flex-1 min-h-0 overflow-auto">
        <div className="relative p-2 @[400px]:p-3" style={{ height: virtualizer.getTotalSize() }}>
          {virtualizer.getVirtualItems().map((virtualRow) => {
            const index = virtualRow.index;
            const span = spansWithParsedRaw[index];
            if (!span) return null;
            return (
              <div
                key={span.span_id}
                data-index={index}
                ref={virtualizer.measureElement}
                className="absolute left-2 right-2 @[400px]:left-3 @[400px]:right-3 pb-2 @[400px]:pb-3"
                style={{ transform: `translateY(${virtualRow.start}px)` }}
              >
                <div className="rounded-lg border bg-card">
                  <div className="flex items-center justify-between gap-1 border-b px-2 py-1.5 @[400px]:gap-2 @[400px]:px-3 @[400px]:py-2">
                    <div className="min-w-0 flex-1">
                      <div className="truncate text-xs font-medium @[400px]:text-sm">
                        {highlightText(span.span_name, trimmedSearch)}
                      </div>
                      <code className="block truncate text-xs text-muted-foreground">
                        {highlightText(span.span_id, trimmedSearch)}
                      </code>
                    </div>
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => span.raw_span && handleCopySpan(span.span_id, span.raw_span)}
                      className="h-6 w-6 shrink-0 p-0 @[400px]:h-7 @[400px]:w-7"
                      aria-label="Copy span"
                    >
                      {copiedSpanId === span.span_id ? (
                        <Check className="h-3 w-3 text-green-600 @[400px]:h-3.5 @[400px]:w-3.5" />
                      ) : (
                        <Copy className="h-3 w-3 @[400px]:h-3.5 @[400px]:w-3.5" />
                      )}
                    </Button>
                  </div>
                  <div className="overflow-x-auto p-2 @[400px]:p-3">
                    <JsonContent data={span.raw_span} disableCollapse highlight={trimmedSearch} />
                  </div>
                </div>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
