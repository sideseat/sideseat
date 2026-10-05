/** Maps a harness model alias to a Strands model. */
import { BedrockModel } from '@strands-agents/sdk';
import {
  awsClientConfig,
  region,
  REQUEST_TIMEOUT_MS,
  requireSurface,
  type Model,
} from '../harness/models.js';

export function build(model: Model, { reasoning = false } = {}): BedrockModel {
  requireSurface(model, ['bedrock'], 'Strands');
  const clientConfig = awsClientConfig();
  return new BedrockModel({
    modelId: model.id,
    region: region(),
    maxTokens: 16_000,
    // Current Claude models think by default and omit the reasoning text; the reasoning scenario
    // asks for a summary so the telemetry carries visible reasoning.
    ...(reasoning && {
      additionalRequestFields: {
        thinking: { type: 'adaptive', display: 'summarized' },
        output_config: { effort: 'max' },
      },
    }),
    ...(clientConfig ? { clientConfig } : { requestTimeout: REQUEST_TIMEOUT_MS }),
  });
}
