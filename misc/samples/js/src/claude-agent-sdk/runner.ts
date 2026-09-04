import { DEFAULT_MODEL, MODEL_ALIASES, resolveModel } from '../shared/config.js';
import {
  setupTelemetry,
  shutdownTelemetry,
  buildClaudeCodeEnv,
  getClient,
  CLAUDE_AGENT_SERVICE_NAME,
  Frameworks,
} from '../shared/telemetry.js';
import { createTraceAttributes } from '../shared/trace.js';

type Sample = {
  run: (modelId: string, env: Record<string, string>) => Promise<void>;
};

let SAMPLES: Record<string, Sample> = {};

// Convert kebab-case to camelCase for sample lookup
const toCamelCase = (s: string) => s.replace(/-([a-z])/g, (_, c) => c.toUpperCase());

// Convert camelCase to kebab-case for display
const toKebabCase = (s: string) =>
  s
    .replace(/([A-Z])/g, '-$1')
    .toLowerCase()
    .replace(/^-/, '');

async function loadSamples(): Promise<void> {
  const samples = await import('./samples/index.js');
  SAMPLES = samples as unknown as Record<string, Sample>;
}

function printHelp() {
  console.log('Usage: npm run claude-agent-sdk -- <sample> [options]');
  console.log('\nOptions:');
  console.log('  --model=<alias>  Model alias or full model ID (default: bedrock-haiku)');
  console.log('  --sideseat       Use SideSeat SDK for telemetry');
  console.log('  --list           List available samples and model aliases');
  console.log('  --help           Show this help message');
  console.log('\nSamples: Use --list to see available samples');
}

function printList() {
  console.log('Available Samples:');
  console.log('-'.repeat(50));
  for (const name of Object.keys(SAMPLES)) {
    console.log(`  ${toKebabCase(name)}`);
  }
  console.log();

  console.log('Model Aliases:');
  console.log('-'.repeat(50));
  for (const [alias, modelId] of Object.entries(MODEL_ALIASES)) {
    console.log(`  ${alias.padEnd(20)} -> ${modelId}`);
  }
  console.log();
  console.log(`Default: ${DEFAULT_MODEL}`);
}

async function runSample(
  name: string,
  sample: Sample,
  modelArg: string,
  useSideseat: boolean
): Promise<boolean> {
  const traceAttrs = createTraceAttributes(name, 'claude-agent-sdk');

  console.log(`Running sample: ${toKebabCase(name)}`);
  console.log(`  Model: ${modelArg}`);
  console.log(`  SideSeat telemetry: ${useSideseat}`);
  console.log(`  Session: ${traceAttrs['session.id']}`);
  console.log();

  // Resolve the alias to a Bedrock inference-profile ID before handing it on:
  // options.model overrides ANTHROPIC_MODEL, so passing the bare alias through
  // would reach Bedrock verbatim and fail with "model identifier is invalid".
  const modelId = resolveModel(modelArg);

  // Env for the Claude Code CLI subprocess, which owns the instrumentation.
  const env = buildClaudeCodeEnv(modelId);

  const client = getClient();
  if (client === null) {
    await sample.run(modelId, env);
    return true;
  }

  // Wrap the run in a root span. The Agent SDK injects TRACEPARENT from the active
  // span, so the CLI's claude_code.* spans nest under this one. Without it every
  // run lands as a separate bare claude_code.interaction trace with no session.
  await client.span(`claude-agent-${toKebabCase(name)}`, async (span) => {
    span.setAttribute('session.id', traceAttrs['session.id']);
    span.setAttribute('user.id', traceAttrs['user.id']);
    await sample.run(modelId, env);
  });
  return true;
}

async function main() {
  const args = process.argv.slice(2);

  const useSideseat = args.includes('--sideseat');
  const showList = args.includes('--list');
  const showHelp = args.includes('--help') || args.includes('-h');
  const modelArg = args.find((a) => a.startsWith('--model='))?.split('=')[1] ?? DEFAULT_MODEL;
  const rawName = args.find((a) => !a.startsWith('--'));

  if (showHelp) {
    printHelp();
    return;
  }

  // Host-process provider. The Agent SDK injects TRACEPARENT from the active span
  // into the CLI subprocess, so the agent run nests under this trace.
  await setupTelemetry({
    useSideseat,
    framework: Frameworks.ClaudeAgentSDK,
    serviceName: CLAUDE_AGENT_SERVICE_NAME,
  });

  await loadSamples();

  if (showList) {
    printList();
    return;
  }

  if (!rawName) {
    printHelp();
    process.exit(1);
  }

  const sampleName = toCamelCase(rawName);

  if (rawName === 'all') {
    const results: Array<{ name: string; ok: boolean; error?: string }> = [];
    for (const [name, sample] of Object.entries(SAMPLES)) {
      console.log(`\n${'='.repeat(60)}\nRunning: ${name}\n${'='.repeat(60)}`);
      try {
        await runSample(name, sample, modelArg, useSideseat);
        results.push({ name, ok: true });
        console.log(`[OK] ${name}`);
      } catch (e) {
        results.push({ name, ok: false, error: String(e) });
        console.error(`[FAILED] ${name}:`, e);
      }
    }
    console.log(`\n${'='.repeat(60)}\nSummary\n${'='.repeat(60)}`);
    const passed = results.filter((r) => r.ok).length;
    const failed = results.length - passed;
    console.log(`Passed: ${passed}/${results.length}, Failed: ${failed}`);
    await shutdownTelemetry();
    if (failed > 0) process.exit(1);
    return;
  }

  const sample = SAMPLES[sampleName];
  if (!sample) {
    console.error(`Unknown sample: ${sampleName}`);
    console.error('Available:', Object.keys(SAMPLES).map(toKebabCase).join(', '));
    process.exit(1);
  }

  await runSample(sampleName, sample, modelArg, useSideseat);

  // Flush pending traces before exit
  await shutdownTelemetry();
}

main().catch(async (e) => {
  console.error('Fatal error:', e);
  await shutdownTelemetry();
  process.exit(1);
});
