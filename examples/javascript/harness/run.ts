/** What a scenario receives: the model, the telemetry mode, and correlation identifiers. */
import type { Span } from '@opentelemetry/api';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { USER_ID } from './content.js';
import type { Model } from './models.js';
import type { Telemetry } from './telemetry.js';

const EXAMPLES = fileURLToPath(new URL('../../', import.meta.url));

export class Run<M = unknown> {
  readonly producer: string;
  readonly scenario: string;
  readonly model: Model;
  readonly telemetry: Telemetry;
  private readonly build: (model: Model) => M;
  private built: M | undefined;

  constructor(
    producer: string,
    scenario: string,
    model: Model,
    telemetry: Telemetry,
    build: (model: Model) => M
  ) {
    this.producer = producer;
    this.scenario = scenario;
    this.model = model;
    this.telemetry = telemetry;
    this.build = build;
  }

  get mode(): 'native' | 'sdk' {
    return this.telemetry.mode;
  }

  /** Deterministic, so a recapture produces comparable fixtures. */
  get sessionId(): string {
    return `${this.producer}-${this.scenario}`;
  }

  get userId(): string {
    return USER_ID;
  }

  /** The framework's model object for `--model`, built once by the suite's `models.ts`. */
  get llm(): M {
    this.built ??= this.build(this.model);
    return this.built;
  }

  /** A root span for one conversation, attributed to this scenario's session and user. */
  trace<T>(fn: (span: Span) => Promise<T>): Promise<T>;
  trace<T>(name: string, fn: (span: Span) => Promise<T>): Promise<T>;
  trace<T>(a: string | ((span: Span) => Promise<T>), b?: (span: Span) => Promise<T>): Promise<T> {
    const [name, fn] = typeof a === 'string' ? [a, b!] : [this.scenario.replaceAll('_', '-'), a];
    return this.telemetry.trace(name, { sessionId: this.sessionId, userId: this.userId }, fn);
  }

  /** An input file from `examples/assets`: `img.jpg` or `task.pdf`. */
  static asset(name: string): Buffer {
    return readFileSync(`${EXAMPLES}assets/${name}`);
  }
}

/** The stdio command that starts the example MCP calculator server. */
export function mcpCalculatorCommand(): { command: string; args: string[] } {
  const directory = fileURLToPath(
    new URL('../../../scripts/tools/mcp-calculator', import.meta.url)
  );
  return {
    command: 'uv',
    args: ['run', '--locked', '--directory', directory, 'mcp-calculator'],
  };
}
