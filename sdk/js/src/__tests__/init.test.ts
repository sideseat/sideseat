import { describe, it, expect, afterEach } from "vitest";
import { VERSION } from "../version.js";
import { init, shutdown } from "../index.js";

describe("sideseat sdk", () => {
  it("exports VERSION", () => {
    expect(VERSION).toBeDefined();
    expect(typeof VERSION).toBe("string");
    expect(VERSION).toMatch(/^\d+\.\d+\.\d+$/);
  });
});

describe("init", () => {
  afterEach(async () => {
    await shutdown();
  });

  it("returns a SideSeat instance", () => {
    const result = init({ disabled: true });
    expect(result).toBeDefined();
  });
});
