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
  const { useSideseat = false, framework = Frameworks.Strands } = options;

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
