/**
 * Shared helpers for Claude Agent SDK samples.
 *
 * The option defaults and message-stream printing are identical across samples, so
 * they live here rather than being repeated nine times.
 */
import type { Options, SDKMessage, SDKResultMessage } from '@anthropic-ai/claude-agent-sdk';
import { context, trace } from '@opentelemetry/api';

// Keep runs bounded so a sample can't loop indefinitely against Bedrock.
export const DEFAULT_MAX_TURNS = 8;

/**
 * Build a W3C traceparent for the currently active span, or null if there is none.
 *
 * The Agent SDK is documented to inject this automatically, but it does not happen in
 * the TypeScript SDK, so the CLI starts its own trace and the agent run shows up
 * detached from the host span. Setting TRACEPARENT explicitly is supported and takes
 * precedence over auto-injection.
 */
function activeTraceparent(): string | null {
  const span = trace.getSpan(context.active());
  if (!span) return null;
  const { traceId, spanId, traceFlags } = span.spanContext();
  if (!traceId || !spanId) return null;
  const flags = traceFlags.toString(16).padStart(2, '0');
  return `00-${traceId}-${spanId}-${flags}`;
}

/**
 * Build Options with the sample defaults applied.
 *
 * `env` REPLACES the inherited environment in the TypeScript SDK (unlike Python,
 * where it merges), hence the `...process.env` spread.
 */
export function buildOptions(
  modelId: string,
  env: Record<string, string>,
  overrides: Partial<Options> = {}
): Options {
  const traceparent = activeTraceparent();
  return {
    model: modelId,
    env: {
      ...process.env,
      ...env,
      ...(traceparent ? { TRACEPARENT: traceparent } : {}),
    },
    maxTurns: DEFAULT_MAX_TURNS,
    // Ignore the developer's own ~/.claude and any project settings so samples
    // behave identically on every machine.
    settingSources: [],
    stderr: printStderr,
    ...overrides,
  };
}

/** Surface CLI diagnostics, including OTLP exporter failures. */
function printStderr(data: string): void {
  const line = data.trim();
  if (line) console.log(`  [cli] ${line}`);
}

/**
 * Print an Agent SDK message stream and return the final result message.
 */
export async function printStream(
  stream: AsyncIterable<SDKMessage>,
  showThinking = false
): Promise<SDKResultMessage | null> {
  let result: SDKResultMessage | null = null;

  for await (const message of stream) {
    if (message.type === 'assistant') {
      for (const block of message.message.content) {
        if (block.type === 'text') {
          console.log(block.text);
        } else if (block.type === 'thinking' && showThinking) {
          console.log(`  [thinking] ${block.thinking.slice(0, 1000)}`);
        } else if (block.type === 'tool_use') {
          console.log(`  [tool] ${block.name} ${preview(block.input)}`);
        }
      }
    } else if (message.type === 'user') {
      // Tool results arrive on a user message, not an assistant message.
      const content = message.message.content;
      if (Array.isArray(content)) {
        for (const block of content) {
          if (block.type === 'tool_result') {
            const label = block.is_error ? 'error' : 'result';
            console.log(`  [${label}] ${preview(block.content)}`);
          }
        }
      }
    } else if (message.type === 'result') {
      result = message;
      printResult(message);
    }
  }

  return result;
}

/**
 * Print the cost and token summary from a result message.
 *
 * total_cost_usd is a client-side estimate, not billing data. Per-step output_tokens
 * on assistant messages is a placeholder; only the result carries the real count.
 */
export function printResult(message: SDKResultMessage): void {
  const usage = message.usage;
  console.log(
    `  [usage] turns=${message.num_turns} ` +
      `in=${usage?.input_tokens ?? 0} ` +
      `out=${usage?.output_tokens ?? 0} ` +
      `cache_read=${usage?.cache_read_input_tokens ?? 0} ` +
      `cost=$${(message.total_cost_usd ?? 0).toFixed(6)}`
  );
}

function preview(value: unknown, limit = 120): string {
  const text = String(typeof value === 'string' ? value : JSON.stringify(value)).replace(
    /\n/g,
    ' '
  );
  return text.length <= limit ? text : `${text.slice(0, limit)}...`;
}
