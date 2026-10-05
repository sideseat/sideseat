import { generateText, type LanguageModel } from 'ai';
import { prompts, SYSTEM } from '../../harness/content.js';
import type { Run } from '../../harness/run.js';

export async function run(run: Run<LanguageModel>): Promise<void> {
  await run.trace(async () => {
    const { text } = await generateText({ model: run.llm, system: SYSTEM, prompt: prompts.chat });
    console.log(text);
  });
}
