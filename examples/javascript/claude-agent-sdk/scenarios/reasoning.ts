import { query } from '@anthropic-ai/claude-agent-sdk';
import { prompts } from '../../harness/content.js';
import type { Run } from '../../harness/run.js';
import { options, show } from '../agent.js';
import type { ClaudeModel } from '../models.js';

export async function run(run: Run<ClaudeModel>): Promise<void> {
  // Current Claude models think by default and omit the text; a summary makes it visible.
  const settings = options(run, {
    systemPrompt: undefined,
    maxTurns: 1,
    thinking: { type: 'adaptive', display: 'summarized' },
    effort: 'max',
  });
  await run.trace(() => show(query({ prompt: prompts.reasoning, options: settings })));
}
