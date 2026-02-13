// Log levels (OTEL pattern)
export const LOG_LEVELS = [
  "none",
  "error",
  "warn",
  "info",
  "debug",
  "verbose",
] as const;
export type LogLevel = (typeof LOG_LEVELS)[number];

// Framework identifiers
export const Frameworks = {
  Strands: "strands",
  VercelAI: "vercel-ai",
  LangChain: "langchain",
  CrewAI: "crewai",
  AutoGen: "autogen",
  OpenAIAgents: "openai-agents",
  GoogleADK: "google-adk",
  PydanticAI: "pydantic-ai",
} as const;

export type Framework = (typeof Frameworks)[keyof typeof Frameworks];

// Configuration options interface
export interface SideSeatOptions {
  disabled?: boolean;
  endpoint?: string;
  apiKey?: string;
  projectId?: string;
  serviceName?: string;
  serviceVersion?: string;
  /** Framework identifier (use Frameworks.* constants or custom string) */
  framework?: Framework | (string & {});
  enableTraces?: boolean;
  logLevel?: LogLevel;
  debug?: boolean;
}

export const DEFAULT_ENDPOINT = "http://127.0.0.1:5388";
export const DEFAULT_PROJECT_ID = "default";

// Custom error class
export class SideSeatError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "SideSeatError";
  }
}

// Internal config props interface
interface ConfigProps {
  disabled: boolean;
  endpoint: string;
  apiKey: string | undefined;
  projectId: string;
  serviceName: string;
  serviceVersion: string;
  framework: string;
  enableTraces: boolean;
  logLevel: LogLevel;
  debug: boolean;
}

// Immutable configuration class
export class Config {
  readonly disabled: boolean;
  readonly endpoint: string;
  readonly apiKey: string | undefined;
  readonly projectId: string;
  readonly serviceName: string;
  readonly serviceVersion: string;
  readonly framework: string;
  readonly enableTraces: boolean;
  readonly logLevel: LogLevel;
  readonly debug: boolean;

  private constructor(props: ConfigProps) {
    this.disabled = props.disabled;
    this.endpoint = props.endpoint;
    this.apiKey = props.apiKey;
    this.projectId = props.projectId;
    this.serviceName = props.serviceName;
    this.serviceVersion = props.serviceVersion;
    this.framework = props.framework;
    this.enableTraces = props.enableTraces;
    this.logLevel = props.logLevel;
    this.debug = props.debug;
  }

  static create(options: SideSeatOptions = {}): Config {
    const parseBoolEnv = (key: string, def: boolean): boolean => {
      const val = process.env[key]?.toLowerCase();
      if (val === "1" || val === "true") return true;
      if (val === "0" || val === "false") return false;
      return def;
    };

    const parseLogLevel = (val: string | undefined): LogLevel | undefined => {
      if (!val) return undefined;
      const lower = val.toLowerCase() as LogLevel;
      if (LOG_LEVELS.includes(lower)) return lower;
      console.warn(`[sideseat] Invalid log level '${val}', ignoring`);
      return undefined;
    };

    const disabled =
      options.disabled ?? parseBoolEnv("SIDESEAT_DISABLED", false);
    const debug = options.debug ?? parseBoolEnv("SIDESEAT_DEBUG", false);

    // Log level: explicit option > env var > (debug ? 'debug' : 'none')
    const logLevel =
      options.logLevel ??
      parseLogLevel(process.env.SIDESEAT_LOG_LEVEL) ??
      (debug ? "debug" : "none");

    const endpoint = normalizeEndpoint(
      options.endpoint ?? process.env.SIDESEAT_ENDPOINT ?? DEFAULT_ENDPOINT,
    );
    const apiKey = options.apiKey ?? process.env.SIDESEAT_API_KEY;
    const projectId =
      options.projectId ??
      process.env.SIDESEAT_PROJECT_ID ??
      DEFAULT_PROJECT_ID;
    // npm_package_name only set when running via npm; fallback to OTEL standard
    const serviceName =
      options.serviceName ??
      process.env.npm_package_name ??
      process.env.OTEL_SERVICE_NAME ??
      "unknown-service";
    const serviceVersion =
      options.serviceVersion ?? process.env.npm_package_version ?? "0.0.0";
    const framework = options.framework ?? "sideseat";
    const enableTraces = options.enableTraces ?? true;

    return new Config({
      disabled,
      endpoint,
      apiKey,
      projectId,
      serviceName,
      serviceVersion,
      framework,
      enableTraces,
      logLevel,
      debug,
    });
  }
}

function normalizeEndpoint(endpoint: string): string {
  const trimmed = endpoint.trim();
  if (!trimmed.startsWith("http://") && !trimmed.startsWith("https://")) {
    throw new SideSeatError(
      `Invalid endpoint: ${endpoint}. Must start with http:// or https://`,
    );
  }
  return trimmed.replace(/\/+$/, ""); // Remove trailing slashes
}
