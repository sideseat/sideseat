/**
 * Turning the reported token counters into figures that add up.
 *
 * Whether a cache or reasoning counter sits *inside* the input and output totals or *beside* them is the
 * provider's convention: OpenAI's `cached_tokens` is part of its prompt total, Anthropic bills cache
 * creation on top of an `input_tokens` that excludes it, and Gemini is the mirror image again (cached
 * content inside the prompt, thoughts beside the output). The browser cannot know which convention applies
 * - a trace row aggregates every span of the trace and those spans may come from different providers.
 *
 * So no counter is ever added into a side here. The server applies the convention per span and its
 * `total_tokens` is the authority; what the total holds beyond input plus output is reported as its own
 * line, which makes the three figures sum exactly whichever convention produced them. Adding the counters
 * unconditionally is what this replaces, and it contradicted the total it displayed beside itself: a cached
 * OpenAI call showed `1,800 -> 50` against a total of 1,050.
 */

export interface TokenCounters {
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
  reasoning_tokens: number;
  total_tokens: number;
}

export interface TokenDisplay {
  /** As the provider reported it, with nothing added. */
  input: number;
  /** As the provider reported it, with nothing added. */
  output: number;
  /** The part of the total that input and output do not account for; 0 under an inclusive convention. */
  separate: number;
  /** Which counters account for `separate`, for the popover's label. Empty when `separate` is 0. */
  separateOf: string[];
  total: number;
}

export function tokenDisplay(counters: TokenCounters): TokenDisplay {
  const {
    input_tokens,
    output_tokens,
    cache_read_tokens,
    cache_write_tokens,
    reasoning_tokens,
    total_tokens,
  } = counters;

  // A total of 0 means nothing was reported rather than "free", so fall back to the two sides.
  const total = total_tokens || input_tokens + output_tokens;
  const separate = Math.max(0, total - input_tokens - output_tokens);

  // Named from whichever counters are actually present. Under a separate-cache convention the residual is
  // the cache counters; under a separate-reasoning one it is the thoughts. Both listed if both are set,
  // since a mixed-provider trace can hold both and the residual is then their combined contribution.
  const separateOf: string[] = [];
  if (separate > 0) {
    if (cache_read_tokens > 0) separateOf.push("cache read");
    if (cache_write_tokens > 0) separateOf.push("cache write");
    if (reasoning_tokens > 0) separateOf.push("reasoning");
  }

  return { input: input_tokens, output: output_tokens, separate, separateOf, total };
}
