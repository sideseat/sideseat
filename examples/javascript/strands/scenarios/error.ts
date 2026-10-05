import { Agent, type BedrockModel } from '@strands-agents/sdk';
import { prompts, SYSTEM } from '../../harness/content.js';
import type { Run } from '../../harness/run.js';
import { bookFlight } from '../tools.js';

export async function run(run: Run<BedrockModel>): Promise<void> {
  const agent = new Agent({
    model: run.llm,
    systemPrompt: SYSTEM,
    tools: [bookFlight],
    printer: false,
  });
  await run.trace(async () => console.log(String(await agent.invoke(prompts.error))));
}
