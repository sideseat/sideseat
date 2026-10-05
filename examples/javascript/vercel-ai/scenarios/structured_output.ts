import { generateText, stepCountIs, type LanguageModel } from 'ai';
import { prompts, SYSTEM } from '../../harness/content.js';
import type { Run } from '../../harness/run.js';
import { planTool } from '../../harness/schemas.js';
import { jsonSchema, tool, type JSONSchema7 } from 'ai';

export async function run(run: Run<LanguageModel>): Promise<void> {
  // Bedrock rejects both native structured output and a forced tool choice for current Claude
  // models, so the schema is a tool the model chooses to hand its plan to.
  const plan = planTool();
  const trip_plan = tool({
    description: plan.description,
    inputSchema: jsonSchema(plan.parameters as JSONSchema7),
    execute: async (input) => plan.record(input),
  });
  await run.trace(async () => {
    await generateText({
      model: run.llm,
      system: `${SYSTEM} ${plan.instruction}`,
      prompt: prompts.structured,
      tools: { trip_plan },
      stopWhen: stepCountIs(4),
    });
  });
  console.log(plan.recorded());
}
