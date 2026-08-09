/**
 * Structured output via a JSON schema.
 *
 * Demonstrates outputFormat with a json_schema and reading structured_output off the
 * result message. The schema mirrors the Person model used by the other sample suites
 * so the extracted shape is comparable across frameworks in the SideSeat UI.
 */
import { query } from '@anthropic-ai/claude-agent-sdk';
import { buildOptions, printResult } from '../helpers.js';

const PERSON_SCHEMA = {
  type: 'object',
  properties: {
    name: { type: 'string', description: 'Full name of the person' },
    age: { type: 'integer', description: 'Age in years' },
    address: {
      type: 'object',
      properties: {
        street: { type: 'string' },
        city: { type: 'string' },
        country: { type: 'string' },
        postal_code: { type: 'string' },
      },
      required: ['street', 'city', 'country'],
      additionalProperties: false,
    },
    contacts: {
      type: 'array',
      items: {
        type: 'object',
        properties: { email: { type: 'string' }, phone: { type: 'string' } },
        additionalProperties: false,
      },
    },
    skills: { type: 'array', items: { type: 'string' } },
  },
  required: ['name', 'age', 'address'],
  additionalProperties: false,
};

const PROMPT =
  'Extract info: Jane Doe, a systems admin, 28, lives at 123 Main St, ' +
  'New York, USA. Email: jane@example.com';

export async function run(modelId: string, env: Record<string, string>) {
  const options = buildOptions(modelId, env, {
    outputFormat: { type: 'json_schema', schema: PERSON_SCHEMA },
    systemPrompt:
      'You are an information extraction assistant. ' +
      'Extract the person information from the provided text.',
  });

  console.log(`--- ${PROMPT} ---`);
  for await (const message of query({ prompt: PROMPT, options })) {
    if (message.type === 'result') {
      // structured_output is only present on the success variant.
      if (message.subtype === 'success') {
        console.log(JSON.stringify(message.structured_output, null, 2));
      } else {
        console.log(`No structured output: ${message.subtype}`);
      }
      printResult(message);
    }
  }
}
