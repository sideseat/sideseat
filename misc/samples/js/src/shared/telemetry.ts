import { init, shutdown, Frameworks, type SideSeat, type Framework } from '@sideseat/sdk';
import { AwsInstrumentation } from '@opentelemetry/instrumentation-aws-sdk';
import { registerInstrumentations } from '@opentelemetry/instrumentation';
import { NodeTracerProvider } from '@opentelemetry/sdk-trace-node';
import { BatchSpanProcessor } from '@opentelemetry/sdk-trace-base';
import { OTLPTraceExporter } from '@opentelemetry/exporter-trace-otlp-http';
import { resourceFromAttributes } from '@opentelemetry/resources';
import { ATTR_SERVICE_NAME } from '@opentelemetry/semantic-conventions';
import { config } from './config.js';

export { Frameworks };

let client: SideSeat | null = null;
let provider: NodeTracerProvider | null = null;

export interface TelemetryOptions {
  useSideseat?: boolean;
  framework?: Framework;
  /**
   * Override service.name. Needed when misc/.env sets OTEL_SERVICE_NAME, which
   * otherwise outranks the SDK's per-framework default and leaves spans
   * unclassified by the server's framework detection.
   */
  serviceName?: string;
}

/**
 * Initialize telemetry with standard configuration.
 *
 * Default: OTLP trace exporter using OTEL_EXPORTER_OTLP_ENDPOINT env var.
 * With useSideseat=true: SideSeat SDK with automatic OTLP setup to SideSeat endpoint.
 *
 * Also instruments AWS SDK (botocore equivalent) for Bedrock call tracing.
 */
export function setupTelemetry(options: TelemetryOptions = {}): SideSeat | null {
  const { useSideseat = false, framework = Frameworks.Strands, serviceName } = options;

  if (client !== null || provider !== null) return client;

  // Register AWS SDK instrumentation for Bedrock call tracing (always)
  registerInstrumentations({
    instrumentations: [
      new AwsInstrumentation({
        suppressInternalInstrumentation: true,
      }),
    ],
  });

  if (useSideseat) {
    // Initialize SideSeat (sets up OTLP trace exporter to SideSeat endpoint)
    client = init({
      endpoint: config.sideseatEndpoint,
      projectId: config.sideseatProjectId,
      framework,
      ...(serviceName ? { serviceName } : {}),
      debug: true,
    });
  } else {
    // Set up OTLP exporter using OTEL_EXPORTER_OTLP_ENDPOINT env var
    const endpoint = process.env.OTEL_EXPORTER_OTLP_ENDPOINT;
    if (endpoint) {
      provider = new NodeTracerProvider({
        resource: resourceFromAttributes({
          [ATTR_SERVICE_NAME]: process.env.OTEL_SERVICE_NAME ?? 'js-samples',
        }),
        spanProcessors: [
          new BatchSpanProcessor(
            new OTLPTraceExporter({
              url: `${endpoint}/v1/traces`,
            })
          ),
        ],
      });
      provider.register();
    }
  }

  return client;
}

/**
 * Shutdown telemetry and flush pending spans.
 */
export async function shutdownTelemetry(): Promise<void> {
  if (provider) {
    await provider.shutdown();
    provider = null;
  }
  await shutdown();
  client = null;
}

/**
 * Get the current SideSeat client instance.
 */
export function getClient(): SideSeat | null {
  return client;
}

/** service.name for host-process spans wrapping the Claude Code CLI. */
export const CLAUDE_AGENT_SERVICE_NAME = 'claude-agent-sdk';

/**
 * Build the environment for the Claude Code CLI subprocess spawned by the Agent SDK.
 *
 * The Agent SDK emits no telemetry itself: the CLI child process carries its own
 * OpenTelemetry instrumentation and is configured entirely through these variables.
 *
 * Spread over `...process.env` at the call site. In the TypeScript SDK `options.env`
 * REPLACES the inherited environment rather than merging, so omitting the spread
 * strips PATH and the AWS credential variables.
 */
export function buildClaudeCodeEnv(modelId?: string): Record<string, string> {
  // Mirrors SideSeat._buildEndpoint: an endpoint that already carries a path (e.g.
  // http://host/otel/myproject) gets /v1/{signal} appended directly. Building
  // /otel/{project} unconditionally would produce a doubled path and a silent 404.
  const collectorBase = (() => {
    const { pathname } = new URL(config.sideseatEndpoint);
    return pathname && pathname !== '/'
      ? config.sideseatEndpoint
      : `${config.sideseatEndpoint}/otel/${config.sideseatProjectId}`;
  })();
  const tracesEndpoint = `${collectorBase}/v1/traces`;

  const env: Record<string, string> = {
    CLAUDE_CODE_USE_BEDROCK: '1',
    AWS_REGION: config.awsRegion,

    CLAUDE_CODE_ENABLE_TELEMETRY: '1',
    // Span tracing is beta and off without this, leaving only metrics and logs.
    CLAUDE_CODE_ENHANCED_TELEMETRY_BETA: '1',
    // Second beta tier, and the only way to get conversation text onto spans:
    // response.model_output (assistant reply), new_context (user turn),
    // user_system_prompt and tool_input are emitted only when this is on. Without
    // it the SideSeat message feed stays empty. Takes the base URL, not a path.
    ENABLE_BETA_TRACING_DETAILED: '1',
    BETA_TRACING_ENDPOINT: collectorBase,

    // Never 'console': the CLI writes telemetry to stdout, which is the Agent SDK's
    // message channel, and would corrupt the stream.
    OTEL_TRACES_EXPORTER: 'otlp',
    // SideSeat accepts metrics and logs but only persists traces, so exporting them
    // would burn bandwidth for data that is dropped.
    OTEL_METRICS_EXPORTER: 'none',
    OTEL_LOGS_EXPORTER: 'none',
    OTEL_EXPORTER_OTLP_TRACES_PROTOCOL: 'http/protobuf',
    OTEL_EXPORTER_OTLP_TRACES_ENDPOINT: tracesEndpoint,
    // Default is 5000ms; shorten so spans land before a short sample exits.
    OTEL_TRACES_EXPORT_INTERVAL: '1000',
    OTEL_SERVICE_NAME: 'claude-code',

    // Content is redacted by default, which leaves the SideSeat message feed empty.
    OTEL_LOG_USER_PROMPTS: '1',
    OTEL_LOG_TOOL_DETAILS: '1',
    // OTEL_LOG_TOOL_CONTENT is deliberately off: it adds a tool.output span event
    // carrying the same result that detailed tracing already reports through
    // new_context, and that copy has no tool_use_id to pair it with its call.
    // Enable it only when running without detailed beta tracing.

    // Surface exporter failures through the stderr callback instead of silently
    // dropping telemetry.
    CLAUDE_CODE_OTEL_DIAG_STDERR: '1',
  };

  // OTLP ingestion is open by default (`otel.auth_required` is a separate flag from
  // the query API's auth and defaults to false), so this is usually a no-op. It only
  // matters when the server runs with `--otel-auth-required`: the CLI exports on its
  // own connection, so it needs the key independently of the host process. Mirrors
  // what the Python sample gets from the SDK's header builder.
  const apiKey = process.env.SIDESEAT_API_KEY;
  if (apiKey) {
    env.OTEL_EXPORTER_OTLP_TRACES_HEADERS = `Authorization=Bearer ${apiKey}`;
  }

  if (modelId) {
    // On Bedrock, background tasks fall back to the default Sonnet model, which may
    // not be enabled in the account. Pin both slots to the sample's model.
    env.ANTHROPIC_MODEL = modelId;
    env.ANTHROPIC_DEFAULT_HAIKU_MODEL = modelId;
  }

  return env;
}
