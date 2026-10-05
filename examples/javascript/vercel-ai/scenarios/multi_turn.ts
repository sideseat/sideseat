import { generateText, type LanguageModel, type ModelMessage } from 'ai';
import { prompts, SYSTEM } from '../../harness/content.js';
import type { Run } from '../../harness/run.js';

export async function run(run: Run<LanguageModel>): Promise<void> {
  // The application keeps the conversation and re-sends it with every question.
  const messages: ModelMessage[] = [];
  await run.trace(async () => {
    for (const question of prompts.multi_turn) {
      messages.push({ role: 'user', content: question });
      const { text, response } = await generateText({ model: run.llm, system: SYSTEM, messages });
      messages.push(...response.messages);
      console.log(text);
    }
  });
}
