import type { Integration } from "./types.js";

/**
 * The Vercel AI SDK. Since AI SDK 7 it delivers telemetry to registered integrations rather than
 * emitting spans itself, so `experimental_telemetry: { isEnabled: true }` produces nothing until one
 * is registered. Registered after the tracer provider exists, because the integration captures a
 * tracer when it is constructed.
 */
export const vercelAI: Integration = {
  name: "vercel-ai",
  packages: ["ai"],
  detectable: true,
  async instrument() {
    const [{ registerTelemetry }, { OpenTelemetry }] = await Promise.all([
      import("ai"),
      import("@ai-sdk/otel"),
    ]);
    registerTelemetry(new OpenTelemetry());
  },
};
