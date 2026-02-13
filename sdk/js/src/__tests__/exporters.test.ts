import { describe, it, expect } from "vitest";
import { encodeValue, spanToDict } from "../exporters.js";
import { SpanKind, SpanStatusCode } from "@opentelemetry/api";
import type { ReadableSpan } from "@opentelemetry/sdk-trace-base";
import { Resource } from "@opentelemetry/resources";

describe("encodeValue", () => {
  it("passes through primitives", () => {
    expect(encodeValue("hello")).toBe("hello");
    expect(encodeValue(42)).toBe(42);
    expect(encodeValue(true)).toBe(true);
    expect(encodeValue(null)).toBe(null);
    expect(encodeValue(undefined)).toBe(undefined);
  });

  it("encodes binary values as base64", () => {
    const result = encodeValue(Buffer.from("hello"));
    expect(result).toBe("aGVsbG8="); // base64 of 'hello'
  });

  it("encodes Uint8Array as base64", () => {
    const arr = new Uint8Array([104, 101, 108, 108, 111]); // 'hello'
    const result = encodeValue(arr);
    expect(result).toBe("aGVsbG8=");
  });

  it("encodes BigInt as string", () => {
    const result = encodeValue(BigInt("12345678901234567890"));
    expect(result).toBe("12345678901234567890");
  });

  it("encodes Date as ISO string", () => {
    const date = new Date("2024-01-15T10:30:00Z");
    expect(encodeValue(date)).toBe("2024-01-15T10:30:00.000Z");
  });

  it("recursively encodes arrays", () => {
    const result = encodeValue([1, Buffer.from("x"), { a: BigInt(2) }]);
    expect(result).toEqual([1, "eA==", { a: "2" }]);
  });

  it("recursively encodes objects", () => {
    const result = encodeValue({
      str: "hello",
      num: 42,
      nested: { bigint: BigInt(123) },
    });
    expect(result).toEqual({
      str: "hello",
      num: 42,
      nested: { bigint: "123" },
    });
  });

  it("handles unknown types", () => {
    const fn = () => {};
    expect(encodeValue(fn)).toBe("<function>");
  });
});

describe("spanToDict", () => {
  function createMockSpan(overrides: Partial<ReadableSpan> = {}): ReadableSpan {
    const defaultSpan: ReadableSpan = {
      name: "test-span",
      spanContext: () => ({
        traceId: "0123456789abcdef0123456789abcdef",
        spanId: "0123456789abcdef",
        traceFlags: 1,
      }),
      parentSpanId: "fedcba9876543210",
      kind: SpanKind.INTERNAL,
      startTime: [1700000000, 0] as [number, number],
      endTime: [1700000001, 500000000] as [number, number],
      duration: [1, 500000000] as [number, number],
      attributes: { "test.attr": "value" },
      events: [],
      links: [],
      status: { code: SpanStatusCode.UNSET },
      resource: new Resource({ "service.name": "test-service" }),
      instrumentationLibrary: { name: "test-lib", version: "1.0.0" },
      ended: true,
      droppedAttributesCount: 0,
      droppedEventsCount: 0,
      droppedLinksCount: 0,
      ...overrides,
    };
    return defaultSpan;
  }

  it("matches expected output format", () => {
    const mockSpan = createMockSpan();
    const result = spanToDict(mockSpan);

    expect(result).toHaveProperty(
      "trace_id",
      "0123456789abcdef0123456789abcdef",
    );
    expect(result).toHaveProperty("span_id", "0123456789abcdef");
    expect(result).toHaveProperty("parent_span_id", "fedcba9876543210");
    expect(result).toHaveProperty("name", "test-span");
    expect(result).toHaveProperty("kind", "INTERNAL");
  });

  it("converts HrTime to ISO8601 timestamps", () => {
    const mockSpan = createMockSpan();
    const result = spanToDict(mockSpan);

    expect(result.start_time).toMatch(/^\d{4}-\d{2}-\d{2}T/); // ISO8601
    expect(result.end_time).toMatch(/^\d{4}-\d{2}-\d{2}T/);
  });

  it("calculates duration_ms correctly", () => {
    const mockSpan = createMockSpan();
    const result = spanToDict(mockSpan);

    expect(result.duration_ms).toBe(1500); // 1 second + 500ms
  });

  it("handles null parent_span_id", () => {
    const mockSpan = createMockSpan({ parentSpanId: undefined });
    const result = spanToDict(mockSpan);

    expect(result.parent_span_id).toBe(null);
  });

  it("encodes attributes", () => {
    const mockSpan = createMockSpan({
      attributes: { "test.string": "value", "test.number": 42 },
    });
    const result = spanToDict(mockSpan);

    expect(result.attributes).toEqual({
      "test.string": "value",
      "test.number": 42,
    });
  });

  it("encodes events", () => {
    const mockSpan = createMockSpan({
      events: [
        {
          name: "test-event",
          time: [1700000000, 500000000] as [number, number],
          attributes: { "event.attr": "value" },
          droppedAttributesCount: 0,
        },
      ],
    });
    const result = spanToDict(mockSpan);

    expect(result.events).toHaveLength(1);
    expect((result.events as Array<{ name: string }>)[0].name).toBe(
      "test-event",
    );
  });

  it("encodes links", () => {
    const mockSpan = createMockSpan({
      links: [
        {
          context: {
            traceId: "linkedtraceid",
            spanId: "linkedspanid",
            traceFlags: 1,
          },
          attributes: { "link.attr": "value" },
          droppedAttributesCount: 0,
        },
      ],
    });
    const result = spanToDict(mockSpan);

    expect(result.links).toHaveLength(1);
    expect((result.links as Array<{ trace_id: string }>)[0].trace_id).toBe(
      "linkedtraceid",
    );
  });

  it("encodes status when set", () => {
    const mockSpan = createMockSpan({
      status: { code: SpanStatusCode.ERROR, message: "error occurred" },
    });
    const result = spanToDict(mockSpan);

    expect(result.status).toEqual({
      status_code: "ERROR",
      description: "error occurred",
    });
  });

  it("status is null when UNSET", () => {
    const mockSpan = createMockSpan({
      status: { code: SpanStatusCode.UNSET },
    });
    const result = spanToDict(mockSpan);

    expect(result.status).toBe(null);
  });

  it("encodes scope from instrumentationLibrary", () => {
    const mockSpan = createMockSpan({
      instrumentationLibrary: {
        name: "test-lib",
        version: "2.0.0",
        schemaUrl: "https://schema.url",
      },
    });
    const result = spanToDict(mockSpan);

    expect(result.scope).toEqual({
      name: "test-lib",
      version: "2.0.0",
      schema_url: "https://schema.url",
    });
  });

  it("handles instrumentationScope (newer property)", () => {
    const mockSpan = createMockSpan() as ReadableSpan & {
      instrumentationScope?: {
        name: string;
        version?: string;
        schemaUrl?: string;
      };
    };
    mockSpan.instrumentationScope = {
      name: "new-scope",
      version: "3.0.0",
    };
    const result = spanToDict(mockSpan);

    expect(result.scope).toEqual({
      name: "new-scope",
      version: "3.0.0",
      schema_url: undefined,
    });
  });
});
