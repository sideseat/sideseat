import { describe, it, expect } from "vitest";
import { formatCurrencyFixed } from "../format";

describe("formatCurrencyFixed", () => {
  it("formats zero as 0.00", () => {
    expect(formatCurrencyFixed(0)).toContain("0.00");
  });

  it("formats normal values", () => {
    expect(formatCurrencyFixed(1.5)).toContain("1.50");
    expect(formatCurrencyFixed(99.99)).toContain("99.99");
  });

  it("rounds sub-cent positive values up to 0.01", () => {
    expect(formatCurrencyFixed(0.001)).toContain("0.01");
    expect(formatCurrencyFixed(0.0001)).toContain("0.01");
    expect(formatCurrencyFixed(0.009)).toContain("0.01");
  });

  it("keeps values at or above one cent exact", () => {
    expect(formatCurrencyFixed(0.01)).toContain("0.01");
    expect(formatCurrencyFixed(0.05)).toContain("0.05");
  });

  it("does not round negative sub-cent values up", () => {
    expect(formatCurrencyFixed(-0.001)).toContain("0.00");
  });
});
