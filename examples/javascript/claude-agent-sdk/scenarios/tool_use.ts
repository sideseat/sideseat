import { query } from '@anthropic-ai/claude-agent-sdk';
import { prompts } from '../../harness/content.js';
import type { Run } from '../../harness/run.js';
import { options, show } from '../agent.js';
import type { ClaudeModel } from '../models.js';
import { getPrecipitation, getWeather, server } from '../tools.js';

export async function run(run: Run<ClaudeModel>): Promise<void> {
  const settings = options(run, server(getWeather, getPrecipitation));
  await run.trace(() => show(query({ prompt: prompts.tool_use, options: settings })));
}
