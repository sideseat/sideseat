import { Agent, type BedrockModel } from '@strands-agents/sdk';
import { prompts, SYSTEM } from '../../harness/content.js';
import type { Run } from '../../harness/run.js';

export async function run(run: Run<BedrockModel>): Promise<void> {
  // Two conversations, each its own trace, both attributed to one session and user.
  for (const [index, question] of prompts.session.entries()) {
    const agent = new Agent({ model: run.llm, systemPrompt: SYSTEM, printer: false });
    await run.trace(`session-turn-${index + 1}`, async () =>
      console.log(String(await agent.invoke(question)))
    );
  }
}
