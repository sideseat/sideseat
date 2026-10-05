import { Agent, type BedrockModel } from '@strands-agents/sdk';
import { prompts, SYSTEM } from '../../harness/content.js';
import type { Run } from '../../harness/run.js';

export async function run(run: Run<BedrockModel>): Promise<void> {
  const agent = new Agent({ model: run.llm, systemPrompt: SYSTEM, printer: false });
  await run.trace(async () => {
    for (const question of prompts.multi_turn) console.log(String(await agent.invoke(question)));
  });
}
