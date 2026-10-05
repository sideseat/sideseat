import { query } from '@anthropic-ai/claude-agent-sdk';
import { prompts } from '../../harness/content.js';
import type { Run } from '../../harness/run.js';
import { options, show } from '../agent.js';
import type { ClaudeModel } from '../models.js';

export async function run(run: Run<ClaudeModel>): Promise<void> {
  // Two conversations, each its own trace, both attributed to one session and user.
  for (const [index, question] of prompts.session.entries()) {
    await run.trace(`session-turn-${index + 1}`, () =>
      show(query({ prompt: question, options: options(run, { maxTurns: 1 }) }))
    );
  }
}
