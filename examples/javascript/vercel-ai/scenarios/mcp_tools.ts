import { Client } from '@modelcontextprotocol/sdk/client';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';
import {
  dynamicTool,
  generateText,
  jsonSchema,
  stepCountIs,
  type JSONSchema7,
  type LanguageModel,
  type ToolSet,
} from 'ai';
import { prompts, SYSTEM } from '../../harness/content.js';
import { mcpCalculatorCommand, type Run } from '../../harness/run.js';

// `@ai-sdk/mcp` 2.0 still opens the 2025-11-25 handshake, which current MCP servers refuse, so the
// tools come from the official MCP client and are handed to the AI SDK as dynamic tools.
async function serverTools(client: Client): Promise<ToolSet> {
  const { tools } = await client.listTools();
  return Object.fromEntries(
    tools.map((spec) => [
      spec.name,
      dynamicTool({
        description: spec.description ?? spec.name,
        inputSchema: jsonSchema(spec.inputSchema as JSONSchema7),
        execute: async (input) => {
          const result = await client.callTool({
            name: spec.name,
            arguments: input as Record<string, unknown>,
          });
          return result.content;
        },
      }),
    ])
  );
}

export async function run(run: Run<LanguageModel>): Promise<void> {
  const client = new Client({ name: 'sideseat-example', version: '1.0.0' });
  await client.connect(new StdioClientTransport(mcpCalculatorCommand()));
  try {
    await run.trace(async () => {
      const { text } = await generateText({
        model: run.llm,
        system: SYSTEM,
        prompt: prompts.mcp,
        tools: await serverTools(client),
        stopWhen: stepCountIs(8),
      });
      console.log(text);
    });
  } finally {
    await client.close();
  }
}
