import { describe, it, expect, afterEach, vi } from "vitest";
import {
  init,
  createClient,
  getClient,
  shutdown,
  isInitialized,
  SideSeat,
  SideSeatError,
  Frameworks,
} from "../index.js";
import {
  ForwardingSpanProcessor,
  resolveExportTimeoutMs,
} from "../sideseat.js";

describe("SideSeat", () => {
  afterEach(async () => {
    await shutdown(); // Clean up global instance
  });

  it("init returns instance", () => {
    const client = init({ framework: Frameworks.VercelAI, disabled: true });
    expect(client).toBeInstanceOf(SideSeat);
    expect(isInitialized()).toBe(true);
  });

  it("double init returns same instance", () => {
    const c1 = init({ framework: Frameworks.VercelAI, disabled: true });
    const c2 = init({ framework: Frameworks.VercelAI, disabled: true });
    expect(c1).toBe(c2);
  });

  it("getClient throws if not initialized", () => {
    expect(() => getClient()).toThrow(SideSeatError);
  });

  it("getClient returns instance after init", () => {
    const c1 = init({ framework: Frameworks.VercelAI, disabled: true });
    const c2 = getClient();
    expect(c1).toBe(c2);
  });

  it("shutdown clears global instance", async () => {
    init({ framework: Frameworks.VercelAI, disabled: true });
    expect(isInitialized()).toBe(true);
    await shutdown();
    expect(isInitialized()).toBe(false);
  });

  it("span executes callback and returns result", async () => {
    const client = init({ framework: Frameworks.VercelAI, disabled: true });
    const result = await client.span("test", async () => 42);
    expect(result).toBe(42);
  });

  it("span sets error status on exception", async () => {
    const client = init({ framework: Frameworks.VercelAI, disabled: true });
    await expect(
      client.span("test", async () => {
        throw new Error("test error");
      }),
    ).rejects.toThrow("test error");
  });

  it("spanSync works for sync callbacks", () => {
    const client = init({ framework: Frameworks.VercelAI, disabled: true });
    const result = client.spanSync("test", () => 42);
    expect(result).toBe(42);
  });

  it("spanSync propagates errors", () => {
    const client = init({ framework: Frameworks.VercelAI, disabled: true });
    expect(() =>
      client.spanSync("test", () => {
        throw new Error("sync error");
      }),
    ).toThrow("sync error");
  });

  it("concurrent createClient returns same promise", async () => {
    const p1 = createClient({ framework: Frameworks.VercelAI, disabled: true });
    const p2 = createClient({ framework: Frameworks.VercelAI, disabled: true });
    const [c1, c2] = await Promise.all([p1, p2]);
    expect(c1).toBe(c2);
  });

  it("shutdown handles concurrent calls", async () => {
    init({ framework: Frameworks.VercelAI, disabled: true });
    await Promise.all([shutdown(), shutdown(), shutdown()]);
    expect(isInitialized()).toBe(false);
  });

  it("shutdown is idempotent", async () => {
    init({ framework: Frameworks.VercelAI, disabled: true });
    await shutdown();
    await shutdown(); // Should not throw
    expect(isInitialized()).toBe(false);
  });

  it("toString returns debug representation", () => {
    const client = init({ framework: Frameworks.VercelAI, disabled: true });
    expect(client.toString()).toContain("SideSeat(");
    expect(client.toString()).toContain("endpoint=");
  });

  it("isDisabled getter returns correct value", () => {
    const client = init({ framework: Frameworks.VercelAI, disabled: true });
    expect(client.isDisabled).toBe(true);
  });

  it("isReady getter returns correct value", () => {
    const client = init({ framework: Frameworks.VercelAI, disabled: true });
    expect(client.isReady).toBe(false); // disabled mode has no provider
  });

  it("config getter returns Config instance", () => {
    const client = init({
      framework: Frameworks.VercelAI,
      disabled: true,
      projectId: "test",
    });
    expect(client.config.projectId).toBe("test");
    expect(client.config.disabled).toBe(true);
  });

  it("getTracer returns a tracer", () => {
    const client = init({ framework: Frameworks.VercelAI, disabled: true });
    const tracer = client.getTracer();
    expect(tracer).toBeDefined();
    expect(typeof tracer.startSpan).toBe("function");
  });

  it("validateConnection returns false when disabled", async () => {
    const client = init({ framework: Frameworks.VercelAI, disabled: true });
    const result = await client.validateConnection();
    expect(result).toBe(false);
  });

  it("forceFlush returns true when disabled", async () => {
    const client = init({ framework: Frameworks.VercelAI, disabled: true });
    const result = await client.forceFlush();
    expect(result).toBe(true);
  });

  it("setupConsoleExporter returns this for chaining", () => {
    const client = init({ framework: Frameworks.VercelAI, disabled: true });
    const result = client.setupConsoleExporter();
    expect(result).toBe(client);
  });

  it("setupFileExporter returns this for chaining", () => {
    const client = init({ framework: Frameworks.VercelAI, disabled: true });
    const result = client.setupFileExporter("/tmp/test-traces.jsonl");
    expect(result).toBe(client);
  });

  it("addSpanProcessor returns this for chaining", () => {
    const client = init({ framework: Frameworks.VercelAI, disabled: true });
    const mockProcessor = {
      onStart: vi.fn(),
      onEnd: vi.fn(),
      shutdown: vi.fn(),
      forceFlush: vi.fn(),
    };
    const result = client.addSpanProcessor(mockProcessor);
    expect(result).toBe(client);
  });

  it("new SideSeat creates independent instance", () => {
    const client1 = init({
      framework: Frameworks.VercelAI,
      disabled: true,
      projectId: "project1",
    });
    const client2 = new SideSeat({
      framework: Frameworks.VercelAI,
      disabled: true,
      projectId: "project2",
    });

    expect(client1.config.projectId).toBe("project1");
    expect(client2.config.projectId).toBe("project2");
    expect(client1).not.toBe(client2);
  });
});

describe("SideSeat.create", () => {
  afterEach(async () => {
    await shutdown();
  });

  it("creates instance asynchronously", async () => {
    const client = await SideSeat.create({
      framework: Frameworks.VercelAI,
      disabled: true,
    });
    expect(client).toBeInstanceOf(SideSeat);
  });
});

describe("setupFileExporter validation", () => {
  afterEach(async () => {
    await shutdown();
  });

  it("throws for non-existent directory when not disabled", () => {
    // Create non-disabled client (will create provider)
    const client = new SideSeat({
      framework: Frameworks.VercelAI,
      enableTraces: false,
    });

    // Try to setup file exporter with non-existent directory
    expect(() =>
      client.setupFileExporter("/nonexistent/path/traces.jsonl"),
    ).toThrow(SideSeatError);
  });

  it("skips validation when disabled", () => {
    const client = init({ framework: Frameworks.VercelAI, disabled: true });
    // Should not throw even for invalid path because validation is skipped
    expect(() =>
      client.setupFileExporter("/nonexistent/path/traces.jsonl"),
    ).not.toThrow();
  });
});

describe("package version", () => {
  it("VERSION matches package.json", async () => {
    // Nothing syncs these two: `make bump` only touches cli/ and server/, so they
    // drifted apart once already (package.json 1.0.8 vs VERSION 1.0.7), which made
    // telemetry.sdk.version and the User-Agent report the wrong version.
    const { readFileSync } = await import("node:fs");
    const { fileURLToPath } = await import("node:url");
    const { dirname, resolve } = await import("node:path");
    const here = dirname(fileURLToPath(import.meta.url));
    const pkg = JSON.parse(
      readFileSync(resolve(here, "../../package.json"), "utf8"),
    ) as { version: string };
    const { VERSION } = await import("../version.js");
    expect(VERSION).toBe(pkg.version);
  });
});

describe("resolveExportTimeoutMs", () => {
  const original = process.env.OTEL_EXPORTER_OTLP_TIMEOUT;
  afterEach(() => {
    if (original === undefined) delete process.env.OTEL_EXPORTER_OTLP_TIMEOUT;
    else process.env.OTEL_EXPORTER_OTLP_TIMEOUT = original;
  });

  it("defaults to 30s when unset", () => {
    delete process.env.OTEL_EXPORTER_OTLP_TIMEOUT;
    expect(resolveExportTimeoutMs()).toBe(30_000);
  });

  it("reads the value as milliseconds, matching OpenTelemetry JS", () => {
    // Deliberately NOT seconds: OTel JS reads this variable as milliseconds (Python reads
    // it as seconds). Multiplying here would disagree with the exporter it is paired with.
    process.env.OTEL_EXPORTER_OTLP_TIMEOUT = "7000";
    expect(resolveExportTimeoutMs()).toBe(7_000);
  });

  it.each(["abc", "0", "-5", ""])("falls back on invalid value %p", (bad) => {
    process.env.OTEL_EXPORTER_OTLP_TIMEOUT = bad;
    expect(resolveExportTimeoutMs()).toBe(30_000);
  });
});

describe("README framework list", () => {
  it("documents every exported Frameworks constant", async () => {
    const fs = await import("node:fs");
    const readme = fs.readFileSync(
      new URL("../../README.md", import.meta.url),
      "utf8",
    );
    const missing = Object.entries(Frameworks).filter(
      ([name, value]) =>
        !readme.includes(`Frameworks.${name}`) ||
        !readme.includes(`"${value}"`),
    );
    expect(missing.map(([n]) => n)).toEqual([]);
  });
});

describe("ForwardingSpanProcessor", () => {
  const noopSpan = {} as Parameters<ForwardingSpanProcessor["onEnd"]>[0];

  function processor(overrides: Record<string, unknown> = {}) {
    return {
      onStart: vi.fn(),
      onEnd: vi.fn(),
      forceFlush: vi.fn(async () => {}),
      shutdown: vi.fn(async () => {}),
      ...overrides,
    };
  }

  it("a throwing processor does not stop the ones after it", () => {
    const forwarder = new ForwardingSpanProcessor();
    const bad = processor({
      onStart: vi.fn(() => {
        throw new Error("boom");
      }),
      onEnd: vi.fn(() => {
        throw new Error("boom");
      }),
    });
    const good = processor();
    forwarder.add(bad as never);
    forwarder.add(good as never);

    // And it does not throw out: these run inside the application's own span.end().
    expect(() =>
      forwarder.onStart(noopSpan as never, {} as never),
    ).not.toThrow();
    expect(() => forwarder.onEnd(noopSpan)).not.toThrow();

    expect(good.onStart).toHaveBeenCalledTimes(1);
    expect(good.onEnd).toHaveBeenCalledTimes(1);
  });

  it("forceFlush flushes every processor and then reports the failures", async () => {
    const forwarder = new ForwardingSpanProcessor();
    const failing = processor({
      forceFlush: vi.fn(async () => {
        throw new Error("exporter unreachable");
      }),
    });
    const healthy = processor();
    forwarder.add(failing as never);
    forwarder.add(healthy as never);

    // Reported, not discarded: resolving regardless made the public forceFlush() return
    // true while spans sat unexported.
    await expect(forwarder.forceFlush()).rejects.toThrow(
      "exporter unreachable",
    );
    expect(healthy.forceFlush).toHaveBeenCalledTimes(1);
  });

  it("forceFlush resolves when every processor succeeds", async () => {
    const forwarder = new ForwardingSpanProcessor();
    forwarder.add(processor() as never);
    forwarder.add(processor() as never);
    await expect(forwarder.forceFlush()).resolves.toBeUndefined();
  });

  it("shutdown clears its delegates even when one fails", async () => {
    const forwarder = new ForwardingSpanProcessor();
    const failing = processor({
      shutdown: vi.fn(async () => {
        throw new Error("no");
      }),
    });
    const healthy = processor();
    forwarder.add(failing as never);
    forwarder.add(healthy as never);

    await expect(forwarder.shutdown()).rejects.toThrow("no");
    expect(healthy.shutdown).toHaveBeenCalledTimes(1);

    // Cleared, so onEnd no longer reaches dead processors.
    forwarder.onEnd(noopSpan);
    expect(healthy.onEnd).not.toHaveBeenCalled();
  });
});

describe("SideSeat.forceFlush reports a failed flush", () => {
  afterEach(async () => {
    await shutdown();
  });

  it("returns false when a processor cannot flush", async () => {
    const client = init({
      framework: Frameworks.VercelAI,
      endpoint: "http://127.0.0.1:1",
    });
    client.addSpanProcessor({
      onStart: () => {},
      onEnd: () => {},
      forceFlush: async () => {
        throw new Error("exporter unreachable");
      },
      shutdown: async () => {},
    });

    // `true` has to mean the spans were flushed, or a caller draining before exit has no
    // way to know it lost them.
    await expect(client.forceFlush(2000)).resolves.toBe(false);
  });
});
