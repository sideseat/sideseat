/**
 * Error telemetry.
 *
 * Queries with a nonexistent model ID so the CLI's API error surfaces on the span.
 * Mirrors the error sample in every other suite, so `-- all` intentionally reports a
 * failure here.
 */
import { query } from '@anthropic-ai/claude-agent-sdk';
import { buildOptions, printStream } from '../helpers.js';

const INVALID_MODEL_ID = 'nonexistent-model-id-12345';

export async function run(_modelId: string, env: Record<string, string>) {
  // Override both the primary and background model pins that buildClaudeCodeEnv
  // applied, otherwise the valid pin would mask the failure.
  const errorEnv = {
    ...env,
    ANTHROPIC_MODEL: INVALID_MODEL_ID,
    ANTHROPIC_DEFAULT_HAIKU_MODEL: INVALID_MODEL_ID,
  };

  const options = buildOptions(INVALID_MODEL_ID, errorEnv, {
    allowedTools: [],
    maxTurns: 1,
  });

  await printStream(query({ prompt: 'What is 2 + 2?', options }));
}
