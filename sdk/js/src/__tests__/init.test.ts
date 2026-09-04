import { describe, it, expect, afterEach } from "vitest";
import { VERSION } from "../version.js";
import { init, shutdown, Frameworks } from "../index.js";

describe("sideseat sdk", () => {
  it("exports VERSION", () => {
    expect(VERSION).toBeDefined();
    expect(typeof VERSION).toBe("string");
    expect(VERSION).toMatch(/^\d+\.\d+\.\d+$/);
  });
});

describe("init", () => {
  afterEach(async () => {
    await shutdown();
  });

  it("returns a SideSeat instance", () => {
    const result = init({ framework: Frameworks.VercelAI, disabled: true });
    expect(result).toBeDefined();
  });
});

describe("framework declaration and wiring", () => {
  afterEach(async () => {
    await shutdown();
  });

  it("declares the configured framework on the resource", async () => {
    // The current OTel GenAI conventions are framework-neutral, so a producer that follows them emits
    // nothing that says who produced it. This attribute is the only evidence the server has, and it reads
    // it as a fallback - per-span evidence still wins.
    const client = await init({
      framework: Frameworks.VercelAI,
      disabled: false,
      endpoint: "http://127.0.0.1:1/otel/default",
    });
    // Through the tracer provider's resource, which is what an exporter sends.
    const provider = client.tracerProvider as unknown as {
      resource?: { attributes?: Record<string, unknown> };
      _resource?: { attributes?: Record<string, unknown> };
    } | null;
    const attrs = (provider?.resource?.attributes ??
      provider?._resource?.attributes ??
      {}) as Record<string, unknown>;
    expect(attrs["sideseat.framework"]).toBe("vercel-ai");
    expect(attrs["telemetry.sdk.name"]).toBe("sideseat");
  });
});
