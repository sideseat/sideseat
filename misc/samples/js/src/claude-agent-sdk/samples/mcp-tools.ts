/**
 * External stdio MCP server integration.
 *
 * Demonstrates mcpServers with a stdio transport, strictMcpConfig to ignore
 * .mcp.json and user-level servers, and the mcp__<server>__<tool> naming
 * convention required by allowedTools.
 */
import { query } from '@anthropic-ai/claude-agent-sdk';
import { fileURLToPath } from 'node:url';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { buildOptions, printStream } from '../helpers.js';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
// misc/samples/js/src/claude-agent-sdk/samples -> misc/mcp
const MCP_SERVER_DIR = path.resolve(__dirname, '../../../../../mcp');

const QUERIES = [
  'Calculate an expression for me: What is 12345 plus 6789?',
  'What is 987 multiplied by 654? Use the calculator.',
];

export async function run(modelId: string, env: Record<string, string>) {
  if (!fs.existsSync(path.join(MCP_SERVER_DIR, 'calculator.py'))) {
    throw new Error(
      `MCP server not found in ${MCP_SERVER_DIR}. Run from the misc/samples/js directory.`
    );
  }

  const options = buildOptions(modelId, env, {
    mcpServers: {
      calculator: {
        type: 'stdio',
        // Launch via uv: misc/mcp has its own venv with fastmcp, so a bare
        // `python` would fail to import it and the server would never start.
        command: 'uv',
        args: ['run', '--directory', MCP_SERVER_DIR, 'mcp-calculator'],
      },
    },
    // Ignore .mcp.json, user settings, and plugin servers so only ours loads.
    strictMcpConfig: true,
    allowedTools: ['mcp__calculator__calculate'],
    systemPrompt: 'You help users calculate expressions. Always use the calculate tool.',
  });

  for (const prompt of QUERIES) {
    console.log(`--- ${prompt} ---`);
    await printStream(query({ prompt, options }));
    console.log();
  }
}
