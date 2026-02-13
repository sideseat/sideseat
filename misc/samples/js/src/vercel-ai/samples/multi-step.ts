/**
 * Multi-step agentic loop sample.
 */

import { generateText, tool, stepCountIs } from 'ai';
import { bedrock } from '@ai-sdk/amazon-bedrock';
import { z } from 'zod';
import { resolveModel } from '../../shared/config.js';

const calculator = tool({
  description: 'Perform basic arithmetic operations.',
  inputSchema: z.object({
    operation: z
      .enum(['add', 'subtract', 'multiply', 'divide'])
      .describe('The operation to perform'),
    a: z.number().describe('First number'),
    b: z.number().describe('Second number'),
  }),
  execute: async ({ operation, a, b }) => {
    const operations: Record<string, (x: number, y: number) => number> = {
      add: (x, y) => x + y,
      subtract: (x, y) => x - y,
      multiply: (x, y) => x * y,
      divide: (x, y) => (y !== 0 ? x / y : Infinity),
    };
    const result = operations[operation]?.(a, b) ?? 0;
    return { operation, a, b, result };
  },
});

// Test problems
const PROBLEMS = [
  'Calculate step by step: What is (123 + 456) * 2?',
  'What is 15% of 240?',
  'Calculate the area of a circle with radius 5 (use 3.14159 for pi)',
];

export async function run(modelId: string) {
  console.log('Multi-step Agentic Loop Sample');
  console.log('='.repeat(60));

  for (let i = 0; i < PROBLEMS.length; i++) {
    const problem = PROBLEMS[i];
    console.log(`\n${'='.repeat(60)}`);
    console.log(`Problem ${i + 1}: ${problem}`);
    console.log('-'.repeat(60));

    const { text, steps } = await generateText({
      model: bedrock(resolveModel(modelId)),
      tools: { calculator },
      stopWhen: stepCountIs(10),
      prompt: problem,
      experimental_telemetry: { isEnabled: true },
    });

    console.log(`Result: ${text}`);
    console.log(`Steps taken: ${steps.length}`);

    for (const step of steps) {
      if (step.toolCalls?.length) {
        const call = step.toolCalls[0];
        console.log('  Tool call:', call.toolName, call.input);
      }
    }
  }
}
