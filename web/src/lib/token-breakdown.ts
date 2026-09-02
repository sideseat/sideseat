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

  // Named only when the arithmetic identifies which counters make up the residual, and left unnamed
  // otherwise. Listing every non-zero counter was wrong in a way that matters: Gemini reports cached
  // content *inside* its prompt total and thoughts beside its output, so a call with cache-read 400 and
  // reasoning 300 leaves a residual of exactly 300 - and naming both told the user 400 cached tokens had
  // been billed on top when they had not. An unlabelled residual is a gap in the explanation; a wrongly
  // labelled one is a false statement about the bill.
  const separateOf = attributeResidual(separate, [
    ["cache read", cache_read_tokens],
    ["cache write", cache_write_tokens],
    ["reasoning", reasoning_tokens],
  ]);

  return { input: input_tokens, output: output_tokens, separate, separateOf, total };
}

/**
 * Which counters sum to the residual, when exactly one combination does.
 *
 * Three counters, so seven non-empty subsets - cheap to enumerate exactly rather than guess. A unique
 * match is proof: those counters are the ones reported beside the two sides. Two or more matching
 * combinations mean the numbers do not say which, and the honest answer is to name none of them.
 */
function attributeResidual(residual: number, counters: [string, number][]): string[] {
  if (residual <= 0) return [];

  const present = counters.filter(([, value]) => value > 0);
  let unique: string[] | null = null;
  for (let mask = 1; mask < 1 << present.length; mask++) {
    const subset = present.filter((_, i) => (mask >> i) & 1);
    const sum = subset.reduce((acc, [, value]) => acc + value, 0);
    if (sum !== residual) continue;
    if (unique !== null) return []; // ambiguous: two combinations explain the same residual
    unique = subset.map(([label]) => label);
  }
  return unique ?? [];
}
