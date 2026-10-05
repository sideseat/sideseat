import { useState, useMemo, useRef, useLayoutEffect } from "react";
import { ChevronDown, ChevronRight, Loader2 } from "lucide-react";
import { ScrollArea } from "@/components/ui/scroll-area";
import { JsonContent } from "@/components/thread/content/json-content";
import { DataInspector } from "@/components/data-inspector";
import { useSpanMessages } from "@/api/otel/hooks/queries";
import { useTraceView } from "../../contexts/use-trace-view";
import { SpanDetailHeader } from "./span-detail-header";
import { blocksToInspectorData, spanInspectorSections } from "./utils";

interface SpanDetailPanelProps {
  projectId: string;
  traceId: string;
}

export function SpanDetailPanel({ projectId, traceId }: SpanDetailPanelProps) {
  const { selectedNode } = useTraceView();
  const [rawSpanExpanded, setRawSpanExpanded] = useState(false);
  const scrollAreaRef = useRef<HTMLDivElement>(null);
  const selectedSpanId = selectedNode?.id;

  // Scroll to top when selected span changes
  useLayoutEffect(() => {
    if (selectedSpanId && scrollAreaRef.current) {
      const viewport = scrollAreaRef.current.querySelector("[data-slot='scroll-area-viewport']");
      if (viewport) {
        viewport.scrollTop = 0;
      }
    }
  }, [selectedSpanId]);

  const { data: messagesData, isLoading: messagesLoading } = useSpanMessages(
    projectId,
    selectedNode?.span.trace_id ?? traceId,
    selectedNode?.id ?? "",
    undefined,
    { enabled: !!selectedNode },
  );

  const messages = messagesData?.messages;
  const messagesForInspector = useMemo(
    () => (messages ? blocksToInspectorData(messages) : {}),
    [messages],
  );

  if (!selectedNode) {
    return (
      <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
        Select a span to view details
      </div>
    );
  }

  const { span } = selectedNode;
  const {
    overview: overviewData,
    tokens: tokensData,
    cost: costData,
    metadata,
    rawSpan,
  } = spanInspectorSections(span);

  const hasMessages = Object.keys(messagesForInspector).length > 0;

  return (
    <ScrollArea ref={scrollAreaRef} className="h-full">
      <SpanDetailHeader node={selectedNode} />

      <div key={span.span_id} className="@container space-y-4 p-4">
        <DataInspector data={overviewData} title="Overview" />

        {messagesLoading ? (
          <div className="flex items-center justify-center py-4">
            <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
          </div>
        ) : hasMessages ? (
          <DataInspector data={messagesForInspector} title="Messages" expandLastItem flatten />
        ) : null}

        {tokensData && <DataInspector data={tokensData} title="Tokens" />}

        {costData && <DataInspector data={costData} title="Cost" />}

        {metadata && <DataInspector data={metadata} title="Metadata" />}

        {rawSpan && (
          <div>
            <button
              type="button"
              onClick={() => setRawSpanExpanded(!rawSpanExpanded)}
              className="mb-2 flex w-full items-center justify-between text-xs font-medium uppercase tracking-wide text-muted-foreground hover:text-foreground"
            >
              <span>Raw Span</span>
              {rawSpanExpanded ? (
                <ChevronDown className="h-4 w-4" />
              ) : (
                <ChevronRight className="h-4 w-4" />
              )}
            </button>
            <div className="rounded-md border bg-muted/30 p-3">
              <JsonContent data={rawSpan} collapsed={rawSpanExpanded ? undefined : 1} />
            </div>
          </div>
        )}
      </div>
    </ScrollArea>
  );
}
