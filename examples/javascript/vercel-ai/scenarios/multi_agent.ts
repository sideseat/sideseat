import { Experimental_Agent as Agent, stepCountIs, type LanguageModel } from 'ai';
import { prompts } from '../../harness/content.js';
import type { Run } from '../../harness/run.js';
import { get_weather } from '../tools.js';

export async function run(run: Run<LanguageModel>): Promise<void> {
  const researcher = new Agent({
    model: run.llm,
    instructions: 'You research weather with the tool and report the forecast.',
    tools: { get_weather },
    stopWhen: stepCountIs(4),
  });
  const writer = new Agent({
    model: run.llm,
    instructions: "You write the final packing list from the researcher's findings.",
  });
  // The researcher's report is the writer's brief: one agent hands its work to the next.
  await run.trace(async () => {
    const research = await researcher.generate({ prompt: prompts.multi_agent });
    const { text } = await writer.generate({
      prompt: `${prompts.multi_agent}\n\nResearch:\n${research.text}`,
    });
    console.log(text);
  });
}
