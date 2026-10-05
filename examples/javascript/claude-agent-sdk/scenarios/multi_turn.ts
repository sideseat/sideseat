import { query, type SDKUserMessage } from '@anthropic-ai/claude-agent-sdk';
import { prompts } from '../../harness/content.js';
import type { Run } from '../../harness/run.js';
import { options, show } from '../agent.js';
import type { ClaudeModel } from '../models.js';

export async function run(run: Run<ClaudeModel>): Promise<void> {
  // Streaming input keeps one CLI session open: each question is a new turn of it, sent once the
  // previous turn has its result.
  let answered: () => void = () => {};
  async function* questions(): AsyncGenerator<SDKUserMessage> {
    for (const question of prompts.multi_turn) {
      const turn = new Promise<void>((resolve) => (answered = resolve));
      yield {
        type: 'user',
        message: { role: 'user', content: question },
        parent_tool_use_id: null,
      };
      await turn;
    }
  }
  await run.trace(() =>
    show(query({ prompt: questions(), options: options(run) }), { until: () => answered() })
  );
}
