// Load .env from misc/ directory (parent of misc/samples/js/)
import { fileURLToPath } from 'url';
import * as path from 'path';
import { config as dotenvConfig } from 'dotenv';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const miscDir = path.resolve(__dirname, '../../../..'); // misc/samples/js/src/shared -> misc/
dotenvConfig({ path: path.join(miscDir, '.env'), override: true, quiet: true });

export const DEFAULT_MODEL = 'bedrock-haiku';

export const MODEL_ALIASES = {
  // Use cross-region inference profiles (global.) for on-demand access
  'bedrock-haiku': 'global.anthropic.claude-haiku-4-5-20251001-v1:0',
  'bedrock-sonnet': 'global.anthropic.claude-sonnet-4-20250514-v1:0',
} as const;

// Models that support extended thinking (reasoning)
export const REASONING_MODELS = new Set(['bedrock-sonnet', 'bedrock-haiku']);

// Default budget_tokens for extended thinking (minimum is 1024)
export const DEFAULT_THINKING_BUDGET = 4096;

export const config = {
  awsRegion: process.env.AWS_REGION ?? 'us-east-1',
  sideseatEndpoint: process.env.SIDESEAT_ENDPOINT ?? 'http://127.0.0.1:5388',
  sideseatProjectId: process.env.SIDESEAT_PROJECT_ID ?? 'default',
  models: {
    embedding: process.env.EMBEDDING_MODEL ?? 'amazon.titan-embed-text-v2:0',
    imageGen: process.env.IMAGE_GEN_MODEL ?? 'amazon.titan-image-generator-v2:0',
  },
} as const;

export type ModelAlias = keyof typeof MODEL_ALIASES;

export const resolveModel = (alias: string): string => MODEL_ALIASES[alias as ModelAlias] ?? alias;
