/**
 * Error sample — queries agent with nonexistent model ID to generate error telemetry.
 */

import { Agent } from '@strands-agents/sdk';
import { resolveModel } from '../../shared/config.js';

const INVALID_MODEL_ID = 'nonexistent-model-id-12345';

export async function run() {
  const agent = new Agent({
    model: resolveModel(INVALID_MODEL_ID),
    systemPrompt: 'You are a helpful assistant.',
  });

  await agent.invoke('What is 2 + 2?');
}
