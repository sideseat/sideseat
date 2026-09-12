/**
 * Extended thinking/reasoning sample.
 *
 * NOTE: As of @ai-sdk/amazon-bedrock v4.x, extended thinking support for Claude
 * models via Bedrock may be limited. The `providerOptions.bedrock.thinking`
 * configuration is passed but thinking blocks may not be extracted from the response.
 *
 * For full extended thinking support, consider using the direct Anthropic API
 * via @ai-sdk/anthropic provider instead.
 */

import { generateText } from 'ai';
import { bedrock } from '@ai-sdk/amazon-bedrock';
import { resolveModel, DEFAULT_THINKING_BUDGET } from '../../shared/config.js';

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

export async function run(modelId: string) {
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

    const { text, reasoning } = await generateText({
      model: bedrock(resolveModel(modelId)),
      system: SYSTEM_PROMPT,
      prompt: problem.prompt,
      experimental_telemetry: { isEnabled: true },
      providerOptions: {
        bedrock: {
          thinking: {
            type: 'enabled',
            budgetTokens: DEFAULT_THINKING_BUDGET,
          },
        },
      },
    });

    if (reasoning) {
      console.log('\n[Thinking Process]');
      console.log('-'.repeat(40));
      const reasoningText = String(reasoning);
      if (reasoningText.length > 1000) {
        console.log(reasoningText.slice(0, 1000) + '\n... (truncated)');
      } else {
        console.log(reasoningText);
      }
      console.log('-'.repeat(40));
    }

    console.log('\n[Answer]');
    console.log(text);
  }

  console.log(`\n${'='.repeat(60)}`);
  console.log('Reasoning sample complete.');
}
