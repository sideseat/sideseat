/**
 * Extended thinking.
 *
 * Demonstrates the thinking config, ThinkingBlock output, and effort levels.
 *
 * The Agent SDK docs state the thinking config is not sent to Bedrock, but verified
 * against bedrock-haiku on 2026-08-25 thinking blocks do arrive. Treat Bedrock thinking
 * support as version-dependent rather than guaranteed: if a run produces no thinking
 * blocks, that is the documented behaviour reasserting itself, not a bug here.
 */
import { query } from '@anthropic-ai/claude-agent-sdk';
import { buildOptions, printStream } from '../helpers.js';

const PROBLEMS = [
  {
    name: 'logic-puzzle',
    prompt:
      'Three switches outside a room control three bulbs inside. You may flip ' +
      'switches freely but enter the room only once. How do you determine which ' +
      'switch controls which bulb?',
  },
  {
    name: 'arithmetic',
    prompt:
      'A train leaves at 14:20 travelling 80 km/h. Another leaves the same station ' +
      'at 15:05 travelling 110 km/h on the same track. When does the second catch ' +
      'the first?',
  },
];

export async function run(modelId: string, env: Record<string, string>) {
  const options = buildOptions(modelId, env, {
    thinking: { type: 'adaptive', display: 'summarized' },
    effort: 'high',
    // No tools needed; this is pure reasoning.
    allowedTools: [],
    systemPrompt: 'Think the problem through, then give a short final answer.',
  });

  for (const { name, prompt } of PROBLEMS) {
    console.log(`--- ${name} ---`);
    await printStream(query({ prompt, options }), true);
    console.log();
  }
}
