import { Agent, type BedrockModel } from '@strands-agents/sdk';
import { prompts, SYSTEM } from '../../harness/content.js';
import { Run } from '../../harness/run.js';

export async function run(run: Run<BedrockModel>): Promise<void> {
  const agent = new Agent({ model: run.llm, systemPrompt: SYSTEM, printer: false });
  const prompt = [
    { text: prompts.files },
    { image: { format: 'jpeg' as const, source: { bytes: new Uint8Array(Run.asset('img.jpg')) } } },
    {
      document: {
        format: 'pdf' as const,
        name: 'task',
        source: { bytes: new Uint8Array(Run.asset('task.pdf')) },
      },
    },
  ];
  await run.trace(async () => console.log(String(await agent.invoke(prompt))));
}
