import { generateText, stepCountIs, type LanguageModel } from 'ai';
import { prompts, SYSTEM } from '../../harness/content.js';
import type { Run } from '../../harness/run.js';
import { get_precipitation, get_weather } from '../tools.js';

export async function run(run: Run<LanguageModel>): Promise<void> {
  await run.trace(async () => {
    const { text } = await generateText({
      model: run.llm,
      system: SYSTEM,
      prompt: prompts.tool_use,
      tools: { get_weather, get_precipitation },
      stopWhen: stepCountIs(8),
    });
    console.log(text);
  });
}
