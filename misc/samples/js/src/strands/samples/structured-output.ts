/**
 * Structured output sample using tool-based approach.
 *
 * Note: Strands JS SDK doesn't have native structured_output() like Python.
 * This sample uses a tool-based workaround where the tool receives validated input.
 */

import { Agent, tool } from '@strands-agents/sdk';
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

type Person = z.infer<typeof PersonSchema>;

export async function run(modelId: string) {
  // Create a tool that receives structured output
  const extractPersonTool = tool({
    name: 'extract_person',
    description:
      'Extract complete person information from text. Always use this tool to return structured person data.',
    inputSchema: PersonSchema,
    callback: (input): Person => input,
  });

  const agent = new Agent({
    model: resolveModel(modelId),
    tools: [extractPersonTool],
    systemPrompt: `You are an information extraction assistant.
When asked to extract person information, ALWAYS use the extract_person tool to return the data.
Never respond with plain text - always use the tool.`,
  });

  const result = await agent.invoke(
    'Extract info: Jane Doe, a systems admin, 28, lives at 123 Main St, New York, USA. Email: jane@example.com'
  );

  console.log(result);
}
