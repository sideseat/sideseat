import { Agent, type BedrockModel } from '@strands-agents/sdk';
import { prompts } from '../../harness/content.js';
import type { Run } from '../../harness/run.js';
import { build } from '../models.js';

export async function run(run: Run<BedrockModel>): Promise<void> {
  const agent = new Agent({ model: build(run.model, { reasoning: true }), printer: false });
  await run.trace(async () => console.log(String(await agent.invoke(prompts.reasoning))));
}
