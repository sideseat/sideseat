import type { SpanProcessor } from "@opentelemetry/sdk-trace-base";
import { ConfigurationError } from "./errors.js";
import type { Integration } from "./integrations/types.js";

export const DEFAULT_ENDPOINT = "http://127.0.0.1:5388";
export const DEFAULT_PROJECT = "default";

/** Options for {@link init}. Every option falls back to its environment variable, then a default. */
export interface SideSeatOptions {
  /** SideSeat server URL, or an OTLP base URL that already has a path. `SIDESEAT_ENDPOINT`. */
  endpoint?: string;
  /** Project that receives the telemetry. `SIDESEAT_PROJECT_ID`. */
  project?: string;
  /** Sent as a bearer token. `SIDESEAT_API_KEY`. */
  apiKey?: string;
  /** `service.name`; defaults to the primary integration's package name. `OTEL_SERVICE_NAME`. */
  serviceName?: string;
  /** `service.version`. `OTEL_SERVICE_VERSION`. */
  serviceVersion?: string;
  /** Integration names or instances; the first is the primary one. `SIDESEAT_INTEGRATIONS`. */
  integrations?: ReadonlyArray<string | Integration>;
  /** Record prompts, responses, and tool payloads. On by default. `SIDESEAT_CAPTURE_CONTENT`. */
  captureContent?: boolean;
  /** Configure nothing; every call becomes a no-op. `SIDESEAT_DISABLED`. */
  disabled?: boolean;
  /** Log the SDK's decisions through OpenTelemetry's diagnostic logger. `SIDESEAT_DEBUG`. */
  debug?: boolean;
  /** Send telemetry over OTLP. Off is useful with `spanProcessors` in tests. */
  export?: boolean;
  /** Export OpenTelemetry metrics. */
  metrics?: boolean;
  /** Export OpenTelemetry log records, which some instrumentations use for GenAI events. */
  logs?: boolean;
  /** Extra resource attributes for every signal, over `OTEL_RESOURCE_ATTRIBUTES`. */
  resourceAttributes?: Readonly<Record<string, string | number | boolean>>;
  /** Extra processors, run after correlation and before export. */
  spanProcessors?: ReadonlyArray<SpanProcessor>;
}

/** Resolved, immutable settings. */
export interface Settings {
  readonly endpoint: string;
  readonly project: string;
  readonly apiKey: string | undefined;
  readonly serviceName: string | undefined;
  readonly serviceVersion: string | undefined;
  readonly integrations: ReadonlyArray<string | Integration> | undefined;
  readonly captureContent: boolean;
  /**
   * Whether `captureContent` overrides the instrumentations' own content switch: when it was an
   * option, or when it is off. `SIDESEAT_CAPTURE_CONTENT=true` alone does not turn on content an
   * application switched off in another variable.
   */
  readonly captureContentOverrides: boolean;
  readonly disabled: boolean;
  readonly debug: boolean;
  readonly export: boolean;
  readonly metrics: boolean;
  readonly logs: boolean;
  readonly resourceAttributes: Readonly<
    Record<string, string | number | boolean>
  >;
  readonly spanProcessors: ReadonlyArray<SpanProcessor>;
}

export function resolveSettings(options: SideSeatOptions = {}): Settings {
  const envIntegrations = text(undefined, "SIDESEAT_INTEGRATIONS");
  const content = flag(
    options.captureContent,
    "SIDESEAT_CAPTURE_CONTENT",
    true,
  );
  return Object.freeze({
    endpoint: normalizeEndpoint(
      text(options.endpoint, "SIDESEAT_ENDPOINT") ??
        text(undefined, "OTEL_EXPORTER_OTLP_ENDPOINT"),
    ),
    project: text(options.project, "SIDESEAT_PROJECT_ID") ?? DEFAULT_PROJECT,
    apiKey: text(options.apiKey, "SIDESEAT_API_KEY"),
    serviceName: text(options.serviceName, "OTEL_SERVICE_NAME"),
    serviceVersion: text(options.serviceVersion, "OTEL_SERVICE_VERSION"),
    integrations:
      options.integrations ??
      (envIntegrations ? splitNames(envIntegrations) : undefined),
    captureContent: content,
    captureContentOverrides: options.captureContent !== undefined || !content,
    disabled: flag(options.disabled, "SIDESEAT_DISABLED", false),
    debug: flag(options.debug, "SIDESEAT_DEBUG", false),
    export: options.export ?? true,
    metrics: options.metrics ?? true,
    logs: options.logs ?? true,
    resourceAttributes: Object.freeze({ ...options.resourceAttributes }),
    spanProcessors: Object.freeze([...(options.spanProcessors ?? [])]),
  });
}

/** The OTLP base URL; each signal is exported to `{base}/v1/{signal}`. */
export function otlpBase(settings: Settings): string {
  const path = new URL(settings.endpoint).pathname;
  return path && path !== "/"
    ? settings.endpoint
    : `${settings.endpoint}/otel/${encodeURIComponent(settings.project)}`;
}

export function signalEndpoint(
  settings: Settings,
  signal: "traces" | "logs" | "metrics",
): string {
  return `${otlpBase(settings)}/v1/${signal}`;
}

/** OTLP headers: `OTEL_EXPORTER_OTLP_HEADERS` plus the API key, which wins. */
export function exportHeaders(settings: Settings): Record<string, string> {
  const headers: Record<string, string> = {};
  for (const pair of (process.env.OTEL_EXPORTER_OTLP_HEADERS ?? "").split(
    ",",
  )) {
    const separator = pair.indexOf("=");
    if (separator > 0) {
      headers[decodeURIComponent(pair.slice(0, separator).trim())] =
        decodeURIComponent(pair.slice(separator + 1).trim());
    }
  }
  if (settings.apiKey) {
    // Header names are case-insensitive; two spellings would send two credentials.
    for (const name of Object.keys(headers)) {
      if (name.toLowerCase() === "authorization") delete headers[name];
    }
    headers.Authorization = `Bearer ${settings.apiKey}`;
  }
  return headers;
}

/** What a second {@link init} call must match to be treated as the same configuration. */
export function identity(settings: Settings): string {
  const names = settings.integrations?.map((i) =>
    typeof i === "string" ? i : i.name,
  );
  return JSON.stringify([
    settings.endpoint,
    settings.project,
    settings.apiKey,
    settings.serviceName,
    settings.serviceVersion,
    names ?? null,
    settings.captureContent,
    settings.captureContentOverrides,
    settings.disabled,
    settings.export,
    settings.metrics,
    settings.logs,
    Object.entries(settings.resourceAttributes).sort(),
  ]);
}

/** An explicit option, else its environment variable; blank values count as unset, trimmed. */
function text(explicit: string | undefined, name: string): string | undefined {
  for (const raw of [explicit, process.env[name]]) {
    const value = raw?.trim();
    if (value) return value;
  }
  return undefined;
}

function normalizeEndpoint(raw: string | undefined): string {
  const value = (raw ?? DEFAULT_ENDPOINT).trim().replace(/\/+$/, "");
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw new ConfigurationError(
      `endpoint must be an http(s) URL, got ${JSON.stringify(raw)}`,
    );
  }
  if ((url.protocol !== "http:" && url.protocol !== "https:") || !url.host) {
    throw new ConfigurationError(
      `endpoint must be an http(s) URL, got ${JSON.stringify(raw)}`,
    );
  }
  return value;
}

function flag(
  explicit: boolean | undefined,
  name: string,
  fallback: boolean,
): boolean {
  if (explicit !== undefined) return explicit;
  const raw = process.env[name];
  if (raw === undefined || raw.trim() === "") return fallback;
  const value = raw.trim().toLowerCase();
  if (["1", "true", "yes"].includes(value)) return true;
  if (["0", "false", "no"].includes(value)) return false;
  throw new ConfigurationError(
    `${name} must be one of 1/0, true/false, yes/no; got ${JSON.stringify(raw)}`,
  );
}

function splitNames(raw: string): string[] {
  return raw
    .split(",")
    .map((name) => name.trim())
    .filter(Boolean);
}
