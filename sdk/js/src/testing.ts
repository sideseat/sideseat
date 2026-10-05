/**
 * Helpers for testing code that uses SideSeat.
 *
 * ```ts
 * import { capture } from "@sideseat/sdk/testing";
 *
 * const spans = await capture({ integrations: [] }, async () => {
 *   await session({ sessionId: "s-1" }, () => runAgent());
 * });
 * ```
 */
import { context, metrics, propagation, trace } from "@opentelemetry/api";
import { logs } from "@opentelemetry/api-logs";
import {
  InMemorySpanExporter,
  SimpleSpanProcessor,
  type ReadableSpan,
} from "@opentelemetry/sdk-trace-base";
import type { SideSeatOptions } from "./config.js";
import { flush, init, shutdown } from "./index.js";

/**
 * Initializes SideSeat without network export, runs `fn`, shuts down, and resolves to the finished
 * spans. The global OpenTelemetry registrations are reset on both sides, which makes this suitable
 * for tests only.
 */
export async function capture(
  options: SideSeatOptions,
  fn: () => unknown | Promise<unknown>,
): Promise<ReadableSpan[]> {
  resetGlobals();
  const exporter = new InMemorySpanExporter();
  await init({
    export: false,
    metrics: false,
    logs: false,
    ...options,
    spanProcessors: [
      ...(options.spanProcessors ?? []),
      new SimpleSpanProcessor(exporter),
    ],
  });
  try {
    await fn();
    // Read before shutdown: the in-memory exporter discards its spans when it shuts down.
    await flush();
    return [...exporter.getFinishedSpans()];
  } finally {
    await shutdown();
    resetGlobals();
  }
}

/** Forgets OpenTelemetry's global providers, context manager, and propagator. For tests only. */
export function resetGlobals(): void {
  trace.disable();
  metrics.disable();
  logs.disable();
  context.disable();
  propagation.disable();
}
