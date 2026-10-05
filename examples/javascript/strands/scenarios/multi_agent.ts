import { Agent, Swarm, type BedrockModel } from '@strands-agents/sdk';
import { prompts } from '../../harness/content.js';
import type { Run } from '../../harness/run.js';
import { getWeather } from '../tools.js';

export async function run(run: Run<BedrockModel>): Promise<void> {
  const researcher = new Agent({
    id: 'researcher',
    name: 'researcher',
    description: 'Researches the weather with the weather tool.',
    model: run.llm,
    systemPrompt: 'You research weather with the tool, then hand off to the writer.',
    tools: [getWeather],
    printer: false,
  });
  const writer = new Agent({
    id: 'writer',
    name: 'writer',
    description: "Writes the packing list from the researcher's findings.",
    model: run.llm,
    systemPrompt: "You write the final packing list from the researcher's findings.",
    printer: false,
  });
  const swarm = new Swarm({ nodes: [researcher, writer], start: 'researcher', maxSteps: 4 });
  await run.trace(async () => console.log(String(await swarm.invoke(prompts.multi_agent))));
}
