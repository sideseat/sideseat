import { stepCountIs, streamText, type LanguageModel } from 'ai';
import { prompts, SYSTEM } from '../../harness/content.js';
import type { Run } from '../../harness/run.js';
import { get_weather } from '../tools.js';

export async function run(run: Run<LanguageModel>): Promise<void> {
  await run.trace(async () => {
    const result = streamText({
      model: run.llm,
      system: SYSTEM,
      prompt: prompts.streaming,
      tools: { get_weather },
      stopWhen: stepCountIs(8),
    });
    for await (const delta of result.textStream) process.stdout.write(delta);
    console.log();
  });
}
