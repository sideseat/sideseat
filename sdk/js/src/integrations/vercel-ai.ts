import type { Integration } from "./types.js";

/** The integration this process registered, so shutdown can withdraw it. */
let registered: object | undefined;

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
    registered = new OpenTelemetry();
    registerTelemetry(registered as Parameters<typeof registerTelemetry>[0]);
  },
  shutdown() {
    // AI SDK 7 has no unregister call. Its registry is this global array; without removing the
    // integration, a second init would deliver every event to two tracers, one of them shut down.
    const registry = (
      globalThis as { AI_SDK_TELEMETRY_INTEGRATIONS?: unknown[] }
    ).AI_SDK_TELEMETRY_INTEGRATIONS;
    const index = registry?.indexOf(registered) ?? -1;
    if (index >= 0) registry!.splice(index, 1);
    registered = undefined;
  },
};
