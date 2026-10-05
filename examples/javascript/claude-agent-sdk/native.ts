/**
 * The Claude Code CLI's documented telemetry setup: `OTEL_*` and `CLAUDE_CODE_*` variables.
 *
 * The Agent SDK spawns the Claude Code CLI, which carries its own OpenTelemetry instrumentation and
 * reads its configuration from the environment it inherits. The host process exports its own spans
 * through a plain provider, and the Agent SDK hands the active span to the CLI as `TRACEPARENT`.
 */
import {
  authHeaders,
  otlpBase,
  tracesEndpoint,
  type NativeTelemetry,
} from '../harness/telemetry.js';

export async function configure(native: NativeTelemetry): Promise<void> {
  await native.provider();
  Object.assign(process.env, {
    CLAUDE_CODE_ENABLE_TELEMETRY: '1',
    // Spans are a beta tier; without it the CLI exports only metrics and logs.
    CLAUDE_CODE_ENHANCED_TELEMETRY_BETA: '1',
    // The detailed tier is the one that records the conversation on spans.
    ENABLE_BETA_TRACING_DETAILED: '1',
    BETA_TRACING_ENDPOINT: otlpBase(),
    // Never "console": the CLI's stdout is the Agent SDK's message channel.
    OTEL_TRACES_EXPORTER: 'otlp',
    OTEL_METRICS_EXPORTER: 'none',
    OTEL_LOGS_EXPORTER: 'none',
    OTEL_EXPORTER_OTLP_TRACES_PROTOCOL: 'http/protobuf',
    OTEL_EXPORTER_OTLP_TRACES_ENDPOINT: tracesEndpoint(),
    OTEL_TRACES_EXPORT_INTERVAL: '1000',
    OTEL_SERVICE_NAME: 'claude-code',
    OTEL_LOG_USER_PROMPTS: '1',
    OTEL_LOG_TOOL_DETAILS: '1',
  });
  const headers = Object.entries(authHeaders());
  if (headers.length > 0) {
    process.env.OTEL_EXPORTER_OTLP_TRACES_HEADERS = headers.map(([k, v]) => `${k}=${v}`).join(',');
  }
}
