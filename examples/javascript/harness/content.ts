/**
 * The prompts and tools every suite uses, so all frameworks, in both languages, hold the same
 * conversations.
 *
 * The text lives in `content.json`, rendered from the Python harness (`examples/python/harness`),
 * which is the single source. The tool bodies are re-implemented here and checked against the
 * results the Python tools produced, so a change on either side fails before a scenario runs.
 */
import { readFileSync } from 'node:fs';
import { isDeepStrictEqual } from 'node:util';

interface ToolExample {
  arguments: Record<string, unknown>;
  result?: unknown;
  error?: { name: string; message: string };
}

/** A tool definition: the name, description and JSON schema of a shared tool. */
export interface ToolSpec {
  name: string;
  description: string;
  parameters: {
    type: 'object';
    properties: Record<
      string,
      { type: 'string' | 'integer' | 'number' | 'boolean'; description?: string }
    >;
    required: string[];
  };
}

export interface ModelEntry {
  surface: string;
  id: string;
  reasoning: boolean;
}

interface Document {
  prompts: {
    system: string;
    chat: string;
    multi_turn: string[];
    tool_use: string;
    session: string[];
    error: string;
    streaming: string;
    structured: string;
    reasoning: string;
    files: string;
    multi_agent: string;
    mcp: string;
  };
  trip_plan: Record<string, unknown> & {
    properties: Record<string, unknown>;
    required: string[];
  };
  tools: ToolSpec[];
  tool_examples: Record<string, ToolExample[]>;
  scenarios: { name: string; summary: string; core: boolean }[];
  user_id: string;
  models: Record<string, ModelEntry>;
  default_model: string;
}

const document = JSON.parse(
  readFileSync(new URL('./content.json', import.meta.url), 'utf8')
) as Document;

export const prompts = document.prompts;
export const SYSTEM = prompts.system;
export const scenarios = document.scenarios;
export const USER_ID = document.user_id;
export const models = document.models;
export const DEFAULT_MODEL = document.default_model;

/** The JSON schema of a trip plan: city, one activity per day, and a budget in euros. */
export const TRIP_PLAN_SCHEMA = document.trip_plan;

export interface TripPlan {
  city: string;
  days: string[];
  budget_eur: number;
}

/** The definition of a shared tool, as the Python harness derives it from the function. */
export function toolSpec(name: string): ToolSpec {
  const found = document.tools.find((tool) => tool.name === name);
  if (!found) throw new Error(`no shared tool ${name}`);
  return found;
}

const RAINY = new Set(['tokyo', 'london', 'oslo', 'barcelona']);

export function getWeather({ city, days = 1 }: { city: string; days?: number }) {
  const rainy = RAINY.has(city.trim().toLowerCase());
  const count = Math.max(1, Math.min(days, 7));
  return {
    city,
    forecast: Array.from({ length: count }, (_, day) => ({
      day: day + 1,
      condition: rainy && day === 0 ? 'rain' : 'sunny',
      high_c: 21 + day,
    })),
  };
}

export function getPrecipitation({ city }: { city: string }): string {
  const chance = RAINY.has(city.trim().toLowerCase()) ? 80 : 10;
  return `${chance}% chance of rain in ${city} tomorrow.`;
}

export class BookingUnavailable extends Error {
  override name = 'BookingUnavailable';
}

export function bookFlight({
  origin,
  destination,
  date,
}: {
  origin: string;
  destination: string;
  date: string;
}): string {
  throw new BookingUnavailable(
    `No seats from ${origin} to ${destination} on ${date}: the booking system is offline.`
  );
}

/** The shared tools by their shared names. */
export const tools = {
  get_weather: getWeather,
  get_precipitation: getPrecipitation,
  book_flight: bookFlight,
} as const;

export type ToolName = keyof typeof tools;

/** The text a model reads for a tool result: JSON for structured values, as the Python tools give. */
export function resultText(value: unknown): string {
  return typeof value === 'string' ? value : JSON.stringify(value);
}

function checkTools(): void {
  for (const [name, examples] of Object.entries(document.tool_examples)) {
    const tool = tools[name as ToolName] as (args: Record<string, unknown>) => unknown;
    for (const example of examples) {
      let outcome: Pick<ToolExample, 'result' | 'error'>;
      try {
        outcome = { result: tool(example.arguments) };
      } catch (error) {
        const { name: kind, message } = error as Error;
        outcome = { error: { name: kind, message } };
      }
      const expected = example.error ? { error: example.error } : { result: example.result };
      if (!isDeepStrictEqual(outcome, expected)) {
        throw new Error(
          `${name}(${JSON.stringify(example.arguments)}) returned ${JSON.stringify(outcome)}; ` +
            `the Python tool returns ${JSON.stringify(expected)}`
        );
      }
    }
  }
}

checkTools();
