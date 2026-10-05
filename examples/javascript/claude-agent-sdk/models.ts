/** Maps a harness model alias to the Claude Code CLI's provider configuration. */
import { modelProxy, region, requireSurface, UsageError, type Model } from '../harness/models.js';

export interface ClaudeModel {
  id: string;
  /** Environment for the CLI: the provider, and this model in every model slot. */
  env: Record<string, string>;
}

export function build(model: Model): ClaudeModel {
  requireSurface(model, ['bedrock', 'bedrock-anthropic'], 'Claude Agent SDK');
  if (!model.id.includes('anthropic.')) {
    throw new UsageError(`the Claude Agent SDK runs Claude models; ${model.alias} is ${model.id}`);
  }
  const env: Record<string, string> = {
    CLAUDE_CODE_USE_BEDROCK: '1',
    AWS_REGION: region(),
    // Background requests (titles, summaries) use the small-model slot; pinning it to the same
    // model keeps every request on a model the account has enabled.
    ANTHROPIC_MODEL: model.id,
    ANTHROPIC_DEFAULT_HAIKU_MODEL: model.id,
    ANTHROPIC_SMALL_FAST_MODEL: model.id,
  };
  const proxy = modelProxy();
  if (proxy) {
    // The capture proxy signs for Bedrock itself, so the CLI sends it unsigned requests.
    env.ANTHROPIC_BEDROCK_BASE_URL = proxy;
    env.CLAUDE_CODE_SKIP_BEDROCK_AUTH = '1';
  }
  return { id: model.id, env };
}
