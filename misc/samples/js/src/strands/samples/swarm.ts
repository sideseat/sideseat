/**
 * Multi-agent swarm orchestration sample using the native Swarm class.
 *
 * Uses Swarm structured-output routing: each agent decides whether to
 * hand off to another agent or produce a final response.
 *
 * Architecture:
 * - Planner: Entry point, coordinates tasks and routes to specialists
 * - Researcher: Gathers information using search and weather tools
 * - Coder: Writes code and implementations
 * - Reviewer: Reviews code and provides feedback
 */

import { Agent, Swarm, tool } from '@strands-agents/sdk';
import { z } from 'zod';
import { resolveModel } from '../../shared/config.js';
import type { ContentBlock } from '@strands-agents/sdk';

const calculator = tool({
  name: 'calculator',
  description: 'Perform basic arithmetic operations.',
  inputSchema: z.object({
    operation: z
      .enum(['add', 'subtract', 'multiply', 'divide'])
      .describe('The operation to perform'),
    a: z.number().describe('First number'),
    b: z.number().describe('Second number'),
  }),
  callback: ({ operation, a, b }) => {
    const operations: Record<string, (x: number, y: number) => number> = {
      add: (x, y) => x + y,
      subtract: (x, y) => x - y,
      multiply: (x, y) => x * y,
      divide: (x, y) => (y !== 0 ? x / y : Infinity),
    };
    return operations[operation]?.(a, b) ?? 0;
  },
});

const weatherForecast = tool({
  name: 'weather_forecast',
  description: 'Get weather forecast for a city.',
  inputSchema: z.object({
    city: z.string().describe('The name of the city'),
    days: z.number().default(3).describe('Number of days for the forecast'),
  }),
  callback: ({ city, days }) => {
    const forecasts: Record<string, string> = {
      'New York': 'Partly cloudy with temperatures around 65F',
      London: 'Rainy with temperatures around 55F',
      Tokyo: 'Clear skies with temperatures around 70F',
      Paris: 'Overcast with temperatures around 60F',
    };
    const base = forecasts[city] ?? 'Weather data unavailable';
    return `${days}-day forecast for ${city}: ${base}`;
  },
});

const webSearch = tool({
  name: 'search_web',
  description: 'Search the web for information.',
  inputSchema: z.object({
    query: z.string().describe('Search query string'),
    max_results: z.number().default(5).describe('Maximum number of results to return'),
  }),
  callback: ({ query, max_results }) => ({
    status: 'success',
    content: [
      {
        json: {
          query,
          results: Array.from({ length: Math.min(max_results, 5) }, (_, i) => ({
            title: `Result ${i + 1} for '${query}'`,
            url: `https://example.com/${i}`,
          })),
        },
      },
    ],
  }),
});

export async function run(modelId: string) {
  const model = resolveModel(modelId);

  console.log('Creating swarm agents...');

  const researcher = new Agent({
    agentId: 'researcher',
    name: 'Researcher',
    description: 'Research specialist that gathers information using web search and weather tools',
    model,
    tools: [webSearch, weatherForecast],
    printer: false,
    systemPrompt: `You are a research specialist. Your role is to:
1. Gather information on topics
2. Provide factual, well-sourced answers
3. Use search_web and weather_forecast tools as needed`,
  });

  const coder = new Agent({
    agentId: 'coder',
    name: 'Coder',
    description: 'Coding specialist that writes clean, efficient code and implementations',
    model,
    tools: [calculator],
    printer: false,
    systemPrompt: `You are a coding specialist. Your role is to:
1. Write clean, efficient code
2. Implement solutions based on requirements
3. Use calculator for any needed computations`,
  });

  const reviewer = new Agent({
    agentId: 'reviewer',
    name: 'Reviewer',
    description:
      'Code reviewer that evaluates code quality, correctness, and suggests improvements',
    model,
    tools: [calculator],
    printer: false,
    systemPrompt: `You are a code reviewer. Your role is to:
1. Review code for quality and correctness
2. Suggest improvements
3. Verify calculations are correct`,
  });

  const planner = new Agent({
    agentId: 'planner',
    name: 'Planner',
    description: 'Project planner that breaks down complex tasks and coordinates specialists',
    model,
    tools: [calculator],
    printer: false,
    systemPrompt: `You are a project planner. Your role is to:
1. Break down complex tasks into steps
2. Identify which specialist should handle each step
3. Delegate to: researcher (for research/info), coder (for implementation), reviewer (for code review)
4. Use calculator directly for simple math
5. Coordinate the overall workflow and synthesize results`,
  });

  const swarm = new Swarm({
    nodes: [researcher, coder, reviewer, planner],
    start: 'planner',
    maxSteps: 20,
  });

  const task =
    'Create a simple plan to build a weather app that shows forecasts for multiple cities';
  console.log(`Task: ${task}`);
  console.log('='.repeat(60));

  const result = await swarm.invoke(task);

  console.log('\n' + '='.repeat(60));
  console.log('Swarm Result:');
  for (const block of result.content as ContentBlock[]) {
    if (block.type === 'textBlock') {
      console.log(block.text);
    }
  }
}
