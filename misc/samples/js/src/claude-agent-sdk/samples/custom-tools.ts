/**
 * In-process custom tools via an SDK MCP server.
 *
 * Demonstrates tool() with a Zod schema, createSdkMcpServer (which runs in this
 * process - no subprocess, no stdio), and ToolAnnotations hints.
 *
 * Unlike samples/mcp-tools.ts, no separate server process is spawned: the handlers
 * below execute inside the sample and are reached over an in-process transport.
 */
import { createSdkMcpServer, query, tool } from '@anthropic-ai/claude-agent-sdk';
import { z } from 'zod';
import { buildOptions, printStream } from '../helpers.js';

const INVENTORY: Record<string, number> = { widget: 12, gasket: 4, flange: 0 };

const QUERIES = [
  'How many widgets and flanges are in stock?',
  'Restock flanges with 25 units, then tell me the new count.',
];

/**
 * Resolve a part name to an inventory key.
 *
 * The model naturally pluralizes ("widgets"), while the keys are singular, so match
 * leniently instead of reporting a bogus "not a known part".
 */
function normalize(part: string): string {
  const name = part.trim().toLowerCase();
  return !(name in INVENTORY) && name.endsWith('s') ? name.slice(0, -1) : name;
}

const checkStock = tool(
  'check_stock',
  'Look up the number of units in stock for a part.',
  { part: z.string().describe('Part name') },
  async ({ part }) => {
    const key = normalize(part);
    const count = INVENTORY[key];
    const text = count === undefined ? `${key}: not a known part` : `${key}: ${count} in stock`;
    return { content: [{ type: 'text' as const, text }] };
  },
  { annotations: { readOnlyHint: true } }
);

const restock = tool(
  'restock',
  "Add units to a part's stock level and return the new total.",
  {
    part: z.string().describe('Part name'),
    count: z.number().describe('Units to add'),
  },
  async ({ part, count }) => {
    const key = normalize(part);
    INVENTORY[key] = (INVENTORY[key] ?? 0) + count;
    return {
      content: [{ type: 'text' as const, text: `${key}: now ${INVENTORY[key]} in stock` }],
    };
  }
);

export async function run(modelId: string, env: Record<string, string>) {
  const inventoryServer = createSdkMcpServer({
    name: 'inventory',
    version: '1.0.0',
    tools: [checkStock, restock],
  });

  const options = buildOptions(modelId, env, {
    mcpServers: { inventory: inventoryServer },
    strictMcpConfig: true,
    allowedTools: ['mcp__inventory__check_stock', 'mcp__inventory__restock'],
    // Without this the model sometimes shells out instead of calling the tools,
    // which defeats the point of the sample.
    disallowedTools: ['Bash'],
    systemPrompt: 'You are a warehouse assistant. Use the inventory tools for all lookups.',
  });

  for (const prompt of QUERIES) {
    console.log(`--- ${prompt} ---`);
    await printStream(query({ prompt, options }));
    console.log();
  }
}
