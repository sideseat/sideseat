import { describe, it, expect, afterEach, vi } from "vitest";
import {
  init,
  createClient,
  getClient,
  shutdown,
  isInitialized,
  SideSeat,
  SideSeatError,
} from "../index.js";

describe("SideSeat", () => {
  afterEach(async () => {
    await shutdown(); // Clean up global instance
  });

  it("init returns instance", () => {
    const client = init({ disabled: true });
    expect(client).toBeInstanceOf(SideSeat);
    expect(isInitialized()).toBe(true);
  });

  it("double init returns same instance", () => {
    const c1 = init({ disabled: true });
    const c2 = init({ disabled: true });
    expect(c1).toBe(c2);
  });

  it("getClient throws if not initialized", () => {
    expect(() => getClient()).toThrow(SideSeatError);
  });

  it("getClient returns instance after init", () => {
    const c1 = init({ disabled: true });
    const c2 = getClient();
    expect(c1).toBe(c2);
  });

  it("shutdown clears global instance", async () => {
    init({ disabled: true });
    expect(isInitialized()).toBe(true);
    await shutdown();
    expect(isInitialized()).toBe(false);
  });

  it("span executes callback and returns result", async () => {
    const client = init({ disabled: true });
    const result = await client.span("test", async () => 42);
    expect(result).toBe(42);
  });

  it("span sets error status on exception", async () => {
    const client = init({ disabled: true });
    await expect(
      client.span("test", async () => {
        throw new Error("test error");
      }),
    ).rejects.toThrow("test error");
  });

  it("spanSync works for sync callbacks", () => {
    const client = init({ disabled: true });
    const result = client.spanSync("test", () => 42);
    expect(result).toBe(42);
  });

  it("spanSync propagates errors", () => {
    const client = init({ disabled: true });
    expect(() =>
      client.spanSync("test", () => {
        throw new Error("sync error");
      }),
    ).toThrow("sync error");
  });

  it("concurrent createClient returns same promise", async () => {
    const p1 = createClient({ disabled: true });
    const p2 = createClient({ disabled: true });
    const [c1, c2] = await Promise.all([p1, p2]);
    expect(c1).toBe(c2);
  });

  it("shutdown handles concurrent calls", async () => {
    init({ disabled: true });
    await Promise.all([shutdown(), shutdown(), shutdown()]);
    expect(isInitialized()).toBe(false);
  });

  it("shutdown is idempotent", async () => {
    init({ disabled: true });
    await shutdown();
    await shutdown(); // Should not throw
    expect(isInitialized()).toBe(false);
  });

  it("toString returns debug representation", () => {
    const client = init({ disabled: true });
    expect(client.toString()).toContain("SideSeat(");
    expect(client.toString()).toContain("endpoint=");
  });

  it("isDisabled getter returns correct value", () => {
    const client = init({ disabled: true });
    expect(client.isDisabled).toBe(true);
  });

  it("isReady getter returns correct value", () => {
    const client = init({ disabled: true });
    expect(client.isReady).toBe(false); // disabled mode has no provider
  });

  it("config getter returns Config instance", () => {
    const client = init({ disabled: true, projectId: "test" });
    expect(client.config.projectId).toBe("test");
    expect(client.config.disabled).toBe(true);
  });

  it("getTracer returns a tracer", () => {
    const client = init({ disabled: true });
    const tracer = client.getTracer();
    expect(tracer).toBeDefined();
    expect(typeof tracer.startSpan).toBe("function");
  });

  it("validateConnection returns false when disabled", async () => {
    const client = init({ disabled: true });
    const result = await client.validateConnection();
    expect(result).toBe(false);
  });

  it("forceFlush returns true when disabled", async () => {
    const client = init({ disabled: true });
    const result = await client.forceFlush();
    expect(result).toBe(true);
  });

  it("setupConsoleExporter returns this for chaining", () => {
    const client = init({ disabled: true });
    const result = client.setupConsoleExporter();
    expect(result).toBe(client);
  });

  it("setupFileExporter returns this for chaining", () => {
    const client = init({ disabled: true });
    const result = client.setupFileExporter("/tmp/test-traces.jsonl");
    expect(result).toBe(client);
  });

  it("addSpanProcessor returns this for chaining", () => {
    const client = init({ disabled: true });
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
    const client1 = init({ disabled: true, projectId: "project1" });
    const client2 = new SideSeat({ disabled: true, projectId: "project2" });

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
    const client = await SideSeat.create({ disabled: true });
    expect(client).toBeInstanceOf(SideSeat);
  });
});

describe("setupFileExporter validation", () => {
  afterEach(async () => {
    await shutdown();
  });

  it("throws for non-existent directory when not disabled", () => {
    // Create non-disabled client (will create provider)
    const client = new SideSeat({ enableTraces: false });

    // Try to setup file exporter with non-existent directory
    expect(() =>
      client.setupFileExporter("/nonexistent/path/traces.jsonl"),
    ).toThrow(SideSeatError);
  });

  it("skips validation when disabled", () => {
    const client = init({ disabled: true });
    // Should not throw even for invalid path because validation is skipped
    expect(() =>
      client.setupFileExporter("/nonexistent/path/traces.jsonl"),
    ).not.toThrow();
  });
});
