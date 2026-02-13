/**
 * Utilities for span detail display
 */

/**
 * Format a timestamp string for display
 */
export function formatTimestamp(timestamp: string): string {
  try {
    const date = new Date(timestamp);
    return date.toLocaleString(undefined, {
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
      fractionalSecondDigits: 3,
    });
  } catch {
    return timestamp;
  }
}

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
