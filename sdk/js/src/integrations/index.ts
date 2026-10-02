import { IntegrationError } from "../errors.js";
import { claudeAgentSDK } from "./claude-agent-sdk.js";
import { firstInstalled } from "./packages.js";
import { strands } from "./strands.js";
import type { Integration } from "./types.js";
import { vercelAI } from "./vercel-ai.js";

/** Every built-in integration, in auto-detection priority order. */
const REGISTRY: readonly Integration[] = [strands, claudeAgentSDK, vercelAI];

export function integrationNames(): string[] {
  return REGISTRY.map((integration) => integration.name);
}

export function loadIntegration(name: string): Integration {
  const found = REGISTRY.find((integration) => integration.name === name);
  if (!found) {
    throw new IntegrationError(
      `unknown integration ${JSON.stringify(name)}; known integrations: ${integrationNames().join(", ")}`,
    );
  }
  return found;
}

/** Integrations for `spec`, and whether they were requested explicitly. `undefined` detects one. */
export function resolveIntegrations(
  spec: ReadonlyArray<string | Integration> | undefined,
): [Integration[], boolean] {
  if (spec === undefined) {
    const detected = REGISTRY.find(
      (i) => i.detectable && firstInstalled(i.packages) !== undefined,
    );
    return [detected ? [detected] : [], false];
  }
  const seen = new Set<string>();
  const result: Integration[] = [];
  for (const item of spec) {
    const integration = typeof item === "string" ? loadIntegration(item) : item;
    if (!seen.has(integration.name)) {
      seen.add(integration.name);
      result.push(integration);
    }
  }
  return [result, true];
}

export { claudeAgentSDK, cliEnvironment } from "./claude-agent-sdk.js";
export { strands } from "./strands.js";
export type { Integration, SetupContext } from "./types.js";
export { vercelAI } from "./vercel-ai.js";
