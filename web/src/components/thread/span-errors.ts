import type { Block, SpanEnvelope } from "@/api/otel/types";

/** Where each failed span's error goes in a thread. */
export interface SpanErrorPlacement {
  /** Failures to show before the first block. */
  leading: SpanEnvelope[];
  /** Failures to show after the block at each index. */
  after: Map<number, SpanEnvelope[]>;
}

export function isFailedEnvelope(envelope: SpanEnvelope): boolean {
  return (
    envelope.status_code === "ERROR" || !!envelope.exception_type || !!envelope.exception_message
  );
}

function spanKey(traceId: string, spanId: string): string {
  return `${traceId}\u0000${spanId}`;
}

/**
 * Places each failed span's exception in the thread.
 *
 * A span's error follows the last block that span contributed, so it reads as the outcome of that
 * step. A span that failed without contributing any block - a model call that raised before it
 * answered - is placed by its start time, before the first block that started after it; without
 * this it had no trace in the thread at all, and a conversation that stopped with an exception
 * looked like one that simply ended.
 */
export function placeSpanErrors(blocks: Block[], envelopes: SpanEnvelope[]): SpanErrorPlacement {
  const placement: SpanErrorPlacement = { leading: [], after: new Map() };
  const failed = envelopes.filter(isFailedEnvelope);
  if (failed.length === 0) return placement;

  const lastBlockOfSpan = new Map<string, number>();
  blocks.forEach((block, index) =>
    lastBlockOfSpan.set(spanKey(block.trace_id, block.span_id), index),
  );

  const place = (index: number, envelope: SpanEnvelope) => {
    if (index < 0) {
      placement.leading.push(envelope);
      return;
    }
    const existing = placement.after.get(index);
    if (existing) existing.push(envelope);
    else placement.after.set(index, [envelope]);
  };

  for (const envelope of failed) {
    const own = lastBlockOfSpan.get(spanKey(envelope.trace_id, envelope.span_id));
    if (own !== undefined) {
      place(own, envelope);
      continue;
    }
    const started = Date.parse(envelope.start_time);
    const next = blocks.findIndex((block) => Date.parse(block.timestamp) > started);
    place(next === -1 ? blocks.length - 1 : next - 1, envelope);
  }
  return placement;
}
