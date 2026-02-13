import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { Config, SideSeatError, LOG_LEVELS, Frameworks } from "../config.js";

describe("Config", () => {
  // Save/restore env vars to prevent leakage between tests
  const originalEnv: Record<string, string | undefined> = {};
  const envKeys = [
    "SIDESEAT_ENDPOINT",
    "SIDESEAT_PROJECT_ID",
    "SIDESEAT_LOG_LEVEL",
    "SIDESEAT_DEBUG",
    "SIDESEAT_DISABLED",
    "SIDESEAT_API_KEY",
    "npm_package_name",
    "npm_package_version",
    "OTEL_SERVICE_NAME",
  ];

  beforeEach(() => {
    envKeys.forEach((key) => {
      originalEnv[key] = process.env[key];
      delete process.env[key];
    });
  });

  afterEach(() => {
    envKeys.forEach((key) => {
      if (originalEnv[key] === undefined) delete process.env[key];
      else process.env[key] = originalEnv[key];
    });
  });

  it("uses defaults when no options", () => {
    const config = Config.create();
    expect(config.endpoint).toBe("http://127.0.0.1:5388");
    expect(config.projectId).toBe("default");
    expect(config.disabled).toBe(false);
    expect(config.enableTraces).toBe(true);
    expect(config.logLevel).toBe("none");
  });

  it("reads from env vars", () => {
    process.env.SIDESEAT_ENDPOINT = "http://custom:8080";
    process.env.SIDESEAT_PROJECT_ID = "test-project";
    const config = Config.create();
    expect(config.endpoint).toBe("http://custom:8080");
    expect(config.projectId).toBe("test-project");
  });

  it("options override env vars", () => {
    process.env.SIDESEAT_ENDPOINT = "http://env:8080";
    const config = Config.create({ endpoint: "http://option:9090" });
    expect(config.endpoint).toBe("http://option:9090");
  });

  it("validates endpoint format", () => {
    expect(() => Config.create({ endpoint: "invalid" })).toThrow(SideSeatError);
    expect(() => Config.create({ endpoint: "ftp://host" })).toThrow(
      SideSeatError,
    );
  });

  it("accepts http and https endpoints", () => {
    expect(() => Config.create({ endpoint: "http://host:8080" })).not.toThrow();
    expect(() =>
      Config.create({ endpoint: "https://host:8080" }),
    ).not.toThrow();
  });

  it("removes trailing slashes from endpoint", () => {
    const config = Config.create({ endpoint: "http://host:8080///" });
    expect(config.endpoint).toBe("http://host:8080");
  });

  it("parses log level from env", () => {
    process.env.SIDESEAT_LOG_LEVEL = "debug";
    const config = Config.create();
    expect(config.logLevel).toBe("debug");
  });

  it("ignores invalid log level with warning", () => {
    process.env.SIDESEAT_LOG_LEVEL = "invalid";
    const config = Config.create();
    expect(config.logLevel).toBe("none"); // falls back to default
  });

  it("debug flag sets log level to debug", () => {
    const config = Config.create({ debug: true });
    expect(config.logLevel).toBe("debug");
  });

  it("explicit logLevel overrides debug flag", () => {
    const config = Config.create({ debug: true, logLevel: "error" });
    expect(config.logLevel).toBe("error");
  });

  it("falls back to OTEL_SERVICE_NAME", () => {
    process.env.OTEL_SERVICE_NAME = "otel-service";
    const config = Config.create();
    expect(config.serviceName).toBe("otel-service");
  });

  it("npm_package_name takes precedence over OTEL_SERVICE_NAME", () => {
    process.env.npm_package_name = "npm-package";
    process.env.OTEL_SERVICE_NAME = "otel-service";
    const config = Config.create();
    expect(config.serviceName).toBe("npm-package");
  });

  it("parses boolean env vars", () => {
    process.env.SIDESEAT_DISABLED = "true";
    expect(Config.create().disabled).toBe(true);

    process.env.SIDESEAT_DISABLED = "1";
    expect(Config.create().disabled).toBe(true);

    process.env.SIDESEAT_DISABLED = "false";
    expect(Config.create().disabled).toBe(false);

    process.env.SIDESEAT_DISABLED = "0";
    expect(Config.create().disabled).toBe(false);
  });
});

describe("LOG_LEVELS", () => {
  it("contains all expected levels", () => {
    expect(LOG_LEVELS).toEqual([
      "none",
      "error",
      "warn",
      "info",
      "debug",
      "verbose",
    ]);
  });
});

describe("Frameworks", () => {
  it("contains framework identifiers", () => {
    expect(Frameworks.Strands).toBe("strands");
    expect(Frameworks.VercelAI).toBe("vercel-ai");
    expect(Frameworks.LangChain).toBe("langchain");
  });
});

describe("SideSeatError", () => {
  it("has correct name", () => {
    const error = new SideSeatError("test message");
    expect(error.name).toBe("SideSeatError");
    expect(error.message).toBe("test message");
  });
});
