import { generateText, type LanguageModel } from 'ai';
import { prompts, SYSTEM } from '../../harness/content.js';
import type { Run } from '../../harness/run.js';

export async function run(run: Run<LanguageModel>): Promise<void> {
  // Two conversations, each its own trace, both attributed to one session and user.
  for (const [index, question] of prompts.session.entries()) {
    await run.trace(`session-turn-${index + 1}`, async () => {
      const { text } = await generateText({ model: run.llm, system: SYSTEM, prompt: question });
      console.log(text);
    });
  }
}
