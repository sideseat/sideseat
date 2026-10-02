import type { Block, SpanSummary } from "@/api/otel/types";

export type RealtimeTab = "messages" | "raw";

export const MAX_BUFFER_SIZE = 1000;
export const DEBOUNCE_MS = 100;
export const MIN_REFETCH_INTERVAL_MS = 500;
export const REFETCH_LIMIT = 100;
export const LATE_MESSAGE_BUFFER_MS = 30000;
// The feed rows render these as Tailwind classes (pb-3 for ITEM_GAP; px-4 and the
// h-4 sentinel for CONTAINER_PADDING). Change both together, or the virtualizer's
// size estimates drift from the rendered rows.
export const ITEM_GAP = 12;
export const CONTAINER_PADDING = 16;

/**
 * Blocks stay grouped by trace so backend order remains canonical within each
 * trace. The frontend only orders whole traces relative to one another.
 */
export interface TraceData {
  blocks: Block[];
  startTime: string;
}

/** Convert trace groups to an oldest-first display array without splitting traces. */
export function tracesToDisplayBlocks(
  traceMap: Map<string, TraceData>,
  maxBlocks: number,
): Block[] {
  const sortedTraces = Array.from(traceMap.values()).sort((a, b) =>
    a.startTime.localeCompare(b.startTime),
  );

  let totalBlocks = 0;
  let startIndex = 0;
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
        break;
      }
    }
  }

  return sortedTraces.slice(startIndex).flatMap((trace) => trace.blocks);
}

export function compareSpansDesc(a: SpanSummary, b: SpanSummary): number {
  const timeCompare = b.timestamp_start.localeCompare(a.timestamp_start);
  if (timeCompare !== 0) return timeCompare;
  return a.span_id.localeCompare(b.span_id);
}

export function estimateBlockHeight(block: Block): number {
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

// Actual heights are measured by the virtualizer after rendering.
export const ESTIMATED_SPAN_HEIGHT = 52 + 300 + 32 + ITEM_GAP;
