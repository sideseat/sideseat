import { afterEach, describe, expect, it } from "vitest";
import * as sideseat from "../index.js";
import { resolveSettings } from "../config.js";
import { loadIntegration } from "../integrations/index.js";
import { capture, resetGlobals } from "../testing.js";
import type { Integration } from "../integrations/types.js";

afterEach(async () => {
  await sideseat.shutdown();
  resetGlobals();
});

describe("integrations", () => {
  it("rejects unknown names and lists the known ones", () => {
    expect(() => loadIntegration("strandz")).toThrow(/strands/);
  });

  it("raises for a requested integration that cannot install", async () => {
    const broken: Integration = {
      name: "broken",
      packages: ["definitely-not-installed"],
      detectable: false,
      instrument() {
        throw new Error("missing module");
      },
    };
    await expect(
      sideseat.init({ integrations: [broken], export: false, logs: false }),
    ).rejects.toBeInstanceOf(sideseat.IntegrationError);
  });

  it("runs hooks in order and lets integration processors see correlation", async () => {
    const log: string[] = [];
    const recording: Integration = {
      name: "recording",
      packages: ["vitest"],
      detectable: false,
      prepare: (ctx) =>
        void log.push(`prepare:${ctx.tracerProvider === undefined}`),
      spanProcessors: () => [
        {
          onStart: (span) =>
            void log.push(
              `start:${String((span as { attributes: Record<string, unknown> }).attributes["session.id"])}`,
            ),
          onEnd: () => undefined,
          forceFlush: async () => undefined,
          shutdown: async () => undefined,
        },
      ],
      instrument: (ctx) =>
        void log.push(`instrument:${ctx.tracerProvider !== undefined}`),
      shutdown: () => void log.push("shutdown"),
    };
    await capture({ integrations: [recording] }, () =>
      sideseat.session({ sessionId: "s-9" }, () =>
        sideseat.span("work", () => undefined),
      ),
    );
    expect(log).toEqual([
      "prepare:true",
      "instrument:true",
      "start:s-9",
      "shutdown",
    ]);
  });

  it("points the Claude Code CLI at the project", () => {
    const env = sideseat.cliEnvironment(
      resolveSettings({
        endpoint: "http://host:5388",
        project: "p1",
        apiKey: "secret",
      }),
    );
    expect(env.OTEL_EXPORTER_OTLP_TRACES_ENDPOINT).toBe(
      "http://host:5388/otel/p1/v1/traces",
    );
    expect(env.BETA_TRACING_ENDPOINT).toBe("http://host:5388/otel/p1");
    expect(env.OTEL_EXPORTER_OTLP_TRACES_HEADERS).toBe(
      "Authorization=Bearer secret",
    );
    expect(env.OTEL_TRACES_EXPORTER).toBe("otlp");
  });
});
