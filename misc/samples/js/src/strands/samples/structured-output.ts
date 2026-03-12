/**
 * Structured output sample using native structuredOutputSchema.
 */

import { Agent } from '@strands-agents/sdk';
import { z } from 'zod';
import { resolveModel } from '../../shared/config.js';

const AddressSchema = z.object({
  street: z.string(),
  city: z.string(),
  country: z.string(),
  postal_code: z.string().optional(),
});

const ContactSchema = z.object({
  email: z.string().optional(),
  phone: z.string().optional(),
});

const PersonSchema = z.object({
  name: z.string().describe('Full name of the person'),
  age: z.number().describe('Age in years'),
  address: AddressSchema.describe('Home address'),
  contacts: z.array(ContactSchema).default([]).describe('Contact methods'),
  skills: z.array(z.string()).default([]).describe('Professional skills'),
});

export async function run(modelId: string) {
  const agent = new Agent({
    model: resolveModel(modelId),
    structuredOutputSchema: PersonSchema,
    printer: false,
    systemPrompt:
      'You are an information extraction assistant. Extract the person information from the provided text.',
  });

  const result = await agent.invoke(
    'Extract info: Jane Doe, a systems admin, 28, lives at 123 Main St, New York, USA. Email: jane@example.com'
  );

  console.log(JSON.stringify(result.structuredOutput, null, 2));
}
