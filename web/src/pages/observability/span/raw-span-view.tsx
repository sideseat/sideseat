import { useState, useEffect, useCallback, useMemo, useRef } from "react";
import { Copy, Check, Download, Search, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ButtonGroup } from "@/components/ui/button-group";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { JsonContent } from "@/components/thread";
import { deepParseJsonStrings, downloadFile } from "@/lib/utils";

interface RawSpanViewProps {
  spanId: string;
  spanName: string;
  rawSpan: Record<string, unknown>;
}

export function RawSpanView({ spanId, spanName, rawSpan }: RawSpanViewProps) {
  const [copied, setCopied] = useState(false);
  const [search, setSearch] = useState("");
  const [debouncedSearch, setDebouncedSearch] = useState("");

  const copyTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const searchDebounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

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

  // Cleanup timeouts on unmount
  useEffect(() => {
    return () => {
      if (copyTimeoutRef.current) clearTimeout(copyTimeoutRef.current);
    };
  }, []);

  // Parse JSON strings in raw span
  const parsedRawSpan = useMemo(() => {
    return deepParseJsonStrings(rawSpan);
  }, [rawSpan]);

  const trimmedSearch = useMemo(() => debouncedSearch.trim(), [debouncedSearch]);

  const handleCopy = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(JSON.stringify(parsedRawSpan, null, 2));
      setCopied(true);
      if (copyTimeoutRef.current) clearTimeout(copyTimeoutRef.current);
      copyTimeoutRef.current = setTimeout(() => setCopied(false), 2000);
    } catch {
      // Clipboard API not available or failed
    }
  }, [parsedRawSpan]);

  const handleDownload = useCallback(() => {
    downloadFile(JSON.stringify(parsedRawSpan, null, 2), `span-${spanId}.json`, "application/json");
  }, [parsedRawSpan, spanId]);

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
        <div className="flex-1" />
        <code className="hidden truncate text-xs text-muted-foreground @[500px]:block @[500px]:max-w-[150px] @[700px]:max-w-[250px]">
          {spanName}
        </code>
        <ButtonGroup>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="outline"
                size="sm"
                onClick={handleCopy}
                className="h-7 w-7 px-0"
                aria-label="Copy span"
              >
                {copied ? <Check className="h-3.5 w-3.5" /> : <Copy className="h-3.5 w-3.5" />}
              </Button>
            </TooltipTrigger>
            <TooltipContent>Copy</TooltipContent>
          </Tooltip>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="outline"
                size="sm"
                onClick={handleDownload}
                className="h-7 w-7 px-0"
                aria-label="Download span"
              >
                <Download className="h-3.5 w-3.5" />
              </Button>
            </TooltipTrigger>
            <TooltipContent>Download</TooltipContent>
          </Tooltip>
        </ButtonGroup>
      </div>
      <ScrollArea className="flex-1 min-h-0">
        <div className="p-2 @[400px]:p-3">
          <div className="rounded-lg border bg-card p-2 @[400px]:p-3">
            <JsonContent data={parsedRawSpan} disableCollapse highlight={trimmedSearch} />
          </div>
        </div>
      </ScrollArea>
    </div>
  );
}
