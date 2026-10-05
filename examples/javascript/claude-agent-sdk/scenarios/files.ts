import { query, type SDKUserMessage } from '@anthropic-ai/claude-agent-sdk';
import { prompts } from '../../harness/content.js';
import { Run } from '../../harness/run.js';
import { options, show } from '../agent.js';
import type { ClaudeModel } from '../models.js';

const base64 = (name: string) => Run.asset(name).toString('base64');

export async function run(run: Run<ClaudeModel>): Promise<void> {
  // Content blocks reach the CLI only through streaming input: an iterable of user messages.
  async function* prompt(): AsyncGenerator<SDKUserMessage> {
    yield {
      type: 'user',
      message: {
        role: 'user',
        content: [
          { type: 'text', text: prompts.files },
          {
            type: 'image',
            source: { type: 'base64', media_type: 'image/jpeg', data: base64('img.jpg') },
          },
          {
            type: 'document',
            source: { type: 'base64', media_type: 'application/pdf', data: base64('task.pdf') },
          },
        ],
      },
      parent_tool_use_id: null,
    };
  }
  await run.trace(() => show(query({ prompt: prompt(), options: options(run, { maxTurns: 1 }) })));
}
