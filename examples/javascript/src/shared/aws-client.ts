import { BedrockRuntimeClient } from '@aws-sdk/client-bedrock-runtime';
import { config } from './config.js';

let client: BedrockRuntimeClient | null = null;

export const getBedrockClient = (): BedrockRuntimeClient =>
  client ?? (client = new BedrockRuntimeClient({ region: config.awsRegion }));
