import { describe, it, expect, beforeEach, afterEach } from "vitest";
import {
  Config,
  SideSeatError,
  LOG_LEVELS,
  Frameworks,
  FRAMEWORK_SERVICE_NAMES,
} from "../config.js";

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

  it("throws when framework is not provided", () => {
    expect(() => Config.create()).toThrow(SideSeatError);
    expect(() => Config.create({})).toThrow(SideSeatError);
    expect(() => Config.create({})).toThrow("framework is required");
  });

  it("uses defaults when framework is provided", () => {
    const config = Config.create({ framework: Frameworks.VercelAI });
    expect(config.endpoint).toBe("http://127.0.0.1:5388");
    expect(config.projectId).toBe("default");
    expect(config.disabled).toBe(false);
    expect(config.enableTraces).toBe(true);
    expect(config.logLevel).toBe("none");
  });

  it("reads from env vars", () => {
    process.env.SIDESEAT_ENDPOINT = "http://custom:8080";
    process.env.SIDESEAT_PROJECT_ID = "test-project";
    const config = Config.create({ framework: Frameworks.VercelAI });
    expect(config.endpoint).toBe("http://custom:8080");
    expect(config.projectId).toBe("test-project");
  });

  it("options override env vars", () => {
    process.env.SIDESEAT_ENDPOINT = "http://env:8080";
    const config = Config.create({
      framework: Frameworks.VercelAI,
      endpoint: "http://option:9090",
    });
    expect(config.endpoint).toBe("http://option:9090");
  });

  it("validates endpoint format", () => {
    expect(() =>
      Config.create({ framework: Frameworks.VercelAI, endpoint: "invalid" }),
    ).toThrow(SideSeatError);
    expect(() =>
      Config.create({ framework: Frameworks.VercelAI, endpoint: "ftp://host" }),
    ).toThrow(SideSeatError);
  });

  it("accepts http and https endpoints", () => {
    expect(() =>
      Config.create({
        framework: Frameworks.VercelAI,
        endpoint: "http://host:8080",
      }),
    ).not.toThrow();
    expect(() =>
      Config.create({
        framework: Frameworks.VercelAI,
        endpoint: "https://host:8080",
      }),
    ).not.toThrow();
  });

  it("removes trailing slashes from endpoint", () => {
    const config = Config.create({
      framework: Frameworks.VercelAI,
      endpoint: "http://host:8080///",
    });
    expect(config.endpoint).toBe("http://host:8080");
  });

  it("parses log level from env", () => {
    process.env.SIDESEAT_LOG_LEVEL = "debug";
    const config = Config.create({ framework: Frameworks.VercelAI });
    expect(config.logLevel).toBe("debug");
  });

  it("ignores invalid log level with warning", () => {
    process.env.SIDESEAT_LOG_LEVEL = "invalid";
    const config = Config.create({ framework: Frameworks.VercelAI });
    expect(config.logLevel).toBe("none"); // falls back to default
  });

  it("debug flag sets log level to debug", () => {
    const config = Config.create({
      framework: Frameworks.VercelAI,
      debug: true,
    });
    expect(config.logLevel).toBe("debug");
  });

  it("explicit logLevel overrides debug flag", () => {
    const config = Config.create({
      framework: Frameworks.VercelAI,
      debug: true,
      logLevel: "error",
    });
    expect(config.logLevel).toBe("error");
  });

  it("falls back to OTEL_SERVICE_NAME", () => {
    process.env.OTEL_SERVICE_NAME = "otel-service";
    const config = Config.create({ framework: Frameworks.VercelAI });
    expect(config.serviceName).toBe("otel-service");
  });

  it("OTEL_SERVICE_NAME takes precedence over npm_package_name", () => {
    process.env.OTEL_SERVICE_NAME = "otel-service";
    process.env.npm_package_name = "npm-package";
    const config = Config.create({ framework: Frameworks.VercelAI });
    expect(config.serviceName).toBe("otel-service");
  });

  it("npm_package_name used when OTEL_SERVICE_NAME not set", () => {
    process.env.npm_package_name = "npm-package";
    const config = Config.create({ framework: Frameworks.VercelAI });
    expect(config.serviceName).toBe("npm-package");
  });

  it("framework derives serviceName when not otherwise set", () => {
    const config = Config.create({ framework: Frameworks.Strands });
    expect(config.serviceName).toBe("strands-agents");
  });

  it("OTEL_SERVICE_NAME overrides framework-derived serviceName", () => {
    process.env.OTEL_SERVICE_NAME = "otel-service";
    const config = Config.create({ framework: Frameworks.Strands });
    expect(config.serviceName).toBe("otel-service");
  });

  it("explicit serviceName overrides framework-derived serviceName", () => {
    const config = Config.create({
      framework: Frameworks.Strands,
      serviceName: "my-app",
    });
    expect(config.serviceName).toBe("my-app");
  });

  it("VercelAI framework has no service name override (detected via span attrs)", () => {
    const config = Config.create({ framework: Frameworks.VercelAI });
    expect(config.serviceName).toBe("unknown-service");
  });

  it("parses boolean env vars", () => {
    const fw = { framework: Frameworks.VercelAI };

    process.env.SIDESEAT_DISABLED = "true";
    expect(Config.create(fw).disabled).toBe(true);

    process.env.SIDESEAT_DISABLED = "1";
    expect(Config.create(fw).disabled).toBe(true);

    process.env.SIDESEAT_DISABLED = "false";
    expect(Config.create(fw).disabled).toBe(false);

    process.env.SIDESEAT_DISABLED = "0";
    expect(Config.create(fw).disabled).toBe(false);
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

describe("FRAMEWORK_SERVICE_NAMES", () => {
  it("maps strands to strands-agents", () => {
    expect(FRAMEWORK_SERVICE_NAMES["strands"]).toBe("strands-agents");
  });

  it("maps openai-agents to openai-agents", () => {
    expect(FRAMEWORK_SERVICE_NAMES["openai-agents"]).toBe("openai-agents");
  });

  it("has no entry for vercel-ai (detected via span attributes)", () => {
    expect(FRAMEWORK_SERVICE_NAMES["vercel-ai"]).toBeUndefined();
  });
});

describe("SideSeatError", () => {
  it("has correct name", () => {
    const error = new SideSeatError("test message");
    expect(error.name).toBe("SideSeatError");
    expect(error.message).toBe("test message");
  });
});
