import { describe, expect, it } from "vitest";
import { tokenDisplay, type TokenCounters } from "../token-breakdown";

function counters(overrides: Partial<TokenCounters> = {}): TokenCounters {
  return {
    input_tokens: 0,
    output_tokens: 0,
    cache_read_tokens: 0,
    cache_write_tokens: 0,
    reasoning_tokens: 0,
    total_tokens: 0,
    ...overrides,
  };
}

describe("tokenDisplay", () => {
  it("never adds a counter the provider already counted inside its input", () => {
    // OpenAI: 800 of the 1,000 prompt tokens were cache hits, and the total says so.
    const d = tokenDisplay(
      counters({
        input_tokens: 1000,
        output_tokens: 50,
        cache_read_tokens: 800,
        total_tokens: 1050,
      }),
    );

    expect(d.input).toBe(1000);
    expect(d.output).toBe(50);
    expect(d.separate).toBe(0);
    expect(d.separateOf).toEqual([]);
    expect(d.total).toBe(1050);
  });

  it("reports what the total holds beyond the two sides, and names it", () => {
    // Anthropic: cache creation is billed on top of an input_tokens that excludes it.
    const d = tokenDisplay(
      counters({
        input_tokens: 10,
        output_tokens: 205,
        cache_write_tokens: 17649,
        total_tokens: 17864,
      }),
    );

    expect(d.input).toBe(10);
    expect(d.output).toBe(205);
    expect(d.separate).toBe(17649);
    expect(d.separateOf).toEqual(["cache write"]);
  });

  it("names the counter that is actually separate, not every counter present", () => {
    // Gemini: cached content inside the prompt total, thoughts beside the output. The residual is 300,
    // which only the reasoning counter explains - naming cache read too said 400 cached tokens had been
    // billed on top when they had not.
    const d = tokenDisplay(
      counters({
        input_tokens: 500,
        output_tokens: 80,
        cache_read_tokens: 400,
        reasoning_tokens: 300,
        total_tokens: 880,
      }),
    );

    expect(d.separate).toBe(300);
    expect(d.separateOf).toEqual(["reasoning"]);
  });

  it("names both when both together explain the residual", () => {
    // Anthropic-shaped: cache read and cache write are each billed on top of input_tokens.
    const d = tokenDisplay(
      counters({
        input_tokens: 10,
        output_tokens: 20,
        cache_read_tokens: 100,
        cache_write_tokens: 200,
        total_tokens: 330,
      }),
    );

    expect(d.separate).toBe(300);
    expect(d.separateOf).toEqual(["cache read", "cache write"]);
  });

  it("names none when the numbers do not say which counter it was", () => {
    // Two counters of equal value: either alone explains a residual of 100, so nothing is proven.
    const d = tokenDisplay(
      counters({
        input_tokens: 10,
        output_tokens: 20,
        cache_read_tokens: 100,
        reasoning_tokens: 100,
        total_tokens: 130,
      }),
    );

    expect(d.separate).toBe(100);
    expect(d.separateOf).toEqual([]);
  });

  it("names none when no combination explains the residual", () => {
    const d = tokenDisplay(
      counters({
        input_tokens: 10,
        output_tokens: 20,
        cache_read_tokens: 7,
        total_tokens: 500,
      }),
    );

    expect(d.separate).toBe(470);
    expect(d.separateOf).toEqual([]);
  });

  it("always sums exactly", () => {
    const cases: TokenCounters[] = [
      counters({
        input_tokens: 1000,
        output_tokens: 50,
        cache_read_tokens: 800,
        total_tokens: 1050,
      }),
      counters({
        input_tokens: 10,
        output_tokens: 205,
        cache_write_tokens: 17649,
        total_tokens: 17864,
      }),
      counters({ input_tokens: 500, output_tokens: 80, reasoning_tokens: 300, total_tokens: 880 }),
      // A provider reporting a total larger than any counter accounts for.
      counters({ input_tokens: 10, output_tokens: 20, total_tokens: 9999 }),
    ];

    for (const c of cases) {
      const d = tokenDisplay(c);
      expect(d.input + d.output + d.separate).toBe(d.total);
    }
  });

  it("falls back to the two sides when no total was reported", () => {
    const d = tokenDisplay(counters({ input_tokens: 7, output_tokens: 3 }));
    expect(d.total).toBe(10);
    expect(d.separate).toBe(0);
  });

  it("never reports a negative residual for an inconsistent total", () => {
    const d = tokenDisplay(counters({ input_tokens: 100, output_tokens: 100, total_tokens: 50 }));
    expect(d.separate).toBe(0);
  });
});
