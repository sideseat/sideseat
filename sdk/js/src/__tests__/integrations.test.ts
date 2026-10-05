import { afterEach, describe, expect, it } from "vitest";
import * as sideseat from "../index.js";
import { resolveSettings } from "../config.js";
import {
  claudeAgentSDK,
  cliEnvironment,
} from "../integrations/claude-agent-sdk.js";
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
    const env = cliEnvironment(
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

  it("rejects a requested integration whose package is not installed", async () => {
    await expect(
      sideseat.init({ integrations: ["strands"], export: false }),
    ).rejects.toThrow(/@strands-agents\/sdk/);
  });

  it("sets the Claude Code CLI variables until shutdown and keeps the application's own", async () => {
    const before = { ...process.env };
    const settings = resolveSettings({ endpoint: "http://host:5388" });
    for (const key of Object.keys(cliEnvironment(settings))) {
      delete process.env[key];
    }
    process.env.OTEL_SERVICE_NAME = "chosen-by-app";
    try {
      claudeAgentSDK.instrument!({
        settings,
      } as Parameters<NonNullable<typeof claudeAgentSDK.instrument>>[0]);
      expect(process.env.BETA_TRACING_ENDPOINT).toBe(
        "http://host:5388/otel/default",
      );
      expect(process.env.OTEL_SERVICE_NAME).toBe("chosen-by-app");
      await claudeAgentSDK.shutdown!();
      expect(process.env.BETA_TRACING_ENDPOINT).toBeUndefined();
      expect(process.env.CLAUDE_CODE_ENABLE_TELEMETRY).toBeUndefined();
      expect(process.env.OTEL_SERVICE_NAME).toBe("chosen-by-app");
    } finally {
      process.env = before;
    }
  });

  it("registers the Vercel AI SDK integration once per pipeline", async () => {
    const registry = () =>
      (globalThis as { AI_SDK_TELEMETRY_INTEGRATIONS?: unknown[] })
        .AI_SDK_TELEMETRY_INTEGRATIONS ?? [];
    const before = registry().length;
    for (let run = 0; run < 2; run += 1) {
      await sideseat.init({ integrations: ["vercel-ai"], export: false });
      expect(registry()).toHaveLength(before + 1);
      await sideseat.shutdown();
      resetGlobals();
    }
    expect(registry()).toHaveLength(before);
  });
});
