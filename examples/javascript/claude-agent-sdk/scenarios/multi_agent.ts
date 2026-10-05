import { query } from '@anthropic-ai/claude-agent-sdk';
import { prompts } from '../../harness/content.js';
import type { Run } from '../../harness/run.js';
import { options, show } from '../agent.js';
import type { ClaudeModel } from '../models.js';
import { getWeather, server } from '../tools.js';

export async function run(run: Run<ClaudeModel>): Promise<void> {
  const { mcpServers, allowedTools: weather } = server(getWeather);
  const settings = options(run, {
    systemPrompt:
      'You coordinate two agents: ask the researcher for the weather, then the writer for the ' +
      "packing list, and return the writer's list.",
    tools: ['Agent'],
    allowedTools: ['Agent', ...weather],
    mcpServers,
    agents: {
      researcher: {
        description: 'Researches the weather for a city with the weather tool.',
        prompt: 'You research weather with the tool and report the forecast.',
        tools: weather,
      },
      writer: {
        description: 'Writes a packing list from a weather report.',
        prompt: "You write the final packing list from the researcher's findings.",
        tools: [],
      },
    },
  });
  await run.trace(() => show(query({ prompt: prompts.multi_agent, options: settings })));
}
