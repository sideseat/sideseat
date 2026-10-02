/**
 * SideSeat: OpenTelemetry for AI agents, configured in one call.
 *
 * ```ts
 * import * as sideseat from "@sideseat/sdk";
 *
 * await sideseat.init({ integrations: ["vercel-ai"] });
 * await sideseat.session({ sessionId: "conversation-42", userId: "user-7" }, () => agent.run());
 * ```
 */
import { identity, resolveSettings, type SideSeatOptions } from "./config.js";
import { SideSeat, type SpanOptions, type TraceOptions } from "./client.js";
import type { Correlation } from "./correlation.js";
import { ConfigurationError, SideSeatError } from "./errors.js";
import type { Span } from "@opentelemetry/api";

let client: SideSeat | undefined;
let starting: Promise<SideSeat> | undefined;
let startingIdentity: string | undefined;

/**
 * Configures telemetry for this process and resolves to the client. Calling it again with the same
 * options returns the same client; different options reject with {@link ConfigurationError}.
 */
export async function init(options: SideSeatOptions = {}): Promise<SideSeat> {
  const settings = resolveSettings(options);
  const key = identity(settings);
  const existing = client ? identity(client.settings) : startingIdentity;
  if (existing !== undefined && existing !== key) {
    throw new ConfigurationError(
      "init was already called with different options; call shutdown() first",
    );
  }
  if (client) return client;
  if (!starting) {
    startingIdentity = key;
    starting = SideSeat.start(settings).then(
      (started) => {
        client = started;
        return started;
      },
      (error: unknown) => {
        starting = undefined;
        startingIdentity = undefined;
        throw error;
      },
    );
  }
  return starting;
}

/** The client {@link init} created. */
export function getClient(): SideSeat {
  if (!client) throw new SideSeatError("call and await init() first");
  return client;
}

/** Exports everything pending. Resolves to whether all of it was exported. */
export function flush(timeoutMs?: number): Promise<boolean> {
  return getClient().flush(timeoutMs);
}

/** Flushes and stops the pipeline. Safe to call more than once. */
export async function shutdown(timeoutMs?: number): Promise<boolean> {
  const current =
    client ?? (starting ? await starting.catch(() => undefined) : undefined);
  client = undefined;
  starting = undefined;
  startingIdentity = undefined;
  return current ? current.shutdown(timeoutMs) : true;
}

/** Attributes every span started inside `fn` to a session and, optionally, a user. */
export function session<T>(correlation: Correlation, fn: () => T): T {
  return getClient().session(correlation, fn);
}

/** Runs `fn` in a new root span, even inside another span. */
export function trace<T>(
  name: string,
  fn: (span: Span) => T | Promise<T>,
): Promise<T>;
export function trace<T>(
  name: string,
  options: TraceOptions,
  fn: (span: Span) => T | Promise<T>,
): Promise<T>;
export function trace<T>(
  name: string,
  a: TraceOptions | ((span: Span) => T | Promise<T>),
  b?: (span: Span) => T | Promise<T>,
): Promise<T> {
  return typeof a === "function"
    ? getClient().trace(name, a)
    : getClient().trace(name, a, b!);
}

/** Runs `fn` in a child of the active span. */
export function span<T>(
  name: string,
  fn: (span: Span) => T | Promise<T>,
): Promise<T>;
export function span<T>(
  name: string,
  options: SpanOptions,
  fn: (span: Span) => T | Promise<T>,
): Promise<T>;
export function span<T>(
  name: string,
  a: SpanOptions | ((span: Span) => T | Promise<T>),
  b?: (span: Span) => T | Promise<T>,
): Promise<T> {
  return typeof a === "function"
    ? getClient().span(name, a)
    : getClient().span(name, a, b!);
}

export { SideSeat } from "./client.js";
export type { SpanOptions, TraceOptions } from "./client.js";
export { DEFAULT_ENDPOINT, DEFAULT_PROJECT } from "./config.js";
export type { SideSeatOptions, Settings } from "./config.js";
export type { Correlation } from "./correlation.js";
export {
  ConfigurationError,
  IntegrationError,
  SideSeatError,
} from "./errors.js";
export { JsonlSpanExporter } from "./exporters.js";
export {
  claudeAgentSDK,
  cliEnvironment,
  integrationNames,
  strands,
  vercelAI,
} from "./integrations/index.js";
export type { Integration, SetupContext } from "./integrations/index.js";
export { VERSION } from "./version.js";
