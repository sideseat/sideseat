/**
 * The two ways a suite configures telemetry, behind one interface.
 *
 * `native` is what a user of the framework writes by following the framework's documentation:
 * plain OpenTelemetry plus whatever the framework needs switched on, and nothing from SideSeat.
 * `sdk` is the same program with `sideseat.init`. Running a scenario in both modes and comparing the
 * captured telemetry is how the SDK is shown to add nothing wrong and lose nothing.
 */
import {
  ROOT_CONTEXT,
  SpanStatusCode,
  trace,
  type Span,
  type TracerProvider,
} from '@opentelemetry/api';

export interface Correlation {
  sessionId: string;
  userId: string;
}

export interface Telemetry {
  readonly mode: 'native' | 'sdk';
  trace<T>(name: string, correlation: Correlation, fn: (span: Span) => Promise<T>): Promise<T>;
  shutdown(): Promise<void>;
}

/** Where both modes export: the SideSeat project endpoint, or the capture recorder. */
export function otlpBase(): string {
  const endpoint = (process.env.SIDESEAT_ENDPOINT || 'http://127.0.0.1:5388').replace(/\/+$/, '');
  const path = new URL(endpoint).pathname;
  return path && path !== '/'
    ? endpoint
    : `${endpoint}/otel/${process.env.SIDESEAT_PROJECT_ID || 'default'}`;
}

export function tracesEndpoint(): string {
  return `${otlpBase()}/v1/traces`;
}

export function authHeaders(): Record<string, string> {
  const key = process.env.SIDESEAT_API_KEY;
  return key ? { Authorization: `Bearer ${key}` } : {};
}

interface Flushable extends TracerProvider {
  forceFlush(): Promise<void>;
  shutdown(): Promise<void>;
}

export class SdkTelemetry implements Telemetry {
  readonly mode = 'sdk';

  static async start(integrations: string[]): Promise<SdkTelemetry> {
    const sideseat = await import('@sideseat/sdk');
    await sideseat.init({ integrations });
    return new SdkTelemetry();
  }

  async trace<T>(name: string, correlation: Correlation, fn: (span: Span) => Promise<T>) {
    const sideseat = await import('@sideseat/sdk');
    return sideseat.trace(name, correlation, fn);
  }

  async shutdown(): Promise<void> {
    const sideseat = await import('@sideseat/sdk');
    if (!(await sideseat.shutdown())) throw new Error('SideSeat could not export every span');
  }
}

/**
 * Plain OpenTelemetry, configured the way each framework's documentation says to.
 *
 * A suite's `native.ts` receives this object and either calls {@link provider}, or builds the
 * provider its framework documents from {@link tracesEndpoint} and hands it to {@link adopt}.
 */
export class NativeTelemetry implements Telemetry {
  readonly mode = 'native';
  readonly serviceName: string;
  private installed: Flushable | undefined;

  constructor(serviceName: string) {
    this.serviceName = serviceName;
  }

  /** A provider exporting over OTLP, registered as the global provider. */
  async provider(): Promise<Flushable> {
    if (this.installed) return this.installed;
    const [{ NodeTracerProvider }, { BatchSpanProcessor }, { OTLPTraceExporter }, resources] =
      await Promise.all([
        import('@opentelemetry/sdk-trace-node'),
        import('@opentelemetry/sdk-trace-base'),
        import('@opentelemetry/exporter-trace-otlp-http'),
        import('@opentelemetry/resources'),
      ]);
    const provider = new NodeTracerProvider({
      resource: resources.resourceFromAttributes({ 'service.name': this.serviceName }),
      spanProcessors: [
        new BatchSpanProcessor(
          new OTLPTraceExporter({ url: tracesEndpoint(), headers: authHeaders() })
        ),
      ],
    });
    provider.register();
    this.installed = provider;
    return provider;
  }

  /** Use a provider the framework's own setup created. */
  adopt(provider: Flushable): void {
    this.installed = provider;
  }

  // Native OpenTelemetry has no session scope: the root span carries the identifiers, which is
  // what the framework documentation tells users to do.
  trace<T>(name: string, correlation: Correlation, fn: (span: Span) => Promise<T>): Promise<T> {
    const tracer = trace.getTracer('example');
    const attributes = { 'session.id': correlation.sessionId, 'user.id': correlation.userId };
    return tracer.startActiveSpan(name, { attributes }, ROOT_CONTEXT, async (span) => {
      try {
        return await fn(span);
      } catch (error) {
        span.recordException(error as Error);
        span.setStatus({ code: SpanStatusCode.ERROR, message: String(error) });
        throw error;
      } finally {
        span.end();
      }
    });
  }

  async shutdown(): Promise<void> {
    if (!this.installed) return;
    await this.installed.forceFlush();
    await this.installed.shutdown();
  }
}
