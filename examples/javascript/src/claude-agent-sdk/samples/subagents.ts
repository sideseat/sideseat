/**
 * Subagent delegation.
 *
 * Demonstrates programmatically defined subagents via the agents option, and
 * delegation nesting: a subagent's llm_request and tool spans appear under the
 * parent's claude_code.tool span, so the whole chain is one trace in SideSeat.
 */
import { query } from '@anthropic-ai/claude-agent-sdk';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { buildOptions, printStream } from '../helpers.js';

const PROMPT =
  'Review the TypeScript files in this directory. Use the style-reviewer subagent to ' +
  'check naming and the docs-reviewer subagent to check comments, then give me a ' +
  'combined two-bullet summary.';

function seedWorkspace(root: string): void {
  fs.writeFileSync(
    path.join(root, 'orders.ts'),
    'export function ProcessOrder(x: number, y: number) {\n' +
      '  return { total: x * y };\n' +
      '}\n\n' +
      '/** Cancel an order. */\n' +
      'export function cancel(orderId: string) {\n' +
      '  return true;\n' +
      '}\n'
  );
  fs.writeFileSync(
    path.join(root, 'shipping.ts'),
    'export const RATES = { ground: 5.0, air: 18.5 };\n\n' +
      'export function quote(weightKg: number, method: keyof typeof RATES) {\n' +
      '  return weightKg * RATES[method];\n' +
      '}\n'
  );
}

export async function run(modelId: string, env: Record<string, string>) {
  const workspace = fs.mkdtempSync(path.join(os.tmpdir(), 'sideseat-subagents-'));
  seedWorkspace(workspace);

  try {
    const options = buildOptions(modelId, env, {
      cwd: workspace,
      allowedTools: ['Read', 'Glob', 'Grep', 'Task'],
      agents: {
        'style-reviewer': {
          description: 'Reviews naming and formatting conventions.',
          prompt:
            'You review TypeScript naming conventions. Report only concrete issues, ' +
            'one line each.',
          tools: ['Read', 'Glob', 'Grep'],
        },
        'docs-reviewer': {
          description: 'Reviews comment coverage and quality.',
          prompt:
            'You review TypeScript doc comments. Report only exported functions missing ' +
            'or with inadequate comments, one line each.',
          tools: ['Read', 'Glob', 'Grep'],
        },
      },
      systemPrompt: 'Delegate the review work to your subagents, then summarize.',
    });

    console.log(`--- ${PROMPT} ---`);
    await printStream(query({ prompt: PROMPT, options }));
  } finally {
    fs.rmSync(workspace, { recursive: true, force: true });
  }
}
