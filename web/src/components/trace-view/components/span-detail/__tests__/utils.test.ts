import { describe, expect, it } from "vitest";

import type { SpanDetail } from "@/api/otel/types";
import { formatTimestampPrecise } from "@/lib/format";
import { spanInspectorSections } from "../utils";

function span(overrides: Partial<SpanDetail> = {}): SpanDetail {
  return {
    trace_id: "t",
    span_id: "s",
    parent_span_id: null,
    span_name: "chat",
    span_kind: null,
    span_category: null,
    observation_type: null,
    framework: null,
    status_code: null,
    timestamp_start: "2026-01-01T00:00:00.123Z",
    timestamp_end: null,
    duration_ms: null,
    environment: null,
    session_id: null,
    user_id: null,
    model: null,
    gen_ai_system: null,
    agent_name: null,
    input_tokens: 0,
    output_tokens: 0,
    total_tokens: 0,
    cache_read_tokens: 0,
    cache_write_tokens: 0,
    reasoning_tokens: 0,
    input_cost: 0,
    output_cost: 0,
    cache_read_cost: 0,
    cache_write_cost: 0,
    reasoning_cost: 0,
    total_cost: 0,
    event_count: 0,
    link_count: 0,
    input_preview: null,
    output_preview: null,
    ...overrides,
  };
}

describe("spanInspectorSections", () => {
  it("omits empty sections rather than showing empty inspectors", () => {
    const sections = spanInspectorSections(span());
    expect(sections.tokens).toBeNull();
    expect(sections.cost).toBeNull();
    expect(sections.metadata).toBeNull();
    expect(sections.overview).toMatchObject({ trace_id: "t", span_id: "s" });
  });

  it("states each cost part as a share of the reported total", () => {
    const sections = spanInspectorSections(
      span({ input_cost: 0.75, output_cost: 0.25, total_cost: 1, input_tokens: 10 }),
    );
    expect(sections.tokens).toEqual({ input: 10 });
    expect(sections.cost).toEqual({
      input: "$0.75 (75%)",
      output: "$0.25 (25%)",
      total: "$1.00",
    });
  });

  it("keeps a single finish reason scalar and several as a list", () => {
    expect(spanInspectorSections(span({ finish_reasons: ["stop"] })).overview.finish_reason).toBe(
      "stop",
    );
    expect(
      spanInspectorSections(span({ finish_reasons: ["stop", "length"] })).overview.finish_reason,
    ).toEqual(["stop", "length"]);
  });
});

describe("formatTimestampPrecise", () => {
  it("returns a dash for a missing value and the input for an unparseable one", () => {
    expect(formatTimestampPrecise(null)).toBe("-");
    expect(formatTimestampPrecise("not a date")).toBe("not a date");
  });
});
