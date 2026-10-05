/**
 * `sample`: the command every JavaScript suite runs, with the Python harness's command line.
 *
 * ```bash
 * cd examples/javascript/strands
 * npm run sample -- tool_use
 * npm run sample -- tool_use --sideseat --model haiku
 * npm run sample -- --list
 * ```
 *
 * The suite is the directory npm was started in (`INIT_CWD`). It holds `suite.json`
 * (`producer`, `integrations` for `--sideseat`, optional `default-model` and `service-name`),
 * `native.ts` exporting `configure(native)`, `models.ts` exporting `build(model)`, and
 * `scenarios/<name>.ts` modules each exporting `run(run)`.
 */
import { config as loadEnv } from 'dotenv';
import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { basename, join } from 'node:path';
import { parseArgs } from 'node:util';
import { pathToFileURL } from 'node:url';
import { scenarios as CATALOG } from './content.js';
import { DEFAULT_MODEL, MODELS, resolve, UsageError, type Model } from './models.js';
import { Run } from './run.js';
import { NativeTelemetry, SdkTelemetry, type Telemetry } from './telemetry.js';

interface Manifest {
  producer: string;
  integrations: string[];
  'default-model'?: string;
  'service-name'?: string;
}

interface SuiteModule<M> {
  configure?: (native: NativeTelemetry) => void | Promise<void>;
  build?: (model: Model) => M;
  run?: (run: Run<M>) => Promise<void>;
}

// The capture tool and CI set endpoints in the environment, and a developer's .env must not
// redirect a capture to their running server, so the process environment wins.
loadEnv({ path: new URL('../../.env', import.meta.url), override: false, quiet: true });

async function main(): Promise<void> {
  const root = process.env.INIT_CWD ?? process.cwd();
  const manifestPath = join(root, 'suite.json');
  if (!existsSync(manifestPath)) {
    throw new UsageError(
      `${root} has no suite.json; run \`npm run sample\` from a suite directory`
    );
  }
  const manifest = JSON.parse(readFileSync(manifestPath, 'utf8')) as Manifest;
  const defaultModel = manifest['default-model'] ?? DEFAULT_MODEL;
  const { values, positionals } = parseArgs({
    allowPositionals: true,
    options: {
      sideseat: { type: 'boolean', default: false },
      model: { type: 'string', default: defaultModel },
      list: { type: 'boolean', default: false },
    },
  });

  const found = readdirSync(join(root, 'scenarios'))
    .filter((file) => file.endsWith('.ts'))
    .map((file) => basename(file, '.ts'));
  const unknown = found.filter((name) => !CATALOG.some((spec) => spec.name === name));
  if (unknown.length > 0) throw new UsageError(`scenarios outside the catalog: ${unknown}`);
  const available = CATALOG.filter((spec) => found.includes(spec.name));

  if (values.list || positionals.length === 0) {
    console.log('Scenarios:');
    for (const spec of available) console.log(`  ${spec.name.padEnd(18)} ${spec.summary}`);
    console.log('\nModels:');
    for (const model of MODELS) {
      const marker = model.alias === defaultModel ? ' (default)' : '';
      console.log(`  ${model.alias.padEnd(18)} ${model.surface}: ${model.id}${marker}`);
    }
    return;
  }

  const names = available.map((spec) => spec.name);
  const selected = positionals.length === 1 && positionals[0] === 'all' ? names : positionals;
  const missing = selected.filter((name) => !names.includes(name));
  if (missing.length > 0) {
    throw new UsageError(`${manifest.producer} has no scenario ${missing}; available: ${names}`);
  }

  const model = resolve(values.model);
  const load = <M>(name: string): Promise<SuiteModule<M>> =>
    import(pathToFileURL(join(root, `${name}.ts`)).href) as Promise<SuiteModule<M>>;
  let telemetry: Telemetry;
  if (values.sideseat) {
    telemetry = await SdkTelemetry.start(manifest.integrations);
  } else {
    const native = new NativeTelemetry(manifest['service-name'] ?? manifest.producer);
    await (
      await load('native')
    ).configure!(native);
    telemetry = native;
  }
  const { build } = await load<unknown>('models');

  const failures: string[] = [];
  try {
    for (const name of selected) {
      console.log(`\n=== ${manifest.producer} / ${name} (${telemetry.mode}, ${model.alias}) ===`);
      const started = performance.now();
      const run = new Run(manifest.producer, name, model, telemetry, build!);
      try {
        await (
          await load<unknown>(`scenarios/${name}`)
        ).run!(run);
      } catch (error) {
        console.error(error);
        failures.push(`${name}: ${String(error)}`);
        continue;
      }
      console.log(`--- ${name} finished in ${((performance.now() - started) / 1000).toFixed(1)}s`);
    }
  } finally {
    await telemetry.shutdown();
  }
  if (failures.length > 0) {
    console.error(failures.join('\n'));
    process.exitCode = 1;
  }
}

main().catch((error: unknown) => {
  console.error(error instanceof UsageError ? error.message : error);
  process.exitCode = error instanceof UsageError ? 2 : 1;
});
