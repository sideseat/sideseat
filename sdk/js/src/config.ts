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
  ClaudeAgentSDK: "claude-agent-sdk",
  // Python-only frameworks, listed so a JS caller can still tag spans consistently.
  Agno: "agno",
  Smolagents: "smolagents",
  AgentScope: "agentscope",
  Langflow: "langflow",
  AG2: "ag2",
  Haystack: "haystack",
  BrowserUse: "browser-use",
  LangGraph: "langgraph",
  AgentFramework: "agent-framework",

  // Providers
  Bedrock: "bedrock",
  Anthropic: "anthropic",
  OpenAI: "openai",
  GoogleGenAI: "google-genai",
  VertexAI: "vertex-ai",
} as const;

// Maps framework identifiers to the service.name the server expects for detection.
// When serviceName is not explicitly set, this is used as the default so the server
// can identify framework via service.name (fallback when span attributes are absent).
// Matches server/src/domain/traces/extract/attributes.rs FRAMEWORK_RULES service_name checks.
export const FRAMEWORK_SERVICE_NAMES: Partial<
  Record<Framework | string, string>
> = {
  strands: "strands-agents",
  "openai-agents": "openai-agents",
  // Host-process spans wrapping the Claude Code CLI subprocess, which reports
  // service.name "claude-code" itself. Both are matched server-side.
  "claude-agent-sdk": "claude-agent-sdk",
};

export type Framework = (typeof Frameworks)[keyof typeof Frameworks];

// Configuration options interface
export interface SideSeatOptions {
  /**
   * Framework identifier (use Frameworks.* constants or a custom string).
   *
   * Required: `Config.create` throws `SideSeatError` without it. It was previously typed
   * as optional, so omitting it compiled cleanly and only failed at runtime.
   */
  framework: Framework | (string & {});
  disabled?: boolean;
  endpoint?: string;
  apiKey?: string;
  projectId?: string;
  serviceName?: string;
  serviceVersion?: string;
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

  // No default `= {}`: framework is required, so an empty call must not type-check. The
  // runtime guard below still stands for plain-JavaScript callers.
  static create(options: SideSeatOptions): Config {
    // Normalised once: the type requires options, but a plain-JavaScript caller can still
    // invoke this with nothing and must reach the "framework is required" error below
    // rather than a TypeError on the first property read.
    const opts = (options ?? {}) as SideSeatOptions;
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

    const disabled = opts.disabled ?? parseBoolEnv("SIDESEAT_DISABLED", false);
    const debug = opts.debug ?? parseBoolEnv("SIDESEAT_DEBUG", false);

    // Log level: explicit option > env var > (debug ? 'debug' : 'none')
    const logLevel =
      opts.logLevel ??
      parseLogLevel(process.env.SIDESEAT_LOG_LEVEL) ??
      (debug ? "debug" : "none");

    const endpoint = normalizeEndpoint(
      opts.endpoint ??
        process.env.SIDESEAT_ENDPOINT ??
        // The standard OpenTelemetry variable, honoured after the SideSeat-specific one.
        // The Python SDK has always fallen back to it, and it is what misc/.env.example
        // and every "without SDK" example set, so a JS caller relying on it used to end
        // up silently on the default endpoint.
        process.env.OTEL_EXPORTER_OTLP_ENDPOINT ??
        DEFAULT_ENDPOINT,
    );
    const apiKey = opts.apiKey ?? process.env.SIDESEAT_API_KEY;
    const projectId =
      opts.projectId ?? process.env.SIDESEAT_PROJECT_ID ?? DEFAULT_PROJECT_ID;
    // Optional chaining: a plain-JavaScript caller can still invoke this with nothing,
    // and must get the SideSeatError below rather than a TypeError.
    const framework = opts.framework;
    if (!framework) {
      throw new SideSeatError(
        "framework is required. Pass a Frameworks.* constant, e.g.: init({ framework: Frameworks.Strands })",
      );
    }
    // Priority: explicit option > OTEL standard env > framework default > npm package name > fallback
    const serviceName =
      opts.serviceName ??
      process.env.OTEL_SERVICE_NAME ??
      FRAMEWORK_SERVICE_NAMES[framework] ??
      process.env.npm_package_name ??
      "unknown-service";
    const serviceVersion =
      opts.serviceVersion ?? process.env.npm_package_version ?? "0.0.0";
    const enableTraces = opts.enableTraces ?? true;

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
