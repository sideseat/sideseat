import { describe, it, expect, afterEach, vi } from "vitest";

// `vi.mock` is hoisted and applies to the whole module graph, which is why these tests live in their own
// file: only `ai` is replaced, so the assertion about *which* integration gets registered is a real one -
// the class still comes from `@ai-sdk/otel`.
const registered: unknown[] = [];
vi.mock("ai", () => ({
  registerTelemetry: (integration: unknown) => {
    registered.push(integration);
  },
}));

import { init, shutdown, Frameworks } from "../index.js";

describe("framework wiring", () => {
  afterEach(async () => {
    registered.length = 0;
    await shutdown();
  });

  it("registers the current AI SDK telemetry integration for VercelAI", async () => {
    // Since AI SDK 7, telemetry is delivered to *registered integrations* rather than emitted as spans, so
    // `experimental_telemetry: { isEnabled: true }` produces nothing at all unless one is registered.
    // Forgetting it fails silently, which is why this is the SDK's job.
    await init({
      framework: Frameworks.VercelAI,
      endpoint: "http://127.0.0.1:1/otel/default",
    });
    expect(registered).toHaveLength(1);
    // The current integration, which emits the present GenAI semantic conventions - not the legacy one.
    expect((registered[0] as object).constructor.name).toBe("OpenTelemetry");
  });

  it("registers nothing for a framework that emits its own telemetry", async () => {
    await init({
      framework: Frameworks.Strands,
      endpoint: "http://127.0.0.1:1/otel/default",
    });
    expect(registered).toHaveLength(0);
  });

  it("registers nothing when telemetry is disabled", async () => {
    await init({ framework: Frameworks.VercelAI, disabled: true });
    expect(registered).toHaveLength(0);
  });
});
