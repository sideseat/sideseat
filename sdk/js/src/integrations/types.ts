import type { Resource } from "@opentelemetry/resources";
import type { NodeTracerProvider } from "@opentelemetry/sdk-trace-node";
import type { SpanProcessor } from "@opentelemetry/sdk-trace-base";
import type { Settings } from "../config.js";

/** What an integration can see while it is installed. Providers exist from `instrument` onwards. */
export interface SetupContext {
  readonly settings: Settings;
  readonly resource: Resource;
  readonly serviceName: string;
  readonly serviceVersion: string;
  tracerProvider?: NodeTracerProvider;
}

/**
 * Turns on the telemetry of one framework or provider.
 *
 * Hooks run in this order: `prepare` for every integration, provider creation, `spanProcessors`,
 * then `instrument`. `shutdown` runs at SDK shutdown in reverse order.
 */
export interface Integration {
  /** Stable identifier, recorded as `sideseat.framework` when this is the primary integration. */
  readonly name: string;
  /** npm packages that identify the integration, most specific first. */
  readonly packages: readonly string[];
  /** Whether auto-detection may activate it. Provider clients are installed transitively, so no. */
  readonly detectable: boolean;
  prepare?(ctx: SetupContext): void | Promise<void>;
  spanProcessors?(ctx: SetupContext): SpanProcessor[];
  instrument?(ctx: SetupContext): void | Promise<void>;
  shutdown?(): void | Promise<void>;
}
