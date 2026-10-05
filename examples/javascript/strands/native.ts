/**
 * Strands' documented telemetry setup: `setupTracer` with its OTLP exporter, which reads the
 * standard `OTEL_EXPORTER_OTLP_*` variables.
 */
import { setupTracer } from '@strands-agents/sdk/telemetry';
import { authHeaders, tracesEndpoint, type NativeTelemetry } from '../harness/telemetry.js';

export function configure(native: NativeTelemetry): void {
  process.env.OTEL_EXPORTER_OTLP_TRACES_ENDPOINT = tracesEndpoint();
  const headers = Object.entries(authHeaders());
  if (headers.length > 0) {
    process.env.OTEL_EXPORTER_OTLP_TRACES_HEADERS = headers.map(([k, v]) => `${k}=${v}`).join(',');
  }
  native.adopt(setupTracer({ exporters: { otlp: true } }));
}
