import { query } from '@anthropic-ai/claude-agent-sdk';
import { prompts } from '../../harness/content.js';
import type { Run } from '../../harness/run.js';
import { options, show } from '../agent.js';
import type { ClaudeModel } from '../models.js';

export async function run(run: Run<ClaudeModel>): Promise<void> {
  await run.trace(() =>
    show(query({ prompt: prompts.chat, options: options(run, { maxTurns: 1 }) }))
  );
}
