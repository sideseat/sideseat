/**
 * Tool permission control.
 *
 * Demonstrates canUseTool (invoked only when the permission flow falls through to a
 * prompt), allowing with a rewritten input, denying, and disallowedTools with a scoped
 * rule that denies in every mode.
 *
 * NOTE: do not also list a gated tool in allowedTools. Allow rules approve the call
 * before canUseTool runs, so the callback would never fire for it.
 */
import { query, type CanUseTool } from '@anthropic-ai/claude-agent-sdk';
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { buildOptions, printStream } from '../helpers.js';

// Relative paths matter: the permission callback redirects into a reviewed/ subdir
// resolved against cwd. An absolute path would land outside the sample workspace.
const PROMPT =
  'Using relative paths in the current directory, create notes.txt containing ' +
  "'hello', then create secrets.txt containing 'token=abc123'. " +
  'Report what happened for each.';

/** Allow writes, except anything that looks like a secrets file. */
const gateTools: CanUseTool = async (toolName, input) => {
  const filePath = String((input as { file_path?: unknown }).file_path ?? '');

  if (toolName === 'Write' && filePath.toLowerCase().includes('secret')) {
    console.log(`  [permission] DENY ${toolName} -> ${filePath}`);
    return { behavior: 'deny', message: 'Writing secrets files is not allowed' };
  }

  if (toolName === 'Write' && filePath) {
    // Redirect every write into a reviewed/ subdirectory.
    const redirected = path.join(path.dirname(filePath), 'reviewed', path.basename(filePath));
    console.log(`  [permission] ALLOW ${toolName} -> ${redirected} (redirected)`);
    return { behavior: 'allow', updatedInput: { ...input, file_path: redirected } };
  }

  console.log(`  [permission] ALLOW ${toolName}`);
  return { behavior: 'allow', updatedInput: input };
};

export async function run(modelId: string, env: Record<string, string>) {
  const workspace = fs.mkdtempSync(path.join(os.tmpdir(), 'sideseat-permissions-'));
  fs.mkdirSync(path.join(workspace, 'reviewed'));

  try {
    const options = buildOptions(modelId, env, {
      cwd: workspace,
      // Write is deliberately absent from allowedTools so gateTools runs.
      allowedTools: ['Read', 'Glob'],
      // A scoped rule denies matching calls even under bypassPermissions.
      disallowedTools: ['Bash(rm *)'],
      canUseTool: gateTools,
      systemPrompt: 'You are a careful file assistant.',
    });

    console.log(`--- ${PROMPT} ---`);
    await printStream(query({ prompt: PROMPT, options }));

    const written = fs.readdirSync(path.join(workspace, 'reviewed')).sort();
    console.log(`\nFiles in reviewed/: ${written.length ? written.join(', ') : 'none'}`);
  } finally {
    fs.rmSync(workspace, { recursive: true, force: true });
  }
}
