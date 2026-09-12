/**
 * Structured output sample using generateObject.
 */

import { generateObject } from 'ai';
import { bedrock } from '@ai-sdk/amazon-bedrock';
import { z } from 'zod';
import { resolveModel } from '../../shared/config.js';

// Schema matching Python's Pydantic model
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
  const { object } = await generateObject({
    model: bedrock(resolveModel(modelId)),
    schema: PersonSchema,
    prompt:
      'Extract info: Jane Doe, a systems admin, 28, lives at 123 Main St, New York, USA. Email: jane@example.com',
    experimental_telemetry: { isEnabled: true },
  });
  console.log('Extracted:', object);
}
