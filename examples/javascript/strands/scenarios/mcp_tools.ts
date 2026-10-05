import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';
import { Agent, McpClient, type BedrockModel } from '@strands-agents/sdk';
import { prompts, SYSTEM } from '../../harness/content.js';
import { mcpCalculatorCommand, type Run } from '../../harness/run.js';

export async function run(run: Run<BedrockModel>): Promise<void> {
  const calculator = new McpClient({ transport: new StdioClientTransport(mcpCalculatorCommand()) });
  try {
    const agent = new Agent({
      model: run.llm,
      systemPrompt: SYSTEM,
      tools: [calculator],
      printer: false,
    });
    await run.trace(async () => console.log(String(await agent.invoke(prompts.mcp))));
  } finally {
    await calculator.disconnect();
  }
}
