import { query } from '@anthropic-ai/claude-agent-sdk';
import { prompts, TRIP_PLAN_SCHEMA } from '../../harness/content.js';
import type { Run } from '../../harness/run.js';
import { tripPlan } from '../../harness/schemas.js';
import { options, show } from '../agent.js';
import type { ClaudeModel } from '../models.js';

export async function run(run: Run<ClaudeModel>): Promise<void> {
  const settings = options(run, {
    outputFormat: { type: 'json_schema', schema: TRIP_PLAN_SCHEMA },
  });
  const result = await run.trace(() =>
    show(query({ prompt: prompts.structured, options: settings }))
  );
  if (result.subtype !== 'success') throw new Error(`no structured output: ${result.subtype}`);
  console.log(tripPlan().parse(result.structured_output));
}
