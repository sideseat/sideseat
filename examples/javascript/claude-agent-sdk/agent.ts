/** Options every scenario shares, and printing of the Agent SDK's message stream. */
import type { Options, SDKMessage, SDKResultMessage } from '@anthropic-ai/claude-agent-sdk';
import { mkdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { SYSTEM } from '../harness/content.js';
import type { Run } from '../harness/run.js';
import type { ClaudeModel } from './models.js';

// The CLI inherits this process's environment. Run from inside a Claude Code session, that holds
// the session's own variables, which would pick the child's model and effort, truncate the content
// its telemetry records, and attach it to the parent session; none of them belong to the scenario.
// That includes the session's `TRACEPARENT`: the Agent SDK passes the active span to the CLI only
// when `options.env` does not already name one, so an inherited value joins the CLI's spans to the
// host session's trace instead of the scenario's.
const HOST_SESSION =
  /^(TRACEPARENT|TRACESTATE|CLAUDECODE|CLAUDE_PID|CLAUDE_EFFORT|ANTHROPIC_DEFAULT_\w+_MODEL|CLAUDE_CODE_(SESSION|CHILD|MESSAGING|ENTRYPOINT|EXECPATH|PATH|OTEL_CONTENT|ENABLE_AUTO)\w*)$/;
for (const name of Object.keys(process.env)) {
  if (HOST_SESSION.test(name)) delete process.env[name];
}

// Without these the CLI adds the developer's memory files, CLAUDE.md instructions, and git
// guidance to the first user turn: private, machine-specific context that is not the scenario's.
const ISOLATED = {
  CLAUDE_CODE_DISABLE_AUTO_MEMORY: '1',
  CLAUDE_CODE_DISABLE_CLAUDE_MDS: '1',
  CLAUDE_CODE_DISABLE_GIT_INSTRUCTIONS: '1',
  CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: '1',
};

/** Outside any repository, so the CLI's environment summary names no project. */
const WORKSPACE = join(tmpdir(), 'sideseat-claude-agent-sdk-js');

/**
 * Options for a run on the harness model, with the shared system prompt and no built-in tools.
 *
 * `settingSources: []` ignores the developer's `~/.claude` and any project settings, so a capture
 * does not depend on the machine it ran on. `env` replaces the CLI's inherited environment, so it
 * carries this process's, which holds the telemetry variables in both modes.
 */
export function options(run: Run<ClaudeModel>, overrides: Partial<Options> = {}): Options {
  mkdirSync(WORKSPACE, { recursive: true });
  return {
    model: run.llm.id,
    env: { ...process.env, ...ISOLATED, ...run.llm.env },
    cwd: WORKSPACE,
    systemPrompt: SYSTEM,
    tools: [],
    settingSources: [],
    maxTurns: 8,
    stderr: (line) => {
      if (line.trim()) console.log(`  [cli] ${line.trimEnd()}`);
    },
    ...overrides,
  };
}

/** Print a message stream and return its result, failing if the run did not succeed. */
export async function show(
  stream: AsyncIterable<SDKMessage>,
  { until }: { until?: (result: SDKResultMessage) => void } = {}
): Promise<SDKResultMessage> {
  let result: SDKResultMessage | undefined;
  for await (const message of stream) {
    if (message.type === 'assistant') {
      for (const block of message.message.content) {
        if (block.type === 'text') console.log(block.text);
        else if (block.type === 'thinking')
          console.log(`  [thinking] ${block.thinking.slice(0, 200)}`);
        else if (block.type === 'tool_use')
          console.log(`  [tool] ${block.name} ${JSON.stringify(block.input)}`);
      }
    } else if (message.type === 'user' && Array.isArray(message.message.content)) {
      for (const block of message.message.content) {
        if (block.type === 'tool_result') {
          console.log(
            `  [${block.is_error ? 'error' : 'result'}] ${JSON.stringify(block.content)}`
          );
        }
      }
    } else if (message.type === 'result') {
      result = message;
      if (message.is_error)
        throw new Error(`the agent run did not succeed: ${JSON.stringify(message)}`);
      until?.(message);
    }
  }
  if (!result) throw new Error('the agent run ended without a result');
  return result;
}
