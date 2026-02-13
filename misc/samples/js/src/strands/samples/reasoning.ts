/**
 * Extended thinking/reasoning sample demonstrating chain-of-thought capabilities.
 *
 * This sample shows how to:
 * 1. Enable extended thinking (reasoning) for supported models
 * 2. Use budget_tokens to control thinking depth
 * 3. Handle models that don't support extended thinking gracefully
 * 4. Extract and display thinking content from responses
 *
 * Supported models:
 * - Bedrock Claude (Sonnet 3.5+, Haiku): via additional_request_fields
 *
 * Note: Extended thinking requires specific model versions. Older models
 * will work normally but without visible reasoning steps.
 */

import { Agent, BedrockModel } from '@strands-agents/sdk';
import { resolveModel, config, DEFAULT_THINKING_BUDGET } from '../../shared/config.js';
import { extractTextFromResponse, extractThinkingFromResponse } from '../../shared/response.js';
import type { SampleOptions } from '../runner.js';

// Challenging problems that benefit from step-by-step reasoning
const REASONING_PROBLEMS = [
  {
    name: 'Logic Puzzle',
    prompt: `Solve this logic puzzle step by step:

Three friends (Alice, Bob, Carol) each have a different pet (cat, dog, fish)
and live in different colored houses (red, blue, green).

Clues:
1. Alice doesn't live in the red house
2. The person with the cat lives in the blue house
3. Bob doesn't have a fish
4. Carol lives in the red house
5. The person in the green house has a dog

Who has which pet and lives in which house?`,
  },
  {
    name: 'Math Problem',
    prompt: `A water tank has two pipes. Pipe A can fill the tank in 6 hours.
Pipe B can empty the tank in 8 hours. If both pipes are opened when the tank
is half full, how long will it take to fill the tank completely?

Show your reasoning step by step.`,
  },
  {
    name: 'Code Analysis',
    prompt: `Analyze this Python function and explain what it computes:

\`\`\`python
def mystery(n):
    if n <= 1:
        return n
    a, b = 0, 1
    for _ in range(2, n + 1):
        a, b = b, a + b
    return b
\`\`\`

What mathematical sequence does this implement? Prove your answer by tracing
through for n=7.`,
  },
];

const SYSTEM_PROMPT = `You are a precise analytical assistant that solves problems
using careful step-by-step reasoning. Always show your work and explain your
thought process clearly. When solving puzzles or problems:

1. First understand what is being asked
2. Identify the relevant information and constraints
3. Work through the problem systematically
4. Verify your answer against all given conditions
5. Present your final answer clearly`;

export async function run(modelId: string, options?: SampleOptions) {
  // Create model with thinking enabled if supported
  const resolvedModelId = resolveModel(modelId);
  let model: BedrockModel | string = resolvedModelId;

  if (options?.enableThinking) {
    model = new BedrockModel({
      modelId: resolvedModelId,
      region: config.awsRegion,
      additionalRequestFields: {
        thinking: {
          type: 'enabled',
          budget_tokens: options.thinkingBudget ?? DEFAULT_THINKING_BUDGET,
        },
      },
    });
  }

  const agent = new Agent({
    model,
    systemPrompt: SYSTEM_PROMPT,
  });

  console.log('Extended Thinking / Reasoning Sample');
  console.log('='.repeat(60));
  console.log();
  console.log('This sample demonstrates chain-of-thought reasoning.');
  console.log("For models that support it, you'll see the thinking process.");
  console.log();

  for (let i = 0; i < REASONING_PROBLEMS.length; i++) {
    const problem = REASONING_PROBLEMS[i];
    console.log(`\n${'='.repeat(60)}`);
    console.log(`Problem ${i + 1}: ${problem.name}`);
    console.log('-'.repeat(60));
    const promptPreview =
      problem.prompt.length > 200 ? problem.prompt.slice(0, 200) + '...' : problem.prompt;
    console.log(promptPreview);
    console.log('-'.repeat(60));

    const response = await agent.invoke(problem.prompt);

    // Try to extract thinking content
    const thinking = extractThinkingFromResponse(response);
    if (thinking) {
      console.log('\n[Thinking Process]');
      console.log('-'.repeat(40));
      // Truncate very long thinking for display
      if (thinking.length > 1000) {
        console.log(thinking.slice(0, 1000) + '\n... (truncated)');
      } else {
        console.log(thinking);
      }
      console.log('-'.repeat(40));
    }

    // Extract and display main response
    const answer = extractTextFromResponse(response);
    console.log('\n[Answer]');
    console.log(answer);
  }

  console.log(`\n${'='.repeat(60)}`);
  console.log('Reasoning sample complete.');
}
