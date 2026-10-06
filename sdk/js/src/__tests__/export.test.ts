import { diag, metrics } from "@opentelemetry/api";
import { OTLPMetricExporter } from "@opentelemetry/exporter-metrics-otlp-http";
import {
  MeterProvider,
  PeriodicExportingMetricReader,
} from "@opentelemetry/sdk-metrics";
import { createServer, type IncomingHttpHeaders } from "node:http";
import type { AddressInfo } from "node:net";
import {
  afterAll,
  afterEach,
  beforeAll,
  describe,
  expect,
  it,
  vi,
} from "vitest";
import * as sideseat from "../index.js";
import { resetGlobals } from "../testing.js";

interface Received {
  path: string;
  headers: IncomingHttpHeaders;
  body: string;
}

const received: Received[] = [];
let endpoint = "";
const server = createServer((request, response) => {
  const chunks: Buffer[] = [];
  request.on("data", (chunk: Buffer) => chunks.push(chunk));
  request.on("end", () => {
    received.push({
      path: request.url ?? "",
      headers: request.headers,
      body: Buffer.concat(chunks).toString("utf8"),
    });
    // A project named "rejected" stands in for a server that refuses the export.
    const status = request.url?.includes("/rejected/") ? 400 : 200;
    response.writeHead(status, { "content-type": "application/json" });
    response.end("{}");
  });
});

beforeAll(async () => {
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  endpoint = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});

afterAll(() => new Promise<void>((resolve) => server.close(() => resolve())));

afterEach(async () => {
  await sideseat.shutdown();
  resetGlobals();
  received.length = 0;
});

/** Every string attribute in an OTLP/JSON body, keyed by attribute name. */
function stringAttributes(body: string): Map<string, string[]> {
  const found = new Map<string, string[]>();
  JSON.stringify(JSON.parse(body), (key, value: unknown) => {
    const kv = value as { key?: string; value?: { stringValue?: string } };
    if (kv && typeof kv.key === "string" && kv.value?.stringValue) {
      found.set(kv.key, [...(found.get(kv.key) ?? []), kv.value.stringValue]);
    }
    return value;
  });
  return found;
}

describe("OTLP export", () => {
  it("sends spans with correlation and resource to the project with the API key", async () => {
    await sideseat.init({
      endpoint,
      project: "team a",
      apiKey: "k-1",
      integrations: [],
      logs: false,
      metrics: false,
    });
    await sideseat.trace(
      "agent-run",
      { sessionId: "s-1", userId: "u-1" },
      () => undefined,
    );
    await expect(sideseat.flush()).resolves.toBe(true);

    const traces = received.filter((r) => r.path.endsWith("/v1/traces"));
    expect(traces.map((r) => r.path)).toEqual(["/otel/team%20a/v1/traces"]);
    expect(traces[0]!.headers.authorization).toBe("Bearer k-1");
    const attributes = stringAttributes(traces[0]!.body);
    expect(attributes.get("session.id")).toEqual(["s-1"]);
    expect(attributes.get("user.id")).toEqual(["u-1"]);
    expect(attributes.get("telemetry.sdk.name")).toEqual(["sideseat"]);
  });

  it("exports metrics through the global meter provider", async () => {
    await sideseat.init({ endpoint, integrations: [], logs: false });
    metrics.getMeter("app").createCounter("app.requests").add(1);
    await expect(sideseat.flush()).resolves.toBe(true);

    const exported = received.filter((r) =>
      r.path.endsWith("/otel/default/v1/metrics"),
    );
    expect(exported.length).toBeGreaterThan(0);
    expect(exported.some((r) => r.body.includes("app.requests"))).toBe(true);
  });

  it("names the reader an application meter provider needs, which then exports", async () => {
    // A built MeterProvider takes no new reader, so the application constructs its provider with
    // SideSeat's reader: the documented one-line change.
    const url = `${endpoint}/otel/default/v1/metrics`;
    const provider = new MeterProvider({
      readers: [
        new PeriodicExportingMetricReader({
          exporter: new OTLPMetricExporter({ url }),
        }),
      ],
    });
    metrics.setGlobalMeterProvider(provider);
    const warn = vi.spyOn(diag, "warn");
    try {
      await sideseat.init({ endpoint, integrations: [], logs: false });
      expect(warn.mock.calls.some(([message]) => message.includes(url))).toBe(
        true,
      );
    } finally {
      warn.mockRestore();
    }
    metrics.getMeter("app").createCounter("app.tokens").add(1);
    await provider.forceFlush();
    await provider.shutdown();
    expect(
      received.some(
        (r) =>
          r.path === "/otel/default/v1/metrics" &&
          r.body.includes("app.tokens"),
      ),
    ).toBe(true);
  });

  it("reports a failed export as false instead of throwing", async () => {
    await sideseat.init({
      endpoint,
      project: "rejected",
      integrations: [],
      logs: false,
      metrics: false,
    });
    await sideseat.span("lost", () => undefined);
    await expect(sideseat.flush()).resolves.toBe(false);
  });
});
