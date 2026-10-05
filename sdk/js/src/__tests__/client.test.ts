import { context, propagation, trace as otelTrace } from "@opentelemetry/api";
import { NodeTracerProvider } from "@opentelemetry/sdk-trace-node";
import { afterEach, describe, expect, it, vi } from "vitest";
import * as sideseat from "../index.js";
import { capture, resetGlobals } from "../testing.js";

afterEach(async () => {
  await sideseat.shutdown();
  resetGlobals();
});

const named = (spans: Awaited<ReturnType<typeof capture>>, name: string) =>
  spans.filter((span) => span.name === name);

describe("spans and traces", () => {
  it("starts a root trace even inside another span", async () => {
    const spans = await capture({ integrations: [] }, () =>
      sideseat.trace("outer", () => sideseat.trace("inner", () => undefined)),
    );
    const [outer] = named(spans, "outer");
    const [inner] = named(spans, "inner");
    expect(inner!.parentSpanContext).toBeUndefined();
    expect(inner!.spanContext().traceId).not.toBe(outer!.spanContext().traceId);
  });

  it("nests a span under the active span", async () => {
    const spans = await capture({ integrations: [] }, () =>
      sideseat.trace("root", () => sideseat.span("child", () => undefined)),
    );
    const [root] = named(spans, "root");
    const [child] = named(spans, "child");
    expect(child!.parentSpanContext?.spanId).toBe(root!.spanContext().spanId);
  });

  it("records an exception and rethrows it", async () => {
    let caught: unknown;
    const spans = await capture({ integrations: [] }, async () => {
      try {
        await sideseat.span("fails", () => {
          throw new Error("boom");
        });
      } catch (error) {
        caught = error;
      }
    });
    expect(caught).toBeInstanceOf(Error);
    const [failed] = named(spans, "fails");
    expect(failed!.status.code).toBe(2);
    expect(failed!.events[0]!.name).toBe("exception");
  });
});

describe("correlation", () => {
  it("reaches spans a framework creates", async () => {
    const spans = await capture({ integrations: [] }, () =>
      sideseat.session({ sessionId: "s-1", userId: "u-1" }, () =>
        otelTrace
          .getTracer("framework")
          .startActiveSpan("llm", (span) => span.end()),
      ),
    );
    expect(named(spans, "llm")[0]!.attributes).toMatchObject({
      "session.id": "s-1",
      "user.id": "u-1",
    });
  });

  it("applies trace correlation to descendants only", async () => {
    const spans = await capture({ integrations: [] }, async () => {
      await sideseat.trace("conversation", { sessionId: "s-2" }, () =>
        sideseat.span("step", () => undefined),
      );
      await sideseat.span("unrelated", () => undefined);
    });
    expect(named(spans, "step")[0]!.attributes["session.id"]).toBe("s-2");
    expect(
      named(spans, "unrelated")[0]!.attributes["session.id"],
    ).toBeUndefined();
  });

  it("lets a nested scope override and then restore", async () => {
    const spans = await capture({ integrations: [] }, () =>
      sideseat.session({ sessionId: "outer", userId: "u" }, async () => {
        await sideseat.session({ sessionId: "inner" }, () =>
          sideseat.span("a", () => undefined),
        );
        await sideseat.span("b", () => undefined);
      }),
    );
    expect(named(spans, "a")[0]!.attributes).toMatchObject({
      "session.id": "inner",
      "user.id": "u",
    });
    expect(named(spans, "b")[0]!.attributes).toMatchObject({
      "session.id": "outer",
      "user.id": "u",
    });
  });

  it("requires a non-empty session id and user id", async () => {
    await sideseat.init({ integrations: [], export: false });
    expect(() =>
      sideseat.session({} as { sessionId: string }, () => undefined),
    ).toThrow(TypeError);
    expect(() => sideseat.session({ sessionId: "" }, () => undefined)).toThrow(
      TypeError,
    );
    expect(() =>
      sideseat.session({ sessionId: "s", userId: "" }, () => undefined),
    ).toThrow(TypeError);
  });

  it("is never sent over the network as baggage", async () => {
    await capture({ integrations: [] }, () =>
      sideseat.session({ sessionId: "private", userId: "person" }, () => {
        expect(propagation.getActiveBaggage()).toBeUndefined();
        const carrier: Record<string, string> = {};
        propagation.inject(context.active(), carrier);
        expect(carrier.baggage).toBeUndefined();
      }),
    );
  });
});

describe("lifecycle", () => {
  it("returns the same client for the same options", async () => {
    const first = await sideseat.init({
      integrations: [],
      export: false,
      logs: false,
    });
    await expect(
      sideseat.init({ integrations: [], export: false, logs: false }),
    ).resolves.toBe(first);
  });

  it("rejects different options", async () => {
    await sideseat.init({ integrations: [], export: false, logs: false });
    await expect(
      sideseat.init({
        integrations: [],
        export: false,
        logs: false,
        project: "other",
      }),
    ).rejects.toBeInstanceOf(sideseat.ConfigurationError);
  });

  it("fails clearly before init", () => {
    expect(() => sideseat.getClient()).toThrow(sideseat.SideSeatError);
  });

  it("does nothing when disabled", async () => {
    const client = await sideseat.init({ disabled: true });
    await client.trace("ignored", { sessionId: "s" }, (span) =>
      expect(span.isRecording()).toBe(false),
    );
    expect(client.integrations).toEqual([]);
    await expect(sideseat.flush()).resolves.toBe(true);
    await expect(sideseat.shutdown()).resolves.toBe(true);
  });

  it("names the SDK and the primary integration in the resource", async () => {
    const spans = await capture({ integrations: ["vercel-ai"] }, () =>
      sideseat.span("work", () => undefined),
    );
    expect(spans[0]!.resource.attributes).toMatchObject({
      "service.name": "ai",
      "service.version": "7.0.127",
      "telemetry.sdk.name": "sideseat",
      "telemetry.sdk.language": "nodejs",
      "telemetry.sdk.version": sideseat.VERSION,
      "sideseat.framework": "vercel-ai",
      "sideseat.integrations": ["vercel-ai"],
    });
  });

  it("falls back to the app name and SDK version without integrations", async () => {
    const spans = await capture({ integrations: [] }, () =>
      sideseat.span("work", () => undefined),
    );
    const attributes = spans[0]!.resource.attributes;
    expect(attributes["service.name"]).toBe("sideseat-app");
    expect(attributes["service.version"]).toBe(sideseat.VERSION);
    expect(attributes["sideseat.framework"]).toBeUndefined();
    expect(attributes["sideseat.integrations"]).toBeUndefined();
  });

  it("layers explicit attributes over OTEL_RESOURCE_ATTRIBUTES", async () => {
    vi.stubEnv(
      "OTEL_RESOURCE_ATTRIBUTES",
      "deployment.environment.name=staging,team=ai",
    );
    try {
      const spans = await capture(
        { integrations: [], resourceAttributes: { team: "platform" } },
        () => sideseat.span("work", () => undefined),
      );
      expect(spans[0]!.resource.attributes).toMatchObject({
        "deployment.environment.name": "staging",
        team: "platform",
        "telemetry.sdk.name": "sideseat",
      });
    } finally {
      vi.unstubAllEnvs();
    }
  });

  it("stays non-recording when disabled even if another provider is registered", async () => {
    const other = new NodeTracerProvider();
    other.register();
    const client = await sideseat.init({ disabled: true });
    await client.span("ignored", (span) =>
      expect(span.isRecording()).toBe(false),
    );
    expect(client.getTracer("lib").startSpan("x").isRecording()).toBe(false);
    await other.shutdown();
  });

  it("is disabled by SIDESEAT_DISABLED", async () => {
    vi.stubEnv("SIDESEAT_DISABLED", "yes");
    try {
      const client = await sideseat.init();
      expect(client.settings.disabled).toBe(true);
      expect(client.tracerProvider).toBeUndefined();
    } finally {
      vi.unstubAllEnvs();
    }
  });

  it("shuts down once and reports success every time", async () => {
    const client = await sideseat.init({
      integrations: [],
      export: false,
      logs: false,
    });
    const first = client.shutdown();
    expect(client.shutdown()).toBe(first);
    await expect(first).resolves.toBe(true);
    await expect(sideseat.shutdown()).resolves.toBe(true);
    await expect(sideseat.shutdown()).resolves.toBe(true);
    expect(() => sideseat.getClient()).toThrow(sideseat.SideSeatError);
  });

  it("reports a processor that cannot shut down", async () => {
    const client = await sideseat.init({
      integrations: [],
      export: false,
      spanProcessors: [
        {
          onStart: () => undefined,
          onEnd: () => undefined,
          forceFlush: async () => undefined,
          shutdown: () => Promise.reject(new Error("stuck")),
        },
      ],
    });
    await expect(client.shutdown()).resolves.toBe(false);
  });

  it("configures again after shutdown", async () => {
    await sideseat.init({ integrations: [], export: false, project: "a" });
    await sideseat.shutdown();
    resetGlobals();
    const next = await sideseat.init({
      integrations: [],
      export: false,
      project: "b",
    });
    expect(next.settings.project).toBe("b");
  });
});
