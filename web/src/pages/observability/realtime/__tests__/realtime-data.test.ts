import { describe, expect, it } from "vitest";

import type { SpanSummary } from "@/api/otel/types";
import { mergeSpans } from "../realtime-data";

function span(trace_id: string, span_id: string, timestamp_start: string): SpanSummary {
  return { trace_id, span_id, timestamp_start } as SpanSummary;
}

describe("mergeSpans", () => {
  it("keeps spans that share an id across traces", () => {
    const existing = [span("trace-a", "1", "2026-01-01T00:00:01Z")];
    const { spans, added } = mergeSpans(
      existing,
      [span("trace-b", "1", "2026-01-01T00:00:02Z")],
      10,
    );
    expect(added).toBe(1);
    expect(spans.map((s) => s.trace_id)).toEqual(["trace-b", "trace-a"]);
  });

  it("drops spans already buffered and duplicates within one response", () => {
    const existing = [span("t", "1", "2026-01-01T00:00:01Z")];
    const again = span("t", "1", "2026-01-01T00:00:01Z");
    const fresh = span("t", "2", "2026-01-01T00:00:02Z");
    const { spans, added } = mergeSpans(existing, [again, fresh, fresh], 10);
    expect(added).toBe(1);
    expect(spans).toHaveLength(2);
  });

  it("keeps the newest spans when over the cap", () => {
    const existing = [span("t", "1", "2026-01-01T00:00:01Z")];
    const { spans } = mergeSpans(existing, [span("t", "2", "2026-01-01T00:00:02Z")], 1);
    expect(spans.map((s) => s.span_id)).toEqual(["2"]);
  });
});
