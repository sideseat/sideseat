/** The shared example tools as AI SDK tools. */
import { jsonSchema, tool } from 'ai';
import { toolSpec, tools, type ToolName } from '../harness/content.js';

function shared(name: ToolName) {
  const { description, parameters } = toolSpec(name);
  const run = tools[name] as (input: unknown) => unknown;
  return tool({
    description,
    inputSchema: jsonSchema(parameters),
    execute: async (input) => run(input),
  });
}

export const get_weather = shared('get_weather');
export const get_precipitation = shared('get_precipitation');
export const book_flight = shared('book_flight');
