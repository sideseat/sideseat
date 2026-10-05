import { Agent, type BedrockModel } from '@strands-agents/sdk';
import { prompts, SYSTEM } from '../../harness/content.js';
import type { Run } from '../../harness/run.js';
import { tripPlan } from '../../harness/schemas.js';

export async function run(run: Run<BedrockModel>): Promise<void> {
  const agent = new Agent({
    model: run.llm,
    systemPrompt: SYSTEM,
    structuredOutputSchema: tripPlan(),
    printer: false,
  });
  await run.trace(async () => {
    const result = await agent.invoke(prompts.structured);
    console.log(result.structuredOutput);
  });
}
