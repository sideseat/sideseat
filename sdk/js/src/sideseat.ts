import {
  trace,
  diag,
  DiagConsoleLogger,
  DiagLogLevel,
  SpanStatusCode,
} from "@opentelemetry/api";
import type { Span, Tracer } from "@opentelemetry/api";
import type { SpanProcessor } from "@opentelemetry/sdk-trace-base";
import { NodeTracerProvider } from "@opentelemetry/sdk-trace-node";
import {
  BatchSpanProcessor,
  SimpleSpanProcessor,
  ConsoleSpanExporter,
} from "@opentelemetry/sdk-trace-base";
import { OTLPTraceExporter } from "@opentelemetry/exporter-trace-otlp-http";
import { resourceFromAttributes } from "@opentelemetry/resources";
import type { Resource } from "@opentelemetry/resources";
import {
  ATTR_SERVICE_NAME,
  ATTR_SERVICE_VERSION,
} from "@opentelemetry/semantic-conventions";
import {
  CompositePropagator,
  W3CBaggagePropagator,
  W3CTraceContextPropagator,
} from "@opentelemetry/core";
import * as nodePath from "node:path";
import * as fs from "node:fs";

import {
  Config,
  SideSeatError,
  type SideSeatOptions,
  type LogLevel,
} from "./config.js";
import { JsonFileSpanExporter } from "./exporters.js";
import { VERSION } from "./version.js";

/**
 * Span processor that forwards to a mutable list of delegates.
 *
 * OTel JS 2.x removed NodeTracerProvider.addSpanProcessor: processors must be passed
 * to the constructor. Registering one of these up front keeps this SDK's public
 * addSpanProcessor / setupConsoleExporter / setupFileExporter API working.
 */
export class ForwardingSpanProcessor implements SpanProcessor {
  private _delegates: SpanProcessor[] = [];

  add(processor: SpanProcessor): void {
    this._delegates.push(processor);
  }

  // Each delegate is isolated, on every hook. A processor that throws must not stop the
  // ones after it - that would silently drop the user's spans from every healthy exporter -
  // and must not throw out of here either, since onStart/onEnd run inside the application's
  // own call to span.end() and OTel's contract is that they do not raise.
  onStart(
    span: Parameters<SpanProcessor["onStart"]>[0],
    parentContext: Parameters<SpanProcessor["onStart"]>[1],
  ): void {
    for (const d of this._delegates) {
      try {
        d.onStart(span, parentContext);
      } catch (e) {
        diag.warn(
          `[sideseat] A span processor threw in onStart: ${describeError(e)}`,
        );
      }
    }
  }

  onEnd(span: Parameters<SpanProcessor["onEnd"]>[0]): void {
    for (const d of this._delegates) {
      try {
        d.onEnd(span);
      } catch (e) {
        diag.warn(
          `[sideseat] A span processor threw in onEnd: ${describeError(e)}`,
        );
      }
    }
  }

  // allSettled, not all: one exporter rejecting must not stop the others from
  // flushing, or pending spans in healthy exporters are lost on exit.
  //
  // But the failures are then *reported*, not discarded. Resolving regardless made
  // `forceFlush()` succeed while spans sat unexported, so the boolean the public API
  // returns said the data was flushed when it was not - the same lie as acknowledging a
  // write before it is durable.
  async forceFlush(): Promise<void> {
    const results = await Promise.allSettled(
      this._delegates.map((d) => settle(() => d.forceFlush())),
    );
    throwIfAnyRejected(results, "forceFlush");
  }

  // The delegate list is cleared whatever happened: shutdown is not retryable and holding
  // references to dead processors only means onEnd keeps calling them.
  async shutdown(): Promise<void> {
    const results = await Promise.allSettled(
      this._delegates.map((d) => settle(() => d.shutdown())),
    );
    this._delegates = [];
    throwIfAnyRejected(results, "shutdown");
  }
}

/**
 * Invoke a delegate so that *however* it fails, the failure is a rejected promise.
 *
 * `allSettled` only settles what it is given, and `map((d) => d.forceFlush())` calls each
 * delegate synchronously while building that array - so a processor whose `forceFlush` is a
 * plain function that throws propagated straight out of the `map`, before `allSettled` was
 * reached, and every delegate after it was never flushed. That is the exact failure the
 * `allSettled` is there to prevent, arriving by a route it cannot see. `Promise.resolve`
 * also covers a delegate that returns a non-promise.
 */
function settle(invoke: () => Promise<void>): Promise<void> {
  try {
    return Promise.resolve(invoke());
  } catch (e) {
    return Promise.reject(e);
  }
}

function describeError(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/**
 * Log every rejection and raise one error describing them all.
 *
 * Logging alone is not enough: the caller needs to know its flush did not complete, and a
 * log line is not something a program can act on. Raising alone is not enough either -
 * an AggregateError from N exporters is far less useful than N messages naming each one.
 */
function throwIfAnyRejected(
  results: PromiseSettledResult<unknown>[],
  operation: string,
): void {
  const failures = results.filter(
    (r): r is PromiseRejectedResult => r.status === "rejected",
  );
  if (failures.length === 0) return;

  for (const failure of failures) {
    diag.warn(
      `[sideseat] Span processor ${operation} failed: ${describeError(failure.reason)}`,
    );
  }
  throw new Error(
    `${failures.length} of ${results.length} span processors failed to ${operation}: ` +
      failures.map((f) => describeError(f.reason)).join("; "),
  );
}

export const DEFAULT_EXPORT_TIMEOUT_MS = 30_000;

/**
 * Batch export timeout in milliseconds, from OTEL_EXPORTER_OTLP_TIMEOUT.
 *
 * The value is read as **milliseconds**, which is how OpenTelemetry JS interprets it (its
 * own default is 10000). Note this differs from OpenTelemetry Python, which reads the same
 * variable as seconds — an upstream inconsistency, not a SideSeat one. Reading it as seconds
 * here would disagree with the exporter this value is paired with.
 *
 * Falls back to the default on a missing, non-numeric or non-positive value.
 */
export function resolveExportTimeoutMs(): number {
  const raw = process.env.OTEL_EXPORTER_OTLP_TIMEOUT;
  if (!raw) return DEFAULT_EXPORT_TIMEOUT_MS;
  const millis = Number(raw);
  if (!Number.isFinite(millis) || millis <= 0) {
    diag.warn(
      `[sideseat] Invalid OTEL_EXPORTER_OTLP_TIMEOUT '${raw}', using ${DEFAULT_EXPORT_TIMEOUT_MS}ms`,
    );
    return DEFAULT_EXPORT_TIMEOUT_MS;
  }
  return millis;
}

// Create OTEL resource with standard attributes
function createResource(config: Config): Resource {
  return resourceFromAttributes({
    [ATTR_SERVICE_NAME]: config.serviceName,
    [ATTR_SERVICE_VERSION]: config.serviceVersion,
    "telemetry.sdk.name": "sideseat",
    "telemetry.sdk.version": VERSION,
    "telemetry.sdk.language": "node",
  });
}

export class SideSeat {
  private _config: Config;
  private _provider: NodeTracerProvider | null = null;
  private _processors = new ForwardingSpanProcessor();
  private _fileExporterPaths: Set<string> = new Set();
  private _shutdownCalled = false;
  private _shutdownPromise: Promise<boolean> | null = null;
  private _cleanupHandlers: Array<() => void> = [];

  constructor(options: SideSeatOptions) {
    this._config = Config.create(options);
    this._setupDiagLogger();

    if (!this._config.disabled) {
      this._setupProvider();
      this._setupOtlp();
      this._registerCleanupHandlers();
    }
  }

  // Async factory pattern (industry best practice)
  static async create(options: SideSeatOptions): Promise<SideSeat> {
    const instance = new SideSeat(options);
    // Validate connection if not disabled
    if (!instance.isDisabled) {
      const connected = await instance.validateConnection(2000);
      if (!connected && instance._config.debug) {
        diag.warn(
          "[sideseat] Could not connect to endpoint - traces may not be exported",
        );
      }
    }
    return instance;
  }

  // State getters
  get isDisabled(): boolean {
    return this._config.disabled;
  }

  get isReady(): boolean {
    return !this._shutdownCalled && this._provider !== null;
  }

  get config(): Config {
    return this._config;
  }

  get tracerProvider(): NodeTracerProvider | null {
    return this._provider;
  }

  // Plugin interface - expose addSpanProcessor for custom exporters
  addSpanProcessor(processor: SpanProcessor): this {
    if (this._provider) {
      this._processors.add(processor);
    }
    return this;
  }

  toString(): string {
    return `SideSeat(endpoint=${this._config.endpoint}, project=${this._config.projectId})`;
  }

  getTracer(name = "sideseat", version?: string): Tracer {
    if (this._config.disabled || !this._provider) {
      return trace.getTracer(name); // Returns NoOp tracer
    }
    return this._provider.getTracer(name, version ?? VERSION);
  }

  // Callback-based span with proper error handling (async)
  async span<T>(name: string, fn: (span: Span) => T | Promise<T>): Promise<T> {
    const tracer = this.getTracer();
    return tracer.startActiveSpan(name, async (span) => {
      try {
        return await fn(span);
      } catch (error) {
        span.setStatus({ code: SpanStatusCode.ERROR, message: String(error) });
        span.recordException(error as Error);
        throw error;
      } finally {
        span.end();
      }
    });
  }

  // Sync version for non-async callbacks (avoids Promise overhead)
  spanSync<T>(name: string, fn: (span: Span) => T): T {
    const tracer = this.getTracer();
    return tracer.startActiveSpan(name, (span) => {
      try {
        return fn(span);
      } catch (error) {
        span.setStatus({ code: SpanStatusCode.ERROR, message: String(error) });
        span.recordException(error as Error);
        throw error;
      } finally {
        span.end();
      }
    });
  }

  async validateConnection(timeoutMs = 5000): Promise<boolean> {
    if (this._config.disabled) return false;
    try {
      const url = new URL(this._config.endpoint);
      const healthUrl = `${url.protocol}//${url.host}/api/v1/health`;
      const controller = new AbortController();
      const timeout = setTimeout(() => controller.abort(), timeoutMs);
      try {
        const response = await fetch(healthUrl, { signal: controller.signal });
        return response.ok;
      } finally {
        clearTimeout(timeout);
      }
    } catch {
      return false;
    }
  }

  async forceFlush(timeoutMs = 30000): Promise<boolean> {
    if (this._config.disabled || !this._provider) return true;

    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      await Promise.race([
        this._provider.forceFlush(),
        new Promise<never>((_, reject) => {
          timer = setTimeout(() => reject(new Error("timeout")), timeoutMs);
        }),
      ]);
      return true;
    } catch (e) {
      // Named, because `false` on its own tells an operator nothing about which exporter
      // failed or whether it was simply slow - and this is the signal that spans were lost.
      diag.warn(`[sideseat] forceFlush did not complete: ${describeError(e)}`);
      return false;
    } finally {
      if (timer) clearTimeout(timer);
    }
  }

  /**
   * Flush and release. Resolves to whether the flush and shutdown **completed**.
   *
   * That is narrower than "no span was lost, ever", and the difference is worth stating: the batch
   * processor has a bounded queue (`maxQueueSize`), and a burst that overruns it is discarded by the OTel
   * SDK at the moment it happens, with no counter this can read. So `true` means everything still queued
   * was exported successfully - it cannot mean nothing was ever dropped.
   *
   * The boolean is the point, and it mirrors {@link forceFlush}: this used to resolve `void`
   * whatever happened, so a caller draining before exit received a fulfilled promise while its
   * spans were being discarded. The diagnostic warnings alone are not a substitute - `diag` is a
   * no-op until the host installs a logger, so by default the loss was reported nowhere at all.
   * It still does not throw: this also runs from a SIGTERM handler, where an unhandled rejection
   * would replace the exit code with a crash.
   */
  async shutdown(timeoutMs = 30000): Promise<boolean> {
    // Return existing promise if shutdown already in progress (concurrent protection)
    if (this._shutdownPromise !== null) {
      return this._shutdownPromise;
    }
    if (this._shutdownCalled) return true;
    this._shutdownCalled = true;

    this._shutdownPromise = this._doShutdown(timeoutMs);
    return this._shutdownPromise;
  }

  // Console exporter (SimpleSpanProcessor - immediate output)
  setupConsoleExporter(): this {
    if (this._config.disabled || !this._provider) return this;
    this._processors.add(new SimpleSpanProcessor(new ConsoleSpanExporter()));
    return this;
  }

  // File exporter (BatchSpanProcessor - batched)
  setupFileExporter(path = "traces.jsonl"): this {
    if (this._config.disabled || !this._provider) return this;

    // Prevent duplicate file handles to same path
    const resolved = nodePath.resolve(path);
    if (this._fileExporterPaths.has(resolved)) {
      diag.warn(`[sideseat] File exporter already exists for path: ${path}`);
      return this;
    }

    // Validate directory exists and is writable
    const dir = nodePath.dirname(resolved);
    try {
      fs.accessSync(dir, fs.constants.W_OK);
    } catch {
      throw new SideSeatError(
        `Cannot write to directory: ${dir}. Ensure it exists and is writable.`,
      );
    }

    const exporter = new JsonFileSpanExporter(path);
    this._processors.add(new BatchSpanProcessor(exporter));
    this._fileExporterPaths.add(resolved);
    return this;
  }

  private _setupDiagLogger(): void {
    const level = this._config.logLevel;
    if (level === "none") return;

    // Only set if not already configured (avoid conflict with other SDKs)
    try {
      const levelMap: Record<LogLevel, DiagLogLevel> = {
        none: DiagLogLevel.NONE,
        error: DiagLogLevel.ERROR,
        warn: DiagLogLevel.WARN,
        info: DiagLogLevel.INFO,
        debug: DiagLogLevel.DEBUG,
        verbose: DiagLogLevel.VERBOSE,
      };
      diag.setLogger(new DiagConsoleLogger(), levelMap[level]);
    } catch {
      // Logger already set by another SDK - ignore
    }
  }

  private _setupProvider(): void {
    const resource = createResource(this._config);

    // Whether another library already owns the global TracerProvider. The OTel API
    // registers a ProxyTracerProvider and refuses a second registration, so we cannot
    // detect this by type - but an unclaimed global hands out non-recording spans with
    // an all-zero trace id, while a claimed one hands out real spans.
    const probe = trace.getTracer("sideseat-detect").startSpan("probe");
    const globalAlreadyOwned =
      probe.spanContext().traceId !== "00000000000000000000000000000000";
    probe.end();

    // Always own our provider: adopting a foreign one is pointless in OTel 2.x, where
    // processors can only be supplied at construction. The forwarding processor is what
    // keeps addSpanProcessor() working after construction.
    this._provider = new NodeTracerProvider({
      resource,
      spanProcessors: [this._processors],
    });

    // register() handles:
    // 1. Setting global tracer provider (trace.setGlobalTracerProvider)
    // 2. Setting up propagators
    // 3. Setting up context manager
    this._provider.register({
      propagator: new CompositePropagator({
        propagators: [
          new W3CBaggagePropagator(),
          new W3CTraceContextPropagator(),
        ],
      }),
    });

    if (globalAlreadyOwned) {
      // Our own spans (client.span/spanSync/getTracer) still export, because they go
      // through this provider directly. What is lost is the global registration, which
      // the OTel API will not hand over: spans created via the global tracer by other
      // instrumentation keep going to whoever registered first.
      diag.warn(
        "[sideseat] Another library already registered the global TracerProvider. " +
          "SideSeat's own spans are exported, but spans from instrumentation using the " +
          "global tracer are not. Initialize SideSeat before that library to capture them.",
      );
    } else {
      diag.info("[sideseat] TracerProvider registered");
    }
  }

  private _setupOtlp(): void {
    if (!this._config.enableTraces || !this._provider) return;

    const url = this._buildEndpoint("traces");
    const headers: Record<string, string> = {
      "User-Agent": `sideseat-sdk-node/${VERSION}`,
    };
    if (this._config.apiKey) {
      headers["Authorization"] = `Bearer ${this._config.apiKey}`;
    }

    // One resolved value for both: left to their own devices the exporter defaults to
    // 10s while the processor was hardcoded to 30s, so whichever was lower silently won.
    const timeoutMillis = resolveExportTimeoutMs();
    const exporter = new OTLPTraceExporter({ url, headers, timeoutMillis });
    const processor = new BatchSpanProcessor(exporter, {
      maxQueueSize: 2048,
      scheduledDelayMillis: 5000,
      maxExportBatchSize: 512,
      exportTimeoutMillis: timeoutMillis,
    });
    this._processors.add(processor);

    if (this._config.debug) {
      diag.debug(`[sideseat] Initialized - sending traces to ${url}`);
    }
  }

  private _buildEndpoint(signal: "traces" | "metrics" | "logs"): string {
    const url = new URL(this._config.endpoint);
    // If endpoint has a path (e.g., /otel/custom), append /v1/{signal}
    if (url.pathname && url.pathname !== "/") {
      return `${this._config.endpoint}/v1/${signal}`;
    }
    // No path - use SideSeat format: /otel/{project}/v1/{signal}
    return `${this._config.endpoint}/otel/${this._config.projectId}/v1/${signal}`;
  }

  private _registerCleanupHandlers(): void {
    // The boolean is deliberately dropped here: a signal handler has nobody to report to, and the
    // warning has already been logged.
    const cleanup = () => void this.shutdown();
    process.once("SIGTERM", cleanup);
    process.once("SIGINT", cleanup);
    process.once("beforeExit", cleanup);
    this._cleanupHandlers = [
      () => process.off("SIGTERM", cleanup),
      () => process.off("SIGINT", cleanup),
      () => process.off("beforeExit", cleanup),
    ];
  }

  private async _doShutdown(timeoutMs: number): Promise<boolean> {
    diag.info("[sideseat] Shutting down...");

    // Remove process listeners (prevent memory leaks)
    for (const handler of this._cleanupHandlers) handler();
    this._cleanupHandlers = [];

    // Flush and shutdown provider (handles all processors + exporters).
    //
    // `forceFlush` reports its own failure and returns false rather than throwing, and the
    // provider's shutdown is caught here: this runs from a SIGTERM/beforeExit handler, where
    // an unhandled rejection would replace the exit code with a crash and tell the operator
    // nothing about the spans that did not make it.
    const flushed = await this.forceFlush(timeoutMs);
    try {
      await this._provider?.shutdown();
    } catch (e) {
      diag.warn(`[sideseat] Provider shutdown failed: ${describeError(e)}`);
      return false;
    }
    if (flushed) {
      diag.info("[sideseat] Shutdown complete");
    } else {
      diag.warn(
        "[sideseat] Shutdown complete, but some spans were not exported",
      );
    }
    return flushed;
  }
}
