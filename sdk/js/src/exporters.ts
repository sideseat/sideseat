import { SpanKind, SpanStatusCode } from "@opentelemetry/api";
import type { HrTime } from "@opentelemetry/api";
import type { ReadableSpan, SpanExporter } from "@opentelemetry/sdk-trace-base";
import { ExportResult, ExportResultCode } from "@opentelemetry/core";
import * as fs from "node:fs";

// Convert OTEL HrTime [seconds, nanoseconds] to ISO8601 string
function hrTimeToIso8601(hrTime: HrTime): string {
  const [seconds, nanos] = hrTime;
  const ms = seconds * 1000 + Math.floor(nanos / 1_000_000);
  return new Date(ms).toISOString();
}

// Convert OTEL HrTime to milliseconds
function hrTimeToMs(hrTime: HrTime): number {
  const [seconds, nanos] = hrTime;
  return seconds * 1000 + nanos / 1_000_000;
}

// Encode value for JSON (match Python encode_value)
export function encodeValue(value: unknown): unknown {
  if (value === null || value === undefined) return value;
  if (
    typeof value === "string" ||
    typeof value === "number" ||
    typeof value === "boolean"
  ) {
    return value;
  }
  if (typeof value === "bigint") {
    return value.toString(); // BigInt can't be JSON.stringify'd
  }
  if (value instanceof Date) {
    return value.toISOString();
  }
  if (value instanceof Uint8Array || Buffer.isBuffer(value)) {
    return Buffer.from(value).toString("base64");
  }
  if (Array.isArray(value)) {
    return value.map(encodeValue);
  }
  if (typeof value === "object") {
    const result: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(value)) {
      result[k] = encodeValue(v);
    }
    return result;
  }
  return `<${typeof value}>`;
}

// Encode OTEL attributes to JSON-safe object
function encodeAttributes(
  attrs: Record<string, unknown> | undefined,
): Record<string, unknown> {
  if (!attrs) return {};
  const result: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(attrs)) {
    result[key] = encodeValue(value);
  }
  return result;
}

// Convert ReadableSpan to dictionary format (match Python span_to_dict)
export function spanToDict(span: ReadableSpan): Record<string, unknown> {
  const ctx = span.spanContext();
  // instrumentationScope is newer name, instrumentationLibrary is deprecated
  const scope =
    (
      span as ReadableSpan & {
        instrumentationScope?: {
          name: string;
          version?: string;
          schemaUrl?: string;
        };
      }
    ).instrumentationScope ?? span.instrumentationLibrary;

  return {
    name: span.name,
    trace_id: ctx.traceId,
    span_id: ctx.spanId,
    parent_span_id: span.parentSpanId ?? null,
    kind: SpanKind[span.kind],
    start_time: hrTimeToIso8601(span.startTime),
    end_time: hrTimeToIso8601(span.endTime),
    duration_ms: hrTimeToMs(span.duration),
    attributes: encodeAttributes(span.attributes as Record<string, unknown>),
    events: span.events.map((e) => ({
      name: e.name,
      timestamp: hrTimeToIso8601(e.time),
      attributes: encodeAttributes(e.attributes as Record<string, unknown>),
    })),
    links: span.links.map((l) => ({
      trace_id: l.context.traceId,
      span_id: l.context.spanId,
      attributes: encodeAttributes(l.attributes as Record<string, unknown>),
    })),
    status:
      span.status.code !== SpanStatusCode.UNSET
        ? {
            status_code: SpanStatusCode[span.status.code],
            description: span.status.message,
          }
        : null,
    resource: encodeAttributes(
      span.resource.attributes as Record<string, unknown>,
    ),
    scope: scope
      ? {
          name: scope.name,
          version: scope.version,
          schema_url: scope.schemaUrl,
        }
      : null,
  };
}

// JSONL file exporter (match Python JsonFileSpanExporter)
export class JsonFileSpanExporter implements SpanExporter {
  private _fh: fs.WriteStream;
  private _closed: boolean = false;
  private _writeError: Error | null = null;

  constructor(path: string, mode: "a" | "w" = "a") {
    const flags = mode === "w" ? "w" : "a";
    this._fh = fs.createWriteStream(path, { flags });
    this._fh.on("error", (err) => {
      this._writeError = err;
    });
  }

  export(
    spans: ReadableSpan[],
    resultCallback: (result: ExportResult) => void,
  ): void {
    if (this._closed || this._writeError) {
      resultCallback({ code: ExportResultCode.FAILED });
      return;
    }
    try {
      for (const span of spans) {
        this._fh.write(JSON.stringify(spanToDict(span)) + "\n");
      }
      resultCallback({ code: ExportResultCode.SUCCESS });
    } catch (err) {
      this._writeError = err as Error;
      resultCallback({ code: ExportResultCode.FAILED });
    }
  }

  async shutdown(): Promise<void> {
    if (this._closed) return;
    this._closed = true;
    return new Promise((resolve, reject) => {
      this._fh.end(() => {
        if (this._writeError) reject(this._writeError);
        else resolve();
      });
    });
  }

  async forceFlush(): Promise<void> {
    // WriteStream flushes on write, nothing to do
    return Promise.resolve();
  }
}
