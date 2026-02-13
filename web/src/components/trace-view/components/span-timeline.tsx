import { useMemo, useRef } from "react";
import { useTraceView } from "../contexts/use-trace-view";
import { flattenTree } from "../lib/tree-builder";
import { TimelineScale } from "./timeline-scale";
import { SpanTimelineRow } from "./span-timeline-row";
import { calculateTimelineMetrics, SCALE_WIDTH, type TimelineMetrics } from "@/components/timeline";
import type { TreeNode } from "../lib/types";

interface FlatTimelineNode {
  node: TreeNode;
  metrics: TimelineMetrics;
}

export function SpanTimeline() {
  const {
    filteredTree,
    traceStart,
    traceDuration,
    selectedSpanId,
    setSelectedSpanId,
    collapsedNodes,
    toggleCollapsed,
    rootDuration,
    showDuration,
    showCost,
  } = useTraceView();

  const scaleRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);

  const flattenedItems = useMemo(() => {
    if (!filteredTree || !traceStart) return [];

    const flatNodes = flattenTree(filteredTree, collapsedNodes);
    return flatNodes.map(
      (node): FlatTimelineNode => ({
        node,
        metrics: calculateTimelineMetrics(node.startTime, node.duration, traceStart, traceDuration),
      }),
    );
  }, [filteredTree, collapsedNodes, traceStart, traceDuration]);

  const handleScaleScroll = (e: React.UIEvent<HTMLDivElement>) => {
    if (contentRef.current) {
      const nextLeft = e.currentTarget.scrollLeft;
      if (contentRef.current.scrollLeft !== nextLeft) {
        contentRef.current.scrollLeft = nextLeft;
      }
    }
  };

  const handleContentScroll = (e: React.UIEvent<HTMLDivElement>) => {
    if (scaleRef.current) {
      const nextLeft = e.currentTarget.scrollLeft;
      if (scaleRef.current.scrollLeft !== nextLeft) {
        scaleRef.current.scrollLeft = nextLeft;
      }
    }
  };

  if (!filteredTree) {
    return (
      <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
        No spans available
      </div>
    );
  }

  return (
    <div className="flex h-full min-w-0 flex-col overflow-hidden">
      <div
        ref={scaleRef}
        className="shrink-0 overflow-x-auto overflow-y-hidden px-3 border-b bg-muted/30"
        onScroll={handleScaleScroll}
      >
        <div className="min-w-fit" style={{ minWidth: SCALE_WIDTH }}>
          <TimelineScale scaleWidth={SCALE_WIDTH} />
        </div>
      </div>

      <div
        ref={contentRef}
        className="flex-1 overflow-auto px-3 py-1"
        onScroll={handleContentScroll}
      >
        <div className="min-w-fit" style={{ minWidth: SCALE_WIDTH }} role="tree">
          {flattenedItems.map(({ node, metrics }) => (
            <SpanTimelineRow
              key={node.id}
              node={node}
              metrics={metrics}
              isSelected={selectedSpanId === node.id}
              isCollapsed={collapsedNodes.has(node.id)}
              onSelect={setSelectedSpanId}
              onToggleCollapse={toggleCollapsed}
              rootDuration={rootDuration}
              showDuration={showDuration}
              showCost={showCost}
            />
          ))}
        </div>
      </div>
    </div>
  );
}
