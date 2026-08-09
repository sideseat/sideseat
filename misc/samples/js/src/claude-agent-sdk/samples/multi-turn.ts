/**
 * Multi-turn conversation over one session.
 *
 * Demonstrates streaming input mode: a single query() call carries several user turns,
 * each emitting its own result message, and the session context is retained between
 * them. All turns share one session, so they group into a single timeline in SideSeat.
 *
 * In streaming input mode `usage` covers only that turn while total_cost_usd carries
 * the running total for the whole call, so the last result holds the call total.
 *
 * NOTE: the generator must wait for each turn's result before yielding the next.
 * Yielding all turns back to back ends the input stream before the later turns are
 * processed, and they are silently dropped.
 */
import { query, type SDKUserMessage } from '@anthropic-ai/claude-agent-sdk';
import { buildOptions, printResult } from '../helpers.js';

const TURNS = [
  'What is the capital of France?',
  'What is the population of that city?',
  'How does that compare to the city I first asked about?',
];

/** One-shot signal that tolerates being fired before it is awaited. */
function createGate() {
  let pending = 0;
  let waiter: (() => void) | null = null;
  return {
    signal(): void {
      if (waiter) {
        const resume = waiter;
        waiter = null;
        resume();
      } else {
        pending += 1;
      }
    },
    async wait(): Promise<void> {
      if (pending > 0) {
        pending -= 1;
        return;
      }
      await new Promise<void>((resolve) => {
        waiter = resolve;
      });
    },
  };
}

export async function run(modelId: string, env: Record<string, string>) {
  const options = buildOptions(modelId, env, {
    allowedTools: [],
    systemPrompt: 'You are a concise geography assistant. Answer in one sentence.',
  });

  const gate = createGate();

  async function* turns(): AsyncGenerator<SDKUserMessage> {
    for (const [index, prompt] of TURNS.entries()) {
      // Let the previous turn finish so its answer is in context.
      if (index > 0) await gate.wait();
      yield {
        type: 'user',
        message: { role: 'user', content: prompt },
        parent_tool_use_id: null,
        session_id: '',
      } as SDKUserMessage;
    }
  }

  let turn = 0;
  let lastTotal = 0;

  for await (const message of query({ prompt: turns(), options })) {
    if (message.type === 'assistant') {
      for (const block of message.message.content) {
        if (block.type === 'text') console.log(block.text);
      }
    } else if (message.type === 'result') {
      turn += 1;
      console.log(`--- Turn ${turn} complete ---`);
      printResult(message);
      // total_cost_usd is the running total for the call, not just this turn.
      lastTotal = message.total_cost_usd ?? lastTotal;
      console.log();
      gate.signal();
    }
  }

  console.log(`Turns completed: ${turn}/${TURNS.length}`);
  console.log(`Call total (estimate): $${lastTotal.toFixed(6)}`);
}
