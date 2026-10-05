import {
  exportHeaders,
  otlpBase,
  signalEndpoint,
  type Settings,
} from "../config.js";
import type { Integration } from "./types.js";

/** Environment variables that send the Claude Code CLI's telemetry to SideSeat. */
export function cliEnvironment(settings: Settings): Record<string, string> {
  const env: Record<string, string> = {
    CLAUDE_CODE_ENABLE_TELEMETRY: "1",
    // Span tracing is a beta tier; without it the CLI emits only metrics and logs.
    CLAUDE_CODE_ENHANCED_TELEMETRY_BETA: "1",
    // The second beta tier is the only one that puts the conversation on spans.
    ENABLE_BETA_TRACING_DETAILED: "1",
    // This exporter appends its own /v1/traces suffix.
    BETA_TRACING_ENDPOINT: otlpBase(settings),
    // Never "console": the CLI's stdout is the Agent SDK's message channel.
    OTEL_TRACES_EXPORTER: "otlp",
    OTEL_METRICS_EXPORTER: "none",
    OTEL_LOGS_EXPORTER: "none",
    OTEL_EXPORTER_OTLP_TRACES_PROTOCOL: "http/protobuf",
    OTEL_EXPORTER_OTLP_TRACES_ENDPOINT: signalEndpoint(settings, "traces"),
    // Short-lived CLI runs exit before the default five-second batch interval elapses.
    OTEL_TRACES_EXPORT_INTERVAL: "1000",
    OTEL_SERVICE_NAME: "claude-code",
    CLAUDE_CODE_OTEL_DIAG_STDERR: "1",
  };
  if (settings.captureContent) {
    env.OTEL_LOG_USER_PROMPTS = "1";
    env.OTEL_LOG_TOOL_DETAILS = "1";
  }
  const headers = Object.entries(exportHeaders(settings));
  if (headers.length > 0) {
    env.OTEL_EXPORTER_OTLP_TRACES_HEADERS = headers
      .map(([k, v]) => `${k}=${v}`)
      .join(",");
  }
  return env;
}

/** The variables this process set for the CLI, so shutdown can remove exactly those. */
const applied = new Map<string, string>();

/**
 * The Claude Agent SDK runs the Claude Code CLI as a child process, which carries its own
 * OpenTelemetry instrumentation configured through environment variables.
 *
 * Python wraps the options constructor to add them to each spawned CLI. ES module exports cannot be
 * wrapped, so here they are set on this process, never over a value the application chose, and
 * removed again at shutdown. Every CLI the SDK spawns inherits them; an application that passes
 * `options.env` replaces the inherited environment and must spread `process.env` into it. The Agent
 * SDK passes the active span to the CLI as `TRACEPARENT`, so its spans join the trace.
 */
export const claudeAgentSDK: Integration = {
  name: "claude-agent-sdk",
  packages: ["@anthropic-ai/claude-agent-sdk"],
  detectable: true,
  instrument(ctx) {
    for (const [key, value] of Object.entries(cliEnvironment(ctx.settings))) {
      if (process.env[key]) continue;
      process.env[key] = value;
      applied.set(key, value);
    }
  },
  shutdown() {
    for (const [key, value] of applied) {
      // A value the application replaced after init is its own now.
      if (process.env[key] === value) delete process.env[key];
    }
    applied.clear();
  },
};
