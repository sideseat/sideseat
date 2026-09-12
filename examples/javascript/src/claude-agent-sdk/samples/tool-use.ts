/**
 * Built-in tool use over a scratch workspace.
 *
 * Demonstrates the Claude Code built-in tools (Read, Glob, Grep, Bash), allowedTools
 * auto-approval scoped to a temporary cwd, and claude_code.tool spans nesting under
 * claude_code.interaction.
 */
import { query } from '@anthropic-ai/claude-agent-sdk';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { buildOptions, printStream } from '../helpers.js';

/**
 * Prompts for the seeded workspace.
 *
 * The Read tool requires absolute paths, so the workspace path is interpolated. Left to
 * guess, the model reaches for a bare "/config.ts", Read fails with ENOENT and the span
 * is flagged as an error — the model recovers, but every run shows up red in the UI.
 * Telling it to use a relative path makes that worse, not better.
 */
const queries = (workspace: string) => [
  'How many TypeScript files are in this directory? Use Glob.',
  `Read ${path.join(workspace, 'config.ts')} and summarize what it configures in one sentence.`,
  `Which file under ${workspace} mentions 'inventory'? Use Grep, then read that file.`,
];

function seedWorkspace(root: string): void {
  fs.writeFileSync(
    path.join(root, 'config.ts'),
    "export const DATABASE_URL = 'postgres://localhost/demo';\n" +
      'export const POOL_SIZE = 10;\n' +
      'export const RETRY_ATTEMPTS = 3;\n'
  );
  // The word "inventory" must appear in the body, not just the filename: Grep
  // matches file contents.
  fs.writeFileSync(
    path.join(root, 'inventory.ts'),
    '// Warehouse inventory tracking.\n' +
      'export const ITEMS: Record<string, number> = { widget: 12, gasket: 4 };\n\n' +
      'export function restock(name: string, count: number): void {\n' +
      '  ITEMS[name] = (ITEMS[name] ?? 0) + count;\n' +
      '}\n'
  );
  fs.writeFileSync(
    path.join(root, 'README.md'),
    '# Demo workspace\n\nA scratch tree for sample runs.\n'
  );
}

export async function run(modelId: string, env: Record<string, string>) {
  const workspace = fs.mkdtempSync(path.join(os.tmpdir(), 'sideseat-tool-use-'));
  seedWorkspace(workspace);

  try {
    const options = buildOptions(modelId, env, {
      cwd: workspace,
      allowedTools: ['Read', 'Glob', 'Grep', 'Bash'],
      systemPrompt: 'You are a concise code explorer. Answer in one or two sentences.',
    });

    for (const prompt of queries(workspace)) {
      console.log(`--- ${prompt} ---`);
      await printStream(query({ prompt, options }));
      console.log();
    }
  } finally {
    fs.rmSync(workspace, { recursive: true, force: true });
  }
}
