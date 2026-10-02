import {
  context,
  diag,
  DiagConsoleLogger,
  DiagLogLevel,
  SpanKind,
  SpanStatusCode,
  trace as otelTrace,
  type Attributes,
  type Context,
  type Span,
  type Tracer,
} from "@opentelemetry/api";
import {
  resourceFromAttributes,
  type Resource,
} from "@opentelemetry/resources";
import {
  BatchSpanProcessor,
  type SpanProcessor,
} from "@opentelemetry/sdk-trace-base";
import { NodeTracerProvider } from "@opentelemetry/sdk-trace-node";
import {
  exportHeaders,
  otlpBase,
  signalEndpoint,
  type Settings,
} from "./config.js";
import {
  CorrelationSpanProcessor,
  withCorrelation,
  type Correlation,
} from "./correlation.js";
import { IntegrationError } from "./errors.js";
import { resolveIntegrations } from "./integrations/index.js";
import { firstInstalled } from "./integrations/packages.js";
import type { Integration, SetupContext } from "./integrations/types.js";
import { VERSION } from "./version.js";

const DEFAULT_TIMEOUT_MS = 30_000;
const GENAI_CAPTURE_CONTENT =
  "OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT";

/** Options for spans started by {@link SideSeat.trace} and {@link SideSeat.span}. */
export interface SpanOptions {
  kind?: SpanKind;
  attributes?: Attributes;
}

/** Options for {@link SideSeat.trace}: a span plus the session and user it belongs to. */
export interface TraceOptions extends SpanOptions, Correlation {}

type SpanCallback<T> = (span: Span) => T | Promise<T>;

/** A configured telemetry pipeline. Create it with `init`. */
export class SideSeat {
  readonly settings: Settings;
  private readonly active: Integration[] = [];
  private provider: NodeTracerProvider | undefined;
  private loggerProvider:
    { forceFlush(): Promise<void>; shutdown(): Promise<void> } | undefined;
  private tracer: Tracer = otelTrace.getTracer("sideseat", VERSION);
  private shutdownPromise: Promise<boolean> | undefined;
  private readonly detachHandlers: Array<() => void> = [];

  private constructor(settings: Settings) {
    this.settings = settings;
  }

  /** Builds the pipeline. Prefer `init`, which also makes the client process-wide. */
  static async start(settings: Settings): Promise<SideSeat> {
    const client = new SideSeat(settings);
    if (settings.debug)
      diag.setLogger(new DiagConsoleLogger(), DiagLogLevel.DEBUG);
    if (!settings.disabled) await client.build();
    return client;
  }

  /** Names of the integrations that were installed, primary first. */
  get integrations(): string[] {
    return this.active.map((integration) => integration.name);
  }

  get tracerProvider(): NodeTracerProvider | undefined {
    return this.provider;
  }

  getTracer(name: string, version?: string): Tracer {
    return this.provider
      ? this.provider.getTracer(name, version)
      : otelTrace.getTracer(name, version);
  }

  /** Attributes every span started inside `fn` to a session and, optionally, a user. */
  session<T>(correlation: Correlation, fn: () => T): T {
    return context.with(withCorrelation(context.active(), correlation), fn);
  }

  /** Runs `fn` in a new root span, even when another span is active. */
  trace<T>(name: string, fn: SpanCallback<T>): Promise<T>;
  trace<T>(
    name: string,
    options: TraceOptions,
    fn: SpanCallback<T>,
  ): Promise<T>;
  trace<T>(
    name: string,
    a: TraceOptions | SpanCallback<T>,
    b?: SpanCallback<T>,
  ): Promise<T> {
    const [options, fn] =
      typeof a === "function" ? [{}, a] : [a, b as SpanCallback<T>];
    const root = otelTrace.deleteSpan(context.active());
    return this.run(name, withCorrelation(root, options), options, fn);
  }

  /** Runs `fn` in a child of the active span. */
  span<T>(name: string, fn: SpanCallback<T>): Promise<T>;
  span<T>(name: string, options: SpanOptions, fn: SpanCallback<T>): Promise<T>;
  span<T>(
    name: string,
    a: SpanOptions | SpanCallback<T>,
    b?: SpanCallback<T>,
  ): Promise<T> {
    const [options, fn] =
      typeof a === "function" ? [{}, a] : [a, b as SpanCallback<T>];
    return this.run(name, context.active(), options, fn);
  }

  /** Exports everything pending. Resolves to whether all of it was exported. */
  async flush(timeoutMs = DEFAULT_TIMEOUT_MS): Promise<boolean> {
    if (!this.provider) return true;
    const pending = [
      this.provider.forceFlush(),
      this.loggerProvider?.forceFlush(),
    ];
    return withTimeout(Promise.all(pending), timeoutMs, "flush");
  }

  /** Flushes and stops the pipeline. Idempotent; runs automatically when the process exits. */
  shutdown(timeoutMs = DEFAULT_TIMEOUT_MS): Promise<boolean> {
    this.shutdownPromise ??= this.stop(timeoutMs);
    return this.shutdownPromise;
  }

  private async run<T>(
    name: string,
    ctx: Context,
    options: SpanOptions,
    fn: SpanCallback<T>,
  ): Promise<T> {
    return this.tracer.startActiveSpan(
      name,
      {
        kind: options.kind ?? SpanKind.INTERNAL,
        attributes: options.attributes,
      },
      ctx,
      async (span) => {
        try {
          return await fn(span);
        } catch (error) {
          span.recordException(error as Error);
          span.setStatus({
            code: SpanStatusCode.ERROR,
            message: String(error),
          });
          throw error;
        } finally {
          span.end();
        }
      },
    );
  }

  private async build(): Promise<void> {
    const settings = this.settings;
    const [candidates, explicit] = resolveIntegrations(settings.integrations);
    const primary = candidates[0]
      ? firstInstalled(candidates[0].packages)
      : undefined;
    const serviceName = settings.serviceName ?? primary?.[0] ?? "sideseat-app";
    const serviceVersion = settings.serviceVersion ?? primary?.[1] ?? VERSION;
    const ctx: SetupContext = {
      settings,
      resource: buildResource(
        settings,
        candidates,
        serviceName,
        serviceVersion,
      ),
      serviceName,
      serviceVersion,
    };
    if (settings.captureContent && !process.env[GENAI_CAPTURE_CONTENT]) {
      process.env[GENAI_CAPTURE_CONTENT] = "true";
    }

    for (const integration of candidates) {
      if (
        await guard(integration, explicit, "prepare", () =>
          integration.prepare?.(ctx),
        )
      ) {
        this.active.push(integration);
      }
    }

    const processors: SpanProcessor[] = [new CorrelationSpanProcessor()];
    for (const integration of this.active)
      processors.push(...(integration.spanProcessors?.(ctx) ?? []));
    processors.push(...settings.spanProcessors);
    if (settings.export) {
      const { OTLPTraceExporter } =
        await import("@opentelemetry/exporter-trace-otlp-http");
      processors.push(
        new BatchSpanProcessor(
          new OTLPTraceExporter({
            url: signalEndpoint(settings, "traces"),
            headers: exportHeaders(settings),
          }),
        ),
      );
    }
    this.provider = new NodeTracerProvider({
      resource: ctx.resource,
      spanProcessors: processors,
    });
    // The OpenTelemetry API keeps the first global provider and ignores later registrations. An
    // unclaimed global hands out non-recording spans with an all-zero trace id.
    const probe = otelTrace.getTracer("sideseat-probe").startSpan("probe");
    const alreadyClaimed =
      probe.spanContext().traceId !== "00000000000000000000000000000000";
    probe.end();
    this.provider.register();
    if (alreadyClaimed) {
      diag.warn(
        "[sideseat] another library registered the global tracer provider first; spans from " +
          "instrumentation that uses the global tracer will not reach SideSeat. Call init earlier.",
      );
    }
    this.tracer = this.provider.getTracer("sideseat", VERSION);
    ctx.tracerProvider = this.provider;

    if (settings.logs && settings.export) await this.startLogs(ctx.resource);

    for (const integration of [...this.active]) {
      if (
        !(await guard(integration, explicit, "instrument", () =>
          integration.instrument?.(ctx),
        ))
      ) {
        this.active.splice(this.active.indexOf(integration), 1);
      }
    }
    this.attachExitHandlers();
    diag.debug(
      `[sideseat] exporting to ${otlpBase(settings)} with integrations ${this.integrations.join(", ")}`,
    );
  }

  private async startLogs(resource: Resource): Promise<void> {
    const [
      { logs },
      { LoggerProvider, BatchLogRecordProcessor },
      { OTLPLogExporter },
    ] = await Promise.all([
      import("@opentelemetry/api-logs"),
      import("@opentelemetry/sdk-logs"),
      import("@opentelemetry/exporter-logs-otlp-http"),
    ]);
    const exporter = new OTLPLogExporter({
      url: signalEndpoint(this.settings, "logs"),
      headers: exportHeaders(this.settings),
    });
    const provider = new LoggerProvider({
      resource,
      processors: [new BatchLogRecordProcessor({ exporter })],
    });
    logs.setGlobalLoggerProvider(provider);
    this.loggerProvider = provider;
  }

  private attachExitHandlers(): void {
    const onBeforeExit = () => void this.shutdown();
    process.once("beforeExit", onBeforeExit);
    this.detachHandlers.push(() => process.off("beforeExit", onBeforeExit));
    for (const signal of ["SIGINT", "SIGTERM"] as const) {
      // A listener replaces Node's default of exiting, so after flushing the signal is raised again
      // with no listener left, and the process ends the way it would have without SideSeat.
      const onSignal = () => {
        void this.shutdown(5_000).finally(() =>
          process.kill(process.pid, signal),
        );
      };
      process.once(signal, onSignal);
      this.detachHandlers.push(() => process.off(signal, onSignal));
    }
  }

  private async stop(timeoutMs: number): Promise<boolean> {
    for (const detach of this.detachHandlers.splice(0)) detach();
    let ok = await this.flush(timeoutMs);
    for (const integration of [...this.active].reverse()) {
      try {
        await integration.shutdown?.();
      } catch (error) {
        ok = false;
        diag.warn(
          `[sideseat] integration ${integration.name} failed to shut down: ${String(error)}`,
        );
      }
    }
    try {
      await Promise.all([
        this.provider?.shutdown(),
        this.loggerProvider?.shutdown(),
      ]);
    } catch (error) {
      ok = false;
      diag.warn(
        `[sideseat] a telemetry provider failed to shut down: ${String(error)}`,
      );
    }
    return ok;
  }
}

/** Runs one integration step. A requested integration must work; a detected one may be skipped. */
async function guard(
  integration: Integration,
  explicit: boolean,
  stage: string,
  step: () => void | Promise<void>,
): Promise<boolean> {
  try {
    await step();
    return true;
  } catch (error) {
    const message =
      `integration ${JSON.stringify(integration.name)} cannot ${stage}: ${String(error)}. ` +
      `Install it with: npm install ${integration.packages[0]}`;
    if (explicit) throw new IntegrationError(message);
    diag.warn(`[sideseat] skipping detected ${message}`);
    return false;
  }
}

function buildResource(
  settings: Settings,
  integrations: Integration[],
  serviceName: string,
  serviceVersion: string,
): Resource {
  const attributes: Attributes = {
    "service.name": serviceName,
    "service.version": serviceVersion,
    "telemetry.sdk.name": "sideseat",
    "telemetry.sdk.language": "nodejs",
    "telemetry.sdk.version": VERSION,
  };
  if (integrations.length > 0) {
    // The current GenAI conventions are framework-neutral, so a producer that follows them says
    // nothing about who it is; the server reads this declaration when per-span evidence is absent.
    attributes["sideseat.framework"] = integrations[0]!.name;
    attributes["sideseat.integrations"] = integrations.map(
      (integration) => integration.name,
    );
  }
  return resourceFromAttributes({
    ...attributes,
    ...settings.resourceAttributes,
  });
}

async function withTimeout(
  work: Promise<unknown>,
  timeoutMs: number,
  operation: string,
): Promise<boolean> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    await Promise.race([
      work,
      new Promise<never>((_, reject) => {
        timer = setTimeout(
          () => reject(new Error(`timed out after ${timeoutMs} ms`)),
          timeoutMs,
        );
      }),
    ]);
    return true;
  } catch (error) {
    diag.warn(`[sideseat] ${operation} did not complete: ${String(error)}`);
    return false;
  } finally {
    if (timer) clearTimeout(timer);
  }
}
