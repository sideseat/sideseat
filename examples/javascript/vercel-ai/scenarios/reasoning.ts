import { generateText, type LanguageModel } from 'ai';
import { prompts } from '../../harness/content.js';
import type { Run } from '../../harness/run.js';
import { REASONING } from '../models.js';

export async function run(run: Run<LanguageModel>): Promise<void> {
  await run.trace(async () => {
    const { text, reasoningText } = await generateText({
      model: run.llm,
      prompt: prompts.reasoning,
      maxOutputTokens: 16_000,
      providerOptions: REASONING,
    });
    console.log(`[thinking] ${reasoningText?.slice(0, 200)}`);
    console.log(text);
  });
}
