import { afterEach, describe, expect, it, vi } from "vitest";
import { exportHeaders, resolveSettings, signalEndpoint } from "../config.js";
import { ConfigurationError } from "../errors.js";

afterEach(() => vi.unstubAllEnvs());

describe("settings", () => {
  it("defaults to the local server and default project", () => {
    const settings = resolveSettings();
    expect(signalEndpoint(settings, "traces")).toBe(
      "http://127.0.0.1:5388/otel/default/v1/traces",
    );
    expect(settings.captureContent).toBe(true);
    expect(settings.integrations).toBeUndefined();
  });

  it("treats an endpoint with a path as an OTLP base", () => {
    const settings = resolveSettings({
      endpoint: "https://collector.example.com/otel/team-a/",
      project: "ignored",
    });
    expect(signalEndpoint(settings, "logs")).toBe(
      "https://collector.example.com/otel/team-a/v1/logs",
    );
  });

  it("prefers arguments, then environment, then defaults", () => {
    vi.stubEnv("SIDESEAT_ENDPOINT", "http://env:1");
    vi.stubEnv("OTEL_EXPORTER_OTLP_ENDPOINT", "http://otel:2");
    vi.stubEnv("SIDESEAT_PROJECT_ID", "from-env");
    expect(resolveSettings().endpoint).toBe("http://env:1");
    expect(resolveSettings().project).toBe("from-env");
    expect(
      signalEndpoint(
        resolveSettings({ endpoint: "http://arg:3", project: "arg" }),
        "traces",
      ),
    ).toBe("http://arg:3/otel/arg/v1/traces");
  });

  it.each(["ftp://host", "localhost:5388", "http://"])("rejects %s", (raw) => {
    expect(() => resolveSettings({ endpoint: raw })).toThrow(
      ConfigurationError,
    );
  });

  it("treats an invalid boolean as an error, not a default", () => {
    vi.stubEnv("SIDESEAT_CAPTURE_CONTENT", "sometimes");
    expect(() => resolveSettings()).toThrow(/SIDESEAT_CAPTURE_CONTENT/);
  });

  it("merges the API key over OTLP headers", () => {
    vi.stubEnv("OTEL_EXPORTER_OTLP_HEADERS", "x-team=a%20b,Authorization=old");
    expect(exportHeaders(resolveSettings({ apiKey: "k" }))).toEqual({
      "x-team": "a b",
      Authorization: "Bearer k",
    });
  });

  it("reads integrations from the environment", () => {
    vi.stubEnv("SIDESEAT_INTEGRATIONS", "strands, vercel-ai,");
    expect(resolveSettings().integrations).toEqual(["strands", "vercel-ai"]);
  });
});
