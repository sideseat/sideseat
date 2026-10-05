import { Agent, type BedrockModel } from '@strands-agents/sdk';
import { prompts, SYSTEM } from '../../harness/content.js';
import type { Run } from '../../harness/run.js';
import { getWeather } from '../tools.js';

export async function run(run: Run<BedrockModel>): Promise<void> {
  const agent = new Agent({
    model: run.llm,
    systemPrompt: SYSTEM,
    tools: [getWeather],
    printer: false,
  });
  await run.trace(async () => {
    for await (const event of agent.stream(prompts.streaming)) {
      if (
        event.type === 'modelStreamUpdateEvent' &&
        event.event.type === 'modelContentBlockDeltaEvent' &&
        event.event.delta.type === 'textDelta'
      ) {
        process.stdout.write(event.event.delta.text);
      }
    }
    console.log();
  });
}
