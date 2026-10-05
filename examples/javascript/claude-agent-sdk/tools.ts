/** The shared example tools as an in-process MCP server, the Agent SDK's way to add tools. */
import { createSdkMcpServer, tool, type McpServerConfig } from '@anthropic-ai/claude-agent-sdk';
import { z } from 'zod';
import { resultText, toolSpec, tools, type ToolName } from '../harness/content.js';

const SERVER = 'travel';

const ZOD = {
  string: () => z.string(),
  integer: () => z.number().int(),
  number: () => z.number(),
  boolean: () => z.boolean(),
} as const;

/** The shared tool `name`, its input shape derived from the shared JSON schema. */
function shared(name: ToolName) {
  const { description, parameters } = toolSpec(name);
  const shape = Object.fromEntries(
    Object.entries(parameters.properties).map(([key, property]) => {
      const base = ZOD[property.type]().describe(property.description ?? key);
      return [key, parameters.required.includes(key) ? base : base.optional()];
    })
  );
  const run = tools[name] as (input: unknown) => unknown;
  // A thrown error reaches the model as an error result, which the error scenario relies on.
  return tool(name, description, shape, async (args) => ({
    content: [{ type: 'text' as const, text: resultText(run(args)) }],
  }));
}

export const getWeather = shared('get_weather');
export const getPrecipitation = shared('get_precipitation');
export const bookFlight = shared('book_flight');

/** `mcpServers` and `allowedTools` values that expose `tools` to the agent. */
export function server(...served: ReturnType<typeof shared>[]): {
  mcpServers: Record<string, McpServerConfig>;
  allowedTools: string[];
} {
  return {
    mcpServers: { [SERVER]: createSdkMcpServer({ name: SERVER, tools: served }) },
    allowedTools: served.map((t) => `mcp__${SERVER}__${t.name}`),
  };
}
