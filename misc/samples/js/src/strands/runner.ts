// IMPORTANT: Telemetry must be set up BEFORE importing samples (which load AWS SDK)
// This ensures AWS SDK instrumentation captures all calls
import {
  DEFAULT_MODEL,
  MODEL_ALIASES,
  REASONING_MODELS,
  DEFAULT_THINKING_BUDGET,
} from '../shared/config.js';
import { setupTelemetry, shutdownTelemetry, Frameworks } from '../shared/telemetry.js';
import { createTraceAttributes } from '../shared/trace.js';

export interface SampleOptions {
  enableThinking?: boolean;
  thinkingBudget?: number;
}

type Sample = {
  run: (modelId: string, options?: SampleOptions) => Promise<void>;
};

// Samples loaded dynamically after telemetry setup
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
  console.log('Usage: npm run strands -- <sample> [options]');
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
  // Create trace attributes for logging (session ID display)
  const traceAttrs = createTraceAttributes(name);

  console.log(`Running sample: ${toKebabCase(name)}`);
  console.log(`  Model: ${modelArg}`);
  console.log(`  SideSeat telemetry: ${useSideseat}`);
  console.log(`  Session: ${traceAttrs['session.id']}`);

  // Enable extended thinking for reasoning sample
  const enableThinking = name === 'reasoning' && REASONING_MODELS.has(modelArg);
  if (enableThinking) {
    console.log(`  Extended thinking: enabled (budget=${DEFAULT_THINKING_BUDGET} tokens)`);
  }
  console.log();

  const options: SampleOptions = enableThinking
    ? { enableThinking: true, thinkingBudget: DEFAULT_THINKING_BUDGET }
    : {};

  await sample.run(modelArg, options);
  return true;
}

async function main() {
  const args = process.argv.slice(2);

  // Parse flags
  const useSideseat = args.includes('--sideseat');
  const showList = args.includes('--list');
  const showHelp = args.includes('--help') || args.includes('-h');
  const modelArg = args.find((a) => a.startsWith('--model='))?.split('=')[1] ?? DEFAULT_MODEL;
  const rawName = args.find((a) => !a.startsWith('--'));

  if (showHelp) {
    printHelp();
    return;
  }

  // Setup telemetry BEFORE loading samples (which import AWS SDK)
  // This ensures AWS SDK instrumentation captures all Bedrock calls
  setupTelemetry({ useSideseat, framework: Frameworks.Strands });

  // Now load samples (which will import Strands SDK -> AWS SDK)
  await loadSamples();

  if (showList) {
    printList();
    return;
  }

  if (!rawName) {
    printHelp();
    process.exit(1);
  }

  // Normalize sample name: tool-use -> toolUse
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
    // Summary
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
