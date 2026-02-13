/**
 * MCP server integration sample.
 */

import { fileURLToPath } from 'url';
import * as path from 'path';
import * as fs from 'fs';
import { Agent, McpClient } from '@strands-agents/sdk';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';
import { resolveModel } from '../../shared/config.js';

// Resolve paths relative to this file
const __dirname = path.dirname(fileURLToPath(import.meta.url));
const MCP_SERVER = path.resolve(__dirname, '../../../../../mcp/calculator.py');

export async function run(modelId: string) {
  if (!fs.existsSync(MCP_SERVER)) {
    throw new Error(`MCP server not found: ${MCP_SERVER}. Run from misc/samples/js directory.`);
  }

  const calculatorTools = new McpClient({
    transport: new StdioClientTransport({ command: 'python', args: [MCP_SERVER] }),
  });

  try {
    const agent = new Agent({
      model: resolveModel(modelId),
      tools: [calculatorTools],
      systemPrompt: 'You help users to calculate expressions.',
    });

    const result = await agent.invoke('Calculate an expression for me: What is 12345 plus 6789?');
    console.log(result);
  } finally {
    await calculatorTools.disconnect();
  }
}
