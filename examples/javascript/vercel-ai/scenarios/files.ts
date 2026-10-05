import { generateText, type LanguageModel } from 'ai';
import { prompts, SYSTEM } from '../../harness/content.js';
import { Run } from '../../harness/run.js';

export async function run(run: Run<LanguageModel>): Promise<void> {
  await run.trace(async () => {
    const { text } = await generateText({
      model: run.llm,
      system: SYSTEM,
      messages: [
        {
          role: 'user',
          content: [
            { type: 'text', text: prompts.files },
            { type: 'image', image: Run.asset('img.jpg'), mediaType: 'image/jpeg' },
            {
              type: 'file',
              data: Run.asset('task.pdf'),
              mediaType: 'application/pdf',
              filename: 'task.pdf',
            },
          ],
        },
      ],
    });
    console.log(text);
  });
}
