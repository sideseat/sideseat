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
const MCP_SERVER_DIR = path.resolve(__dirname, '../../../../../mcp');

export async function run(modelId: string) {
  if (!fs.existsSync(path.join(MCP_SERVER_DIR, 'calculator.py'))) {
    throw new Error(
      `MCP server not found in ${MCP_SERVER_DIR}. Run from misc/samples/js directory.`
    );
  }

  const calculatorTools = new McpClient({
    // Launch via uv, matching the claude-agent-sdk suite: misc/mcp has its own venv with
    // fastmcp, so a bare `python` would fail to import it - and on macOS the `python`
    // command does not exist at all, only `python3`.
    transport: new StdioClientTransport({
      command: 'uv',
      args: ['run', '--directory', MCP_SERVER_DIR, 'mcp-calculator'],
    }),
  });

  try {
    const agent = new Agent({
      model: resolveModel(modelId),
      tools: [calculatorTools],
      printer: false,
      systemPrompt: 'You help users to calculate expressions.',
    });

    const result = await agent.invoke('Calculate an expression for me: What is 12345 plus 6789?');
    console.log(result.toString());
  } finally {
    await calculatorTools.disconnect();
  }
}
