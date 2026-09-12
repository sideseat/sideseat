/**
 * Error sample — queries with nonexistent model ID to generate error telemetry.
 */

import { generateText } from 'ai';
import { bedrock } from '@ai-sdk/amazon-bedrock';

const INVALID_MODEL_ID = 'nonexistent-model-id-12345';

export async function run() {
  await generateText({
    model: bedrock(INVALID_MODEL_ID),
    prompt: 'What is 2 + 2?',
    experimental_telemetry: { isEnabled: true },
  });
}
