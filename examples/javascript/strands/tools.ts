/** The shared example tools as Strands tools. */
import { tool, type JSONValue } from '@strands-agents/sdk';
import { toolSpec, tools, type ToolName } from '../harness/content.js';

function shared(name: ToolName) {
  const { description, parameters } = toolSpec(name);
  const run = tools[name] as (input: unknown) => JSONValue;
  return tool({ name, description, inputSchema: parameters, callback: (input) => run(input) });
}

export const getWeather = shared('get_weather');
export const getPrecipitation = shared('get_precipitation');
export const bookFlight = shared('book_flight');
