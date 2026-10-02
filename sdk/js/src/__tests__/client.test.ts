import { context, propagation, trace as otelTrace } from "@opentelemetry/api";
import { afterEach, describe, expect, it } from "vitest";
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
    const spans = await capture(
      { integrations: ["strands"], serviceName: "travel-agent" },
      () => sideseat.span("work", () => undefined),
    );
    expect(spans[0]!.resource.attributes).toMatchObject({
      "service.name": "travel-agent",
      "telemetry.sdk.name": "sideseat",
      "sideseat.framework": "strands",
      "sideseat.integrations": ["strands"],
    });
  });
});
