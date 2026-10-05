/** Maps a harness model alias to an AI SDK model on Amazon Bedrock. */
import { createAmazonBedrock } from '@ai-sdk/amazon-bedrock';
import type { LanguageModel } from 'ai';
import {
  modelProxy,
  PROXY_CREDENTIALS,
  region,
  requireSurface,
  type Model,
} from '../harness/models.js';

export function build(model: Model): LanguageModel {
  requireSurface(model, ['bedrock'], 'Vercel AI SDK');
  const proxy = modelProxy();
  const bedrock = createAmazonBedrock({
    region: region(),
    ...(proxy && { baseURL: proxy, ...PROXY_CREDENTIALS }),
  });
  return bedrock(model.id);
}

/** Visible reasoning: current Claude models think by default and omit the text unless asked. */
export const REASONING = {
  bedrock: {
    reasoningConfig: { type: 'adaptive', display: 'summarized', maxReasoningEffort: 'max' },
  },
} as const;
