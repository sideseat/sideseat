import { query } from '@anthropic-ai/claude-agent-sdk';
import { prompts } from '../../harness/content.js';
import { mcpCalculatorCommand, type Run } from '../../harness/run.js';
import { options, show } from '../agent.js';
import type { ClaudeModel } from '../models.js';

export async function run(run: Run<ClaudeModel>): Promise<void> {
  const settings = options(run, {
    mcpServers: { calculator: { type: 'stdio', ...mcpCalculatorCommand() } },
    allowedTools: ['mcp__calculator'],
  });
  await run.trace(() => show(query({ prompt: prompts.mcp, options: settings })));
}
