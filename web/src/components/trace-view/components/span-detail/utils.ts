/**
 * Utilities for span detail display
 */

import type { Block, SpanDetail } from "@/api/otel/types";
import { formatCost, formatTimestampPrecise } from "@/lib/format";

/**
 * Raw span structure from OTLP
 */
export interface RawSpan {
  trace_id: string;
  span_id: string;
  parent_span_id: string | null;
  name: string;
  kind: number;
  start_time_unix_nano: number;
  end_time_unix_nano: number;
  status: { code: number; message: string } | null;
  attributes: Record<string, unknown>;
  events: Array<{
    name: string;
    timestamp: string;
    attributes: Record<string, unknown>;
    dropped_attributes_count: number;
  }>;
  links: Array<{
    trace_id: string;
    span_id: string;
    attributes: Record<string, unknown>;
  }>;
  resource: Record<string, unknown>;
}

/** The data-inspector sections of a span: one definition for the span page and the trace panel. */
export interface SpanInspectorSections {
  overview: Record<string, unknown>;
  tokens: Record<string, number> | null;
  cost: Record<string, string> | null;
  metadata: Record<string, unknown> | null;
  rawSpan: RawSpan | undefined;
}

const TOKEN_FIELDS = [
  ["input", "input_tokens"],
  ["output", "output_tokens"],
  ["cache_read", "cache_read_tokens"],
  ["cache_write", "cache_write_tokens"],
  ["reasoning", "reasoning_tokens"],
  ["total", "total_tokens"],
] as const;

const COST_PARTS = [
  ["input", "input_cost"],
  ["output", "output_cost"],
  ["cache_read", "cache_read_cost"],
  ["cache_write", "cache_write_cost"],
  ["reasoning", "reasoning_cost"],
] as const;

function nonEmpty<T extends Record<string, unknown>>(record: T): T | null {
  return Object.keys(record).length > 0 ? record : null;
}

export function spanInspectorSections(span: SpanDetail): SpanInspectorSections {
  const finishReason =
    span.finish_reasons?.length === 1
      ? span.finish_reasons[0]
      : span.finish_reasons?.length
        ? span.finish_reasons
        : undefined;

  const overview: Record<string, unknown> = {
    ...(span.gen_ai_system && { system: span.gen_ai_system }),
    ...(span.agent_name && { agent: span.agent_name }),
    ...(span.user_id && { user_id: span.user_id }),
    ...(span.session_id && { session_id: span.session_id }),
    trace_id: span.trace_id,
    span_id: span.span_id,
    ...(span.parent_span_id && { parent_span_id: span.parent_span_id }),
    start_time: formatTimestampPrecise(span.timestamp_start),
    ...(span.timestamp_end && { end_time: formatTimestampPrecise(span.timestamp_end) }),
    ...(finishReason && { finish_reason: finishReason }),
  };

  const tokens: Record<string, number> = {};
  for (const [label, field] of TOKEN_FIELDS) {
    if (span[field] > 0) tokens[label] = span[field];
  }

  // Shares are of the reported total when there is one, else of the parts' sum.
  const totalCost =
    span.total_cost > 0
      ? span.total_cost
      : COST_PARTS.reduce((sum, [, field]) => sum + span[field], 0);
  const withShare = (value: number) => {
    if (totalCost === 0) return formatCost(value);
    const pct = (value / totalCost) * 100;
    const pctStr = pct > 0 && pct < 1 ? "<1%" : `${Math.round(pct)}%`;
    return `${formatCost(value)} (${pctStr})`;
  };
  const cost: Record<string, string> = {};
  for (const [label, field] of COST_PARTS) {
    if (span[field] > 0) cost[label] = withShare(span[field]);
  }
  if (span.total_cost > 0) cost.total = formatCost(span.total_cost);

  const rawSpan = span.raw_span as RawSpan | undefined;
  const resourceAttributes = rawSpan?.resource?.attributes as Record<string, unknown> | undefined;
  const metadata: Record<string, unknown> = {};
  if (rawSpan?.attributes && Object.keys(rawSpan.attributes).length > 0) {
    metadata.attributes = rawSpan.attributes;
  }
  if (resourceAttributes && Object.keys(resourceAttributes).length > 0) {
    metadata.resourceAttributes = resourceAttributes;
  }
  if (rawSpan?.links && rawSpan.links.length > 0) {
    metadata.links = rawSpan.links;
  }

  return {
    overview,
    tokens: nonEmpty(tokens),
    cost: nonEmpty(cost),
    metadata: nonEmpty(metadata),
    rawSpan,
  };
}

/** A span's messages as data-inspector entries, numbered in thread order. */
export function blocksToInspectorData(blocks: Block[]): Record<string, unknown> {
  const result: Record<string, unknown> = {};
  blocks.forEach((block, i) => {
    const key = `${i + 1}. ${block.role}${block.name ? ` (${block.name})` : ""}`;
    const entry: Record<string, unknown> = { type: block.entry_type, content: block.content };
    if (block.tool_use_id) entry.tool_use_id = block.tool_use_id;
    result[key] = entry;
  });
  return result;
}
