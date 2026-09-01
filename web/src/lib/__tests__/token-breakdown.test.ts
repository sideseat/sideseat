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

  it("names the residual after the counter that is actually separate", () => {
    // Gemini: cached content inside the prompt total, thoughts beside the output.
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
    // Both counters are present, so both are listed: the residual is their combined contribution and the
    // browser cannot attribute it further without knowing the provider.
    expect(d.separateOf).toEqual(["cache read", "reasoning"]);
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
