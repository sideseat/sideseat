/**
 * The model aliases every suite accepts with `--model`: the Python harness's catalog
 * (`harness/models.py`), read through `content.json`.
 */
import { NodeHttpHandler } from '@smithy/node-http-handler';
import { models, DEFAULT_MODEL, type ModelEntry } from './content.js';

export interface Model extends ModelEntry {
  alias: string;
}

export { DEFAULT_MODEL };

export const MODELS: Model[] = Object.entries(models).map(([alias, entry]) => ({
  alias,
  ...entry,
}));

export function resolve(alias: string): Model {
  const found = MODELS.find((model) => model.alias === alias);
  if (!found) {
    const known = MODELS.map((model) => model.alias).join(', ');
    throw new UsageError(`unknown model ${JSON.stringify(alias)}; choose one of: ${known}`);
  }
  return found;
}

export function region(): string {
  return process.env.AWS_REGION || process.env.AWS_DEFAULT_REGION || 'us-east-1';
}

/**
 * The capture tool's recording proxy, when one stands in front of bedrock-runtime.
 *
 * The proxy signs each request it forwards with the ambient credentials and replays recorded
 * answers without any, so a client pointed at it needs no credentials of its own.
 */
export function modelProxy(): string | undefined {
  return process.env.SIDESEAT_MODEL_PROXY || undefined;
}

/** Credentials for a client that talks to the proxy, which discards the client's signature. */
export const PROXY_CREDENTIALS = { accessKeyId: 'proxy', secretAccessKey: 'proxy' } as const;

/**
 * AWS SDK client settings that send bedrock-runtime calls to the capture proxy, or none without one.
 * The proxy speaks HTTP/1.1, so the client's default HTTP/2 handler is replaced.
 */
export function awsClientConfig() {
  const proxy = modelProxy();
  if (!proxy) return undefined;
  return {
    endpoint: proxy,
    credentials: PROXY_CREDENTIALS,
    requestHandler: new NodeHttpHandler({ requestTimeout: REQUEST_TIMEOUT_MS }),
  };
}

/**
 * How long one model request may take. A summarised reasoning request at maximum effort can run for
 * minutes, and a client that times out and retries records a second, different answer.
 */
export const REQUEST_TIMEOUT_MS = 600_000;

/** A mistake in the command line, reported without a stack trace. */
export class UsageError extends Error {
  override name = 'UsageError';
}

export function requireSurface(model: Model, surfaces: string[], suite: string): void {
  if (!surfaces.includes(model.surface)) {
    throw new UsageError(
      `the ${suite} suite runs ${surfaces.join(' or ')} models; ${model.alias} is ${model.surface}`
    );
  }
}
