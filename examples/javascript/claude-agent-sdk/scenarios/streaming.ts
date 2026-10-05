import { query } from '@anthropic-ai/claude-agent-sdk';
import { prompts } from '../../harness/content.js';
import type { Run } from '../../harness/run.js';
import { options } from '../agent.js';
import type { ClaudeModel } from '../models.js';
import { getWeather, server } from '../tools.js';

export async function run(run: Run<ClaudeModel>): Promise<void> {
  const settings = options(run, { ...server(getWeather), includePartialMessages: true });
  await run.trace(async () => {
    for await (const message of query({ prompt: prompts.streaming, options: settings })) {
      if (message.type === 'stream_event') {
        const { event } = message;
        if (event.type === 'content_block_delta' && event.delta.type === 'text_delta') {
          process.stdout.write(event.delta.text);
        }
      } else if (message.type === 'result' && message.is_error) {
        throw new Error(`the agent run did not succeed: ${JSON.stringify(message)}`);
      }
    }
    console.log();
  });
}
