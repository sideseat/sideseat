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
import { Resource } from "@opentelemetry/resources";
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

// Create OTEL resource with standard attributes
function createResource(config: Config): Resource {
  return new Resource({
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
  private _fileExporterPaths: Set<string> = new Set();
  private _shutdownCalled = false;
  private _shutdownPromise: Promise<void> | null = null;
  private _cleanupHandlers: Array<() => void> = [];

  constructor(options?: SideSeatOptions) {
    this._config = Config.create(options);
    this._setupDiagLogger();

    if (!this._config.disabled) {
      this._setupProvider();
      this._setupOtlp();
      this._registerCleanupHandlers();
    }
  }

  // Async factory pattern (industry best practice)
  static async create(options?: SideSeatOptions): Promise<SideSeat> {
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
      this._provider.addSpanProcessor(processor);
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
    } catch {
      return false;
    } finally {
      if (timer) clearTimeout(timer);
    }
  }

  async shutdown(timeoutMs = 30000): Promise<void> {
    // Return existing promise if shutdown already in progress (concurrent protection)
    if (this._shutdownPromise !== null) {
      return this._shutdownPromise;
    }
    if (this._shutdownCalled) return;
    this._shutdownCalled = true;

    this._shutdownPromise = this._doShutdown(timeoutMs);
    return this._shutdownPromise;
  }

  // Console exporter (SimpleSpanProcessor - immediate output)
  setupConsoleExporter(): this {
    if (this._config.disabled || !this._provider) return this;
    this._provider.addSpanProcessor(
      new SimpleSpanProcessor(new ConsoleSpanExporter()),
    );
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
    this._provider.addSpanProcessor(new BatchSpanProcessor(exporter));
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

    // Check for existing SDK TracerProvider (e.g., from another SDK)
    const existing = trace.getTracerProvider();
    if (
      existing &&
      typeof (existing as NodeTracerProvider).addSpanProcessor === "function"
    ) {
      diag.warn(
        "[sideseat] TracerProvider already exists; adding processors to existing",
      );
      this._provider = existing as NodeTracerProvider;
      return;
    }

    this._provider = new NodeTracerProvider({ resource });

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

    diag.info("[sideseat] TracerProvider registered");
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

    const exporter = new OTLPTraceExporter({ url, headers });
    const processor = new BatchSpanProcessor(exporter, {
      maxQueueSize: 2048,
      scheduledDelayMillis: 5000,
      maxExportBatchSize: 512,
      exportTimeoutMillis: 30000,
    });
    this._provider.addSpanProcessor(processor);

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

  private async _doShutdown(timeoutMs: number): Promise<void> {
    diag.info("[sideseat] Shutting down...");

    // Remove process listeners (prevent memory leaks)
    for (const handler of this._cleanupHandlers) handler();
    this._cleanupHandlers = [];

    // Flush and shutdown provider (handles all processors + exporters)
    await this.forceFlush(timeoutMs);
    await this._provider?.shutdown();
    diag.info("[sideseat] Shutdown complete");
  }
}
