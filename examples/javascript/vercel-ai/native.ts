/**
 * The AI SDK's documented telemetry setup: a registered OpenTelemetry provider, then the
 * `@ai-sdk/otel` integration, which turns the SDK's telemetry events into spans.
 */
import { OpenTelemetry } from '@ai-sdk/otel';
import { registerTelemetry } from 'ai';
import type { NativeTelemetry } from '../harness/telemetry.js';

export async function configure(native: NativeTelemetry): Promise<void> {
  await native.provider();
  // Constructed after the provider is registered: the integration captures a tracer.
  registerTelemetry(new OpenTelemetry());
}
