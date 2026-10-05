import { generateText, stepCountIs, type LanguageModel } from 'ai';
import { prompts, SYSTEM } from '../../harness/content.js';
import type { Run } from '../../harness/run.js';
import { book_flight } from '../tools.js';

export async function run(run: Run<LanguageModel>): Promise<void> {
  // A tool that throws reaches the model as a tool error, which it reads and answers.
  await run.trace(async () => {
    const { text } = await generateText({
      model: run.llm,
      system: SYSTEM,
      prompt: prompts.error,
      tools: { book_flight },
      stopWhen: stepCountIs(8),
    });
    console.log(text);
  });
}
